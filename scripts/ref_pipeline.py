"""PP-OCRv6 参考实现（Python + onnxruntime）。

用途：
1. 作为 Rust 实现的**精度基线**（黄金样本）——同一张图，Python 与 Rust 输出必须一致。
2. 验证字典映射（rec 输出 18710 类 vs 字典 18708 字符）。
3. 产出测试图与期望结果，供 Rust 单测使用。

用法:
    python ref_pipeline.py models/small assets/test_zh.png

对齐约定（与 PaddleOCR / RapidOCR 一致）：
    det_db_thresh     = 0.3   二值化阈值
    det_db_box_thresh = 0.6   框得分阈值
    det_db_unclip_ratio = 1.5 unclip 膨胀比
    use_dilation      = False
    min_box_size      = 3
    rec 输入          = (3, 48, 320)，右侧补零
    CTC               = blank 索引 0，字符 = 索引 - 1
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort
import pyclipper
from shapely.geometry import Polygon

# --- 环境兼容补丁（仅本参考脚本）------------------------------------------
# 本机 onnxruntime 处于混装状态：原生 DLL/pyd 来自 onnxruntime-directml 1.24.3，
# 而 Python 封装源码是 1.31.0。新版封装在 run() 里会调用
# InferenceSession.is_webgpu_graph_capture_enabled()，1.24.3 的 pyd 没有该方法，
# 导致任何推理都 AttributeError。我们不使用 WebGPU，该校验无意义，直接短路。
# 这是本地参考工具，不进入 Rust 交付代码。
if not hasattr(ort.InferenceSession, "_patched_no_webgpu_check"):
    def _noop(self, run_options=None):  # noqa: ANN001, ANN202
        return None

    ort.InferenceSession._validate_graph_capture_run_api = _noop
    ort.InferenceSession._patched_no_webgpu_check = True
# -------------------------------------------------------------------------

ROOT = Path(__file__).resolve().parent.parent

DET_DB_THRESH = 0.3
DET_DB_BOX_THRESH = 0.6
DET_DB_UNCLIP_RATIO = 1.5
USE_DILATION = False
MIN_BOX_SIZE = 3
DET_LIMIT_SIDE_LEN = 960
REC_IMG_H, REC_IMG_W = 48, 320

DET_MEAN = DET_STD = 0.5
REC_MEAN = REC_STD = 0.5


def imread_unicode(path: Path) -> np.ndarray:
    """cv2.imread 在 Windows 上无法处理非 ASCII 路径，改用 imdecode。"""
    data = np.frombuffer(path.read_bytes(), dtype=np.uint8)
    img = cv2.imdecode(data, cv2.IMREAD_COLOR)
    if img is None:
        raise FileNotFoundError(f"读图失败: {path}")
    return img


# ---------------------------------------------------------------- 预处理
def det_preprocess(img: np.ndarray, limit_side_len: int = DET_LIMIT_SIDE_LEN,
                   limit_type: str = "max"):
    """resize 使长边不超过 limit_side_len，且 H/W 均 32 对齐。"""
    h, w = img.shape[:2]
    if limit_type == "max":
        ratio = 1.0
        if max(h, w) > limit_side_len:
            ratio = limit_side_len / max(h, w)
    else:
        ratio = 1.0
        if min(h, w) < limit_side_len:
            ratio = limit_side_len / min(h, w)

    resize_h = max(int(round(h * ratio / 32) * 32), 32)
    resize_w = max(int(round(w * ratio / 32) * 32), 32)

    resized = cv2.resize(img, (resize_w, resize_h))
    x = (resized.astype(np.float32) / 255.0 - DET_MEAN) / DET_STD
    x = x.transpose(2, 0, 1)[np.newaxis, :]
    return x, ratio


def rec_preprocess(box_img: np.ndarray, img_h: int = REC_IMG_H,
                   img_w: int = REC_IMG_W):
    """按宽高比缩放文本行，高度固定 48，右侧补零到 320。"""
    h, w = box_img.shape[:2]
    ratio = w / float(h)
    if ratio * img_h > img_w:
        resized_w = img_w
    else:
        resized_w = max(int(round(img_h * ratio)), 1)
    resized = cv2.resize(box_img, (resized_w, img_h))
    x = (resized.astype(np.float32) / 255.0 - REC_MEAN) / REC_STD
    x = x.transpose(2, 0, 1)[np.newaxis, :]
    pad = np.zeros((1, 3, img_h, img_w), dtype=np.float32)
    pad[0, :, :, :resized_w] = x
    return pad, resized_w


# ---------------------------------------------------------------- 后处理
def get_mini_boxes(contour):
    """复刻 PaddleOCR get_mini_boxes：最小外接矩形四点有序化。"""
    bounding_box = cv2.minAreaRect(contour)
    points = sorted(list(cv2.boxPoints(bounding_box)), key=lambda p: p[0])
    if len(points) < 4:
        return None, 0
    if points[1][1] > points[0][1]:
        i1, i4 = 0, 1
    else:
        i1, i4 = 1, 0
    if points[3][1] > points[2][1]:
        i2, i3 = 2, 3
    else:
        i2, i3 = 3, 2
    box = [points[i1], points[i2], points[i3], points[i4]]
    return box, min(bounding_box[1])


def box_score(prob_map: np.ndarray, box) -> float:
    """框内平均概率（复刻 PaddleOCR box_score_fast）。"""
    h, w = prob_map.shape[:2]
    mask = np.zeros((h, w), dtype=np.uint8)
    pts = np.round(np.array(box)).astype(np.int32)
    cv2.fillPoly(mask, [pts], 1)
    return float(cv2.mean(prob_map, mask)[0])


def unclip(box, ratio: float):
    """按 area*ratio/perimeter 做多边形外扩。"""
    poly = Polygon(box)
    if (not poly.is_valid) or poly.area <= 0.0 or poly.length <= 0.0:
        return None
    distance = poly.area * ratio / poly.length
    offset = pyclipper.PyclipperOffset()
    offset.AddPath([tuple(float(v) for v in p) for p in box],
                   pyclipper.JT_ROUND, pyclipper.ET_CLOSEDPOLYGON)
    expanded = offset.Execute(distance)
    if not expanded:
        return None
    return np.array(expanded[0]).reshape(-1, 1, 2)


def db_postprocess(prob: np.ndarray, ratio: float,
                   db_thresh: float = DET_DB_THRESH,
                   box_thresh: float = DET_DB_BOX_THRESH,
                   unclip_ratio: float = DET_DB_UNCLIP_RATIO,
                   use_dilation: bool = USE_DILATION,
                   min_box_size: int = MIN_BOX_SIZE):
    """DB 后处理全流程：二值化 -> 轮廓 -> mini box -> 打分 -> unclip -> 还原坐标。"""
    prob_map = prob[0, 0]                       # (H, W) 概率图
    height, width = prob_map.shape

    bitmap = (prob_map > db_thresh).astype(np.uint8)
    if use_dilation:
        bitmap = cv2.dilate(bitmap, np.ones((1, 1), np.uint8))

    contours, _ = cv2.findContours(
        (bitmap * 255).astype(np.uint8),
        cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)

    boxes, scores = [], []
    for i in range(min(len(contours), 1000)):
        contour = contours[i]
        box, sside = get_mini_boxes(contour)
        if box is None or sside < min_box_size:
            continue
        score = box_score(prob_map, box)
        if score < box_thresh:
            continue

        expanded = unclip(box, unclip_ratio)
        if expanded is None:
            continue
        box2, sside2 = get_mini_boxes(expanded)
        if box2 is None or sside2 < min_box_size + 2:
            continue

        b = np.array(box2, dtype=np.float32)
        b[:, 0] = np.clip(b[:, 0], 0, width - 1)
        b[:, 1] = np.clip(b[:, 1], 0, height - 1)
        boxes.append(b)
        scores.append(score)

    if not boxes:
        return []

    # 还原到原图坐标，并按 PaddleOCR sorted_boxes 排阅读顺序
    pairs = [(b / ratio, scores[i]) for i, b in enumerate(boxes)]
    pairs = sorted_boxes(pairs)
    return [{"box": np.round(r).astype(int).tolist(), "score": float(s)}
            for r, s in pairs]


def sorted_boxes(pairs, y_tol: int = 10):
    """复刻 PaddleOCR sorted_boxes：先按 y 总和排序，再对相邻同行的框按 x 冒泡。"""
    pairs = sorted(pairs, key=lambda p: float(np.sum(p[0][:, 1])))
    n = len(pairs)
    for i in range(n - 1):
        for j in range(i, -1, -1):
            if (abs(pairs[j + 1][0][0][1] - pairs[j][0][0][1]) < y_tol
                    and pairs[j + 1][0][0][0] < pairs[j][0][0][0]):
                pairs[j], pairs[j + 1] = pairs[j + 1], pairs[j]
    return pairs


def ctc_decode(prob: np.ndarray, chars: list[str]):
    """CTC：argmax -> 合并重复 -> 去 blank(索引 0) -> 映射字典。

    rec 输出已是 softmax 概率，取逐帧 argmax 即可。
    返回 (文本, 平均置信度)。
    """
    idx = prob.argmax(axis=1)
    text, conf_sum, conf_cnt = [], 0.0, 0
    prev = -1
    for t, i in enumerate(idx.tolist()):
        if i != 0 and i != prev:
            if 1 <= i <= len(chars):
                text.append(chars[i - 1])
                conf_sum += float(prob[t, i])
                conf_cnt += 1
        prev = i
    return "".join(text), (conf_sum / conf_cnt if conf_cnt else 0.0)


# ---------------------------------------------------------------- 图像裁剪
def crop_by_box(img: np.ndarray, box) -> np.ndarray | None:
    """按四点做透视变换，裁出正视图文行。"""
    (tl, tr, br, bl) = order_points(np.array(box, dtype=np.float32))
    width_a = float(np.hypot(br[0] - bl[0], br[1] - bl[1]))
    width_b = float(np.hypot(tr[0] - tl[0], tr[1] - tl[1]))
    height_a = float(np.hypot(tr[0] - br[0], tr[1] - br[1]))
    height_b = float(np.hypot(tl[0] - bl[0], tl[1] - bl[1]))
    max_w = max(int(width_a), int(width_b))
    max_h = max(int(height_a), int(height_b))
    if max_w <= 0 or max_h <= 0:
        return None
    dst = np.array([[0, 0], [max_w - 1, 0], [max_w - 1, max_h - 1],
                    [0, max_h - 1]], dtype=np.float32)
    M = cv2.getPerspectiveTransform(
        np.array([tl, tr, br, bl], dtype=np.float32), dst)
    return cv2.warpPerspective(img, M, (max_w, max_h))


def order_points(box):
    """四点排序为 [左上, 右上, 右下, 左下]。"""
    pts = np.array(box, dtype=np.float32)
    s = pts.sum(axis=1)
    diff = np.diff(pts, axis=1)
    tl = pts[np.argmin(s)]
    br = pts[np.argmax(s)]
    tr = pts[np.argmin(diff)]
    bl = pts[np.argmax(diff)]
    return np.array([tl, tr, br, bl], dtype=np.float32)


# ---------------------------------------------------------------- 主流程
def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    model_dir = ROOT / (args[0] if args else "models/small")
    if len(args) < 2:
        print("用法: python ref_pipeline.py <模型目录> <测试图片>")
        return

    img = imread_unicode(Path(args[1]))
    print(f"输入图像: {img.shape[1]}x{img.shape[0]}")

    so = ort.SessionOptions()
    so.log_severity_level = 3
    providers = ["DmlExecutionProvider", "CPUExecutionProvider"]
    det_sess = ort.InferenceSession(str(model_dir / "det.onnx"), so,
                                    providers=providers)
    rec_sess = ort.InferenceSession(str(model_dir / "rec.onnx"), so,
                                    providers=providers)
    print(f"det provider: {det_sess.get_providers()[0]}")
    print(f"rec provider: {rec_sess.get_providers()[0]}")

    x, ratio = det_preprocess(img)
    print(f"det 输入 shape: {x.shape}  ratio: {ratio:.4f}")
    prob = det_sess.run(None, {det_sess.get_inputs()[0].name: x})[0]
    print(f"det 输出 shape: {prob.shape}  "
          f"值域 [{prob.min():.4f}, {prob.max():.4f}]")

    boxes = db_postprocess(prob, ratio)
    print(f"检测到 {len(boxes)} 个文本行")

    keys = (model_dir / "keys.txt").read_text(encoding="utf-8").split("\n")
    chars = [k for k in keys if k != ""]
    print(f"字典字符数: {len(chars)}")

    results = []
    for b in boxes:
        crop = crop_by_box(img, b["box"])
        if crop is None or crop.size == 0:
            continue
        rx, rw = rec_preprocess(crop)
        out = rec_sess.run(None, {rec_sess.get_inputs()[0].name: rx})[0]
        text, conf = ctc_decode(out[0], chars)
        results.append({"text": text, "score": round(conf, 4),
                        "box": b["box"], "rec_width": rw})
        print(f"  [{conf:.3f}] w={rw:3d}  {text}")

    out_path = model_dir / "reference_output.json"
    out_path.write_text(json.dumps(results, ensure_ascii=False, indent=2),
                        encoding="utf-8")
    print(f"参考输出已写入 {out_path}")


if __name__ == "__main__":
    main()
