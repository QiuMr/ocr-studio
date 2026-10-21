"""probe_resize3：融合式定点方案，配合 64->2 探针确认的 floor(v+0.5) 舍入。

候选：
  E 全精度权重 + floor(v+0.5)
  F 11bit 权重(整数运算) + (S + 2^21) >> 22   <- OpenCV 经典 H/V 融合式
  G 11bit 权重，但权重按平局取整到 11bit
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


def report(name, ref_img, got):
    d = np.abs(ref_img.astype(np.int64) - got.astype(np.int64))
    n_bad = int((d > 0).sum())
    print(f"  {name:38s} 最大差 {d.max():3d}  不一致 {n_bad:6d}/{d.size}"
          + ("   <== 完全一致" if n_bad == 0 else ""))


def scheme_E(img, dw, dh):
    """全精度权重，floor(v+0.5)"""
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.uint8)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            v = (img[y0, x0].astype(np.float64) * (1 - fx) * (1 - fy)
                 + img[y0, x1].astype(np.float64) * fx * (1 - fy)
                 + img[y1, x0].astype(np.float64) * (1 - fx) * fy
                 + img[y1, x1].astype(np.float64) * fx * fy)
            out[dy, dx] = np.floor(v + 0.5).astype(np.uint8)
    return out


def scheme_F(img, dw, dh, weight_round="even"):
    """融合式 11bit 定点：S = p00*(2048-ax)*(2048-ay) + ...; out = (S + 2^21) >> 22"""
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.uint8)
    if weight_round == "even":
        q = np.round
    else:
        q = lambda v: np.floor(v + 0.5)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        ay = int(q(fy * 2048))
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            ax = int(q(fx * 2048))
            p00 = img[y0, x0].astype(np.int64)
            p10 = img[y0, x1].astype(np.int64)
            p01 = img[y1, x0].astype(np.int64)
            p11 = img[y1, x1].astype(np.int64)
            S = (p00 * (2048 - ax) * (2048 - ay) + p10 * ax * (2048 - ay)
                 + p01 * (2048 - ax) * ay + p11 * ax * ay)
            out[dy, dx] = ((S + (1 << 21)) >> 22).astype(np.uint8)
    return out


def scheme_H(img, dw, dh):
    """坐标整体量化到 1/2048（含整数位），权重 = 低位；再融合式整数运算。"""
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.uint8)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        Y = int(np.round(sy * 2048))
        y0 = Y >> 11; ay = Y & 2047
        y1 = min(y0 + 1, sh - 1)
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            X = int(np.round(sx * 2048))
            x0 = X >> 11; ax = X & 2047
            x1 = min(x0 + 1, sw - 1)
            p00 = img[y0, x0].astype(np.int64)
            p10 = img[y0, x1].astype(np.int64)
            p01 = img[y1, x0].astype(np.int64)
            p11 = img[y1, x1].astype(np.int64)
            S = (p00 * (2048 - ax) * (2048 - ay) + p10 * ax * (2048 - ay)
                 + p01 * (2048 - ax) * ay + p11 * ax * ay)
            out[dy, dx] = ((S + (1 << 21)) >> 22).astype(np.uint8)
    return out


cases = [
    ("box0 379x31->320x48", ref.imread_unicode(DUMP / "box_0_src.png"), 320, 48),
    ("box6 33x31->320x48", ref.imread_unicode(DUMP / "box_6_src.png"), 320, 48),
    ("box2 360x30->320x48", ref.imread_unicode(DUMP / "box_2_src.png"), 320, 48),
]
rng = np.random.default_rng(3)
noise = rng.integers(0, 256, (20, 17, 3), dtype=np.uint8)
cases.append(("噪声 17x20->51x60 放大", noise, 51, 60))
cases.append(("噪声 17x20->8x9 缩小", noise, 8, 9))

for label, img, dw, dh in cases:
    r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    report("E 全精度+floor(v+0.5)", r, scheme_E(img, dw, dh))
    report("F 融合11bit(权重取偶)", r, scheme_F(img, dw, dh, "even"))
    report("G 融合11bit(权重半数上)", r, scheme_F(img, dw, dh, "up"))
    report("H 坐标量化2048+融合", r, scheme_H(img, dw, dh))
    print()
