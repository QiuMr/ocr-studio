"""probe 6：x0 越界（sx<0）但 x1 在界内时，cv2 的行为。

img: 4x1，值 [180, 180, 180, 180]（统一值便于算）。
M 让 dst(x) -> src(x - 0.5)：dst x=0 时 sx=-0.5, x0=-1 越界, x1=0 界内, ax=0.5。
"""
import cv2
import numpy as np

img = np.full((1, 4, 3), 180, np.uint8)

M = np.float32([[1.0, 0, 0.5], [0, 1, 0], [0, 0, 1]])  # dst(x) = src(x) + 0.5
out = cv2.warpPerspective(img, M, (1, 1), flags=cv2.INTER_LINEAR)
print("dst(0,0) sx=-0.5 (x0=-1 越界, x1=0 界内, ax=1/2): cv2 =", out[0, 0])
print("  逐 tap 补 0  -> 0.5*0 + 0.5*180 = 90")
print("  整像素填 0   -> 0")

# sx=-0.25: x0=-1 越界, x1=0 界内, ax=3/4
M2 = np.float32([[1.0, 0, 0.25], [0, 1, 0], [0, 0, 1]])
out2 = cv2.warpPerspective(img, M2, (1, 1), flags=cv2.INTER_LINEAR)
print("dst(0,0) sx=-0.25 (ax=3/4): cv2 =", out2[0, 0], " 逐tap -> 0.75*180 = 135")

# sx=-1.5: x0=-2, x1=-1 都越界
M3 = np.float32([[1.0, 0, 1.5], [0, 1, 0], [0, 0, 1]])
out3 = cv2.warpPerspective(img, M3, (1, 1), flags=cv2.INTER_LINEAR)
print("dst(0,0) sx=-1.5 (x0,x1 都越界): cv2 =", out3[0, 0], " 应 0")

# 竖直方向: sy=-0.5, y0=-1 越界, y1=0 界内
img4 = np.full((4, 1, 3), 180, np.uint8)
M4 = np.float32([[1.0, 0, 0], [0, 1, 0.5], [0, 0, 1]])
out4 = cv2.warpPerspective(img4, M4, (1, 1), flags=cv2.INTER_LINEAR)
print("dst(0,0) sy=-0.5 (y0=-1 越界, y1=0 界内): cv2 =", out4[0, 0])
