"""Dump rec 输入张量，用于定位 Rust 与 Python 的像素级差异。

用法:
    python dump_rec_tensor.py models/small assets/test_zh.png models/small/dump

产出:
    <out>/box_<i>_src.png     裁剪后的原图（透视变换结果）
    <out>/box_<i>_rec.f32     rec 预处理后的张量（raw little-endian f32）
    <out>/meta.json           每个框的形状信息
"""

from __future__ import annotations

import json
import struct
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import ref_pipeline as ref  # noqa: E402


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    model_dir = ROOT / (args[0] if args else "models/small")
    img_path = ROOT / (args[1] if len(args) > 1 else "assets/test_zh.png")
    out_dir = ROOT / (args[2] if len(args) > 2 else "models/small/dump")
    out_dir.mkdir(parents=True, exist_ok=True)

    img = ref.imread_unicode(img_path)

    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    det_sess = ort.InferenceSession(
        str(model_dir / "det.onnx"), so,
        providers=["CPUExecutionProvider"])

    x, ratio = ref.det_preprocess(img)
    prob = det_sess.run(None, {det_sess.get_inputs()[0].name: x})[0]
    boxes = ref.db_postprocess(prob, ratio)

    meta = []
    for i, b in enumerate(boxes):
        crop = ref.crop_by_box(img, np.array(b["box"], dtype=np.float32))
        if crop is None:
            continue
        # 存裁剪原图（Rust 侧用同一张图重放）
        ok, buf = cv2.imencode(".png", crop)
        assert ok
        (out_dir / f"box_{i}_src.png").write_bytes(buf.tobytes())

        tensor, rw = ref.rec_preprocess(crop)
        flat = tensor.reshape(-1).astype(np.float32)
        (out_dir / f"box_{i}_rec.f32").write_bytes(struct.pack(f"<{flat.size}f", *flat))

        meta.append({
            "index": i,
            "crop_shape": list(crop.shape),
            "rec_shape": list(tensor.shape),
            "valid_width": rw,
            "box": b["box"],
        })
        print(f"box {i}: crop={crop.shape} rec={tensor.shape} w={rw}")

    (out_dir / "meta.json").write_text(
        json.dumps(meta, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"已写出到 {out_dir}")


if __name__ == "__main__":
    main()
