"""直接读出 cv2.resize INTER_LINEAR 的源坐标/权重粒度。

img = 1x2, 值 [0, 200]，放大到 1x64。dst(x) 的值 = 200 * fx，
于是 fx_eff = out/200，可以直接和候选 fx 对比，看出量化到几位。
"""
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

P0, P1 = 0, 200


def exact_fx(dx, dst_n, src_n):
    scale = src_n / dst_n
    s = (dx + 0.5) * scale - 0.5
    s = min(max(s, 0.0), src_n - 1.0)
    return s - int(np.floor(s))


def analyze(label, img_shape, dst, axis):
    """axis='x': 沿宽放大; axis='y': 沿高放大。"""
    img = np.zeros(img_shape, np.uint8)
    if axis == "x":
        img[0, 0] = P0; img[0, 1] = P1
    else:
        img[0, 0] = P0; img[1, 0] = P1
    out = cv2.resize(img, dst, interpolation=cv2.INTER_LINEAR)
    src_n = img.shape[1] if axis == "x" else img.shape[0]
    dst_n = dst[0] if axis == "x" else dst[1]
    print(f"[{label}] {img.shape[1]}x{img.shape[0]} -> {dst[0]}x{dst[1]}")
    n_show = 0
    for d in range(dst_n):
        v = int(out[0, d, 0] if axis == "x" else out[d, 0, 0])
        fe = exact_fx(d, dst_n, src_n)
        # 反推 fx: v = P0*(1-fx) + P1*fx = 200*fx
        fx_from_v = v / (P1 - P0)
        if n_show < 8 or d >= dst_n - 3:
            print(f"  d={d:3d} out={v:3d}  fx_eff={fx_from_v:.6f}  精确fx={fe:.6f}  "
                  f"差={fx_from_v - fe:+.6f}")
            n_show += 1
    # 统计 fx_eff 是 1/2^k 的倍数的程度
    fxs = []
    for d in range(dst_n):
        v = int(out[0, d, 0] if axis == "x" else out[d, 0, 0])
        fxs.append(v / (P1 - P0))
    fxs = np.array(fxs)
    for bits in (5, 6, 8, 11, 12):
        k = 1 << bits
        q = np.round(fxs * k) / k
        # 允许 ±1/200 的舍入误差
        ok = np.abs(q - fxs) <= 1.0 / (P1 - P0) + 1e-9
        print(f"  fx 是否为 1/{k} ({bits}bit) 的倍数: {int(ok.sum())}/{dst_n}")


analyze("水平 2->64", (1, 2, 3), (64, 1), "x")
print()
analyze("垂直 2->64", (2, 1, 3), (1, 64), "y")

# 缩小方向: 64 -> 2（看坐标是否也是 (d+0.5)*scale-0.5）
img = np.zeros((1, 64, 3), np.uint8)
for x in range(64):
    img[0, x] = (x * 3) % 256
out = cv2.resize(img, (2, 1), interpolation=cv2.INTER_LINEAR)
print(f"\n[水平 64->2] 源 = {list(img[0, :, 0])}")
print(f"  cv2 out = {list(out[0, :, 0])}")
for d in range(2):
    s = (d + 0.5) * 32 - 0.5
    s = min(max(s, 0.0), 63.0)
    x0 = int(np.floor(s)); fx = s - x0
    x1 = min(x0 + 1, 63)
    exact = img[0, x0, 0] * (1 - fx) + img[0, x1, 0] * fx
    print(f"  d={d}: sx={s:.4f} x0={x0} fx={fx:.4f} 精确={exact:.2f} "
          f"-> round={round(exact)} floor+.5={int(exact + 0.5)}")
