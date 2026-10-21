"""probe_resize6：按 OpenCV 4.10 源码的通用 8u 路径精确复刻。

源码（modules/imgproc/src/resize.cpp）：
  H 段（无舍入，11bit 定点）:
      D = S[sx]*a0 + S[sx+1]*a1           (a0+a1 = 2048)
  V 段（8u 特化）:
      out = ((b0*(D0>>4))>>16 + (b1*(D1>>4))>>16 + 2) >> 2

未知量（在被截断的主循环里）:
  - 坐标用 float 还是 double
  - a/b 系数量化: cvRound(半数取偶) / 截断 / floor(x+0.5)
逐个组合试，找出 0 不一致的那个。
"""
import struct
import sys
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import ref_pipeline as ref  # noqa: E402

DUMP = ROOT / "models" / "small" / "dump"
SCALE = 1 << 11


def coords(n_dst, n_src, dtype):
    """返回 (x0数组, x1数组, a0数组, a1数组)，复刻 cbuf/xofs 填充。"""
    scale = n_src / n_dst
    x0 = np.zeros(n_dst, np.int64)
    a0 = np.zeros(n_dst, np.int64)
    a1 = np.zeros(n_dst, np.int64)
    for d in range(n_dst):
        if dtype == "f32":
            s = np.float32((d + 0.5) * np.float32(scale) - np.float32(0.5))
            s = float(s)
        else:
            s = (d + 0.5) * scale - 0.5
        # 钳位到 [0, n_src-1]
        s = min(max(s, 0.0), n_src - 1.0)
        i0 = int(np.floor(s))
        fx = s - i0
        if i0 >= n_src - 1:
            # 右边界: 单 tap（HResizeLinear 的 dx>=xmax 分支）
            x0[d] = n_src - 1
            a0[d] = SCALE
            a1[d] = 0
        else:
            x0[d] = i0
            if dtype == "f32":
                fx = float(np.float32(fx))
            if quant == "round":
                ax = int(np.round(fx * SCALE))       # cvRound: 半数取偶
            elif quant == "trunc":
                ax = int(fx * SCALE)                 # C++ (short) 强转: 截断
            else:
                ax = int(np.floor(fx * SCALE + 0.5))  # floor(x+0.5)
            ax = min(max(ax, 0), SCALE)
            a1[d] = ax
            a0[d] = SCALE - ax
    return x0, a0, a1


def resize_cv(img, dw, dh, dtype, quant):
    sh, sw = img.shape[:2]
    # 横向: 每行产出 11bit 定点
    x0, ax0, ax1 = coords(dw, sw, dtype)
    D = np.zeros((sh, dw, 3), np.int64)
    for y in range(sh):
        row = img[y].astype(np.int64)
        for d in range(dw):
            sx = x0[d]
            p0 = row[sx]
            p1 = row[sx + 1] if ax1[d] != 0 or sx + 1 < sw else p0
            # 注意: 右边界单 tap 时 a1=0，p1 取什么无所谓；但 sx+1 可能越界
            if sx + 1 < sw:
                D[y, d] = p0 * ax0[d] + p1 * ax1[d]
            else:
                D[y, d] = p0 * SCALE
    # 纵向
    y0, ay0, ay1 = coords(dh, sh, dtype)
    out = np.zeros((dh, dw, 3), np.uint8)
    for d in range(dh):
        sy = y0[d]
        b0 = ay0[d]
        b1 = ay1[d]
        r0 = D[sy] >> 4
        r1 = D[sy + 1] >> 4 if (sy + 1 < sh and b1 != 0) else r0
        # 源码: ((b0*(S0>>4))>>16 + (b1*(S1>>4))>>16 + 2)>>2
        t0 = (b0 * r0) >> 16
        t1 = (b1 * r1) >> 16
        out[d] = ((t0 + t1 + 2) >> 2).astype(np.uint8)
    return out


def report(name, r, got):
    d = np.abs(r.astype(np.int64) - got.astype(np.int64))
    n = int((d > 0).sum())
    print(f"  {name:36s} 最大差 {d.max():3d}  不一致 {n:6d}/{d.size}"
          + ("   <== 完全一致" if n == 0 else ""))


cases = [
    ("box2 360x30->320x48", ref.imread_unicode(DUMP / "box_2_src.png"), 320, 48),
    ("box0 379x31->320x48", ref.imread_unicode(DUMP / "box_0_src.png"), 320, 48),
    ("box6 33x31->320x48", ref.imread_unicode(DUMP / "box_6_src.png"), 320, 48),
]
rng = np.random.default_rng(3)
noise = rng.integers(0, 256, (20, 17, 3), dtype=np.uint8)
cases.append(("噪声 17x20->51x60", noise, 51, 60))
cases.append(("噪声 17x20->8x9", noise, 8, 9))

for label, img, dw, dh in cases:
    r = cv2.resize(img, (dw, dh), interpolation=cv2.INTER_LINEAR)
    print(f"[{label}]")
    for dtype in ("f64", "f32"):
        for quant in ("round", "trunc", "halfup"):
            globals()["quant"] = quant
            report(f"源码路径 {dtype}/{quant}", r,
                   resize_cv(img, dw, dh, dtype, quant))
    print()
