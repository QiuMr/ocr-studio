#!/usr/bin/env python3
"""导出 YOLOE 的「运行时提示」三件套到 models/yoloe/。

与 fetch_yoloe.py 的区别
------------------------
fetch_yoloe.py 是**烘焙**：把提示向量在导出时就融进分类头权重，产出的 onnx 是普通
检测模型，换目标必须重新导出。
本脚本是**运行时**：提示向量作为一张额外的图输入 `pe`，换目标只要换这一张输入。

三件套
------
  yoloe26n_prompt.onnx  检测器，输入 images[1,3,640,640] + pe[1,80,512] → 检测框
  yoloe26n_vpe.onnx     视觉提示编码器，输入 ref[1,3,640,640] + vp[1,Q,80,80] → vpe[1,Q,512]
  mobileclip2b_text.onnx 文本提示编码器，输入 token_ids[1,77] → tpe[1,77,512]

两个提示编码器都产出 `pe` 行向量，喂给同一个检测器即可 —— 这就是「图找图」和
「语言找图」共用一个检测器的原因。

关键实现细节（都是踩出来的）
----------------------------
1. **不能 fuse**。ultralytics 导出时 `exporter.py:524` 会把 `model.pe` 融进 cv3/cv4，
   同时把 `reprta` 换成 Identity，那样 `pe` 就不再是输入了。这里完全不碰 fuse。
2. **`pe` 走 `vpe` 形参**。`YOLOEModel.predict` 里 `get_vpe` 对 ndim==3 的 vpe 直接透传，
   而 `get_cls_pe(tpe=None, vpe=pe)` 恰好等于 `pe`。所以 `model(x, vpe=pe)` 就是
   「用 pe 当类别提示」，不需要改 ultralytics 一行代码。
3. **必须补 topk 补丁**。`Detect.get_topk_index` 里
   `idx = ori_index[torch.arange(B)[..., None], index // nc]` 是两个索引张量的高级索引，
   torch.onnx 会警告并**错误地** trace 成 Gather（每行取到同一个 anchor，框全一样）。
   改成 `torch.gather` 即可。
4. **`export` 要设到所有子模块**。head 的 forward 读的是 head 自己的 `self.export`，
   只设顶层会让它返回 `(y, preds)` 元组并拖出一堆中间张量当输出。
5. **pe 固定 80 槽**。head 内部 `F.normalize(w)`，补零行归一化成 0 → 分数恒为
   bias(-10) → sigmoid≈4.5e-5，天然低于任何阈值；且输出形状仍是 [1,300,6+nm]，
   与烘焙版完全一致，Rust 侧解码器不用改。80 也是 yoloe-26 的原生 nc。
6. **TorchScript 读不了中文路径**（`torch.jit.load` 走 C++ fopen）。文本编码器那步
   会先把 .ts 复制到 ASCII 临时目录再加载。

用法
----
  python scripts/export_yoloe_prompt.py all
  python scripts/export_yoloe_prompt.py detector --size n
  python scripts/export_yoloe_prompt.py visual
  python scripts/export_yoloe_prompt.py text
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
os.environ.setdefault("YOLO_CONFIG_DIR", str(ROOT / ".ultralytics"))

OUT_DIR = ROOT / "models" / "yoloe"
WEIGHTS_CACHE = ROOT / "models" / "_fixtures" / "weights"
ASSETS = ROOT / "assets"

IMGSZ = 640
NC_SLOTS = 80  # 提示槽位数 = yoloe-26 原生 nc
EMBED = 512


# --------------------------------------------------------------------------- #
# 公共
# --------------------------------------------------------------------------- #
def _load_yoloe(size: str):
    """加载 YOLOE 权重；本地没有就让 ultralytics 下到夹具目录（别落仓库根）。"""
    from ultralytics import YOLOE

    weights = f"yoloe-26{size}-seg.pt"
    if Path(weights).exists():
        return YOLOE(weights)
    WEIGHTS_CACHE.mkdir(parents=True, exist_ok=True)
    cwd = Path.cwd()
    try:
        os.chdir(WEIGHTS_CACHE)
        print(f"      本地没有 {weights}，下载到 {WEIGHTS_CACHE}")
        return YOLOE(weights)
    finally:
        os.chdir(cwd)


def _prep_model(size: str):
    """干净模型 + export 标志铺到所有子模块。"""
    m = _load_yoloe(size)
    model = m.model.float().eval()
    for mm in model.modules():
        mm.export = True
    return m, model


def _patch_topk_for_export() -> None:
    """把 end2end 的 topk 索引改成 trace 友好的写法（只动那一处高级索引）。"""
    import torch
    from ultralytics.nn.modules.head import Detect

    if getattr(Detect.get_topk_index, "_trace_patched", False):
        return

    def _get_topk_index(self, scores, max_det):
        batch_size, anchors, nc = scores.shape
        k = max_det if self.export else min(max_det, anchors)
        if self.agnostic_nms:
            scores, labels = scores.max(dim=-1, keepdim=True)
            scores, indices = scores.topk(k, dim=1)
            labels = labels.gather(1, indices)
            return scores, labels, indices
        ori_index = scores.max(dim=-1)[0].topk(k)[1].unsqueeze(-1)
        scores = scores.gather(dim=1, index=ori_index.repeat(1, 1, nc))
        scores, index = scores.flatten(1).topk(k)
        # 原写法 ori_index[arange(B)[..., None], index // nc] 会被错误 trace；
        # gather 是 GatherElements，语义等价且忠实。
        idx = torch.gather(ori_index.squeeze(-1), 1, index // nc).unsqueeze(-1)
        return scores[..., None], (index % nc)[..., None].float(), idx

    _get_topk_index._trace_patched = True
    Detect.get_topk_index = _get_topk_index


def _letterbox_tensor(img_bgr: np.ndarray, imgsz: int = IMGSZ):
    """复刻 ultralytics 的推理前处理，返回 (1,3,H,W) float32 [0,1]。

    **必须先 BGR→RGB**：`cv2.imread` 给的是 BGR，而 ultralytics 的
    `BasePredictor.preprocess` 里有一句 `im = im[..., ::-1]`，LetterBox 吃的是
    RGB。少了这一步，导出的 onnx 照样能用（它对输入值无感，torch 与 ort 用的是
    同一份张量），但**参考值会整体偏掉**——视觉提示编码器是个浅层卷积网，通道
    顺序反了 pe 的最大差能到 4e-2，正是那种「不报错但结果不对」的错。
    """
    import torch
    from ultralytics.data.augment import LetterBox

    im = LetterBox((imgsz, imgsz), auto=False, stride=32)(image=np.ascontiguousarray(img_bgr[..., ::-1]))
    return torch.from_numpy(np.ascontiguousarray(im.transpose(2, 0, 1)[None])).float() / 255.0


def _imread_any(path: Path) -> np.ndarray | None:
    """读图。cv2.imread 走 ANSI 路径，读不了本项目的中文目录 —— 用 imdecode 绕开。"""
    import cv2

    try:
        buf = np.frombuffer(path.read_bytes(), dtype=np.uint8)
        return cv2.imdecode(buf, cv2.IMREAD_COLOR)
    except Exception:  # noqa: BLE001
        return None


def _sample_image() -> np.ndarray:
    for name in ("bus.png", "zidane.png", "test_zh.png", "bus.jpg"):
        p = ASSETS / name
        if p.exists():
            im = _imread_any(p)
            if im is not None:
                return im
    sys.exit(f"找不到可读的测试图，请往 {ASSETS} 放一张 png/jpg")


def _iou_match(a: np.ndarray, b: np.ndarray, conf: float) -> tuple[int, float, float]:
    """按置信度过滤后贪心 IoU 匹配，返回 (框数, 最小 IoU, 分数最大差)。

    end2end 输出是 top-300，尾部全是近并列的低分框，排序在 torch 与 onnxruntime
    之间必然不同（topk 并列无稳定序）。所以**只能**按置信度过滤后做 IoU 匹配来验收，
    逐值对照永远不通过 —— 这是 topk 的固有性质，不是导出 bug。
    """
    fa = a[a[:, 4] > conf]
    fb = b[b[:, 4] > conf]
    if len(fa) != len(fb):
        return -1, 0.0, float("inf")
    if len(fa) == 0:
        return 0, 1.0, 0.0
    ious, used = [], set()
    for row in fa:
        best_i, best_v = -1, -1.0
        for j, cand in enumerate(fb):
            if j in used:
                continue
            x1 = max(row[0] - row[2] / 2, cand[0] - cand[2] / 2)
            y1 = max(row[1] - row[3] / 2, cand[1] - cand[3] / 2)
            x2 = min(row[0] + row[2] / 2, cand[0] + cand[2] / 2)
            y2 = min(row[1] + row[3] / 2, cand[1] + cand[3] / 2)
            inter = max(0.0, x2 - x1) * max(0.0, y2 - y1)
            union = row[2] * row[3] + cand[2] * cand[3] - inter
            v = inter / union if union > 0 else 0.0
            if v > best_v:
                best_v, best_i = v, j
        used.add(best_i)
        ious.append(best_v)
    sc = float(np.abs(np.sort(fa[:, 4]) - np.sort(fb[:, 4])).max())
    return len(fa), float(min(ious)), sc


def _verify_onnx(onnx_path: Path, feed: dict, torch_out: np.ndarray, tag: str) -> bool:
    """跑一遍 onnxruntime，与 torch 逐值对照（适用于无 topk 的图）。"""
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    sess = ort.InferenceSession(str(onnx_path), so, providers=["CPUExecutionProvider"])
    got = sess.run(None, feed)[0]
    if got.shape != torch_out.shape:
        print(f"  ❌ {tag}: 形状不一致 ort{got.shape} vs torch{torch_out.shape}")
        return False
    d = np.abs(got - torch_out).max()
    print(f"  {'✅' if d < 1e-4 else '❌'} {tag}: 逐值最大差 {d:.3e}")
    return d < 1e-4


def _freeze_outputs(path: Path, feed: dict) -> None:
    """把图输出里 torch 推不出来的符号维度换成**实测值**。

    为什么非做不可：end2end 的 topk 形状 torch 的形状推导跟不出来，导出后
    `output0` 的 batch 维和候选数维都是符号维度（`Concatoutput0_dim_0/1`）。
    Rust 侧是**靠输出形状**判断「这是 NMS-free 端到端头（`[1,N,6+nm]`）还是
    普通单输出 `[1,C,N]`」的，遇到符号维度只能拒绝加载——它不能猜，猜错了解码
    不会崩，只会给一堆看着像框的垃圾（项目记忆里最难查的那类 bug）。

    这里跑一遍实测、把真实数字写回 graph.output。图的计算没变，只是把「我们
    已经知道的事实」告诉 ORT 和下游。顺带也把输入的动态 batch 固定成 1。
    """
    import onnx
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    sess = ort.InferenceSession(str(path), so, providers=["CPUExecutionProvider"])
    got = sess.run(None, feed)

    mo = onnx.load(str(path))
    for o, arr in zip(mo.graph.output, got):
        dims = o.type.tensor_type.shape.dim
        if len(dims) != arr.ndim:
            sys.exit(f"输出 {o.name} 声明 {len(dims)} 维、实测 {arr.ndim} 维，无法冻结")
        for d, v in zip(dims, arr.shape):
            d.ClearField("dim_param")
            d.dim_value = int(v)
        print(f"  冻结 {o.name} → {list(arr.shape)}")
    for i in mo.graph.input:
        dims = i.type.tensor_type.shape.dim
        for d in dims:
            if d.HasField("dim_param") and d.dim_param == "b":
                d.ClearField("dim_param")
                d.dim_value = 1
    onnx.checker.check_model(mo)
    onnx.save(mo, str(path))


def _report(path: Path) -> None:
    import onnx

    mo = onnx.load(str(path))
    ins = [(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in mo.graph.input]
    outs = [(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in mo.graph.output]
    print(f"  输入 {ins}")
    print(f"  输出 {outs}")
    print(f"  体积 {path.stat().st_size / 1e6:.1f} MB")


# --------------------------------------------------------------------------- #
# 1) 检测器（运行时 pe）
# --------------------------------------------------------------------------- #
def export_detector(size: str, imgsz: int, name: str) -> Path:
    import torch
    import torch.nn as nn

    _patch_topk_for_export()
    print(f"[detector] 加载 yoloe-26{size}-seg（不 fuse，pe 保持为图输入）")
    _, model = _prep_model(size)

    class Wrap(nn.Module):
        def __init__(self, inner: nn.Module) -> None:
            super().__init__()
            self.inner = inner

        def forward(self, x, pe):
            # ndim==3 的 vpe 在 get_vpe 里直接透传，等价于直接给类别提示
            return self.inner(x, vpe=pe)

    wrapped = Wrap(model).eval()

    x = _letterbox_tensor(_sample_image(), imgsz)
    # 验证用的 pe 必须**非退化**：全零 pe 会让 head 里 F.normalize(0)=0，
    # 所有类别分数恒等于 bias(-10)，topk 出现海量并列，尾部行的排序在 torch 与
    # ort 之间会不同 —— 那是噪声不是 bug（项目记忆里「误检框置信度是混沌的」）。
    rng = np.random.default_rng(0)
    pe = rng.standard_normal((1, NC_SLOTS, EMBED)).astype(np.float32)
    pe /= np.maximum(np.linalg.norm(pe, axis=-1, keepdims=True), 1e-12)
    pe_t = torch.from_numpy(pe)
    with torch.no_grad():
        y = wrapped(x, pe_t)
    y_main = (y[0] if isinstance(y, tuple) else y).numpy()
    print(f"  torch 输出 {y_main.shape}（应为 [1,300,{6 + 32}]）")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / f"{name}.onnx"
    with torch.no_grad():
        # 不写 dynamic_axes：我们只做单图推理，batch 固定 1 才是诚实的声明。
        # （即便不写，output0 的候选数维仍是符号维度，由 _freeze_outputs 补上实测值。）
        torch.onnx.export(
            wrapped, (x, pe_t), str(out),
            input_names=["images", "pe"], output_names=["output0", "proto"],
            opset_version=12, do_constant_folding=True,
        )
    _freeze_outputs(out, {"images": x.numpy(), "pe": pe})
    _report(out)
    # 不做逐值对照：end2end 输出是 top-300，尾部近并列框的排序在 torch/ort 间不稳定，
    # 逐值必然不等。真正的验收是下面的「有语义 pe → IoU 匹配」。
    print("  （逐值对照对 topk 输出无意义，改用语义 pe + IoU 匹配验收，见 export 末尾）")

    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    sess = ort.InferenceSession(str(out), so, providers=["CPUExecutionProvider"])

    # 换 pe 必须换结果 —— 这才是「运行时提示」的意义
    pe2 = rng.standard_normal((1, NC_SLOTS, EMBED)).astype(np.float32)
    pe2 /= np.maximum(np.linalg.norm(pe2, axis=-1, keepdims=True), 1e-12)
    got2 = sess.run(["output0"], {"images": x.numpy(), "pe": pe2})[0]
    delta = np.abs(got2 - y_main).max()
    print(f"  {'✅' if delta > 1e-3 else '❌'} 换 pe 输出变化 {delta:.3e}（>0 = 提示运行时生效）")

    # 运行时真实形态：前 K 槽是真提示、其余补零。补零槽归一化成 0 → 分数恒为 bias，
    # 不该冒出高置信检测。
    K = 3
    pe3 = np.zeros((1, NC_SLOTS, EMBED), dtype=np.float32)
    pe3[0, :K] = rng.standard_normal((K, EMBED))
    pe3 /= np.maximum(np.linalg.norm(pe3, axis=-1, keepdims=True), 1e-12)
    got3 = sess.run(["output0"], {"images": x.numpy(), "pe": pe3})[0]
    idx = got3[0, :, 5].astype(int)
    top = got3[0, np.argsort(-got3[0, :, 4])[:5]]
    print(f"  {'✅' if idx.max() < K else '❌'} 补零形态：类别下标 max={idx.max()}（应 < {K}），"
          f"前 5 高分类别 {top[:, 5].astype(int).tolist()} 分数 "
          f"{np.array2string(top[:, 4], precision=6)}")

    (OUT_DIR / f"{name}.json").write_text(
        json.dumps({"kind": "yoloe-prompt-detector", "imgsz": imgsz, "nc_slots": NC_SLOTS,
                    "embed": EMBED, "task": "segment"}, indent=2) + "\n", encoding="utf-8")
    return out


def verify_end_to_end(size: str, imgsz: int) -> bool:
    """端到端验收：用视觉编码器产出的真语义 vpe 当 pe，跑检测器，按 IoU 匹配。

    这条链路正是运行时「图找图」走的路径：参考图 + 框 → vpe → 喂检测器 → 找同款。
    """
    import torch
    import onnxruntime as ort

    det = OUT_DIR / f"yoloe26{size}_prompt.onnx"
    vpe_path = OUT_DIR / f"yoloe26{size}_vpe.onnx"
    if not det.exists() or not vpe_path.exists():
        print("[e2e] 缺少检测器或视觉编码器，跳过")
        return False

    so = ort.SessionOptions()
    so.log_severity_level = 3
    det_sess = ort.InferenceSession(str(det), so, providers=["CPUExecutionProvider"])
    vpe_sess = ort.InferenceSession(str(vpe_path), so, providers=["CPUExecutionProvider"])

    im0 = _sample_image()
    ref = _letterbox_tensor(im0, imgsz)
    h0, w0 = im0.shape[:2]
    gain = min(imgsz / h0, imgsz / w0)
    boxes = np.array([[31, 230, 801, 775]], dtype=np.float32) * gain
    boxes[..., 0::2] += round((imgsz - w0 * gain) / 2 - 0.1)
    boxes[..., 1::2] += round((imgsz - h0 * gain) / 2 - 0.1)
    from ultralytics.data.augment import LoadVisualPrompt

    vp = LoadVisualPrompt().get_visuals(np.array([0]), (imgsz, imgsz), boxes, None).unsqueeze(0)

    vpe = vpe_sess.run(["vpe"], {"ref": ref.numpy(), "vp": vp.numpy()})[0]  # (1,Q,512)
    q = vpe.shape[1]
    pe = np.zeros((1, NC_SLOTS, EMBED), dtype=np.float32)
    pe[0, :q] = vpe[0]
    print(f"[e2e] vpe {vpe.shape} → pe 前 {q} 槽")

    # torch 参考：直接把同一份 pe 喂进原始模型
    _, model = _prep_model(size)
    with torch.no_grad():
        y = model(ref, vpe=torch.from_numpy(pe))
    y_t = (y[0] if isinstance(y, tuple) else y).numpy()

    got = det_sess.run(["output0"], {"images": ref.numpy(), "pe": pe})[0]
    for conf in (0.25, 0.1, 0.05):
        n, iou, sc = _iou_match(got[0], y_t[0], conf)
        print(f"  conf>{conf}: {n} 框  IoU 最小 {iou:.4f}  分数最大差 {sc:.2e}  "
              f"{'✅' if n >= 0 and iou > 0.99 and sc < 1e-4 else '❌'}")
        if n > 0 and iou > 0.99 and sc < 1e-4:
            return True
    return False


# --------------------------------------------------------------------------- #
# 2) 视觉提示编码器
# --------------------------------------------------------------------------- #
def export_visual(size: str, imgsz: int, name: str) -> Path:
    import torch
    import torch.nn as nn

    print(f"[visual] 加载 yoloe-26{size}-seg（取 savpe 分支）")
    _, model = _prep_model(size)

    class VWrap(nn.Module):
        def __init__(self, inner: nn.Module) -> None:
            super().__init__()
            self.inner = inner

        def forward(self, ref, vp):
            return self.inner(ref, vpe=vp, return_vpe=True)

    wrapped = VWrap(model).eval()

    im0 = _sample_image()
    ref = _letterbox_tensor(im0, imgsz)
    h0, w0 = im0.shape[:2]
    # 复刻 YOLOEVPSegPredictor._process_single_image 的框搬运（含 round(-0.1)）
    gain = min(imgsz / h0, imgsz / w0)
    boxes = np.array([[31, 230, 801, 775]], dtype=np.float32) * gain
    boxes[..., 0::2] += round((imgsz - w0 * gain) / 2 - 0.1)
    boxes[..., 1::2] += round((imgsz - h0 * gain) / 2 - 0.1)
    from ultralytics.data.augment import LoadVisualPrompt

    vp = LoadVisualPrompt().get_visuals(np.array([0]), (imgsz, imgsz), boxes, None).unsqueeze(0)
    print(f"  提示掩码 {tuple(vp.shape)}，置位 {int(vp.sum())} 像素")

    with torch.no_grad():
        vpe = wrapped(ref, vp)
    vpe_np = vpe.numpy()
    print(f"  torch vpe {tuple(vpe.shape)}")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / f"{name}.onnx"
    with torch.no_grad():
        torch.onnx.export(
            wrapped, (ref, vp), str(out),
            input_names=["ref", "vp"], output_names=["vpe"],
            dynamic_axes={"ref": {0: "b"}, "vp": {0: "b", 1: "q"}, "vpe": {0: "b", 1: "q"}},
            opset_version=12, do_constant_folding=True,
        )
    _report(out)
    ok = _verify_onnx(out, {"ref": ref.numpy(), "vp": vp.numpy()}, vpe_np, "visual")
    if not ok:
        sys.exit("视觉编码器导出不忠实")

    # Q 维动态：喂两个掩码应当得到 (1,2,512)
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    sess = ort.InferenceSession(str(out), so, providers=["CPUExecutionProvider"])
    vp2 = np.concatenate([vp.numpy(), np.roll(vp.numpy(), 8, axis=2)], axis=1)
    g2 = sess.run(["vpe"], {"ref": ref.numpy(), "vp": vp2})[0]
    print(f"  {'✅' if g2.shape == (1, 2, EMBED) else '❌'} Q 动态：喂 2 个掩码 → {g2.shape}")
    return out


# --------------------------------------------------------------------------- #
# 3) 文本提示头（reprta + 归一化）
# --------------------------------------------------------------------------- #
def export_text(name: str) -> Path:
    """只导出 YOLOE 的 `reprta` 提示头（feat → pe）。

    文本塔（MobileCLIP2-B）**不导 ONNX**：ultralytics 给的 `mobileclip2_b.ts` 里嵌的
    TorchScript 图**缺张量秩信息**，legacy 导出器在 multi_head_attention 的
    `unflatten(-1, [3, size])` 上抛 "transpose for tensor of unknown rank"；
    换 torch 2.4 的 dynamo 导出器（onnxscript 0.1.0）同样失败，把 batch 改静态也没用。
    Rust 侧改用 `mobileclip2-b-rs`（纯 Rust，已对官方 Python/OpenCLIP 参考做过数值
    对齐：COCO-80 类名 80x512 最大差 5.2e-7、余弦 1.000000），这里只导出提示头。

    运行时链路：词 → mobileclip2-b-rs 分词+编码 → feat (N,512) → 本 ONNX → pe (1,N,512)
    """
    import torch
    import torch.nn as nn
    import torch.nn.functional as F
    import clip as clip_pkg

    m = _load_yoloe("n")
    model = m.model.float().eval()
    head = model.model[-1]
    print("[text] 只导出 YOLOE 提示头 reprta（文本塔交给 Rust 的 mobileclip2-b-rs）")

    class TextHead(nn.Module):
        """feat (N,512) → pe (1,N,512)，等价于 head.get_tpe(feat.unsqueeze(0))。"""

        def __init__(self, rt) -> None:
            super().__init__()
            self.rt = rt

        def forward(self, feat):
            return F.normalize(self.rt(feat.unsqueeze(0)), dim=-1, p=2)

    te = TextHead(head.reprta).eval()
    texts = ["person", "bus", "traffic light"]
    ids = clip_pkg.clip.tokenize(texts, truncate=True)

    # 验收用的 feat 走官方链路取：TorchScript 文本塔 → feat。
    # （get_text_pe = head.get_tpe(enc(tokenize(text)))，与本文件的 te(enc(ids)) 同构）
    text_model = getattr(model, "text_model", "mobileclip2:b")
    base, ver = text_model.split(":")
    ts_name = f"mobileclip2_{ver}.ts" if base == "mobileclip2" else "mobileclip_blt.ts"
    ts_path = WEIGHTS_CACHE / ts_name
    feat_ref = None
    if ts_path.exists():
        ascii_root = Path("D:/tmp-ocrstudio")
        ascii_root.mkdir(parents=True, exist_ok=True)
        tmpdir = Path(tempfile.mkdtemp(prefix="mc_", dir=str(ascii_root)))
        a = tmpdir / ts_name
        shutil.copy2(ts_path, a)  # torch.jit.load 走 C++ fopen，读不了中文路径
        try:
            enc = torch.jit.load(str(a), map_location="cpu").eval()
            with torch.no_grad():
                feat_ref = enc(ids)
            print(f"  ✅ 官方文本塔参照 feat {tuple(feat_ref.shape)}（{ts_name}）")
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)
    else:
        print(f"  ⚠️ 没有 {ts_name}，跳过与官方链路对照，只用随机 feat 做形状/归一化自检")

    if feat_ref is None:
        feat_ref = torch.randn(len(texts), EMBED)

    with torch.no_grad():
        pe = te(feat_ref)
    print(f"  torch pe {tuple(pe.shape)} 行范数 {pe[0].norm(dim=-1).tolist()}")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / f"{name}.onnx"
    with torch.no_grad():
        torch.onnx.export(
            te, (feat_ref,), str(out),
            input_names=["feat"], output_names=["pe"],
            dynamic_axes={"feat": {0: "n"}, "pe": {1: "n"}},
            opset_version=13, do_constant_folding=True,
        )
    _report(out)
    if not _verify_onnx(out, {"feat": feat_ref.numpy()}, pe.numpy(), "text-head"):
        sys.exit("提示头导出不忠实")

    (OUT_DIR / f"{name}.json").write_text(
        json.dumps(
            {
                "kind": "yoloe-text-head",
                "embed": EMBED,
                "tower": {
                    "crate": "mobileclip2-b-rs",
                    "weights": ts_name.replace(".ts", ".safetensors"),
                    "tokenizer": "tokenizer.min.json",
                },
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    return out




# --------------------------------------------------------------------------- #
def main() -> None:
    ap = argparse.ArgumentParser(description="导出 YOLOE 运行时提示三件套")
    ap.add_argument("mode", choices=["all", "detector", "visual", "text"])
    ap.add_argument("--size", default="n", choices=["n", "s", "m", "l", "x"])
    ap.add_argument("--imgsz", type=int, default=IMGSZ)
    args = ap.parse_args()

    if args.mode in ("all", "detector"):
        export_detector(args.size, args.imgsz, f"yoloe26{args.size}_prompt")
    if args.mode in ("all", "visual"):
        export_visual(args.size, args.imgsz, f"yoloe26{args.size}_vpe")
    if args.mode in ("all", "text"):
        export_text(f"yoloe26{args.size}_texthead")
    if args.mode == "all":
        print("\n[e2e] 端到端验收（参考图 → vpe → 检测器）")
        if not verify_end_to_end(args.size, args.imgsz):
            sys.exit("端到端验收未通过")

    print(f"\n[OK] 产物在 {OUT_DIR}")


if __name__ == "__main__":
    main()
