"""打印 Python 侧 det 后处理的全精度框，与 Rust 的 pts 直接对比。

Rust pipeline_match 的调试输出（OCR_DEBUG_REC=1）给出：
  #5 pts=[(49.044, 362.258), (382.768, 360.003), (382.994, 393.383), (49.270, 395.638)]
  #6 pts=[(445.271, 364.271), (477.729, 364.271), (477.729, 394.729), (445.271, 394.729)]
本脚本用同一张图、同一个 det.onnx（DmlExecutionProvider，与 Rust 一致）复现。
"""
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import ref_pipeline as ref  # noqa: E402


def main():
    model_dir = ROOT / "models" / "small"
    img = ref.imread_unicode(ROOT / "assets" / "test_zh.png")
    print(f"图像: {img.shape[1]}x{img.shape[0]}")

    so = ort.SessionOptions()
    so.log_severity_level = 3
    providers = ["DmlExecutionProvider", "CPUExecutionProvider"]
    det = ort.InferenceSession(str(model_dir / "det.onnx"), so, providers=providers)
    print(f"det provider: {det.get_providers()[0]}")

    x, ratio = ref.det_preprocess(img)
    print(f"det 输入 shape: {x.shape}  ratio: {ratio:.6f}")
    prob = det.run(None, {det.get_inputs()[0].name: x})[0]
    print(f"det 输出 shape: {prob.shape}")

    boxes = ref.db_postprocess(prob, ratio)
    print(f"框数: {len(boxes)}")

    # db_postprocess 内部会取整，这里内联复刻以保留全精度
    prob_map = prob[0, 0]
    height, width = prob_map.shape
    import cv2
    bitmap = (prob_map > ref.DET_DB_THRESH).astype(np.uint8)
    contours, _ = cv2.findContours((bitmap * 255).astype(np.uint8),
                                   cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
    raw = []
    for i in range(min(len(contours), 1000)):
        contour = contours[i]
        box, sside = ref.get_mini_boxes(contour)
        if box is None or sside < ref.MIN_BOX_SIZE:
            continue
        score = ref.box_score(prob_map, box)
        if score < ref.DET_DB_BOX_THRESH:
            continue
        expanded = ref.unclip(box, ref.DET_DB_UNCLIP_RATIO)
        if expanded is None:
            continue
        box2, sside2 = ref.get_mini_boxes(expanded)
        if box2 is None or sside2 < ref.MIN_BOX_SIZE + 2:
            continue
        b = np.array(box2, dtype=np.float64)
        b[:, 0] = np.clip(b[:, 0], 0, width - 1)
        b[:, 1] = np.clip(b[:, 1], 0, height - 1)
        raw.append((b / ratio, score))
    raw = ref.sorted_boxes(raw)

    # 第二轮：打印每个轮廓的中间量（mini box / 距离 / 外扩框）
    from shapely.geometry import Polygon
    print("\n=== 中间量（按轮廓顺序）===")
    for i in range(min(len(contours), 1000)):
        contour = contours[i]
        box, sside = ref.get_mini_boxes(contour)
        if box is None or sside < ref.MIN_BOX_SIZE:
            continue
        score = ref.box_score(prob_map, box)
        if score < ref.DET_DB_BOX_THRESH:
            continue
        poly = Polygon(box)
        distance = poly.area * ref.DET_DB_UNCLIP_RATIO / poly.length
        exp = ref.unclip(box, ref.DET_DB_UNCLIP_RATIO)
        b2, s2 = ref.get_mini_boxes(exp)
        print(f"contour#{i}: 轮廓点数={len(contour)} sside={sside}")
        print(f"   mini box = {[tuple(round(float(v), 4) for v in p) for p in box]}")
        print(f"   area={poly.area:.6f} length={poly.length:.6f} distance={distance:.6f}")
        print(f"   expanded 点数={len(exp)}  mini box2 = {[tuple(round(float(v), 4) for v in p) for p in b2]} sside2={s2}")

    for i, (b, s) in enumerate(raw):
        pts = np.array(b, dtype=np.float64)
        print(f"#{i} score={s:.6f}")
        for p in pts:
            print(f"    ({p[0]:.6f}, {p[1]:.6f})")
        w = np.hypot(pts[2][0] - pts[3][0], pts[2][1] - pts[3][1])
        h = np.hypot(pts[1][0] - pts[2][0], pts[1][1] - pts[2][1])
        print(f"    width={w:.4f} -> int {int(w)}   height={h:.4f} -> int {int(h)}")


if __name__ == "__main__":
    main()
