//! 检测后处理：复刻 PaddleOCR / RapidOCR 的 DBPostProcess。
//!
//! 流程与 Python 参考实现 `scripts/ref_pipeline.py::db_postprocess` 逐步对应：
//! 1. 概率图二值化
//! 2. 轮廓追踪（Suzuki-Abe，对应 `cv2.findContours(RETR_LIST, CHAIN_APPROX_SIMPLE)`）
//! 3. 每个轮廓取最小外接矩形 + 框内均分打分
//! 4. unclip 外扩
//! 5. 再取最小外接矩形 + 打分过滤
//! 6. 坐标裁剪 + 按阅读顺序排序
//!
//! 已知与 OpenCV 的偏差（不影响识别文本，只影响框角点）：
//! - 轮廓点集不同：OpenCV 用 CHAIN_APPROX_SIMPLE 压缩共线点，imageproc 返回完整边界点。
//!   取最小外接矩形时两者等价（都由点集凸包决定）。
//! - `box_score` 的扫描线填充与 `cv2.fillPoly` 可能差 1 个边界像素，对均值影响可忽略。

use image::{GrayImage, Luma};
use imageproc::contours::find_contours;
use ndarray::ArrayView4;

use super::geometry::{box_score, contour_area, contour_length, get_mini_boxes, unclip, Pt};

/// DB 后处理参数。默认值与 PaddleOCR / RapidOCR 一致。
#[derive(Debug, Clone, Copy)]
pub struct DetConfig {
    /// 二值化阈值
    pub db_thresh: f32,
    /// 框得分阈值
    pub box_thresh: f32,
    /// unclip 膨胀比
    pub unclip_ratio: f64,
    /// 是否对二值图做膨胀（默认 false）
    pub use_dilation: bool,
    /// 最小框短边（像素）
    pub min_box_size: f64,
    /// 最多处理的轮廓数
    pub max_candidates: usize,
}

impl Default for DetConfig {
    fn default() -> Self {
        Self {
            db_thresh: 0.3,
            box_thresh: 0.6,
            unclip_ratio: 1.5,
            use_dilation: false,
            min_box_size: 3.0,
            max_candidates: 1000,
        }
    }
}

/// 一个检测到的文本行框。
#[derive(Debug, Clone)]
pub struct DetBox {
    /// 四点，原图坐标，顺序 `[左上, 右上, 右下, 左下]`
    pub points: [Pt; 4],
    /// 框内平均概率
    pub score: f64,
}

/// DB 后处理主入口。
///
/// `prob` 为 det 模型输出，形状 `[1, 1, H, W]`。
/// `ratio` 是预处理时的缩放比，用于把坐标还原回原图。
pub fn db_postprocess(
    prob: &ArrayView4<f32>,
    ratio: f64,
    cfg: &DetConfig,
) -> Vec<DetBox> {
    // 先去掉 batch 维，再去掉通道维，得到 (H, W) 概率图。
    // 注意必须分两步绑定：index_axis 返回的视图借自上一个临时值，
    // 链式调用会让中间临时值在语句结束时被释放。
    let prob3 = prob.index_axis(ndarray::Axis(0), 0);
    let prob_map = prob3.index_axis(ndarray::Axis(0), 0);
    let (height, width) = prob_map.dim();
    if height == 0 || width == 0 {
        return Vec::new();
    }

    // 1. 二值化（对应 (prob > thresh).astype(uint8)，再 *255 给 findContours）
    let mut bitmap = GrayImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let v = prob_map[[y, x]];
            bitmap.put_pixel(x as u32, y as u32, Luma([if v > cfg.db_thresh { 255 } else { 0 }]));
        }
    }

    // use_dilation：对应 PaddleOCR 的 2x2 核膨胀。默认关闭，开启时半径取 1。
    if cfg.use_dilation {
        bitmap = dilate3x3(&bitmap);
    }

    // 2. 轮廓追踪
    let contours: Vec<imageproc::contours::Contour<u32>> =
        find_contours(&bitmap);

    let mut boxes: Vec<[Pt; 4]> = Vec::new();
    let mut scores: Vec<f64> = Vec::new();

    for c in contours.iter().take(cfg.max_candidates) {
        // RETR_LIST 语义：外轮廓与孔洞轮廓都参与。
        // 孔洞轮廓会产生被前景包围的小框，靠 box_thresh 过滤掉。
        let contour: Vec<Pt> = c.points.iter().map(|p| (p.x as f64, p.y as f64)).collect();
        if contour.len() < 3 {
            continue;
        }

        // 3. 最小外接矩形 + 打分
        let Some((box_pts, short_side)) = get_mini_boxes(&contour) else {
            continue;
        };
        if short_side < cfg.min_box_size {
            continue;
        }
        let score = box_score(&prob_map, &box_pts);
        if score < cfg.box_thresh as f64 {
            continue;
        }

        // 4. unclip
        let area = contour_area(&box_pts);
        let length = contour_length(&box_pts);
        if area <= 0.0 || length <= 0.0 {
            continue;
        }
        let distance = area * cfg.unclip_ratio / length;
        let Some(expanded) = unclip(&box_pts, distance) else {
            continue;
        };

        // 5. 再取最小外接矩形
        let Some((box2, short_side2)) = get_mini_boxes(&expanded) else {
            continue;
        };
        if short_side2 < cfg.min_box_size + 2.0 {
            continue;
        }

        // 6. 坐标裁剪到概率图范围内
        let mut b = box2;
        for p in b.iter_mut() {
            p.0 = p.0.clamp(0.0, width as f64 - 1.0);
            p.1 = p.1.clamp(0.0, height as f64 - 1.0);
        }
        boxes.push(b);
        scores.push(score);
    }

    if boxes.is_empty() {
        return Vec::new();
    }

    // 还原到原图坐标 + 阅读顺序排序
    let pairs: Vec<([Pt; 4], f64)> = boxes
        .into_iter()
        .map(|b| {
            let mut r = b;
            for p in r.iter_mut() {
                p.0 /= ratio;
                p.1 /= ratio;
            }
            (r, 0.0)
        })
        .zip(scores)
        .map(|((r, _), s)| (r, s))
        .collect();

    let mut pairs = pairs;
    sort_boxes(&mut pairs, SORT_Y_TOL);

    pairs
        .into_iter()
        .map(|(points, score)| DetBox { points, score })
        .collect()
}

