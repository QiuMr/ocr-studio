"""验证：rec 输入张量的 1-LSB 系统性差异，能否解释 #6 置信度 0.896 vs 0.748？

做法：读 dump 的 box_6_rec.f32（Python 值），把 Rust 与 Python 不一致的
147 个元素按 Rust 的方向各 +0.0078（即像素 +1），构造「Rust 等价张量」，
分别跑 rec 模型，对比置信度。
"""
import struct
import sys
from pathlib import Path

import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

DUMP = ROOT / "models" / "small" / "dump"
MODEL = ROOT / "models" / "small" / "rec.onnx"

# Rust 侧张量：重放 rec_preprocess 会消耗 dump 的裁剪图，这里直接用张量 diff 的结论：
# 差异元素全部是 Rust - Python = +0.0078（像素 +1），共 147 个。
# 逐个元素：我们知道 Rust > Py，所以 Rust 值 = Py + 0.0078。
LSB = 1.0 / 127.5  # 归一化单位

so = ort.SessionOptions()
so.log_severity_level = 3
rec = ort.InferenceSession(str(MODEL), so, providers=["DmlExecutionProvider"])

keys = (ROOT / "models" / "small" / "keys.txt").read_text(encoding="utf-8").split("\n")
chars = [k for k in keys if k != ""]
import ref_pipeline as ref  # noqa: E402


def conf_of(t: np.ndarray) -> tuple[str, float]:
    out = rec.run(None, {rec.get_inputs()[0].name: t})[0]
    return ref.ctc_decode(out[0], chars)


for i in (6, 3, 5):
    raw = (DUMP / f"box_{i}_rec.f32").read_bytes()
    py = np.array(struct.unpack(f"<{len(raw) // 4}f", raw), dtype=np.float32)
    t_py = py.reshape(1, 3, 48, 320).copy()

    text_a, conf_a = conf_of(t_py)

    # Rust 等价张量：找出一阶差分里「Rust 比 Py 大 1 LSB」的位置。
    # 没有 Rust 原值，就用探测法：把每个像素 +1 再舍回，看哪些位置会变成
    # Rust 报告的值不可行——换个思路：
    # tensor_diff 报告「147 个元素不一致，全部 Rust-Py=+0.0078」。
    # 我们不知道是哪些 147 个，但可以给一个上界实验：
    # 把全部像素随机选 0.32%（147/46080）加 1 LSB，看置信度抖动范围。
    rng = np.random.default_rng(0)
    flat = t_py.reshape(-1).copy()
    n = 147
    idx = rng.choice(flat.size, n, replace=False)
    flat[idx] += np.float32(LSB)
    t_rust = flat.reshape(1, 3, 48, 320).copy()
    text_b, conf_b = conf_of(t_rust)

    print(f"box {i}: Py={conf_a:.4f} ({text_a[:12]!r})  "
          f"Rust≈{conf_b:.4f} ({text_b[:12]!r})  差={abs(conf_a - conf_b):.4f}")
