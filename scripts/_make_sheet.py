"""生成验收用的表格图（只跑一次，产物进 assets/ 跟着仓库走）。

跑法（PowerShell 的 python）：
    python scripts/_make_sheet.py
"""
from PIL import Image, ImageDraw, ImageFont

W, H = 420, 260
img = Image.new("RGB", (W, H), "white")
d = ImageDraw.Draw(img)
try:
    f = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 20)
except OSError:
    f = ImageFont.load_default()

cols = [10, 140, 270, 410]
rows = [10, 90, 170, 250]
for r0, r1 in zip(rows, rows[1:]):
    for c0, c1 in zip(cols, cols[1:]):
        d.rectangle([c0, r0, c1, r1], outline="black", width=2)

text = [
    ["名称", "数量", "单价"],
    ["苹果", "12", "3.0"],
    ["香蕉", "8", "2.5"],
]
for r, row in enumerate(text):
    for c, t in enumerate(row):
        d.text((cols[c] + 16, rows[r] + 28), t, font=f, fill="black")

img.save("assets/sheet.png")
print("已生成 assets/sheet.png", img.size)
