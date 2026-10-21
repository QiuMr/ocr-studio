"""钉死 cv2.resize(INTER_LINEAR) 的定点方案。

用 box_0 的裁剪图（379x31 -> 320x48，宽缩窄+高拉高，两个方向都覆盖），
外加纯放大、纯缩小各一组，逐一试这些候选：
  A 全精度权重 + 远离零舍入（当前 Rust 实现）
  B 全精度权重 + 平局取偶
  C 11bit 权重量化 + 单次双线性 + 平局取偶
  D 两段式(H/V 各一次 11bit 定点 + 各自舍入)
"""
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import ref_pipeline as ref  # noqa: E402

DUMP = ROOT / "models" / "small" / "dump"


def load_box(i):
    # cv2.imread 读不了中文路径，用参考实现的 unicode 版
    return ref.imread_unicode(DUMP / f"box_{i}_src.png")


def coord(dx, dst_n, src_n):
    """OpenCV 约定: 源坐标 = (dx+0.5)*scale - 0.5，再钳位。"""
    scale = src_n / dst_n
    s = (dx + 0.5) * scale - 0.5
    return min(max(s, 0.0), src_n - 1.0)


def round_half_even(v):
    return np.round(v)  # numpy 的 round 就是平局取偶


def scheme_full(img, dw, dh, rounding):
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.float64)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            p00 = img[y0, x0].astype(np.float64)
            p10 = img[y0, x1].astype(np.float64)
            p01 = img[y1, x0].astype(np.float64)
            p11 = img[y1, x1].astype(np.float64)
            v = (p00 * (1 - fx) * (1 - fy) + p10 * fx * (1 - fy)
                 + p01 * (1 - fx) * fy + p11 * fx * fy)
            out[dy, dx] = v
    if rounding == "away":
        # numpy round 是平局取偶；远离零 = floor(v+0.5) for v>=0
        return np.floor(out + 0.5).astype(np.uint8)
    else:
        return np.round(out).astype(np.uint8)


def scheme_11bit(img, dw, dh, rounding):
    sh, sw = img.shape[:2]
    out = np.zeros((dh, dw, 3), np.float64)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        qfy = round_half_even(fy * 2048) / 2048
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            qfx = round_half_even(fx * 2048) / 2048
            p00 = img[y0, x0].astype(np.float64)
            p10 = img[y0, x1].astype(np.float64)
            p01 = img[y1, x0].astype(np.float64)
            p11 = img[y1, x1].astype(np.float64)
            v = (p00 * (1 - qfx) * (1 - qfy) + p10 * qfx * (1 - qfy)
                 + p01 * (1 - qfx) * qfy + p11 * qfx * qfy)
            out[dy, dx] = v
    if rounding == "away":
        return np.floor(out + 0.5).astype(np.uint8)
    return np.round(out).astype(np.uint8)


def scheme_two_stage(img, dw, dh):
    """两段式：先横向 11bit 定点（每行独立），再纵向 11bit 定点。
    每段各自 (sum + 1024) >> 11 舍入。"""
    sh, sw = img.shape[:2]
    # 横向
    tmp = np.zeros((sh, dw, 3), np.int64)
    for dy in range(sh):
        for dx in range(dw):
            sx = coord(dx, dw, sw)
            x0 = int(np.floor(sx)); fx = sx - x0
            x1 = min(x0 + 1, sw - 1)
            ax = int(round_half_even(fx * 2048))
            a = img[dy, x0].astype(np.int64)
            b = img[dy, x1].astype(np.int64)
            s = a * (2048 - ax) + b * ax
            tmp[dy, dx] = (s + 1024) >> 11
    tmp = tmp.astype(np.uint8)
    out = np.zeros((dh, dw, 3), np.uint8)
    for dy in range(dh):
        sy = coord(dy, dh, sh)
        y0 = int(np.floor(sy)); fy = sy - y0
        y1 = min(y0 + 1, sh - 1)
        ay = int(round_half_even(fy * 2048))
        for dx in range(dw):
            a = tmp[y0, dx].astype(np.int64)
            b = tmp[y1, dx].astype(np.int64)
            s = a * (2048 - ay) + b * ay
            out[dy, dx] = (s + 1024) >> 11
    return out


def report(name, ref, got):
    d = np.abs(ref.astype(np.int64) - got.astype(np.int64))
    n_bad = int((d > 0).sum())
    print(f"  {name:34s} 最大差 {d.max():3d}  不一致 {n_bad:6d}/{d.size}"
          + ("   <== 完全一致" if n_bad == 0 else ""))


cases = [
    ("box0 379x31->320x48", load_box(0), 320, 48),
    ("box6 33x31->320x48", load_box(6), 320, 48),
    ("box2 360x30->320x48", load_box(2), 320, 48),
]

for label, img, dw, dh in cases:
    ref = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    report("A 全精度+远离零(现Rust)", ref, scheme_full(img, dw, dh, "away"))
    report("B 全精度+平局取偶", ref, scheme_full(img, dw, dh, "even"))
    report("C 11bit+平局取偶", ref, scheme_11bit(img, dw, dh, "even"))
    report("D 两段式11bit+各自舍入", ref, scheme_two_stage(img, dw, dh))
    print()

# 附加: 纯放大 / 纯缩小
rng = np.random.default_rng(3)
img = rng.integers(0, 256, (20, 17, 3), dtype=np.uint8)
for label, dw, dh in [("纯放大 17x20->51x60", 51, 60), ("纯缩小 17x20->8x9", 8, 9)]:
    ref = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    report("A 全精度+远离零(现Rust)", ref, scheme_full(img, dw, dh, "away"))
    report("B 全精度+平局取偶", ref, scheme_full(img, dw, dh, "even"))
    report("C 11bit+平局取偶", ref, scheme_11bit(img, dw, dh, "even"))
    report("D 两段式11bit+各自舍入", ref, scheme_two_stage(img, dw, dh))
    print()
