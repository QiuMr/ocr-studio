"""钉死 cv2.warpPerspective 的两个行为：
1. 坐标约定：整数坐标 = 像素中心（identity warp 应原样返回），还是像素左上角？
2. 插值精度：INTER_LINEAR 是否用定点量化（INTER_BITS）？
"""
import cv2
import numpy as np

rng = np.random.default_rng(42)
img = rng.integers(0, 256, (8, 8, 3), dtype=np.uint8)

# ---- 实验 1：identity warp ----
out = cv2.warpPerspective(img, np.eye(3, dtype=np.float64), (8, 8),
                          flags=cv2.INTER_LINEAR)
same = np.array_equal(out, img)
print(f"[实验1] identity warp 原样返回: {same}")
if not same:
    d = np.abs(out.astype(int) - img.astype(int))
    print(f"  最大差 {d.max()}  不一致 {int((d > 0).sum())}/{d.size}")

# ---- 实验 2：精确双线性参考 vs cv2 ----
def exact_bilinear(img, sx, sy):
    """sx/sy: 连续坐标，约定 整数=像素中心。越界填 0。"""
    h, w = img.shape[:2]
    x0 = np.floor(sx).astype(int); y0 = np.floor(sy).astype(int)
    fx = sx - x0; fy = sy - y0
    x1 = x0 + 1; y1 = y0 + 1
    ok = (x0 >= 0) & (y0 >= 0) & (x1 < w) & (y1 < h)
    x0c = np.clip(x0, 0, w - 1); x1c = np.clip(x1, 0, w - 1)
    y0c = np.clip(y0, 0, h - 1); y1c = np.clip(y1, 0, h - 1)
    out = np.zeros(img.shape, np.float64)
    for c in range(img.shape[2]):
        p00 = img[y0c, x0c, c].astype(np.float64)
        p01 = img[y0c, x1c, c].astype(np.float64)
        p10 = img[y1c, x0c, c].astype(np.float64)
        p11 = img[y1c, x1c, c].astype(np.float64)
        v = (p00 * (1 - fx) * (1 - fy) + p01 * fx * (1 - fy)
             + p10 * (1 - fx) * fy + p11 * fx * fy)
        out[..., c] = np.where(ok, v, 0.0)
    return out

# 一个非平凡透视矩阵（映射到同样 8x8）
src = np.float32([[1, 1], [6, 0], [7, 6], [0, 7]])
dst = np.float32([[0, 0], [7, 0], [7, 7], [0, 7]])
M = cv2.getPerspectiveTransform(src, dst)
Mi = np.linalg.inv(M.astype(np.float64))

out = cv2.warpPerspective(img, M, (8, 8), flags=cv2.INTER_LINEAR)

ys, xs = np.mgrid[0:8, 0:8].astype(np.float64)
den = Mi[2, 0] * xs + Mi[2, 1] * ys + Mi[2, 2]
sx = (Mi[0, 0] * xs + Mi[0, 1] * ys + Mi[0, 2]) / den
sy = (Mi[1, 0] * xs + Mi[1, 1] * ys + Mi[1, 2]) / den

ref = exact_bilinear(img, sx, sy)  # 约定: dst(x,y)=src(Mi·(x,y))，整数=中心
d = np.abs(out.astype(np.float64) - ref)
print(f"[实验2] cv2 vs 精确双线性(整数=中心约定): 最大差 {d.max():.2f} "
      f"不一致 {int((d > 0).sum())}/{d.size}")

# 对照: 若 cv2 用 (x+0.5,y+0.5) 约定
sx2 = (Mi[0, 0] * (xs + .5) + Mi[0, 1] * (ys + .5) + Mi[0, 2]) / \
      (Mi[2, 0] * (xs + .5) + Mi[2, 1] * (ys + .5) + Mi[2, 2])
sy2 = (Mi[1, 0] * (xs + .5) + Mi[1, 1] * (ys + .5) + Mi[1, 2]) / \
      (Mi[2, 0] * (xs + .5) + Mi[2, 1] * (ys + .5) + Mi[2, 2])
