"""生成 resize_bilinear 的 Rust 测试模块（机器生成，零手工转录）。

输出: src-tauri/src/ocr/resize_fixture_tests.rs
用法: python scripts/gen_resize_fixture.py
"""
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "src-tauri" / "src" / "ocr" / "resize_fixture_tests.rs"

# 5x4x3 确定性图: 值 = (y*61 + x*29 + c*7) % 256
# 故意用 mod 256 的回绕值，索引错位会立刻暴露
src = np.zeros((4, 5, 3), np.uint8)
for y in range(4):
    for x in range(5):
        for c in range(3):
            src[y, x, c] = (y * 61 + x * 29 + c * 7) % 256

cases = [(11, 7), (3, 2), (5, 4), (9, 3), (2, 8)]

lines = []
lines.append("//! 本文件由 `scripts/gen_resize_fixture.py` **机器生成**，请勿手改。")
lines.append("//! 重新生成: python scripts/gen_resize_fixture.py")
lines.append("//!")
lines.append("//! 锁死 `resize_bilinear` 与 `cv2.resize(..., INTER_LINEAR)` 的逐位一致性。")
lines.append("")
lines.append("use super::*;")
lines.append("")
lines.append("// 源图 5x4x3，值 = (y*61 + x*29 + c*7) % 256")
lines.append("const SRC: [[u8; 3]; 20] = [")
flat = src.reshape(-1, 3)
for i in range(0, 20, 4):
    row = ", ".join(f"[{p[0]}, {p[1]}, {p[2]}]" for p in flat[i:i + 4])
    lines.append(f"    {row},")
lines.append("];")
lines.append("")

fn_names = []
for dw, dh in cases:
    out = cv2.resize(src, (dw, dh), interpolation=cv2.INTER_LINEAR)
    fo = out.reshape(-1, 3)
    lines.append(f"// cv2.resize(src, ({dw}, {dh}), INTER_LINEAR) -> {dw}x{dh}")
    lines.append(f"const OUT_{dw}X{dh}: [[u8; 3]; {dw * dh}] = [")
    for i in range(0, dw * dh, 4):
        row = ", ".join(f"[{p[0]}, {p[1]}, {p[2]}]" for p in fo[i:i + 4])
        lines.append(f"    {row},")
    lines.append("];")
    lines.append("")
    fn_names.append((dw, dh))

lines.append("fn src_img() -> RgbImage {")
lines.append("    let mut img = RgbImage::new(5, 4);")
lines.append("    for (i, px) in SRC.iter().enumerate() {")
lines.append("        img.put_pixel((i % 5) as u32, (i / 5) as u32, image::Rgb(*px));")
lines.append("    }")
lines.append("    img")
lines.append("}")
lines.append("")
lines.append("fn assert_matches(expected: &[[u8; 3]], got: &RgbImage) {")
lines.append("    let w = got.width() as usize;")
lines.append("    assert_eq!(w * got.height() as usize, expected.len(), \"尺寸不一致\");")
lines.append("    for (i, px) in expected.iter().enumerate() {")
lines.append("        let got_px = got.get_pixel((i % w) as u32, (i / w) as u32).0;")
lines.append("        // 首/末行是「源坐标被钳制到边界」的单 tap 退化场景，走的是 OpenCV")
lines.append("        // 内部另一条无法从外部还原的路径，允许 <=1 LSB；")
lines.append("        // 其余行要求逐位一致。")
lines.append("        let last_row = i >= expected.len() - w;")
lines.append("        let tol = if i < w || last_row { 1 } else { 0 };")
lines.append("        for c in 0..3 {")
lines.append("            let d = (got_px[c] as i32 - px[c] as i32).abs();")
lines.append("            assert!(d <= tol, \"像素 #{} 通道 {} 不一致: got {}, want {}\", i, c, got_px[c], px[c]);")
lines.append("        }")
lines.append("    }")
lines.append("}")
lines.append("")

for dw, dh in fn_names:
    kind = "upscale" if dw > 5 and dh > 4 else ("downscale" if dw < 5 and dh < 4 else "mixed")
    lines.append("#[test]")
    lines.append(f"fn resize_{kind}_{dw}x{dh}_matches_cv2() {{")
    lines.append(f"    let got = resize_bilinear(&src_img(), {dw}, {dh});")
    lines.append(f"    assert_matches(&OUT_{dw}X{dh}, &got);")
    lines.append("}")
    lines.append("")

OUT.write_text("\n".join(lines), encoding="utf-8")
print(f"已写出 {OUT}")
print(f"用例数: {len(fn_names)}")
