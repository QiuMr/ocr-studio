"""钉死 pyclipper 的取整语义：坐标怎么取整、delta 怎么取整。

已知：mini box (452,371)-(471,388)，distance=6.729167 -> 实际外扩 7。
反推：坐标被取整 + delta 被取整，之后全整数运算。
"""
from __future__ import annotations

import numpy as np
import pyclipper
from shapely.geometry import Polygon

RATIO = 1.5


def offset(box, distance):
    o = pyclipper.PyclipperOffset()
    o.AddPath([tuple(float(v) for v in p) for p in box],
              pyclipper.JT_ROUND, pyclipper.ET_CLOSEDPOLYGON)
    r = o.Execute(distance)
    return r


def bbox_of(res):
    """返回结果的轴对齐包围盒（对轴对齐矩形即其本身）。"""
    if not res:
        return None
    pts = np.array(res[0], dtype=np.float64)
    return (pts[:, 0].min(), pts[:, 1].min(), pts[:, 0].max(), pts[:, 1].max())


print("=== 实验 1: delta 取整阈值 ===")
rect = [(452, 371), (471, 371), (471, 388), (452, 388)]
for d in [6.0, 6.4, 6.49, 6.5, 6.51, 6.729167, 6.9, 7.0]:
    bb = bbox_of(offset(rect, d))
    print(f"  delta={d:<10} -> bbox={bb}  外扩={452 - bb[0]:.4f}")

print("\n=== 实验 2: 坐标取整（浮点角点）===")
for c in [(452.0, 371.0), (452.4, 371.4), (452.5, 371.5), (452.6, 371.6), (452.9, 371.9)]:
    r = [(c[0], c[1]), (c[0] + 19, c[1]), (c[0] + 19, c[1] + 17), (c[0], c[1] + 17)]
    bb = bbox_of(offset(r, 7.0))
    print(f"  角点={c} -> bbox={bb}  左扩={c[0] - bb[0]:.4f}")

print("\n=== 实验 3: 旋转框（复刻 contour#14）===")
mini = [(58.9529, 372.0341), (372.9926, 369.9122), (373.0852, 383.6075), (59.0455, 385.7294)]
poly = Polygon(mini)
d = poly.area * RATIO / poly.length
print(f"  distance={d:.6f}  round={round(d)}")
exp = offset(mini, d)
pts = np.array(exp[0], dtype=np.float64)
print(f"  expanded 点数={len(exp[0])}")
print(f"  角点范围 x[{pts[:,0].min():.4f},{pts[:,0].max():.4f}] y[{pts[:,1].min():.4f},{pts[:,1].max():.4f}]")
import cv2
mr = cv2.minAreaRect(np.array(exp[0], dtype=np.float32).reshape(-1, 1, 2))
bp = cv2.boxPoints(mr)
print(f"  minAreaRect center={mr[0]} size={mr[1]} angle={mr[2]}")
print(f"  boxPoints={[tuple(round(float(v),4) for v in p) for p in bp]}")
print(f"  期望 mini box2 = (47.8958, 362.0966), (382.7793, 358.897), (383.1042, 392.9035), (48.2207, 396.103)")

print("\n=== 实验 4: 整数坐标 + 整数 delta 的纯矩形外扩 ===")
r_int = [(59, 372), (373, 370), (373, 384), (59, 386)]
for dd in [9, 10, 11]:
    exp = offset(r_int, float(dd))
    pts = np.array(exp[0], dtype=np.float64)
    print(f"  delta={dd} -> 点数={len(exp[0])} x[{pts[:,0].min():.1f},{pts[:,0].max():.1f}] y[{pts[:,1].min():.1f},{pts[:,1].max():.1f}]")
