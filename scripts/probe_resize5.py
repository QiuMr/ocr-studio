"""看 cv2.resize 不一致像素的结构：位置分布 + 精确值 + cv2 值。"""
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


def exact(img, dw, dh):
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
            out[dy, dx] = (img[y0, x0] * (1 - fx) * (1 - fy)
                           + img[y0, x1] * fx * (1 - fy)
                           + img[y1, x0] * (1 - fx) * fy
                           + img[y1, x1] * fx * fy)
    return out


img = ref.imread_unicode(DUMP / "box_2_src.png")  # 360x30
dw, dh = 320, 48
r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
v = exact(img, dw, dh)
mine = np.floor(v + 0.5).astype(np.int64)
bad = mine != r.astype(np.int64)

print(f"总不一致 {int(bad.sum())}/{bad.size}（按通道计）")
# 按像素计（任一通道不一致）
bad_px = bad.any(axis=2)
print(f"按像素计 {int(bad_px.sum())}/{dh * dw}")

# 空间分布: 按列直方图
cols = bad_px.sum(axis=0)
print("\n按列的不一致数（每 16 列取一个）:")
print(" ".join(str(int(cols[i])) for i in range(0, dw, 16)))
rows = bad_px.sum(axis=1)
print("\n按行的不一致数:")
print(" ".join(str(int(rows[i])) for i in range(dh)))

# 看精确值与 cv2 的差: 是 v 接近 .5 边界吗?
frac = v - np.floor(v)
print(f"\n不一致像素的 frac 分布: min={frac[bad].min():.4f} "
      f"max={frac[bad].max():.4f} 均值={frac[bad].mean():.4f}")
print(f"一致像素的 frac 分布:   min={frac[~bad].min():.4f} "
      f"max={frac[~bad].max():.4f} 均值={frac[~bad].mean():.4f}")
print(f"frac 接近 0.5 (|frac-0.5|<0.05) 的像素: "
      f"不一致 {int(((np.abs(frac - 0.5) < 0.05) & bad).sum())} / "
      f"共 {int((np.abs(frac - 0.5) < 0.05).sum())}")

# cv2 相对 mine 的方向
d = r.astype(np.int64) - mine
print(f"\ncv2 - mine 的分布: 最小 {d.min()} 最大 {d.max()}")
uniq, cnt = np.unique(d[bad], return_counts=True)
print(f"差值直方图: {dict(zip(uniq.tolist(), cnt.tolist()))}")

# 抽 10 个样本看细节
ys, xs = np.where(bad_px)
print("\n样本 (dy, dx, 精确v[ch0], cv2, mine):")
for k in range(0, len(ys), max(1, len(ys) // 10)):
    y, x = ys[k], xs[k]
    print(f"  ({y:2d},{x:3d}) v={v[y, x, 0]:9.4f} frac={v[y, x, 0] - np.floor(v[y, x, 0]):.4f} "
          f"cv2={int(r[y, x, 0]):3d} mine={mine[y, x, 0]:3d}")
