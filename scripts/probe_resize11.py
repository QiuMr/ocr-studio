"""probe_resize11：小干净用例上穷举「第一行」的算法。

用例: 5x4 -> 11x7（放大），cv2 的第一行是已知的。
候选 = {H 系数精度 8bit / 11bit / 全精度} x {系数 round / trunc}
       x {输出 round / trunc}
看哪个能 33/33 命中第一行。
"""
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

src = np.zeros((4, 5, 3), np.uint8)
for y in range(4):
    for x in range(5):
        for c in range(3):
            src[y, x, c] = (y * 61 + x * 29 + c * 7) % 256

dw, dh = 11, 7
r = cv2.resize(src, (dw, dh), interpolation=cv2.INTER_LINEAR)
cv0 = r[0].astype(np.int64)  # 第一行

sw, sh = 5, 4


def first_row(bits, coef_round, out_round):
    k = 1 << bits
    out = np.zeros((dw, 3), np.int64)
    for d in range(dw):
        s = (d + 0.5) * sw / dw - 0.5
        s = min(max(s, 0.0), sw - 1.0)
        i0 = int(np.floor(s))
        fx = s - i0
        if i0 >= sw - 1:
            m1 = 0
        else:
            v = fx * k
            m1 = int(np.round(v)) if coef_round == "round" else int(v)
        m0 = k - m1
        for c in range(3):
            p0 = int(src[0, i0, c])
            p1 = int(src[0, min(i0 + 1, sw - 1), c])
            H = p0 * m0 + p1 * m1  # scale 2^bits
            if out_round == "round":
                out[d, c] = (H + (k >> 1)) >> bits
            else:
                out[d, c] = H >> bits
    return out


def report(name, got):
    d = np.abs(got - cv0)
    n = int((d > 0).sum())
    print(f"  {name:44s} 不一致 {n:3d}/33"
          + ("   <== 完全一致" if n == 0 else ""))


print("第一行 33 个值，穷举:")
for bits in (8, 11, 16):
    for cr in ("round", "trunc"):
        for orr in ("round", "trunc"):
            report(f"H系数{bits}bit/{cr} + 输出{orr}",
                   first_row(bits, cr, orr))

# 另外试: H 用 11bit，但输出按通用 V 公式（t0 = H>>4 等 b0=2048 的退化）
print("\n通用 V 公式退化版 (b0=2048, b1=0):")
for bits in (8, 11):
    k = 1 << bits
    out = np.zeros((dw, 3), np.int64)
    for d in range(dw):
        s = (d + 0.5) * sw / dw - 0.5
        s = min(max(s, 0.0), sw - 1.0)
        i0 = int(np.floor(s)); fx = s - i0
        m1 = 0 if i0 >= sw - 1 else int(np.round(fx * k))
        m0 = k - m1
        for c in range(3):
            H = int(src[0, i0, c]) * m0 + int(src[0, min(i0 + 1, sw - 1), c]) * m1
            t0 = (2048 * (H >> 4)) >> 16
            out[d, c] = (t0 + 2) >> 2
    report(f"H系数{bits}bit + V退化公式", out)

print("\ncv2 第一行:", cv0[:, 0].tolist())
