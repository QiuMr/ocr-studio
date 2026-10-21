"""probe_resize9：dy=0 的混合行为，找判别特征。"""
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


img = ref.imread_unicode(DUMP / "box_2_src.png")
dw, dh = 320, 48
sh, sw = img.shape[:2]
r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)

x0, ax0, ax1 = coords(dw, sw)
D = np.zeros((sh, dw, 3), np.int64)
for y in range(sh):
    row = img[y].astype(np.int64)
    for d in range(dw):
        sx = x0[d]
        D[y, d] = (row[sx] * ax0[d] + row[sx + 1] * ax1[d]) if sx + 1 < sw else row[sx] * SCALE

# dy=0 的分析
print("dy=0: cv2 vs 两种规则")
print(f"{'dx':>4} {'D0':>8} {'h':>9} {'frac':>7} {'t0':>5} {'cv2':>4} {'+2':>4} {'trunc':>5} {'想哪个':>6}")
n_want_trunc = n_want_round = 0
for d in range(dw):
    D0 = int(D[0, d, 0])
    h = D0 / SCALE
    frac = h - np.floor(h)
    t0 = (SCALE * (D0 >> 4)) >> 16
    v_round = (t0 + 2) >> 2
    v_trunc = t0 >> 2
    cv = int(r[0, d, 0])
    if cv == v_round and cv == v_trunc:
        continue
    want = "截断" if cv == v_trunc else ("舍入" if cv == v_round else "???")
    if want == "截断": n_want_trunc += 1
    elif want == "舍入": n_want_round += 1
    if n_want_trunc + n_want_round <= 25 or want == "???":
        print(f"{d:4d} {D0:8d} {h:9.4f} {frac:7.4f} {t0:5d} {cv:4d} {v_round:4d} {v_trunc:5d} {want:>6}")
print(f"\n合计: 想要截断 {n_want_trunc} 个, 想要舍入 {n_want_round} 个")

# 判别: 想要截断的像素，其 frac / t0 奇偶 / D0 mod 16 有什么特征?
print("\n想要截断的像素特征:")
for d in range(dw):
    D0 = int(D[0, d, 0])
    t0 = (SCALE * (D0 >> 4)) >> 16
    v_round = (t0 + 2) >> 2
    v_trunc = t0 >> 2
    cv = int(r[0, d, 0])
    if cv == v_trunc and cv != v_round:
        h = D0 / SCALE
        print(f"  dx={d:3d} D0={D0:8d} D0%16={D0 % 16:2d} t0={t0:5d} t0%4={t0 % 4} "
              f"h={h:.4f} frac={h - np.floor(h):.4f}")
