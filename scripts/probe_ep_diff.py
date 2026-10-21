"""同一份 Rust 已逐位对齐的裁剪与预处理，换 CPU / DML 两种 EP 跑 rec，
看置信度差多少。若 DML vs CPU 本身就差 0.15，则 pipeline_match 的残差
来自 EP 数值差异，不是 Rust 实现的问题。
"""
import json
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import ref_pipeline as ref  # noqa: E402

img = ref.imread_unicode(ROOT / "assets" / "test_zh.png")
model_dir = ROOT / "models" / "small"
keys = (model_dir / "keys.txt").read_text(encoding="utf-8").split("\n")
chars = [k for k in keys if k != ""]

# 用 CPU 跑 det（与 dump_rec_tensor.py 一致，拿框）
import onnxruntime as ort

so = ort.SessionOptions()
so.log_severity_level = 3
det_sess = ort.InferenceSession(str(model_dir / "det.onnx"), so,
                                providers=["CPUExecutionProvider"])
x, ratio = ref.det_preprocess(img)
prob = det_sess.run(None, {det_sess.get_inputs()[0].name: x})[0]
boxes = ref.db_postprocess(prob, ratio)
crops = []
for b in boxes:
    c = ref.crop_by_box(img, np.array(b["box"], dtype=np.float32))
    if c is not None and c.size:
        crops.append((b["box"], c))

results = {}
for tag, providers in [("CPU", ["CPUExecutionProvider"]),
                       ("DML", ["DmlExecutionProvider"])]:
    rec_sess = ort.InferenceSession(str(model_dir / "rec.onnx"), so,
                                    providers=providers)
    got = rec_sess.get_providers()[0]
    out = []
    for box, crop in crops:
        rx, rw = ref.rec_preprocess(crop)
        o = rec_sess.run(None, {rec_sess.get_inputs()[0].name: rx})[0]
        text, conf = ref.ctc_decode(o[0], chars)
        out.append({"text": text, "score": round(conf, 4)})
    results[tag] = out
    print(f"[{tag}] provider={got}")

print("\n逐框对比 CPU vs DML:")
max_d = 0.0
for i, (a, b) in enumerate(zip(results["CPU"], results["DML"])):
    d = abs(a["score"] - b["score"])
    max_d = max(max_d, d)
    same = "同" if a["text"] == b["text"] else "异"
    flag = "  <== 超 0.05" if d > 0.05 else ""
    print(f"  #{i}: CPU={a['score']:.4f}  DML={b['score']:.4f}  差={d:.4f} 文本={same}{flag}")
print(f"\n最大置信度差: {max_d:.4f}")

# 与 Rust 的 pipeline_match 结果对照
print("\nRust(Directml) 参考值来自 tests/pipeline_match.rs 输出:")
print("  #3: 0.990 / 1.000 / 0.990   #4: 0.976 / 0.984   #5: 0.794 / 0.803   #6: 0.896 / 0.748")
