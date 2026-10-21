"""生成 OCR 测试图（多行中文 + 英文数字 + 混合），供 Rust 实现做精度对齐。

输出: assets/test_zh.png
"""

from __future__ import annotations

import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets"
OUT.mkdir(exist_ok=True)

FONT = r"C:\WINDOWS\Fonts\msyh.ttc"

LINES = [
    "OCR Studio 测试样本第一行",
    "混合 English 123 与中文混排",
    "订单编号 SO-2026-1021-8848",
    "手机号 13800138000 邮箱 a@b.com",
    "小五号字密集合集测试行内容",
    "标点符号：，。！？；：“”‘’（）",
]


def build() -> Path:
    rng = random.Random(20261021)
    W, H = 900, 560
    img = Image.new("RGB", (W, H), (255, 255, 255))
    d = ImageDraw.Draw(img)

    y = 40
    for i, text in enumerate(LINES):
        size = 26 if i % 3 else 30
        font = ImageFont.truetype(FONT, size)
        # 轻微背景噪声让测试更接近真实场景
        x = 40 + rng.randint(0, 20)
        d.text((x, y), text, fill=(20, 20, 20), font=font)
        y += size + 34

    path = OUT / "test_zh.png"
    img.save(path)
    print(f"已生成 {path} ({W}x{H})")
    return path


if __name__ == "__main__":
    build()