ref2 = exact_bilinear(img, sx2, sy2)
d2 = np.abs(out.astype(np.float64) - ref2)
print(f"[实验2b] cv2 vs (x+0.5 约定): 最大差 {d2.max():.2f} "
      f"不一致 {int((d2 > 0).sum())}/{d2.size}")

# ---- 实验 3：定点量化检验 ----
# 若 cv2 把源坐标量化为 k 位小数，则与精确值的差应呈量化台阶
# 直接看: cv2 - 精确 的分布是否有系统性偏移
diff = out.astype(np.float64) - ref
print(f"[实验3] cv2-精确 的偏差范围 [{diff.min():.3f}, {diff.max():.3f}] "
      f"均值 {diff.mean():.4f}")

# ---- 实验 4: 定点候选 - INTER_TAB_SIZE=32 (INTER_BITS=5) ----
def fixed32(img, sx, sy):
    h, w = img.shape[:2]
    X = np.round(sx * 32).astype(int)
    Y = np.round(sy * 32).astype(int)
    x0 = X >> 5; y0 = Y >> 5
    ax = (X & 31) / 32.0; ay = (Y & 31) / 32.0
    x1 = x0 + 1; y1 = y0 + 1
    ok = (x0 >= 0) & (y0 >= 0) & (x1 < w) & (y1 < h)
    x0c = np.clip(x0, 0, w - 1); x1c = np.clip(x1, 0, w - 1)
    y0c = np.clip(y0, 0, h - 1); y1c = np.clip(y1, 0, h - 1)
    out = np.zeros(img.shape, np.float64)
    for c in range(img.shape[2]):
        p00 = img[y0c, x0c, c].astype(np.float64)
        p01 = img[y0c, x1c, c].astype(np.float64)
        p10 = img[y1c, x0c, c].astype(np.float64)
        p11 = img[y1c, x1c, c].astype(np.float64)
        v = (p00 * (1 - ax) * (1 - ay) + p01 * ax * (1 - ay)
             + p10 * (1 - ax) * ay + p11 * ax * ay)
        out[..., c] = np.where(ok, v, 0.0)
    return out

ref4 = fixed32(img, sx, sy)
d4 = np.abs(out.astype(np.float64) - ref4)
print(f"[实验4] cv2 vs 定点(5bit, 整数=中心): 最大差 {d4.max():.2f} "
      f"不一致 {int((d4 > 0).sum())}/{d4.size}")

# ---- 实验 5: INTER_BITS=11 ----
def fixedN(img, sx, sy, bits):
    k = 1 << bits
    X = np.round(sx * k).astype(np.int64)
    Y = np.round(sy * k).astype(np.int64)
    x0 = X >> bits; y0 = Y >> bits
    ax = (X & (k - 1)) / float(k); ay = (Y & (k - 1)) / float(k)
    h, w = img.shape[:2]
    x1 = x0 + 1; y1 = y0 + 1
    ok = (x0 >= 0) & (y0 >= 0) & (x1 < w) & (y1 < h)
    x0c = np.clip(x0, 0, w - 1); x1c = np.clip(x1, 0, w - 1)
    y0c = np.clip(y0, 0, h - 1); y1c = np.clip(y1, 0, h - 1)
    out = np.zeros(img.shape, np.float64)
    for c in range(img.shape[2]):
        p00 = img[y0c, x0c, c].astype(np.float64)
        p01 = img[y0c, x1c, c].astype(np.float64)
        p10 = img[y1c, x0c, c].astype(np.float64)
        p11 = img[y1c, x1c, c].astype(np.float64)
        v = (p00 * (1 - ax) * (1 - ay) + p01 * ax * (1 - ay)
             + p10 * (1 - ax) * ay + p11 * ax * ay)
        out[..., c] = np.where(ok, v, 0.0)
    return out

for bits in (10, 11, 12):
    r = fixedN(img, sx, sy, bits)
    dd = np.abs(out.astype(np.float64) - r)
    print(f"[实验5] cv2 vs 定点({bits}bit, 整数=中心): 最大差 {dd.max():.2f} "
          f"不一致 {int((dd > 0).sum())}/{dd.size}")

print(f"\nOpenCV 版本: {cv2.__version__}")