/// PaddleOCR `sorted_boxes` 里判定「同一行」的 y 容差，固定为 10 像素。
pub const SORT_Y_TOL: f64 = 10.0;

/// 3x3 方形膨胀（对应 cv2.dilate 的方形核）。
fn dilate3x3(img: &GrayImage) -> GrayImage {
    let (w, h) = img.dimensions();
    let mut out = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut hit = false;
            'outer: for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    if img.get_pixel(nx as u32, ny as u32).0[0] > 0 {
                        hit = true;
                        break 'outer;
                    }
                }
            }
            out.put_pixel(x, y, Luma([if hit { 255 } else { 0 }]));
        }
    }
    out
}

/// 复刻 PaddleOCR `sorted_boxes`：先按 y 总和排序，再对相邻同行的框按 x 冒泡交换。
///
/// `y_tol` 为判定「同一行」的 y 容差，PaddleOCR 用 10。
pub fn sort_boxes(pairs: &mut [([Pt; 4], f64)], y_tol: f64) {
    pairs.sort_by(|a, b| {
        let ya: f64 = a.0.iter().map(|p| p.1).sum();
        let yb: f64 = b.0.iter().map(|p| p.1).sum();
        ya.partial_cmp(&yb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let n = pairs.len();
    for i in 0..n.saturating_sub(1) {
        for j in (0..=i).rev() {
            let cur_top = pairs[j].0[0].1;
            let next_top = pairs[j + 1].0[0].1;
            if (next_top - cur_top).abs() < y_tol && pairs[j + 1].0[0].0 < pairs[j].0[0].0 {
                pairs.swap(j, j + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn contour_area_rectangle() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)];
        assert!(approx(contour_area(&pts), 50.0));
        assert!(approx(contour_length(&pts), 30.0));
    }

    #[test]
    fn unclip_expands_box() {
        // 100x30 的矩形，外扩 5 后应变成 110x40
        let box_pts = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
        let out = unclip(&box_pts, 5.0).expect("should expand");
        let area = contour_area(&out);
        assert!(area > 3000.0, "expanded area {area} should exceed 3000");
        assert!(approx(area, 110.0 * 40.0), "expected 4400, got {area}");
    }

    #[test]
    fn unclip_rotated_rect() {
        // 旋转 30° 的矩形。注意：unclip 复刻 pyclipper 的整数语义
        // （输入坐标截断 + 输出四舍五入），所以「刚体变换下面积不变」不再成立，
        // 真正的断言见 geometry.rs 的 unclip_rotated_rect_diverges_from_pyclipper_by_at_most_2px。
        let ang = 30f64.to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let base = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
        let rot: Vec<Pt> = base
            .iter()
            .map(|(x, y)| (x * c - y * s, x * s + y * c))
            .collect();
        let out = unclip(&rot, 5.0).expect("should expand");
        // 外扩必须生效：面积大于原框
        let area = contour_area(&out);
        assert!(area > 3000.0, "expanded area {area} should exceed 3000");
        // 且输出全部为整数（pyclipper 语义）
        for p in &out {
            assert_eq!(p.0.fract(), 0.0);
            assert_eq!(p.1.fract(), 0.0);
        }
    }

    #[test]
    fn min_area_rect_on_square() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let (box_pts, short_side) = get_mini_boxes(&pts).expect("rect");
        assert!(approx(short_side, 10.0), "short side {short_side}");
        assert_eq!(box_pts.len(), 4);
    }

    #[test]
    fn sort_boxes_reading_order() {
        // 三个框：第二行最左、第一行中间、第一行最左
        let a = ([(50.0, 10.0), (90.0, 10.0), (90.0, 30.0), (50.0, 30.0)], 0.9);
        let b = ([(10.0, 60.0), (70.0, 60.0), (70.0, 80.0), (10.0, 80.0)], 0.9);
        let c = ([(100.0, 10.0), (160.0, 10.0), (160.0, 30.0), (100.0, 30.0)], 0.9);
        let mut pairs = vec![b.clone(), c.clone(), a.clone()];
        sort_boxes(&mut pairs, 10.0);
        // 期望顺序：a(第一行左), c(第一行右), b(第二行)
        assert_eq!(pairs[0].0[0], a.0[0]);
        assert_eq!(pairs[1].0[0], c.0[0]);
        assert_eq!(pairs[2].0[0], b.0[0]);
    }
}
