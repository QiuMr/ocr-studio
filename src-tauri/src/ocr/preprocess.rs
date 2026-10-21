//! 预处理：把图像变成 ONNX 模型需要的 NCHW float32 张量。
//!
//! 所有参数与 Python 参考实现（scripts/ref_pipeline.py）保持一致，
//! 这是精度对齐的前提——改这里任何一个常量，都必须同步改参考实现。

use image::{DynamicImage, GenericImageView, RgbImage};
use ndarray::Array4;

/// det 预处理参数
pub const DET_LIMIT_SIDE_LEN: u32 = 960;
pub const DET_STRIDE: u32 = 32;
pub const DET_MIN_SIDE: u32 = 32;
pub const DET_MEAN: f32 = 0.5;
pub const DET_STD: f32 = 0.5;

/// rec 预处理参数（PP-OCRv6 rec.onnx 输入为 [N,3,48,W]）
pub const REC_IMG_H: u32 = 48;
pub const REC_IMG_W: u32 = 320;
pub const REC_MEAN: f32 = 0.5;
pub const REC_STD: f32 = 0.5;

/// cls 预处理参数（PP-OCRv5 cls.onnx 输入固定为 [N,3,80,160]）
pub const CLS_IMG_H: u32 = 80;
pub const CLS_IMG_W: u32 = 160;

/// 双线性缩放，**逐位复刻 `cv2.resize(..., INTER_LINEAR)`**。
///
/// 为什么不用 `image` crate 的 `resize_exact(Triangle)`：两者的采样位置与
/// 权重公式不同，会系统性改变识别置信度。为了和参考实现严格对齐，这里按
/// OpenCV 4.10 源码（`modules/imgproc/src/resize.cpp`）的通用 8u 路径逐位复刻：
///
/// ```text
/// 横向（HResizeLinear，11bit 定点，不舍入）:
///     H[y][x] = p[y][x0] * a0 + p[y][x0+1] * a1      (a0 + a1 = 2048)
/// 纵向（VResizeLinear 的 8u 特化，分级截断）:
///     out = ((b0*(H[y0][x] >> 4)) >> 16
///          + (b1*(H[y1][x] >> 4)) >> 16 + 2) >> 2
/// ```
///
/// 四个关键细节，全部实测确认（见 `scripts/probe_resize*.py`）：
/// 1. **坐标约定**：`sx = (dx+0.5)*scale - 0.5`，钳位到 `[0, size-1]`。
///    整数坐标 = 像素中心，不要再 ±0.5。
/// 2. **系数量化到 1/2048**（`INTER_RESIZE_COEF_BITS=11`），`cvRound`
///    就近舍入、平局取偶；`a0` 取补数保证 `a0+a1 == 2048`。
/// 3. **横向结果保留 11bit 定点**（int 缓冲，不舍入）；纵向先 `>>4`
///    **截断**到 7bit，再 `>>16` 截断，最后 `+2 >> 2` 舍入。
///    这些截断都朝零，所以结果系统性略低于精确值——实测 cv2 在精确值
///    frac∈[0.5,0.73] 时会给小 1，就是这个原因。
/// 4. **右/下边界退化成单 tap**（`a1`/`b1` 为 0，直接乘 2048）。
///
/// 已知残差：首输出行仍有 ≤1 LSB、约 0.1% 的偏差（OpenCV 在该行走另一条
/// 内部路径，外部无法完全还原；实测全精度实现是 4%，本实现是 0.12%）。
/// 该残差不影响识别：实测真实文本框置信度差 ≤0.02。
pub fn resize_bilinear(img: &RgbImage, dst_w: u32, dst_h: u32) -> RgbImage {
    let (sw, sh) = img.dimensions();
    if dst_w == 0 || dst_h == 0 || sw == 0 || sh == 0 {
        return RgbImage::new(dst_w, dst_h);
    }
    if sw == dst_w && sh == dst_h {
        return img.clone();
    }

    let xmap = axis_map(dst_w, sw);
    let ymap = axis_map(dst_h, sh);
    let dw = dst_w as usize;
    let src = img.as_raw();
    let sstride = sw as usize * 3;

    // ---- 横向：每个源行产出 11bit 定点缓冲 ----
    let mut hbuf = vec![0i32; sh as usize * dw * 3];
    for y in 0..sh as usize {
        let row = &src[y * sstride..y * sstride + sstride];
        let out_row = &mut hbuf[y * dw * 3..(y + 1) * dw * 3];
        for x in 0..dw {
            let sx = xmap.idx[x] * 3;
            let (a0, a1) = (xmap.a0[x], xmap.a1[x]);
            let p0 = &row[sx..sx + 3];
            if a1 != 0 {
                let p1 = &row[sx + 3..sx + 6];
                for c in 0..3 {
                    out_row[x * 3 + c] = p0[c] as i32 * a0 + p1[c] as i32 * a1;
                }
            } else {
                // 右/左边界单 tap：a0 == 2048
                for c in 0..3 {
                    out_row[x * 3 + c] = p0[c] as i32 * a0;
                }
            }
        }
    }

    // ---- 纵向：分级截断 + 最后舍入 ----
    let mut raw = vec![0u8; dw * dst_h as usize * 3];
    for y in 0..dst_h as usize {
        let sy = ymap.idx[y];
        let (b0, b1) = (ymap.a0[y], ymap.a1[y]);
        let r0 = &hbuf[sy * dw * 3..(sy + 1) * dw * 3];
        let r1 = if b1 != 0 {
            &hbuf[(sy + 1) * dw * 3..(sy + 2) * dw * 3]
        } else {
            r0
        };
        let out_row = &mut raw[y * dw * 3..(y + 1) * dw * 3];
        for i in 0..dw * 3 {
            let t0 = (b0 * (r0[i] >> 4)) >> 16;
            let t1 = (b1 * (r1[i] >> 4)) >> 16;
            out_row[i] = ((t0 + t1 + 2) >> 2) as u8;
        }
    }
    RgbImage::from_raw(dst_w, dst_h, raw).expect("缓冲尺寸与目标尺寸一致")
}

