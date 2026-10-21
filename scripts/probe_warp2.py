"""probe v2：排除边界干扰后，定位 cv2.warpPerspective INTER_LINEAR 的定点方案。

用一张 32x32 噪声图 + 严格内缩的透视变换（所有源坐标都在 [2, 29] 内），
这样 x0/x1、y0/y1 全部在界内，排除 border 干扰，只比插值本身。
"""
import cv2
import numpy as np

rng = np.random.default_rng(7)
img = rng.integers(0, 256, (32, 32, 3), dtype=np.uint8)

# 源四点 -> 目标四点（都严格在内缩区域）
src = np.float32([[4, 4], [28, 3], [29, 27], [3, 28]])
dst = np.float32([[0, 0], [31, 0], [31, 31], [0, 31]])
M = cv2.getPerspectiveTransform(src, dst)
Mi = np.linalg.inv(M.astype(np.float64))

out = cv2.warpPerspective(img, M, (32, 32), flags=cv2.INTER_LINEAR)

ys, xs = np.mgrid[0:32, 0:32].astype(np.float64)
den = Mi[2, 0] * xs + Mi[2, 1] * ys + Mi[2, 2]
sx = (Mi[0, 0] * xs + Mi[0, 1] * ys + Mi[0, 2]) / den
sy = (Mi[1, 0] * xs + Mi[1, 1] * ys + Mi[1, 2]) / den
print(f"源坐标范围 sx [{sx.min():.3f}, {sx.max():.3f}]  sy [{sy.min():.3f}, {sy.max():.3f}]")


def interp(img, ax, ay, x0, y0):
    """ax/ay: [0,1) 小数权重；x0/y0: 整数像素。"""
    x1 = x0 + 1; y1 = y0 + 1
    out = np.zeros(img.shape, np.float64)
    for c in range(img.shape[2]):
        p00 = img[y0, x0, c].astype(np.float64)
        p01 = img[y0, x1, c].astype(np.float64)
        p10 = img[y1, x0, c].astype(np.float64)
        p11 = img[y1, x1, c].astype(np.float64)
        out[..., c] = (p00 * (1 - ax) * (1 - ay) + p01 * ax * (1 - ay)
                       + p10 * (1 - ax) * ay + p11 * ax * ay)
    return out


def report(name, ref):
    d = np.abs(out.astype(np.float64) - ref)
    print(f"  {name:34s} 最大差 {d.max():7.2f}  不一致 {int((d > 0).sum()):5d}/{d.size}")


# 基准：全精度
x0 = np.floor(sx).astype(int); y0 = np.floor(sy).astype(int)
ax_f = sx - x0; ay_f = sy - y0
print("cv2 对比:")
report("全精度双线性", interp(img, ax_f, ay_f, x0, y0))

# 权重量化到 1/2^k
for bits in (5, 6, 8, 10, 11):
    k = 1 << bits
    aq = np.round(ax_f * k) / k
    bq = np.round(ay_f * k) / k
    report(f"权重量化 1/{k} ({bits}bit)", interp(img, aq, bq, x0, y0))

# 坐标整体量化到 1/2^k
for bits in (5, 11):
    k = 1 << bits
    X = np.round(sx * k).astype(int); Y = np.round(sy * k).astype(int)
    qx0 = X >> bits; qy0 = Y >> bits
    aq = (X & (k - 1)) / k; bq = (Y & (k - 1)) / k
    report(f"坐标量化 1/{k} ({bits}bit)", interp(img, aq, bq, qx0, qy0))

# 混合：坐标量化到 1/2048 + 权重量化到 1/32（OpenCV 经典组合）
k1, k2 = 2048, 32
X = np.round(sx * k1).astype(int); Y = np.round(sy * k1).astype(int)
qx0 = X >> 11; qy0 = Y >> 11
aq = np.round(((X & (k1 - 1)) / k1) * k2) / k2
bq = np.round(((Y & (k1 - 1)) / k1) * k2) / k2
report("坐标2048+权重32 量化", interp(img, aq, bq, qx0, qy0))

# 最后看 cv2 - 全精度 的残差是否像「只量化权重」的形状
resid = out.astype(np.float64) - interp(img, ax_f, ay_f, x0, y0)
print(f"\n残差范围 [{resid.min():.3f}, {resid.max():.3f}]")
