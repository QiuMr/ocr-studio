"""算出 Rust 单测三个旋转/公式用例的 pyclipper 真值，用于更新测试期望。"""
from __future__ import annotations

import math

import numpy as np
import pyclipper
from shapely.geometry import Polygon


def offset(box, distance):
    o = pyclipper.PyclipperOffset()
    o.AddPath([tuple(float(v) for v in p) for p in box],
              pyclipper.JT_ROUND, pyclipper.ET_CLOSEDPOLYGON)
    return o.Execute(distance)


def shoelace_area(pts):
    p = np.array(pts, dtype=np.float64)
    x, y = p[:, 0], p[:, 1]
    return abs(float(np.sum(x * np.roll(y, -1) - np.roll(x, -1) * y)) / 2.0)


def perimeter(pts):
    p = np.array(pts, dtype=np.float64)
    return float(np.sum(np.hypot(*(np.roll(p, -1, axis=0) - p).T)))


print("=== 用例 A: 100x30 旋转 30°，distance=5 ===")
ang = math.radians(30)
c, s = math.cos(ang), math.sin(ang)
base = [(0, 0), (100, 0), (100, 30), (0, 30)]
rot = [(x * c - y * s, x * s + y * c) for (x, y) in base]
print("  rot =", [tuple(round(v, 6) for v in p) for p in rot])
res = offset(rot, 5.0)
pts = np.array(res[0], dtype=np.float64)
print(f"  输出点数={len(res[0])}")
print(f"  坐标={[tuple(round(float(v), 4) for v in p) for p in res[0]]}")
print(f"  shoelace area={shoelace_area(res[0]):.6f}")
print(f"  minAreaRect:")
import cv2
mr = cv2.minAreaRect(pts.astype(np.float32).reshape(-1, 1, 2))
print(f"    center={mr[0]} size={mr[1]} angle={mr[2]}")

print("\n=== 用例 B: 380x13，distance = area*1.5/length ===")
w, h = 380.0, 13.0
box = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
area = shoelace_area(box)
length = perimeter(box)
d = area * 1.5 / length
print(f"  area={area} length={length} distance={d:.6f}")
res = offset(box, d)
pts = np.array(res[0], dtype=np.float64)
print(f"  输出点数={len(res[0])}")
print(f"  shoelace area={shoelace_area(res[0]):.6f}")
print(f"  bbox x[{pts[:,0].min():.4f},{pts[:,0].max():.4f}] y[{pts[:,1].min():.4f},{pts[:,1].max():.4f}]")
mr = cv2.minAreaRect(pts.astype(np.float32).reshape(-1, 1, 2))
print(f"  minAreaRect size={mr[1]}")
new_h = shoelace_area(res[0]) / (w + 2 * d)
print(f"  out_area/(w+2d) = {new_h:.6f}   理论 h+2d = {h + 2*d:.6f}")

print("\n=== 用例 C: mini box (452,371)-(471,388)，distance=6.729167 ===")
box = [(452.0, 371.0), (471.0, 371.0), (471.0, 388.0), (452.0, 388.0)]
res = offset(box, 6.729166666666667)
print(f"  坐标={[tuple(float(v) for v in p) for p in res[0]]}")