/// 单轴的坐标/系数表，对应 OpenCV 主循环里的 `xofs` / `cbuf` 填充。
struct AxisMap {
    /// 源像素索引
    idx: Vec<usize>,
    /// 两个权重，`a0 + a1 == 2048`
    a0: Vec<i32>,
    a1: Vec<i32>,
}

fn axis_map(dst_n: u32, src_n: u32) -> AxisMap {
    const SCALE: f64 = (1 << 11) as f64;
    let scale = src_n as f64 / dst_n as f64;
    let max_i = src_n as usize - 1;
    let mut idx = Vec::with_capacity(dst_n as usize);
    let mut a0v = Vec::with_capacity(dst_n as usize);
    let mut a1v = Vec::with_capacity(dst_n as usize);
    for d in 0..dst_n {
        // OpenCV: sx = (dx+0.5)*scale - 0.5，再钳位
        let s = ((d as f64 + 0.5) * scale - 0.5).clamp(0.0, max_i as f64);
        let i0 = s.floor();
        let fx = s - i0;
        let i0u = i0 as usize;
        if i0u >= max_i {
            // 右/下边界：单 tap
            idx.push(max_i);
            a0v.push(2048);
            a1v.push(0);
        } else {
            idx.push(i0u);
            let ax = super::geometry::round_half_even(fx * SCALE) as i32;
            a1v.push(ax);
            a0v.push(2048 - ax);
        }
    }
    AxisMap {
        idx,
        a0: a0v,
        a1: a1v,
    }
}

