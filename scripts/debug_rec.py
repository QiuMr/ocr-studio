"""调试 rec 阶段：检查 logits 分布、argmax 索引，定位 CTC 解码为空的原因。"""

from __future__ import annotations

import sys
from collections import Counter
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parent.parent

# 复用参考脚本里的 onnxruntime 兼容补丁
import importlib.util
spec = importlib.util.spec_from_file_location(
    "ref_pipeline", ROOT / "scripts" / "ref_pipeline.py")
ref = importlib.util.module_from_spec(spec)
sys.modules["ref_pipeline"] = ref
spec.loader.exec_module(ref)


def imread_unicode(path: Path):
    """cv2.imread 在 Windows 上无法处理非 ASCII 路径，改用 imdecode。"""
    data = np.frombuffer(path.read_bytes(), dtype=np.uint8)
    return cv2.imdecode(data, cv2.IMREAD_COLOR)


def main() -> None:
    model_dir = ROOT / "models" / "small"
    img = imread_unicode(ROOT / "assets" / "test_zh.png")

    so = ort.SessionOptions()
    so.log_severity_level = 3
    rec_sess = ort.InferenceSession(
        str(model_dir / "rec.onnx"), so,
        providers=["DmlExecutionProvider", "CPUExecutionProvider"])

    keys = (model_dir / "keys.txt").read_text(encoding="utf-8").split("\n")
    chars = [k for k in keys if k != ""]

    # 手动裁第一行文字区域（测试图第一行在 y≈40..70, x≈40..500）
    crop = img[30:75, 30:560]
    print(f"crop shape: {crop.shape}")

    for tag, x in [
        ("std(0.5)", ref.rec_preprocess(crop)[0]),
    ]:
        logits = rec_sess.run(
            None, {rec_sess.get_inputs()[0].name: x})[0]
        print(f"--- {tag} --- logits shape {logits.shape} "
              f"min {logits.min():.3f} max {logits.max():.3f}")
        lg = logits[0]
        idx = lg.argmax(axis=1)
        print(f"argmax 序列: {idx.tolist()}")
        c = Counter(idx.tolist())
        print(f"最常见索引: {c.most_common(8)}")
        print(f"第 0 类的平均 logit: {lg[:, 0].mean():.3f}")
        print(f"第 1 类的平均 logit: {lg[:, 1].mean():.3f}")
        print(f"第 100 类的平均 logit: {lg[:, 100].mean():.3f}")

    # 尝试不加归一化 / 用 ImageNet 归一化，对比 argmax 是否变化
    h, w = crop.shape[:2]
    resized = cv2.resize(crop, (320, 48))
    variants = {
        "raw/255": resized.astype(np.float32) / 255.0,
        "(x-0.5)/0.5": (resized.astype(np.float32) / 255.0 - 0.5) / 0.5,
        "imagenet": (resized.astype(np.float32) / 255.0
                     - np.array([0.485, 0.456, 0.406]))
        / np.array([0.229, 0.224, 0.225]),
        "mean-only": (resized.astype(np.float32) / 255.0
                      - np.array([0.5, 0.5, 0.5])),
    }
    for name, v in variants.items():
        x = v.transpose(2, 0, 1)[np.newaxis, :].astype(np.float32)
        logits = rec_sess.run(
            None, {rec_sess.get_inputs()[0].name: x})[0][0]
        idx = logits.argmax(axis=1)
        text = "".join(chars[i - 1] for i in idx
                       if i != 0 and i != idx[max(0, idx.tolist().index(i) - 1)]
                       if i - 1 < len(chars))
        print(f"[{name:12s}] argmax[:20]={idx[:20].tolist()} "
              f"logit[0]={logits[:, 0].mean():.2f} "
              f"logit[1]={logits[:, 1].mean():.2f}")


if __name__ == "__main__":
    main()
