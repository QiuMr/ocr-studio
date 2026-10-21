"""probe 4：部分越界（x0 在界内, x1 越界）时 cv2 的行为。

img: 4x1x3，每列值 0/60/120/180。
矩阵让 dst(x) -> src(x + 3.5)：dst x=0 时 sx=3.5，x0=3 在界内, x1=4 越界。
"""
import cv2
import numpy as np

img = np.array([[[0, 0, 0], [60, 60, 60], [120, 120, 120], [180, 180, 180]]],
               dtype=np.uint8)

M = np.float32([[1.0, 0, -3.5], [0, 1, 0], [0, 0, 1]])  # dst(x) = src(x) - 3.5
out = cv2.warpPerspective(img, M, (4, 1), flags=cv2.INTER_LINEAR)
print("dst x=0: sx=3.5 (x0=3 界内, x1=4 越界)  cv2 =", out[0, 0])
print("  整像素填 0        -> 0")
print("  越界项当 border 0 -> 0.5*180 + 0.5*0 = 90")
print("  越界项钳制到 p(3) -> 0.5*180 + 0.5*180 = 180")

# 再测负数方向: dst(x) -> src(x - 0.5)，x=0 时 sx=-0.5（x0=-1 越界, x1=0 界内）
M2 = np.float32([[1.0, 0, 0.5], [0, 1, 0], [0, 0, 1]])
out2 = cv2.warpPerspective(img, M2, (4, 1), flags=cv2.INTER_LINEAR)
print("\ndst x=0: sx=-0.5 (x0=-1 越界, x1=0 界内)  cv2 =", out2[0, 0])
print("  整像素填 0        -> 0")
print("  越界项当 border 0 -> 0.5*0 + 0.5*0 = 0")
print("  越界项钳制到 p(0) -> 0.5*0 + 0.5*0 = 0")

# 竖直方向同理
img3 = np.array([[[0], [60], [120], [180]]], dtype=np.uint8).repeat(3, 1)
M3 = np.float32([[1.0, 0, 0], [0, 1, -3.5], [0, 0, 1]])
out3 = cv2.warpPerspective(img3, M3, (3, 1), flags=cv2.INTER_LINEAR)
print("\ndst y=0: sy=3.5 (y0=3 界内, y1=4 越界)  cv2 =", out3[0, 0])

# 双边都越界
M4 = np.float32([[1.0, 0, -3.5], [0, 1, -3.5], [0, 0, 1]])
out4 = cv2.warpPerspective(img, M4, (1, 1), flags=cv2.INTER_LINEAR)
print("\ndst(0,0): sx=sy=3.5   cv2 =", out4[0, 0])