/// det 预处理：按比例缩放使长边不超过 `limit_side_len`，且 H/W 均 32 对齐。
///
/// 返回 `(张量, ratio)`，`ratio` 是把模型输出坐标还原回原图时要除的比例。
pub fn det_preprocess(
    img: &DynamicImage,
    limit_side_len: u32,
    limit_type: DetLimitType,
) -> (Array4<f32>, f64) {
    let (w, h) = img.dimensions();
    let ratio = match limit_type {
        DetLimitType::Max => {
            if h.max(w) > limit_side_len {
                limit_side_len as f64 / h.max(w) as f64
            } else {
                1.0
            }
        }
        DetLimitType::Min => {
            if h.min(w) < limit_side_len {
                limit_side_len as f64 / h.min(w) as f64
            } else {
                1.0
            }
        }
    };

    let resize_h = ((h as f64 * ratio / DET_STRIDE as f64).round() as u32 * DET_STRIDE).max(DET_MIN_SIDE);
    let resize_w = ((w as f64 * ratio / DET_STRIDE as f64).round() as u32 * DET_STRIDE).max(DET_MIN_SIDE);

    let resized = resize_bilinear(&img.to_rgb8(), resize_w, resize_h);

    // HWC -> CHW，并做 (x/255 - mean) / std 归一化
    let rgb = resized;
    let mut tensor = Array4::<f32>::zeros((1, 3, resize_h as usize, resize_w as usize));
    for (x, y, px) in rgb.enumerate_pixels() {
        let [r, g, b] = px.0;
        tensor[[0, 0, y as usize, x as usize]] = (r as f32 / 255.0 - DET_MEAN) / DET_STD;
        tensor[[0, 1, y as usize, x as usize]] = (g as f32 / 255.0 - DET_MEAN) / DET_STD;
        tensor[[0, 2, y as usize, x as usize]] = (b as f32 / 255.0 - DET_MEAN) / DET_STD;
    }

    (tensor, ratio)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetLimitType {
    Max,
    Min,
}

/// rec 预处理：高度固定 48，宽按原图宽高比缩放，右侧补零到 320。
///
/// 返回 `(张量, 有效宽度)`。`有效宽度` 之外的部分是补的零，
/// CTC 解码时理论上可以据此截断，但参考实现没有截断，这里保持一致。
pub fn rec_preprocess(box_img: &DynamicImage) -> (Array4<f32>, usize) {
    let (w, h) = box_img.dimensions();
    if w == 0 || h == 0 {
        // 退化输入：返回全零张量，调用方应自行判空跳过
        return (Array4::<f32>::zeros((1, 3, REC_IMG_H as usize, REC_IMG_W as usize)), 0);
    }

    let wh_ratio = w as f64 / h as f64;
    let resized_w = if wh_ratio * REC_IMG_H as f64 > REC_IMG_W as f64 {
        REC_IMG_W
    } else {
        ((REC_IMG_H as f64 * wh_ratio).round() as u32).max(1)
    };

    let resized = resize_bilinear(&box_img.to_rgb8(), resized_w, REC_IMG_H);
    let rgb = resized;

    let mut tensor = Array4::<f32>::zeros((1, 3, REC_IMG_H as usize, REC_IMG_W as usize));
    for (x, y, px) in rgb.enumerate_pixels() {
        let [r, g, b] = px.0;
        tensor[[0, 0, y as usize, x as usize]] = (r as f32 / 255.0 - REC_MEAN) / REC_STD;
        tensor[[0, 1, y as usize, x as usize]] = (g as f32 / 255.0 - REC_MEAN) / REC_STD;
        tensor[[0, 2, y as usize, x as usize]] = (b as f32 / 255.0 - REC_MEAN) / REC_STD;
    }

    (tensor, resized_w as usize)
}

/// 把 RgbImage 归一化成 rec 输入（供已裁剪好的图使用）。
pub fn rgb_to_rec_tensor(rgb: &RgbImage) -> (Array4<f32>, usize) {
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        return (Array4::<f32>::zeros((1, 3, REC_IMG_H as usize, REC_IMG_W as usize)), 0);
    }
    let dyn_img = DynamicImage::ImageRgb8(rgb.clone());
    rec_preprocess(&dyn_img)
}

/// 便捷函数：直接对 (H, W, 3) 的 ndarray 做 rec 预处理。
pub fn array_to_rec_tensor(arr: &ndarray::ArrayView3<u8>) -> (Array4<f32>, usize) {
    let h = arr.shape()[0] as u32;
    let w = arr.shape()[1] as u32;
    let mut rgb = RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            rgb.put_pixel(x, y, image::Rgb([arr[[y as usize, x as usize, 0]],
                                             arr[[y as usize, x as usize, 1]],
                                             arr[[y as usize, x as usize, 2]]]));
        }
    }
    rgb_to_rec_tensor(&rgb)
}

/// 便捷函数：DynamicImage 的宽高。
pub fn dims(img: &DynamicImage) -> (u32, u32) {
    img.dimensions()
}

/// 由 `scripts/gen_resize_fixture.py` 生成的 cv2 对齐 fixture 测试。
#[cfg(test)]
#[path = "resize_fixture_tests.rs"]
mod resize_fixture_tests;
