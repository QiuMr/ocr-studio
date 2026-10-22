#!/usr/bin/env python3
"""探针：视觉提示编码器（参考图 + 提示掩码 → vpe）能否导出成 ONNX。

链路（对照 ultralytics 的 YOLOEVPSegPredictor）：
    ref  = letterbox(参考图, 640)                       # (1,3,640,640)
    vp   = LoadVisualPrompt.get_visuals(...)            # (1,Q,80,80) 二值掩码，1/8 尺度
    vpe  = model(ref, vpe=vp, return_vpe=True)          # 骨干 → savpe → (1,Q,512)

要验证：
  1. 双输入（ref + vp）能否导出；
  2. Q 维能否动态；
  3. ONNX 与 torch 逐值一致。
"""
from __future__ import annotations

import os
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
os.environ.setdefault("YOLO_CONFIG_DIR", str(ROOT / ".ultralytics"))
WEIGHTS = ROOT / "models" / "_fixtures" / "weights"
TMP = ROOT / "models" / "_fixtures" / "_probe"
TMP.mkdir(parents=True, exist_ok=True)


def main() -> None:
    import torch
    import torch.nn as nn
    import cv2
    from ultralytics import YOLOE
    from ultralytics.data.augment import LetterBox, LoadVisualPrompt

    os.chdir(WEIGHTS)
    m = YOLOE("yoloe-26n-seg.pt")
    os.chdir(ROOT)
    model = m.model.float().eval()
    for mm in model.modules():
        mm.export = True

    class VWrap(nn.Module):
        """ref + vp → vpe。"""

        def __init__(self, inner: nn.Module) -> None:
            super().__init__()
            self.inner = inner

        def forward(self, ref, vp):
            return self.inner(ref, vpe=vp, return_vpe=True)

    wrapped = VWrap(model).eval()

    img_path = ROOT / "assets" / "bus.png"
    im0 = cv2.imread(str(img_path))
    lb = LetterBox((640, 640), auto=False, stride=32)
    im = lb(image=im0)
    ref = torch.from_numpy(np.ascontiguousarray(im.transpose(2, 0, 1)[None])).float() / 255.0
    h0, w0 = im0.shape[:2]
    print(f"[1] 参考图 {w0}x{h0} → letterbox {tuple(ref.shape)}")

    # 按 _process_single_image 的口径把框搬到 640 空间（含 round(-0.1) 偏移）
    gain = min(640 / h0, 640 / w0)
    boxes = np.array([[31, 230, 801, 775]], dtype=np.float32)
    boxes *= gain
    boxes[..., 0::2] += round((640 - w0 * gain) / 2 - 0.1)
    boxes[..., 1::2] += round((640 - h0 * gain) / 2 - 0.1)
    print(f"[1] 640 空间框 {boxes.tolist()}  gain={gain:.4f}")

    vp = LoadVisualPrompt().get_visuals(np.array([0]), (640, 640), boxes, None)
    vp = vp.unsqueeze(0)
    print(f"[2] 提示掩码 {tuple(vp.shape)} 置位像素 {int(vp.sum())}")

    with torch.no_grad():
        vpe = wrapped(ref, vp)
    print(f"[3] torch vpe {tuple(vpe.shape)} 行范数 {vpe[0].norm(dim=-1).tolist()}")

    import onnx
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3

    # 先试动态 Q
    for dyn in (True, False):
        out = TMP / f"vpe_{'dyn' if dyn else 'fix'}.onnx"
        da = {"ref": {0: "b"}, "vp": {0: "b", 1: "q"}, "vpe": {0: "b", 1: "q"}} if dyn else {"ref": {0: "b"}}
        try:
            with torch.no_grad():
                torch.onnx.export(
                    wrapped, (ref, vp), str(out),
                    input_names=["ref", "vp"], output_names=["vpe"],
                    dynamic_axes=da, opset_version=12, do_constant_folding=True,
                )
        except Exception as e:  # noqa: BLE001
            print(f"[4] Q {'动态' if dyn else '固定'}: 导出失败 {type(e).__name__}: {str(e)[:120]}")
            continue
        mo = onnx.load(str(out))
        print(f"[4] Q {'动态' if dyn else '固定'}: 输入 "
              f"{[(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in mo.graph.input]}"
              f" 输出 {[(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in mo.graph.output]}")
        sess = ort.InferenceSession(str(out), so, providers=["CPUExecutionProvider"])
        got = sess.run(["vpe"], {"ref": ref.numpy(), "vp": vp.numpy()})[0]
        ok = got.shape == tuple(vpe.shape) and np.abs(got - vpe.numpy()).max() < 1e-4
        print(f"[5] Q {'动态' if dyn else '固定'}: ort {got.shape} vs torch {tuple(vpe.shape)} "
              f"最大差 {np.abs(got - vpe.numpy()).max():.3e} {'✅ 一致' if ok else '❌'}")
        if ok and dyn:
            # 动态 Q：换 2 个提示掩码
            vp2 = np.concatenate([vp.numpy(), np.roll(vp.numpy(), 8, axis=2)], axis=1)
            g2 = sess.run(["vpe"], {"ref": ref.numpy(), "vp": vp2})[0]
            print(f"[6] 动态 Q 喂 2 个掩码 → {g2.shape}（应为 (1,2,512)）")

    print(f"\n[OK] 探针完成，产物在 {TMP}")


if __name__ == "__main__":
    main()
