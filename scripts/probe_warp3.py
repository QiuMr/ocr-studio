"""probe 3：cv2.warpPerspective 越界时的 border 行为。

覆盖三种情况：整体越界、x1 单侧越界、恰好在边界上。
"""
import cv2
import numpy as np

img = np.full((8, 8, 3), 100, np.uint8)
img[:, :] = np.arange(8)[None, :, None] * 10  # 每列值不同，便于定位

# 构造把 dst 映射到 src 外侧的矩阵：dst(x,y) -> src(x*2 - 4, y)
# 即 M: src -> dst 为 x_dst = (x_src + 4)/2
M = np.float32([[0.5, 0, 2.0], [0, 1, 0], [0, 0, 1]])
out = cv2.warpPerspective(img, M, (8, 8), flags=cv2.INTER_LINEAR)
Mi = np.linalg.inv(M.astype(np.float64))

ys, xs = np.mgrid[0:8, 0:8].astype(np.float64)
den = Mi[2, 0] * xs + Mi[2, 1] * ys + Mi[2, 2]
sx = (Mi[0, 0] * xs + Mi[0, 1] * ys + Mi[0, 2]) / den
sy = (Mi[1, 0] * xs + Mi[1, 1] * ys + Mi[1, 2]) / den

print("dst(0..7, 3) 的源坐标 sx 与 cv2 输出:")
for x in range(8):
    print(f"  x={x}  sx={sx[x, 3]:8.4f}  cv2={out[3, x]}")

print("\n结论：")
print("  sx < 0        -> 全越界，应填 0")
print("  0<=sx<=6      -> 正常双线性")
print("  6<sx<7        -> x0=6 在界内, x1=7 在界内(因为 w=8, x1=7<=7)")
print("  7<=sx         -> x0=7 在界内, x1=8 越界 -> 看 cv2 给什么")

# 再测一版更极端的：w=4 的小图
img2 = (np.arange(4) * 60).astype(np.uint8)[None, :, None].repeat(4, 0).repeat(3, 2)
M2 = np.float32([[1.0, 0, 0], [0, 1, 0], [0, 0, 1]])  # identity
out2 = cv2.warpPerspective(img2, M2, (4, 4), flags=cv2.INTER_LINEAR)
print("\nidentity 4x4 一致:", np.array_equal(out2, img2))

# 平移 0.5: dst(x) -> src(x-0.5)
M3 = np.float32([[1.0, 0, 0.5], [0, 1, 0], [0, 0, 1]])
out3 = cv2.warpPerspective(img2, M3, (4, 4), flags=cv2.INTER_LINEAR)
print("平移 0.5 后 out3[0] =", out3[0, :, 0], " (img2[0] =", img2[0, :, 0], ")")
print("  dst x=0 -> sx=-0.5 全越界; x=1 -> sx=0.5 -> 0.5*p0+0.5*p1; ...")
print("  out3[0,0] =", out3[0, 0, 0], " 期望 0.5*0+0.5*60=30 ->", out3[0, 0, 0] == 30)
print("  out3[0,3] =", out3[0, 3, 0], " sx=2.5 -> 0.5*120+0.5*180=150 ->", out3[0, 3, 0] == 150)
