"""probe_resize4：低 bit 权重粒度（SIMD maddubs 路径猜想）。

_mm_maddubs_epi16 做 u8 x i8，系数必须 <=127。若 OpenCV 的 SIMD resize
用 6bit(64) 系数，权重粒度就是 1/64，与全精度的差可达 255/4096，正好是
「差 1」的量级。
"""
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import ref_pipeline as ref  # noqa: E402

DUMP = ROOT / "models" / "small" / "dump"


def coord(d, dst_n, src_n):
    scale = src_n / dst_n
    s = (d + 0.5) * scale - 0.5
    return min(max(s, 0.0), src_n - 1.0)


def report(name, r, got):
    d = np.abs(r.astype(np.int64) - got.astype(np.int64))
    print(f"  {name:34s} 最大差 {d.max():3d}  不一致 {int((d > 0).sum()):6d}/{d.size}"
          + ("   <== 完全一致" if (d > 0).sum() == 0 else ""))


def scheme_bits(img, dw, dh, bits):
    """融合式: 权重量化到 1/2^bits, 整数运算, floor(v+0.5)。"""
    k = 1 << bits
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.uint8)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        ay = int(np.round(fy * k))
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            ax = int(np.round(fx * k))
            p00 = img[y0, x0].astype(np.int64)
            p10 = img[y0, x1].astype(np.int64)
            p01 = img[y1, x0].astype(np.int64)
            p11 = img[y1, x1].astype(np.int64)
            S = (p00 * (k - ax) * (k - ay) + p10 * ax * (k - ay)
                 + p01 * (k - ax) * ay + p11 * ax * ay)
            out[dy, dx] = ((S + (k * k // 2)) >> (2 * bits)).astype(np.uint8)
    return out


def scheme_hv_bits(img, dw, dh, bits):
    """两段式但中间不降精度到 u8：横向出 (k bit 定点)，再纵向，最后一次舍入。"""
    k = 1 << bits
    sh, sw = img.shape[:2]
    tmp = np.zeros((sh, dw, 3), np.int64)
    for dy in range(sh):
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            ax = int(np.round(fx * k))
            tmp[dy, dx] = (img[dy, x0].astype(np.int64) * (k - ax)
                           + img[dy, x1].astype(np.int64) * ax)
    out = np.zeros((dh, dw, 3), np.uint8)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        ay = int(np.round(fy * k))
        for dx in range(dw):
            S = tmp[y0, dx] * (k - ay) + tmp[y1, dx] * ay
            out[dy, dx] = ((S + (k * k // 2)) >> (2 * bits)).astype(np.uint8)
    return out


cases = [
    ("box0 379x31->320x48", ref.imread_unicode(DUMP / "box_0_src.png"), 320, 48),
    ("box6 33x31->320x48", ref.imread_unicode(DUMP / "box_6_src.png"), 320, 48),
    ("box2 360x30->320x48", ref.imread_unicode(DUMP / "box_2_src.png"), 320, 48),
]
rng = np.random.default_rng(3)
noise = rng.integers(0, 256, (20, 17, 3), dtype=np.uint8)
cases.append(("噪声 17x20->51x60", noise, 51, 60))
cases.append(("噪声 17x20->8x9", noise, 8, 9))

for label, img, dw, dh in cases:
    r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    for bits in (5, 6, 7, 8, 11):
        report(f"融合 {bits}bit 权重", r, scheme_bits(img, dw, dh, bits))
    for bits in (6, 8, 11):
        report(f"两段不降精度 {bits}bit", r, scheme_hv_bits(img, dw, dh, bits))
    print()
