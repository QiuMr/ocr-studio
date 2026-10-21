"""probe_resize10：暴力求解 dy=0 的 (b0, b1)。

假设第一行的 V 通式是 ((b0*(D0>>4))>>16 + (b1*(D1>>4))>>16 + 2)>>2，
src = [row0, row1]。枚举 b1 ∈ [0, 2048]，看哪个能让 dy=0 全部 320 像素
与 cv2 一致。若 b1=0 最优且仍有错，则第一行根本不是这条公式。
"""
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
    x0 = np.zeros(n_dst, np.int64); a0 = np.zeros(n_dst, np.int64); a1 = np.zeros(n_dst, np.int64)
    for d in range(n_dst):
        s = (d + 0.5) * scale - 0.5
        s = min(max(s, 0.0), n_src - 1.0)
        i0 = int(np.floor(s)); fx = s - i0
        if i0 >= n_src - 1:
            x0[d] = n_src - 1; a0[d] = SCALE; a1[d] = 0
        else:
            x0[d] = i0
            ax = int(np.round(fx * SCALE))
            a1[d] = ax; a0[d] = SCALE - ax
    return x0, a0, a1


for label, box_i, dw, dh in [("box2", 2, 320, 48), ("box0", 0, 320, 48)]:
    img = ref.imread_unicode(DUMP / f"box_{box_i}_src.png")
    sh, sw = img.shape[:2]
    r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)

    x0, ax0, ax1 = coords(dw, sw)
    D = np.zeros((sh, dw, 3), np.int64)
    for y in range(sh):
        row = img[y].astype(np.int64)
        for d in range(dw):
            sx = x0[d]
            D[y, d] = (row[sx] * ax0[d] + row[sx + 1] * ax1[d]) if sx + 1 < sw else row[sx] * SCALE

    D0s = D[0] >> 4   # 第一行
    D1s = D[1] >> 4   # 第二行
    cv0 = r[0].astype(np.int64)  # cv2 第一行

    best = []
    for b1 in range(0, 2049, 1):
        b0 = SCALE - b1
        t0 = (b0 * D0s) >> 16
        t1 = (b1 * D1s) >> 16
        out = (t0 + t1 + 2) >> 2
        n = int((out != cv0).sum())
        best.append((n, b1))
    best.sort()
    print(f"[{label}] dy=0 最优 b1: " + ", ".join(f"b1={b}(错{n})" for n, b in best[:5]))
    # b1=0 时错多少
    n0 = [n for n, b in best if b == 0][0]
    print(f"  b1=0(纯单tap): 错 {n0}/{cv0.size}")
