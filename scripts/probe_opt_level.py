"""判定：rec 会话的 graph_optimization_level 是否导致置信度差。

Rust engine.rs 用 GraphOptimizationLevel::Level3 (= ORT_ENABLE_ALL)，
而 ref_pipeline.py 用 SessionOptions() 默认值 (= ORT_ENABLE_BASIC)。
本脚本在 Python 侧复现 Level3，看 box 6 置信度是否从 0.7475 跳到 0.8937。
"""
from __future__ import annotations

from pathlib import Path

import numpy as np
import onnxruntime as ort

ROOT = Path(__file__).resolve().parent.parent
DUMP = ROOT / "models" / "small" / "dump"
M = ROOT / "models" / "small"

import ref_pipeline as ref  # noqa: E402

PROVIDERS = ["DmlExecutionProvider", "CPUExecutionProvider"]


def run(t: np.ndarray, opt) -> tuple[str, float]:
    so = ort.SessionOptions()
    so.log_severity_level = 3
    if opt is not None:
        so.graph_optimization_level = opt
    rec = ort.InferenceSession(str(M / "rec.onnx"), so, providers=PROVIDERS)
    out = rec.run(None, {rec.get_inputs()[0].name: t[None]})[0]
    return ref.ctc_decode(out[0], ref_chars), rec.get_providers()[0]


ref_chars = (M / "keys.txt").read_text(encoding="utf-8").splitlines()


def main():
    levels = {
        "BASIC (默认)": ort.GraphOptimizationLevel.ORT_ENABLE_BASIC,
        "EXTENDED": ort.GraphOptimizationLevel.ORT_ENABLE_EXTENDED,
        "ALL (Rust 同款)": ort.GraphOptimizationLevel.ORT_ENABLE_ALL,
    }
    for i in range(7):
        raw = np.frombuffer((DUMP / f"box_{i}_rec.f32").read_bytes(), dtype="<f4")
        t = raw.reshape(3, 48, 320).copy()
        print(f"--- box {i} ---")
        for name, lv in levels.items():
            text, conf, prov = None, None, None
            so = ort.SessionOptions()
            so.log_severity_level = 3
            so.graph_optimization_level = lv
            rec = ort.InferenceSession(str(M / "rec.onnx"), so, providers=PROVIDERS)
            out = rec.run(None, {rec.get_inputs()[0].name: t[None]})[0]
            text, conf = ref.ctc_decode(out[0], ref_chars)
            prov = rec.get_providers()[0]
            print(f"  {name:18s} conf={conf:.4f}  text={text!r}  [{prov}]")


if __name__ == "__main__":
    main()
