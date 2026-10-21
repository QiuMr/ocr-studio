"""最终判定：box 6 的置信度差是否完全由那 3 个 1-LSB 像素引起。

做法：读 Python dump 的 box_6_rec.f32，在 Rust 报告不一致的位置上 +1 LSB，
重跑 rec 推理，看置信度是否从 0.7475 跳到 Rust 的 0.8937。
"""
from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parent.parent
DUMP = ROOT / "models" / "small" / "dump"
M = ROOT / "models" / "small"

# 复用 ref_pipeline 的补丁与解码
import ref_pipeline as ref  # noqa: E402

LSB = 1.0 / 255.0


def run(t: np.ndarray, rec, chars):
    out = rec.run(None, {rec.get_inputs()[0].name: t[None]})[0]
    return ref.ctc_decode(out[0], chars)


def main():
    chars = (M / "keys.txt").read_text(encoding="utf-8").splitlines()
    so = ort.SessionOptions()
    so.log_severity_level = 3
    rec = ort.InferenceSession(str(M / "rec.onnx"), so,
                               providers=["CPUExecutionProvider"])

    raw = np.frombuffer((DUMP / "box_6_rec.f32").read_bytes(), dtype="<f4")
    t = raw.reshape(3, 48, 320).copy()
    base = run(t, rec, chars)
    print(f"原始                : text={base[0]!r} conf={base[1]:.4f}")

    # Rust 报告的差异位置（来自 tensor_diff 输出）：全部在 y=0
    # box6: 首差 idx=24 (c=0,y=0,x=8)，共 3 个元素
    # 直接把 y=0 整行按 Rust 的方式处理：找出 Rust 与 Python 不同的确切位置不可行，
    # 这里改为枚举"在首行加 k 个 LSB"的敏感性曲线。
    n = t.shape[1] * t.shape[2]
    for k in (1, 3, 10, 30, 51, 100, 320):
        tt = t.copy()
        flat = tt[0, 0, :].reshape(-1)  # y=0, c=0
        idx = np.linspace(0, flat.size - 1, k).astype(int)
        flat[idx] = np.clip(flat[idx] + LSB, -1, 1)
        r = run(tt, rec, chars)
        print(f"首行 +{k:3d} 个 LSB   : text={r[0]!r} conf={r[1]:.4f}  delta={r[1]-base[1]:+.4f}")

    # 参考：Rust 全量置信度
    print(f"Rust 实测           : text='0' conf=0.8937")


if __name__ == "__main__":
    main()
