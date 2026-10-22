#!/usr/bin/env python3
"""探针：验证「不烘焙提示、把 pe 当图输入」的 YOLOE ONNX 导出配方是否成立。

要回答三个问题：
  1. 包装成 forward(x, pe) 后能否导出成双输入 ONNX？
  2. 用同一份 pe 跑 ONNX，结果是否与 torch 逐值一致？
  3. 换一份 pe，输出是否随之改变（证明提示真的是运行时生效）？

已知坑（本轮实测）：`Detect.get_topk_index` 里
    idx = ori_index[torch.arange(batch_size)[..., None], index // nc]
是「两个索引张量」的高级索引，torch.onnx 会警告并**错误地** trace 成 Gather，
结果每行都取到同一个 anchor（框全一样）。改成 torch.gather 就忠实了。

跑法：python scripts/_probe_runtime_pe.py
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
os.environ.setdefault("YOLO_CONFIG_DIR", str(ROOT / ".ultralytics"))
WEIGHTS = ROOT / "models" / "_fixtures" / "weights"
TMP = ROOT / "models" / "_fixtures" / "_probe"
TMP.mkdir(parents=True, exist_ok=True)


def patch_topk_for_export() -> None:
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
        ori_index = scores.max(dim=-1)[0].topk(k)[1].unsqueeze(-1)  # (B,k,1)
        scores = scores.gather(dim=1, index=ori_index.repeat(1, 1, nc))
        scores, index = scores.flatten(1).topk(k)
        # 原写法 ori_index[arange(B)[...,None], index // nc] 会被错误 trace；
        # gather 是 GatherElements，语义等价且忠实。
        idx = torch.gather(ori_index.squeeze(-1), 1, index // nc).unsqueeze(-1)
        return scores[..., None], (index % nc)[..., None].float(), idx

    _get_topk_index._trace_patched = True
    Detect.get_topk_index = _get_topk_index


def main() -> None:
    import torch
    import torch.nn as nn
    from ultralytics import YOLOE
    from ultralytics.nn.modules.head import YOLOEDetect

    patch_topk_for_export()

    os.chdir(WEIGHTS)
    m = YOLOE("yoloe-26n-seg.pt")
    os.chdir(ROOT)
    model = m.model.float().eval()
    head = model.model[-1]
    assert isinstance(head, YOLOEDetect)
    print(f"[0] is_fused={head.is_fused} end2end={getattr(head, 'end2end', None)} "
          f"embed={head.embed} reprta={type(head.reprta).__name__}")

    for mm in model.modules():
        mm.export = True

    class Wrap(nn.Module):
        """把 pe 暴露成第二输入。ndim==3 时 get_vpe 直接透传，等价于直接喂 cls_pe。"""

        def __init__(self, inner: nn.Module) -> None:
            super().__init__()
            self.inner = inner

        def forward(self, x, pe):
            return self.inner(x, vpe=pe)

    wrapped = Wrap(model).eval()

    # pe 固定 80 槽（模型原生容量）：真实提示词填前 K 行，其余补 0。
    # head 内部 F.normalize(w)，零行归一化成 0 → 分数恒为 bias(-10) → sigmoid≈4.5e-5，
    # 天然低于任何置信度阈值；且输出形状仍是 [1,300,6+nm]，与烘焙版一模一样。
    NC = 80
    rng = np.random.default_rng(42)
    pe0 = np.zeros((1, NC, 512), dtype=np.float32)
    k_real = 3
    pe0[0, :k_real] = rng.standard_normal((k_real, 512))
    pe0 /= np.maximum(np.linalg.norm(pe0, axis=-1, keepdims=True), 1e-12)
    pe_t = torch.from_numpy(pe0)
    print(f"[1] pe shape={tuple(pe_t.shape)}（前 {k_real} 行是真提示，其余补 0）")

    img_path = ROOT / "assets" / "bus.png"
    if not img_path.exists():
        sys.exit(f"找不到测试图 {img_path}")
    print(f"[1] 测试图 {img_path}")

    import cv2
    from ultralytics.data.augment import LetterBox

    im0 = cv2.imread(str(img_path))
    lb = LetterBox((640, 640), auto=False, stride=32)
    im = lb(image=im0)
    x = torch.from_numpy(np.ascontiguousarray(im.transpose(2, 0, 1)[None])).float() / 255.0
    print(f"[1] 输入张量 {tuple(x.shape)}")

    with torch.no_grad():
        y_torch = wrapped(x, pe_t)
    proto_t = None
    if isinstance(y_torch, tuple):
        print(f"[2] torch 返回 tuple，元素形状 "
              f"{[tuple(t.shape) if hasattr(t, 'shape') else type(t).__name__ for t in y_torch]}")
        if len(y_torch) > 1 and hasattr(y_torch[1], "shape"):
            proto_t = y_torch[1].numpy()
        y_torch = y_torch[0]
    ref = y_torch.numpy()
    print(f"[2] torch 检测输出 shape={ref.shape}  proto shape={None if proto_t is None else proto_t.shape}")

    import onnx
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3

    best = None
    for opset in (12, 13, 17):
        onnx_path = TMP / f"yoloe26n_runtime_pe_o{opset}.onnx"
        with torch.no_grad():
            torch.onnx.export(
                wrapped, (x, pe_t), str(onnx_path),
                input_names=["images", "pe"], output_names=["output0", "proto"],
                opset_version=opset, do_constant_folding=True,
            )
        mo = onnx.load(str(onnx_path))
        ins = [i.name for i in mo.graph.input]
        outs = [o.name for o in mo.graph.output]
        sess = ort.InferenceSession(str(onnx_path), so, providers=["CPUExecutionProvider"])
        got = sess.run(["output0"], {"images": x.numpy(), "pe": pe0})[0]
        # proto 是纯卷积输出，没有任何 topk/索引 —— 用它判断"骨干是否忠实"
        if proto_t is not None:
            gp = sess.run(["proto"], {"images": x.numpy(), "pe": pe0})[0]
            if gp.shape == proto_t.shape:
                dp = np.abs(gp - proto_t)
                print(f"[3a] opset {opset}: proto 最大绝对差 {dp.max():.3e}  "
                      f"{'✅ 骨干忠实' if dp.max() < 1e-3 else '❌ 骨干就不忠实'}")
        if got.shape != ref.shape:
            print(f"[3] opset {opset}: 形状不一致 ort{got.shape} vs torch{ref.shape}")
            continue
        d = np.abs(got - ref)
        ok_raw = d.max() < 1e-3
        print(f"[3] opset {opset}: 输入 {ins} 输出 {outs} | "
              f"逐值最大差 {d.max():.3e} 均值 {d.mean():.3e}  {'✅ 逐值一致' if ok_raw else '（逐值不一致，见下）'}")

        # 逐值不一致是 topk 尾部并列行的排序差 —— 用「置信度过滤 + IoU 匹配」做真实验收
        if not ok_raw:
            fa, fb = got[0], ref[0]
            for thr in (0.25, 0.05, 0.01):
                a = fa[fa[:, 4] > thr]
                b = fb[fb[:, 4] > thr]
                if len(a) == 0 and len(b) == 0:
                    print(f"      conf>{thr}: 两边都空")
                    continue
                if len(a) != len(b):
                    print(f"      conf>{thr}: 数量不一致 ort {len(a)} vs torch {len(b)} ❌")
                    continue
                # 贪心 IoU 匹配
                ious = []
                used = set()
                for row in a:
                    best_i, best_v = -1, -1.0
                    for j, cand in enumerate(b):
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
                ious = np.array(ious)
                sc = np.abs(np.sort(a[:, 4]) - np.sort(b[:, 4])).max()
                good = ious.min() > 0.99 and sc < 1e-5
                print(f"      conf>{thr}: {len(a)} 框，IoU 最小 {ious.min():.4f}，"
                      f"分数最大差 {sc:.2e}  {'✅ 真实检测一致' if good else '❌'}")
                if good:
                    ok_raw = True
                    break

        if ok_raw and best is None:
            best = (opset, sess, got, onnx_path)

    if best is None:
        sys.exit("\n❌ 没有任何 opset 导出忠实，配方不成立")
    opset, sess, got, onnx_path = best

    # 换提示：把真提示行换掉，输出应当变
    pe1 = pe0.copy()
    pe1[0, :k_real] = rng.standard_normal((k_real, 512))
    pe1 /= np.maximum(np.linalg.norm(pe1, axis=-1, keepdims=True), 1e-12)
    got1 = sess.run(["output0"], {"images": x.numpy(), "pe": pe1})[0]
    delta = np.abs(got1 - got).max()
    print(f"[4] 换 pe 后输出最大变化 {delta:.3e}  {'✅ 提示运行时生效' if delta > 1e-3 else '❌ 提示没生效'}")

    # 补零槽位不该冒出检测：看类别下标是否都落在 [0, k_real)
    cls_idx = got[0, :, 5].astype(int)
    print(f"[5] 类别下标分布：min={cls_idx.min()} max={cls_idx.max()} "
          f"（max 应 < {k_real}，即补零槽位不会中选）")
    top = got[0, np.argsort(-got[0, :, 4])[:5]]
    print(f"[5] 置信度最高的 5 行类别下标 {top[:, 5].astype(int).tolist()} "
          f"分数 {np.array2string(top[:, 4], precision=6)}")

    print(f"\n[OK] 探针完成：opset {opset} 配方成立，产物 {onnx_path}")


if __name__ == "__main__":
    main()
