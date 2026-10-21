"""看源码路径下剩余 57 个不一致的特征。"""
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import ref_pipeline as ref  # noqa: E402

DUMP = ROOT / "models" / "small" / "dump"
SCALE = 1 << 11


def coords(n_dst, n_src):
    scale = n_src / n_dst
    x0 = np.zeros(n_dst, np.int64)
    a0 = np.zeros(n_dst, np.int64)
    a1 = np.zeros(n_dst, np.int64)
    for d in range(n_dst):
        s = (d + 0.5) * scale - 0.5
        s = min(max(s, 0.0), n_src - 1.0)
        i0 = int(np.floor(s))
        fx = s - i0
        if i0 >= n_src - 1:
            x0[d] = n_src - 1; a0[d] = SCALE; a1[d] = 0
        else:
            x0[d] = i0
            ax = int(np.round(fx * SCALE))
            a1[d] = ax; a0[d] = SCALE - ax
    return x0, a0, a1


img = ref.imread_unicode(DUMP / "box_2_src.png")  # 360x30
dw, dh = 320, 48
sh, sw = img.shape[:2]
r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)

x0, ax0, ax1 = coords(dw, sw)
D = np.zeros((sh, dw, 3), np.int64)
for y in range(sh):
    row = img[y].astype(np.int64)
    for d in range(dw):
        sx = x0[d]
        if sx + 1 < sw:
            D[y, d] = row[sx] * ax0[d] + row[sx + 1] * ax1[d]
        else:
            D[y, d] = row[sx] * SCALE

y0, ay0, ay1 = coords(dh, sh)
out = np.zeros((dh, dw, 3), np.uint8)
for d in range(dh):
    sy = y0[d]
    b0, b1 = ay0[d], ay1[d]
    r0 = D[sy] >> 4
    r1 = D[sy + 1] >> 4 if (sy + 1 < sh and b1 != 0) else r0
    out[d] = (((b0 * r0) >> 16) + ((b1 * r1) >> 16) + 2) >> 2

bad = out != r
print(f"不一致 {int(bad.sum())}/{bad.size}")
ys, xs, cs = np.where(bad)
for k in range(len(ys)):
    y, x, c = ys[k], xs[k], cs[k]
    sy = y0[y]; sx = x0[x]
    print(f"  (dy={y:2d},dx={x:3d},ch={c}) cv2={int(r[y, x, c]):3d} "
          f"mine={int(out[y, x, c]):3d}  sx={sx} ax1={ax1[x]} "
          f"sy={sy} by1={ay1[y]}  D0={int(D[sy, x, c])} D1={int(D[sy + 1, x, c])}")
