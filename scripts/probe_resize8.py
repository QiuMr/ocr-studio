"""probe_resize8：验证 dy=0 的截断规则。

两个候选解释:
  A: b1==0（垂直退化成单 tap）时 -> (t0+t1)>>2，无 +2 舍入
  B: 仅 dy==0 时截断，其余行仍 +2
看哪个能在所有用例上归零。
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


def resize_cv(img, dw, dh, rule):
    sh, sw = img.shape[:2]
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
        t0 = (b0 * r0) >> 16
        t1 = (b1 * r1) >> 16
        if rule == "A" and b1 == 0:
            out[d] = (t0 + t1) >> 2
        elif rule == "B" and d == 0:
            out[d] = (t0 + t1) >> 2
        else:
            out[d] = (t0 + t1 + 2) >> 2
    return out


cases = [
    ("box2 360x30->320x48", ref.imread_unicode(DUMP / "box_2_src.png"), 320, 48),
    ("box0 379x31->320x48", ref.imread_unicode(DUMP / "box_0_src.png"), 320, 48),
    ("box1 339x31->320x48", ref.imread_unicode(DUMP / "box_1_src.png"), 320, 48),
    ("box3 508x34->320x48", ref.imread_unicode(DUMP / "box_3_src.png"), 320, 48),
    ("box4 337x30->320x48", ref.imread_unicode(DUMP / "box_4_src.png"), 320, 48),
    ("box5 335x34->320x48", ref.imread_unicode(DUMP / "box_5_src.png"), 320, 48),
    ("box6 33x31->320x48", ref.imread_unicode(DUMP / "box_6_src.png"), 320, 48),
]
rng = np.random.default_rng(3)
noise = rng.integers(0, 256, (20, 17, 3), dtype=np.uint8)
cases.append(("噪声 17x20->51x60", noise, 51, 60))
cases.append(("噪声 17x20->8x9", noise, 8, 9))
cases.append(("噪声 17x20->48x48", noise, 48, 48))
noise2 = rng.integers(0, 256, (7, 13, 3), dtype=np.uint8)
cases.append(("小图 13x7->320x48", noise2, 320, 48))
cases.append(("小图 13x7->64x48", noise2, 64, 48))

for label, img, dw, dh in cases:
    r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    for rule in ("A", "B"):
        got = resize_cv(img, dw, dh, rule)
        d = np.abs(r.astype(np.int64) - got.astype(np.int64))
        n = int((d > 0).sum())
        print(f"  规则{rule}: 不一致 {n:5d}/{d.size}"
              + ("   <== 完全一致" if n == 0 else ""))
