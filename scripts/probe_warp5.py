"""probe 5：修正版竖直方向部分越界测试。img: 4 高 1 宽，值 0/60/120/180。"""
import cv2
import numpy as np

img = np.array([[[0, 0, 0]], [[60, 60, 60]], [[120, 120, 120]], [[180, 180, 180]]],
               dtype=np.uint8)  # (4, 1, 3)

# dst(0,0) -> src(0, 3.5): y0=3 界内, y1=4 越界
M = np.float32([[1.0, 0, 0], [0, 1, -3.5], [0, 0, 1]])
out = cv2.warpPerspective(img, M, (1, 1), flags=cv2.INTER_LINEAR)
print("竖直 sy=3.5 (y0=3 界内, y1=4 越界): cv2 =", out[0, 0])
print("  期望(水平方向同行为): 0.5*180 + 0.5*0 = 90")

# dst(0,0) -> src(0, -0.5): y0=-1 越界, y1=0 界内
M2 = np.float32([[1.0, 0, 0], [0, 1, 0.5], [0, 0, 1]])
out2 = cv2.warpPerspective(img, M2, (1, 1), flags=cv2.INTER_LINEAR)
print("竖直 sy=-0.5 (y0=-1 越界, y1=0 界内): cv2 =", out2[0, 0])

# 双向都部分越界: img 4x4, dst(0,0) -> src(3.5, 3.5)
img4 = np.arange(16, dtype=np.uint8).reshape(4, 4, 1).repeat(3, 2)
M3 = np.float32([[1.0, 0, -3.5], [0, 1, -3.5], [0, 0, 1]])
out3 = cv2.warpPerspective(img4, M3, (1, 1), flags=cv2.INTER_LINEAR)
print("\nsx=sy=3.5 双向部分越界: cv2 =", out3[0, 0])
print("  p(3,3)=15, 三个越界 tap 计 0 -> 0.25*15 =", 0.25 * 15)

# 量化验证: 上面所有场景都走 1/32 定点吗? 用一个有 .5/32 小数的坐标
# sx = 2.015625 = 2 + 1/64 -> X = round(2.015625*32) = round(64.5) = 64 or 65?
img5 = np.array([[[0, 0, 0], [64, 64, 64], [128, 128, 128], [192, 192, 192]]],
                dtype=np.uint8)
M4 = np.float32([[1.0, 0, -2.015625], [0, 1, 0], [0, 0, 1]])
out4 = cv2.warpPerspective(img5, M4, (1, 1), flags=cv2.INTER_LINEAR)
print("\nsx=2.015625 (=2+1/64, X=round(64.5) 平局): cv2 =", out4[0, 0])
print("  X=64 -> ax=0  -> 64")
print("  X=65 -> ax=1/32 -> 64 + (128-64)/32 = 66")
print("  (顺便验证 1/32 量化: 若全精度, 64 + 64/64 = 65)")
