#!/usr/bin/env python3
"""生成「运行时提示」的对照基准 `models/yoloe/reference_prompt.json`。

给谁用
------
`src-tauri/tests/yoloe_prompt_parity.rs`。它跑 Rust 侧的 `PromptEngine`，把
文本提示向量 `pe`、视觉提示向量 `pe`、以及端到端检测框与本文件比对。

为什么必须有一份 Python 侧的基准
--------------------------------
导出脚本只证明了 **ONNX 与 torch 一致**（1e-7 量级）。但运行时链路上还有两段
是 Rust 独有的、导出脚本碰不到的：

1. **文本塔**：`mobileclip2-b-rs`（纯 Rust）替掉了 Python 的 `mobileclip2_b.ts`。
   这是第三方 crate，它算得对不对只能拿官方 TorchScript 的输出比。
2. **提示组装**：分词、`pad_pe` 补零、掩码栅格化、letterbox 搬运坐标——
   任何一处差一点，`pe` 就变，而 `pe` 变了对用户就是「同样的词/框找不到东西」，
   **不会有任何报错**。这类错只能靠逐值比对钉死。

所以本文件记录的是**中间量**（`feat` / `vpe` / `pe`），而不只是最终框：
只看框的话，一个错的 `pe` 也可能碰巧检出相似的东西，掩盖问题。

用法
----
  python scripts/ref_yoloe_prompt.py

产物约 200 KB（文本 4 词 × 512 + 视觉 1 槽 × 512 + 框），可以进仓库。
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

# 复用导出脚本里的复刻件，避免两处实现漂移
from export_yoloe_prompt import (  # noqa: E402
    ASSETS,
    EMBED,
    IMGSZ,
    NC_SLOTS,
    OUT_DIR,
    _imread_any,
    _letterbox_tensor,
    _prep_model,
)

REF_PATH = OUT_DIR / "reference_prompt.json"

# 参照词：两个 COCO 常见类 + 一个多词 + 一个中文。
# 中文不是为了「支持中文」——MobileCLIP2 是英文词表，中文只会被拆成一堆字节
# token；留一条是为了确认这条路**不会崩**，且 Rust 与 Python 结果一致。
TEXT_WORDS = ["person", "bus", "traffic light", "确认按钮"]

# 视觉提示：bus.png 里的一个人（与导出脚本 verify_end_to_end 用的是同一个框）
VISUAL_IMAGE = "bus.png"
VISUAL_BOXES = [[31, 230, 801, 775]]
VISUAL_CLASSES = [0]

# 端到端：拿语言提示去 zidane.png 里找人。conf 取很低是为了让框多一点，
# 匹配样本更有说服力（阈值附近框太少的话，全错过也看不出来）。
E2E_WORDS = ["person"]
E2E_IMAGE = "zidane.png"
E2E_CONF = 0.05


def _round(a: np.ndarray, digits: int = 7) -> list:
    return np.round(np.asarray(a, dtype=np.float64), digits).tolist()


def _load_ts_tower(name: str = "mobileclip2_b.ts"):
    """加载官方 TorchScript 文本塔。**必须先复制到 ASCII 路径**（fopen 读不了中文）。"""
    import shutil
    import tempfile

    import torch

    src = Path(__file__).resolve().parent.parent / "models" / "_fixtures" / "weights" / name
    if not src.is_file():
        sys.exit(f"找不到官方文本塔 {src}")
    root = Path("D:/tmp-ocrstudio")
    root.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix="mc_", dir=str(root)))
    dst = tmp / name
    shutil.copy2(src, dst)
    try:
        return torch.jit.load(str(dst), map_location="cpu").eval()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> None:
    import clip as clip_pkg
    import torch
    from ultralytics.data.augment import LoadVisualPrompt

    # ---------------------------------------------------------------- 文本
    print("[ref] 文本提示")
    # 必须走 `_prep_model`：它会把 `export=True` 铺到所有子模块。少了这步，
    # head 返回的是训练态的三层特征图元组，而不是端到端的 (1,300,38)。
    _, model = _prep_model("n")
    head = model.model[-1]
    tower = _load_ts_tower()

    ids = clip_pkg.clip.tokenize(TEXT_WORDS, truncate=True)
    with torch.no_grad():
        feat = tower(ids)                       # (N,512) 官方文本塔输出
        pe_t = head.get_tpe(feat.unsqueeze(0))  # (1,N,512) reprta + 归一化
    feat_np = feat.numpy()
    pe_np = pe_t.numpy()
    print(f"  feat {feat_np.shape}  pe {pe_np.shape}  行范数 "
          f"{np.linalg.norm(pe_np[0], axis=-1).round(6).tolist()}")

    # ---------------------------------------------------------------- 视觉
    print("[ref] 视觉提示")
    im0 = _imread_any(ASSETS / VISUAL_IMAGE)
    if im0 is None:
        sys.exit(f"读不了 {ASSETS / VISUAL_IMAGE}")
    h0, w0 = im0.shape[:2]
    ref = _letterbox_tensor(im0, IMGSZ)

    gain = min(IMGSZ / h0, IMGSZ / w0)
    boxes = np.array(VISUAL_BOXES, dtype=np.float32) * gain
    boxes[..., 0::2] += round((IMGSZ - w0 * gain) / 2 - 0.1)
    boxes[..., 1::2] += round((IMGSZ - h0 * gain) / 2 - 0.1)
    vp = LoadVisualPrompt().get_visuals(
        np.array(VISUAL_CLASSES), (IMGSZ, IMGSZ), boxes, None
    ).unsqueeze(0)
    with torch.no_grad():
        vpe_t = model(ref, vpe=vp, return_vpe=True)  # (1,Q,512)
    vpe_np = vpe_t.numpy()
    q = vpe_np.shape[1]
    print(f"  掩码 {tuple(vp.shape)} 置位 {int(vp.sum())} 像素 → vpe {vpe_np.shape}")

    # 只存 Q 行：Rust 侧的 `visual_pe` 会把它补零到 80 槽，尾部全 0 是**约定**
    # 而不是数值，测试里单独断言，不必进基准文件。

    # ---------------------------------------------------------------- 端到端
    print("[ref] 端到端（语言提示找人）")
    e2e_ids = clip_pkg.clip.tokenize(E2E_WORDS, truncate=True)
    with torch.no_grad():
        e2e_feat = tower(e2e_ids)
        e2e_pe = head.get_tpe(e2e_feat.unsqueeze(0)).numpy()

    im1 = _imread_any(ASSETS / E2E_IMAGE)
    if im1 is None:
        sys.exit(f"读不了 {ASSETS / E2E_IMAGE}")
    ref1 = _letterbox_tensor(im1, IMGSZ)
    with torch.no_grad():
        y = model(ref1, vpe=torch.from_numpy(e2e_pe))
    y = (y[0] if isinstance(y, tuple) else y).numpy()[0]  # (300, 6+nm)

    # end2end head 给的坐标是 **letterbox 后的 640 空间**，必须换回原图坐标才和
    # Rust 的 `detect()` 可比（那边输出的是原图像素）。用 ultralytics 自己的
    # `scale_boxes`，语义与 Rust 的 letterbox 反算一致。
    from ultralytics.utils.ops import scale_boxes

    # 列语义是 `x1,y1,x2,y2,score,cls`——**不是** `cx,cy,w,h`。
    # 这是端到端导出与普通 raw head 最容易搞混的一处：raw head 是
    # `[1,C,N]` 且前 4 行是 `cx,cy,w,h`；end2end 头是 `[1,N,6+nm]` 且已经是角点。
    # 当成 cxcywh 去解，框会整体偏掉（实测 IoU 只有 0.39），而分数一模一样，
    # 很容易误判成「解码没错，只是框有偏差」。
    keep = y[:, 4] > E2E_CONF
    scaled = scale_boxes((IMGSZ, IMGSZ), y[keep, :4].copy(), (im1.shape[0], im1.shape[1]))

    dets = []
    for row, box in zip(y[keep], scaled):
        cid = int(row[5])
        dets.append(
            {
                "class_id": cid,
                "class_name": E2E_WORDS[cid] if cid < len(E2E_WORDS) else "",
                "score": float(row[4]),
                "xyxy": [float(v) for v in box],
            }
        )
    dets.sort(key=lambda d: -d["score"])
    print(f"  conf>{E2E_CONF} 共 {len(dets)} 框，最高 {dets[0]['score']:.4f}"
          if dets else "  ⚠️ 一个框都没有，参考值没意义")

    payload = {
        "_note": "由 scripts/ref_yoloe_prompt.py 生成；给 tests/yoloe_prompt_parity.rs 用",
        "imgsz": IMGSZ,
        "nc_slots": NC_SLOTS,
        "embed": EMBED,
        "text": {
            "words": TEXT_WORDS,
            "feat": _round(feat_np),
            "pe": _round(pe_np[0]),
        },
        "visual": {
            "image": VISUAL_IMAGE,
            "width": w0,
            "height": h0,
            "boxes": VISUAL_BOXES,
            "classes": VISUAL_CLASSES,
            "names": ["参考目标"],
            "mask_set_pixels": int(vp.sum()),
            "q": int(q),
            "vpe": _round(vpe_np[0]),
        },
        "end2end": {
            "image": E2E_IMAGE,
            "width": im1.shape[1],
            "height": im1.shape[0],
            "words": E2E_WORDS,
            "conf": E2E_CONF,
            "detections": dets,
        },
    }

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    REF_PATH.write_text(json.dumps(payload, ensure_ascii=False, indent=1) + "\n",
                        encoding="utf-8")
    print(f"\n✅ 已写出 {REF_PATH}（{REF_PATH.stat().st_size / 1024:.0f} KB）")


if __name__ == "__main__":
    main()
