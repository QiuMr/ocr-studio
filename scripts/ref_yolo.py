"""YOLO 逐框比对的 Python 参考：用本机 ultralytics 跑导出的 onnx，输出 JSON。

为什么用 onnx 而不是 .pt 做参考：Rust 侧加载的也是这份 onnx，
这样比对差异只可能来自我们自己的预处理/解码/NMS，不会混进导出误差。

默认阈值 conf=0.25 / iou=0.7 / max_det=300，与 ultralytics predict 一致，
也与 Rust `DetectOptions::default()` 一致。

用法：
    python scripts/ref_yolo.py <model.onnx> <image> <out.json> [conf] [iou]
"""

import json
import sys

import cv2
import numpy as np


def main() -> int:
    from ultralytics import YOLO

    model_path, image_path, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    conf = float(sys.argv[4]) if len(sys.argv) > 4 else 0.25
    iou = float(sys.argv[5]) if len(sys.argv) > 5 else 0.7

    model = YOLO(model_path, task="detect")
    r = model.predict(
        image_path,
        imgsz=640,
        conf=conf,
        iou=iou,
        max_det=300,
        device="cpu",
        verbose=False,
        half=False,
    )[0]

    h, w = r.orig_shape
    names = r.names
    boxes = []
    for b in r.boxes:
        x1, y1, x2, y2 = (float(v) for v in b.xyxy[0].tolist())
        boxes.append(
            {
                "class_id": int(b.cls.item()),
                "class_name": names[int(b.cls.item())],
                "score": float(b.conf.item()),
                "xyxy": [x1, y1, x2, y2],
            }
        )

    payload = {
        "image": image_path.replace("\\", "/"),
        "width": w,
        "height": h,
        "conf": conf,
        "iou": iou,
        "boxes": boxes,
    }
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False, indent=1)
    print(f"[INFO] {len(boxes)} boxes -> {out_path}")
    for b in boxes:
        print(
            f"  {b['class_name']:<14} {b['score']:.4f}  "
            + " ".join(f"{v:8.2f}" for v in b["xyxy"])
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
