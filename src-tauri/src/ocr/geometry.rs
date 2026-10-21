//! 几何与光栅化原语。
//!
//! 这些函数对应 Python 参考实现里的 OpenCV / pyclipper 调用：
//!
//! | 本模块 | Python 侧 |
//! |---|---|
//! | [`min_area_rect_exact`] | `cv2.boxPoints(cv2.minAreaRect(...))` |
//! | [`unclip`] | `pyclipper.PyclipperOffset.Execute(distance)` |
//! | [`box_score`] | `cv2.fillPoly` + `cv2.mean(prob, mask)` |
//! | [`warp_perspective`] | `cv2.getPerspectiveTransform` + `cv2.warpPerspective` |
//!
//! 关于 [`min_area_rect_exact`] 与 imageproc 的取舍：
//! imageproc 0.27 也提供 `min_area_rect`，但它对结果做 floor/ceil 取整，
//! 而 OpenCV 的 `boxPoints` 不取整。取整会让框角点偏移最多 1px，
//! 再经过 unclip 放大后会传导到最终框坐标。为了和参考实现严格对齐，
//! 这里自己实现一个不取整的旋转卡壳版本。

use image::{DynamicImage, GenericImageView, Rgb};

/// 二维点。用 f64 保存中间计算，避免累积误差。
pub type Pt = (f64, f64);

/// 多边形面积（有向，逆时针为正）。对应 OpenCV 的 `contourArea`。
pub fn contour_area(pts: &[Pt]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..pts.len() {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % pts.len()];
        s += x1 * y2 - x2 * y1;
    }
    (s / 2.0).abs()
}

/// 多边形周长。
pub fn contour_length(pts: &[Pt]) -> f64 {
    let n = pts.len();
    if n < 2 {
        return 0.0;
    }
    (0..n)
        .map(|i| {
            let (x1, y1) = pts[i];
            let (x2, y2) = pts[(i + 1) % n];
            ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt()
        })
        .sum()
}

/// 求凸包。直接复用 imageproc 的 Graham 扫描实现（与 OpenCV 的凸包语义一致），
/// 避免重复造轮子。
pub fn convex_hull(pts: &[Pt]) -> Vec<Pt> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let input: Vec<imageproc::point::Point<f64>> =
        pts.iter().map(|(x, y)| imageproc::point::Point::new(*x, *y)).collect();
    imageproc::geometry::convex_hull(input)
        .into_iter()
        .map(|p| (p.x, p.y))
        .collect()
}

/// 最小面积外接旋转矩形的四个角点（不取整）。
///
/// 返回顺序为 `[左上, 右上, 右下, 左下]`，与 PaddleOCR `get_mini_boxes`
/// 期望的输入顺序一致（该函数内部还会再排序，所以顺序不敏感）。
/// 点数不足时返回 `None`。
pub fn min_area_rect_exact(pts: &[Pt]) -> Option<[Pt; 4]> {
    let hull = convex_hull(pts);
    match hull.len() {
        0 => None,
        1 => Some([hull[0]; 4]),
        2 => Some([hull[0], hull[1], hull[1], hull[0]]),
        _ => rotating_calipers(&hull),
    }
}

fn rotating_calipers(hull: &[Pt]) -> Option<[Pt; 4]> {
    let n = hull.len();
    let mut best_area = f64::MAX;
    let mut best: Option<[Pt; 4]> = None;

    for i in 0..n {
        let (px, py) = hull[i];
        let (qx, qy) = hull[(i + 1) % n];
        let (dx, dy) = (qx - px, qy - py);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-12 {
            continue;
        }
        // 边方向 u 与法向 v
        let (ux, uy) = (dx / len, dy / len);
        let (vx, vy) = (-uy, ux);

        let (mut min_u, mut max_u, mut min_v, mut max_v) =
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &(hx, hy) in hull {
            let hu = hx * ux + hy * uy;
            let hv = hx * vx + hy * vy;
            min_u = min_u.min(hu);
            max_u = max_u.max(hu);
            min_v = min_v.min(hv);
            max_v = max_v.max(hv);
        }
        let area = (max_u - min_u) * (max_v - min_v);
        if area < best_area {
            best_area = area;
            // (u,v) 局部坐标 -> 世界坐标
            let to_world = |a: f64, b: f64| (a * ux - b * uy, a * uy + b * ux);
            let c0 = to_world(min_u, min_v);
            let c1 = to_world(max_u, min_v);
            let c2 = to_world(max_u, max_v);
            let c3 = to_world(min_u, max_v);
            best = Some([c0, c1, c2, c3]);
        }
    }
    best
}

/// 复刻 PaddleOCR `get_mini_boxes`：对轮廓取最小外接矩形，并按规则排序。
///
/// 返回 `(四点, 短边长度)`。短边长度即 `cv2.minAreaRect().size` 的较小者，
/// 用于过滤过小的框。
pub fn get_mini_boxes(contour: &[Pt]) -> Option<([Pt; 4], f64)> {
    let rect = min_area_rect_exact(contour)?;
    // 对应 OpenCV boxPoints 后按 x 排序再按 y 整理的逻辑
    let mut pts: Vec<Pt> = rect.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if pts.len() < 4 {
        return None;
    }
    let (i1, i4) = if pts[1].1 > pts[0].1 { (0, 1) } else { (1, 0) };
    let (i2, i3) = if pts[3].1 > pts[2].1 { (2, 3) } else { (3, 2) };
    let box_pts = [pts[i1], pts[i2], pts[i3], pts[i4]];

    // 短边
    let w = dist(box_pts[0], box_pts[1]);
    let h = dist(box_pts[0], box_pts[3]);
    let short_side = w.min(h);
    Some((box_pts, short_side))
}

fn dist(a: Pt, b: Pt) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// unclip：把凸多边形向外扩 `distance`。
///
/// 对应 pyclipper 的 `PyclipperOffset.Execute(distance)`（JT_ROUND 圆角连接）。
/// 对凸多边形，圆角连接与斜接连接在后续取最小外接矩形时结果一致
/// （圆角上的点都在斜接多边形的外侧边缘附近，不改变外接矩形），
/// 因此这里用斜接实现，并对过尖的角做限幅避免数值爆炸。
/// 复刻 `pyclipper` 的整数语义：**坐标截断**。
///
/// pyclipper 的 `IntPoint` 由 double 强转而来，向零截断。实测：
/// 角点 452.6 配 delta=7，输出 445（= 452-7），而不是 446（= 452.6-7 再四舍五入）。
fn pyclipper_trunc(v: f64) -> f64 {
    v.trunc()
}

/// 复刻 `pyclipper` 的输出取整：四舍五入、**半数向上**。
///
/// 实测：delta=6.5 作用在 (452..471) 上得到 (446..478)，
/// 即 445.5→446、477.5→478，都是 half-up 而非 half-even。
fn pyclipper_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// 多边形外扩（复刻 `scripts/ref_pipeline.py::unclip`，即 PaddleOCR 的 DB unclip）。
///
/// **必须复刻 pyclipper 的整数语义**，否则框会系统性偏小：
/// 1. 输入坐标先**截断**成整数（pyclipper 的 IntPoint 是 cInt 强转）；
/// 2. 偏移距离保持 double 参与运算（截断距离会错，实测 delta=6.5 时左扩 6 右扩 7）；
/// 3. 输出坐标再**四舍五入**成整数。
///
/// 实证：mini box (452,371)-(471,388)、distance=6.729167，
/// pyclipper 给出恰好 (445,364)-(478,395)（整数），
/// 而纯浮点外扩会给出 (445.2708,364.2708)-(477.7292,394.7292)。
/// 这 0.27px 的差会让 32x30 的裁剪图变成 33x31，误检框的识别置信度能差 0.15。
///
/// join 类型用斜接（miter）而非 pyclipper 的 JT_ROUND：对凸多边形，
/// 两者取最小外接矩形的结果一致（圆角弧都在外扩矩形内部），无需复刻弧的离散化。
pub fn unclip(box_pts: &[Pt], distance: f64) -> Option<Vec<Pt>> {
    // pyclipper 语义第 1 步：输入坐标截断为整数
    let ints: Vec<Pt> = box_pts.iter().map(|&(x, y)| (pyclipper_trunc(x), pyclipper_trunc(y))).collect();
    let out = unclip_float(&ints, distance)?;
    // pyclipper 语义第 3 步：输出坐标四舍五入为整数
    Some(
        out.into_iter()
            .map(|(x, y)| (pyclipper_round(x), pyclipper_round(y)))
            .collect(),
    )
}

/// 纯浮点的多边形外扩（斜接 join）。`unclip` 的整数语义封装见其文档。
fn unclip_float(box_pts: &[Pt], distance: f64) -> Option<Vec<Pt>> {
    let n = box_pts.len();
    if n < 3 || distance <= 0.0 {
        return None;
    }
    // 有向面积符号决定外法向方向
    let signed: f64 = (0..n)
        .map(|i| {
            let (x1, y1) = box_pts[i];
            let (x2, y2) = box_pts[(i + 1) % n];
            x1 * y2 - x2 * y1
        })
        .sum();
    let sign = if signed >= 0.0 { 1.0 } else { -1.0 };

    // 每条边偏移后的直线：过「边起点 + distance * 该边外法向」，方向为该边方向。
    // 顶点 cur 的新位置 = 入射边(prev->cur)偏移线 与 出射边(cur->next)偏移线 的交点。
    //
    // 易错点：入射边的偏移线起点必须用 prev 而不是 cur，方向必须用 (cur - prev)。
    // 若误用 cur 作起点、next-cur 作方向，两条线会平行，交点求不出来，
    // 结果就是原框不变（表现：外扩完全无效）。
    let mut out: Vec<Pt> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = box_pts[(i + n - 1) % n];
        let cur = box_pts[i];
        let next = box_pts[(i + 1) % n];

        let n_prev = outward_normal(prev, cur, sign);
        let n_cur = outward_normal(cur, next, sign);

        let a = (prev.0 + distance * n_prev.0, prev.1 + distance * n_prev.1);
        let b = (a.0 + (cur.0 - prev.0), a.1 + (cur.1 - prev.1));
        let c = (cur.0 + distance * n_cur.0, cur.1 + distance * n_cur.1);
        let d = (c.0 + (next.0 - cur.0), c.1 + (next.1 - cur.1));
        match line_intersection(a, b, c, d) {
            Some(p) => {
                // 斜接限幅：顶点离原始角点过远时退化为该边中点方向
                let dd = dist(p, cur);
                if dd > distance * 10.0 {
                    out.push((
                        cur.0 + distance * (n_prev.0 + n_cur.0) / 2.0,
                        cur.1 + distance * (n_prev.1 + n_cur.1) / 2.0,
                    ));
                } else {
                    out.push(p);
                }
            }
            None => out.push(cur),
        }
    }
    Some(out)
}

fn outward_normal(from: Pt, to: Pt, sign: f64) -> Pt {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-12 {
        return (0.0, 0.0);
    }
    // 有向面积为正（代数意义逆时针）时，外法向是边方向旋转 -90°： (dy, -dx)
    // 有向面积为负时取反。
    // 注意：图像坐标系 y 向下，所以「代数逆时针」在视觉上是顺时针，
    // 但公式只依赖代数符号，与视觉无关。
    if sign > 0.0 {
        (dy / len, -dx / len)
    } else {
        (-dy / len, dx / len)
    }
}

fn line_intersection(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<Pt> {
    let r = (b.0 - a.0, b.1 - a.1);
    let s = (d.0 - c.0, d.1 - c.1);
    let denom = r.0 * s.1 - r.1 * s.0;
    if denom.abs() < 1e-12 {
        return None;
    }
    let t = ((c.0 - a.0) * s.1 - (c.1 - a.1) * s.0) / denom;
    Some((a.0 + t * r.0, a.1 + t * r.1))
}

/// 框内平均概率：把多边形光栅化进 mask，再对概率图取均值。
///
/// 对应 `cv2.fillPoly(mask, [box], 1)` + `cv2.mean(prob, mask)`。
/// 采用扫描线填充；与 OpenCV 的填充规则在像素级别可能差 1 个像素，
/// 对均值影响可忽略。
pub fn box_score(prob: &ndarray::ArrayView2<f32>, box_pts: &[Pt]) -> f64 {
    let (h, w) = prob.dim();
    if box_pts.len() < 3 || h == 0 || w == 0 {
        return 0.0;
    }

    let mut y_min = f64::MAX;
    let mut y_max = f64::MIN;
    for &(_, y) in box_pts {
        y_min = y_min.min(y);
        y_max = y_max.max(y);
    }
    let y0 = (y_min.floor() as isize).max(0) as usize;
    let y1 = ((y_max.ceil() as isize).min(h as isize - 1)).max(0) as usize;
    if y1 < y0 {
        return 0.0;
    }

    let mut sum = 0.0f64;
    let mut cnt = 0u64;
    for y in y0..=y1 {
        let yc = y as f64 + 0.5;
        // 求扫描线与多边形各边的交点 x
        let mut xs: Vec<f64> = Vec::new();
        for i in 0..box_pts.len() {
            let (x1, y1) = box_pts[i];
            let (x2, y2) = box_pts[(i + 1) % box_pts.len()];
            if (y1 <= yc && yc < y2) || (y2 <= yc && yc < y1) {
                let t = (yc - y1) / (y2 - y1);
                xs.push(x1 + t * (x2 - x1));
            }
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut k = 0;
        while k + 1 < xs.len() {
            let xa = xs[k];
            let xb = xs[k + 1];
            let x0 = (xa.floor() as isize).max(0) as usize;
            let x1i = ((xb.ceil() as isize).min(w as isize - 1)).max(0) as usize;
            for x in x0..=x1i {
                sum += prob[[y, x]] as f64;
                cnt += 1;
            }
            k += 2;
        }
    }
    if cnt == 0 {
        0.0
    } else {
        sum / cnt as f64
    }
}

/// 四点排序为 `[左上, 右上, 右下, 左下]`。
///
/// 与 PaddleOCR/RapidOCR 的 `order_points` 逐字对应：
/// TL = argmin(x+y)，BR = argmax(x+y)，TR = argmin(y-x)，BL = argmax(y-x)。
/// 四个极值各自在全部 4 个点上独立取，平局取下标最小者（对应 numpy 的
/// argmin/argmax 语义，所以循环里用严格比较、先到者得）。
///
/// 两个容易踩的坑：
/// - numpy 的 `diff(pts, axis=1)` 是 **y - x**，不是 x - y；
/// - 不要「先按 x+y 挑走 TL/BR，再在剩下两点里比大小」——那是我最早写的
///   启发式，对贴边/退化四边形会和 Python 分叉，且两点 x 相等时顺序不定。
pub fn order_points(pts: [Pt; 4]) -> [Pt; 4] {
    let mut tl_i = 0usize;
    let mut br_i = 0usize;
    let mut tr_i = 0usize;
    let mut bl_i = 0usize;
    for i in 1..4 {
        if pts[i].0 + pts[i].1 < pts[tl_i].0 + pts[tl_i].1 {
            tl_i = i;
        }
        if pts[i].0 + pts[i].1 > pts[br_i].0 + pts[br_i].1 {
            br_i = i;
        }
        if pts[i].1 - pts[i].0 < pts[tr_i].1 - pts[tr_i].0 {
            tr_i = i;
        }
        if pts[i].1 - pts[i].0 > pts[bl_i].1 - pts[bl_i].0 {
            bl_i = i;
        }
    }
    [pts[tl_i], pts[tr_i], pts[br_i], pts[bl_i]]
}

/// 透视变换：把 `box` 围起来的区域裁成 `out_w x out_h` 的正视图。
///
/// 对应 `cv2.getPerspectiveTransform` + `cv2.warpPerspective`。
/// 实现方式：求 4x4 单应矩阵，再对输出像素做逆映射 + 双线性采样。
pub fn warp_perspective(
    img: &DynamicImage,
    box_pts: [Pt; 4],
) -> Option<DynamicImage> {
    WarpMap::new(box_pts)?.warp(img)
}

/// 四点框 -> 正视图的单应映射，可正反两用。
///
/// 为什么单独抽出来：单字级对齐需要把「裁剪图里的坐标」映射回原图
/// （`/ocr/danzijson` 的每个字框），而裁剪本身也要用同一份矩阵。
/// 早先 `warp_perspective` 内部算完就丢，导致外面只能再算一遍，
/// 两次数值万一不一致，字框就会整体偏移。
pub struct WarpMap {
    /// 正视图坐标 -> 原图坐标
    inv: [[f64; 3]; 3],
    out_w: u32,
    out_h: u32,
}

impl WarpMap {
    pub fn new(box_pts: [Pt; 4]) -> Option<Self> {
        let [tl, tr, br, bl] = order_points(box_pts);

        let width_a = dist(br, bl);
        let width_b = dist(tr, tl);
        let height_a = dist(tr, br);
        let height_b = dist(tl, bl);
        let out_w = width_a.max(width_b) as u32;
        let out_h = height_a.max(height_b) as u32;
        if out_w == 0 || out_h == 0 {
            return None;
        }

        // 目标矩形四角
        let dst = [
            (0.0f64, 0.0f64),
            (out_w as f64 - 1.0, 0.0f64),
            (out_w as f64 - 1.0, out_h as f64 - 1.0),
            (0.0f64, out_h as f64 - 1.0),
        ];
        let src = [tl, tr, br, bl];

        // 求 src -> dst 的单应矩阵，然后从 dst 反查 src
        let h_mat = homography(&src, &dst)?;
        let inv = invert3(&h_mat)?;
        Some(Self { inv, out_w, out_h })
    }

    /// 正视图尺寸 `(宽, 高)`。
    pub fn size(&self) -> (u32, u32) {
        (self.out_w, self.out_h)
    }

    /// 正视图坐标 -> 原图坐标。
    pub fn to_source(&self, x: f64, y: f64) -> Pt {
        apply_h(&self.inv, x, y)
    }

    /// 裁出正视图。
    pub fn warp(&self, img: &DynamicImage) -> Option<DynamicImage> {
        let (out_w, out_h) = (self.out_w, self.out_h);
        let rgb = img.to_rgb8();
        let mut out = image::RgbImage::new(out_w, out_h);

        for y in 0..out_h {
            for x in 0..out_w {
                // OpenCV 约定：**整数坐标 = 像素中心**。
                // 所以 dst(x, y) 直接取整数 (x, y) 过逆矩阵，不要再 +0.5；
                // 实测 identity 矩阵 warp 会原样返回图像，可反证这一点。
                let (sx, sy) = self.to_source(x as f64, y as f64);
                let px = sample_bilinear(&rgb, sx, sy);
                out.put_pixel(x, y, px);
            }
        }
        Some(DynamicImage::ImageRgb8(out))
    }
}

fn apply_h(m: &[[f64; 3]; 3], x: f64, y: f64) -> (f64, f64) {
    let d = m[2][0] * x + m[2][1] * y + m[2][2];
    if d.abs() < 1e-12 {
        return (0.0, 0.0);
    }
    (
        (m[0][0] * x + m[0][1] * y + m[0][2]) / d,
        (m[1][0] * x + m[1][1] * y + m[1][2]) / d,
    )
}

/// 解 4 组对应点求 3x3 单应矩阵（DLT，8 未知数）。
fn homography(src: &[Pt; 4], dst: &[Pt; 4]) -> Option<[[f64; 3]; 3]> {
    let mut a = [[0.0f64; 8]; 8];
    let mut b = [0.0f64; 8];
    for i in 0..4 {
        let (x, y) = src[i];
        let (u, v) = dst[i];
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y];
        b[2 * i] = u;
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y];
        b[2 * i + 1] = v;
    }
    let sol = solve_linear(&a, &b)?;
    Some([
        [sol[0], sol[1], sol[2]],
        [sol[3], sol[4], sol[5]],
        [sol[6], sol[7], 1.0],
    ])
}

/// 高斯消元解线性方程组（含部分选主元）。
fn solve_linear(a: &[[f64; 8]; 8], b: &[f64; 8]) -> Option<[f64; 8]> {
    let mut m = *a;
    let mut v = *b;
    for col in 0..8 {
        // 选主元
        let mut piv = col;
        for r in (col + 1)..8 {
            if m[r][col].abs() > m[piv][col].abs() {
                piv = r;
            }
        }
        if m[piv][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        v.swap(col, piv);
        for r in (col + 1)..8 {
            let f = m[r][col] / m[col][col];
            if f == 0.0 {
                continue;
            }
            for c in col..8 {
                m[r][c] -= f * m[col][c];
            }
            v[r] -= f * v[col];
        }
    }
    let mut x = [0.0f64; 8];
    for i in (0..8).rev() {
        let mut s = v[i];
        for j in (i + 1)..8 {
            s -= m[i][j] * x[j];
        }
        x[i] = s / m[i][i];
    }
    Some(x)
}

fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let a = m[0][0]; let b = m[0][1]; let c = m[0][2];
    let d = m[1][0]; let e = m[1][1]; let f = m[1][2];
    let g = m[2][0]; let h = m[2][1]; let i = m[2][2];

    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    Some([
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ])
}

/// 双线性采样，越界时钳位到边缘（对应 OpenCV BORDER_REPLICATE 的近似）。
/// 双线性采样，**逐位复刻** `cv2.warpPerspective` 的
/// `INTER_LINEAR + BORDER_CONSTANT(0)` 行为。
///
/// 以下四条全部经 OpenCV 4.10 实测确认（见 `scripts/probe_warp*.py`），
/// 每一条都踩过的坑写在注释里：
///
/// 1. **坐标约定**：整数坐标 = 像素中心。调用方直接传连续坐标，
///    不要自己 +0.5/-0.5（我最早两边各偏半像素，裁出来整幅图平移）。
/// 2. **源坐标量化到 1/32**（`INTER_TAB_SIZE`）：`X = cvRound(sx * 32)`，
///    `x0 = X >> 5`，`ax = X & 31`。用全精度双线性会差最多 5 级灰度。
///    `cvRound` 是**就近舍入、平局取偶**（x86 默认 FPU 模式），
///    不是 Rust `f64::round` 的「远离零」。
/// 3. **整数双线性**：`value = (sum + 512) >> 10`，其中
///    `sum = p00*(32-ax)*(32-ay) + p10*ax*(32-ay) + p01*(32-ax)*ay + p11*ax*ay`。
/// 4. **越界逐 tap 补 0**：某一个 tap 越界时只把那一项当 border 值 0 参与
///    插值，既不是整像素填 0，也不是钳制到边缘像素（实测 sx=3.5 在 8 宽图上
///    得 90 = 0.5*180 + 0.5*0）。
fn sample_bilinear(img: &image::RgbImage, sx: f64, sy: f64) -> Rgb<u8> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Rgb([0, 0, 0]);
    }

    let xq = round_half_even(sx * 32.0) as i64;
    let yq = round_half_even(sy * 32.0) as i64;
    // 算术右移 = floor，负数也对（-1 >> 5 == -1）
    let x0 = xq >> 5;
    let y0 = yq >> 5;
    let ax = xq & 31;
    let ay = yq & 31;

    let mut out = [0u8; 3];
    for ch in 0..3 {
        let mut sum: i64 = 0;
        // (dx, dy, wx, wy)：wx/wy ∈ [0, 32]，越界的 tap 权重照算但取不到像素
        for &(dx, dy, wx, wy) in &[
            (0, 0, 32 - ax, 32 - ay),
            (1, 0, ax, 32 - ay),
            (0, 1, 32 - ax, ay),
            (1, 1, ax, ay),
        ] {
            let px = x0 + dx;
            let py = y0 + dy;
            if px >= 0 && py >= 0 && px < w as i64 && py < h as i64 {
                sum += img.get_pixel(px as u32, py as u32).0[ch] as i64 * wx * wy;
            }
        }
        // sum <= 255*32*32 = 261120，+512 后 >> 10 不会溢出 u8
        out[ch] = ((sum + 512) >> 10) as u8;
    }
    Rgb(out)
}

/// `cvRound` 语义：就近舍入，**平局取偶**（x86 默认 FPU 舍入模式）。
pub(crate) fn round_half_even(v: f64) -> f64 {
    let f = v.floor();
    let frac = v - f;
    if (frac - 0.5).abs() < 1e-9 {
        // 平局：两边取偶数那个
        if (f as i64) % 2 == 0 {
            f
        } else {
            f + 1.0
        }
    } else if frac > 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// 便捷函数：DynamicImage 的宽高。
pub fn dims(img: &DynamicImage) -> (u32, u32) {
    img.dimensions()
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
    fn unclip_expands_axis_aligned_box() {
        // 100x30 矩形外扩 5，应变成 110x40
        let box_pts = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
        let out = unclip(&box_pts, 5.0).expect("should expand");
        let area = contour_area(&out);
        assert!(approx(area, 110.0 * 40.0), "expected 4400, got {area}");
    }

    #[test]
    fn unclip_expands_reversed_order_box() {
        // 顶点顺序反过来（代数有向面积为负），外扩结果必须一致
        let box_pts = [(0.0, 0.0), (0.0, 30.0), (100.0, 30.0), (100.0, 0.0)];
        let out = unclip(&box_pts, 5.0).expect("should expand");
        let area = contour_area(&out);
        assert!(approx(area, 110.0 * 40.0), "expected 4400, got {area}");
    }

    #[test]
    fn unclip_matches_paddle_formula() {
        // 复刻 PaddleOCR：distance = area * ratio / length
        // 380x13 的框，ratio=1.5 -> distance = 1.5*4940/786 = 9.427481
        let w = 380.0;
        let h = 13.0;
        let box_pts = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
        let area = contour_area(&box_pts);
        let length = contour_length(&box_pts);
        let distance = area * 1.5 / length;
        let out = unclip(&box_pts, distance).expect("expand");

        // pyclipper 真值（probe_unclip_truth.py 实测）：
        //   bbox = (-9, -9) .. (389, 22)，即 398 x 31
        // 轴对齐时斜接与 JT_ROUND 的最小外接矩形完全一致，可逐位断言。
        let (xs, ys): (Vec<f64>, Vec<f64>) =
            (out.iter().map(|p| p.0).collect(), out.iter().map(|p| p.1).collect());
        let x0 = xs.iter().cloned().fold(f64::MAX, f64::min);
        let x1 = xs.iter().cloned().fold(f64::MIN, f64::max);
        let y0 = ys.iter().cloned().fold(f64::MAX, f64::min);
        let y1 = ys.iter().cloned().fold(f64::MIN, f64::max);
        assert!(approx(x0, -9.0), "x0 {x0}");
        assert!(approx(x1, 389.0), "x1 {x1}");
        assert!(approx(y0, -9.0), "y0 {y0}");
        assert!(approx(y1, 22.0), "y1 {y1}");
        // 输出必须是整数（pyclipper 的坐标就是整数）
        for p in &out {
            assert_eq!(p.0.fract(), 0.0, "x 不是整数: {}", p.0);
            assert_eq!(p.1.fract(), 0.0, "y 不是整数: {}", p.1);
        }
    }

    /// 锁死 pyclipper 的整数语义（这是框偏小的根因）。
    ///
    /// mini box (452,371)-(471,388)，distance = 323*1.5/72 = 6.729167。
    /// pyclipper 给出**恰好** (445,364)-(478,395)；
    /// 纯浮点外扩会给出 (445.2708, 364.2708)-(477.7292, 394.7292)。
    #[test]
    fn unclip_matches_pyclipper_integer_semantics() {
        let box_pts = [(452.0, 371.0), (471.0, 371.0), (471.0, 388.0), (452.0, 388.0)];
        let distance = 323.0 * 1.5 / 72.0;
        let out = unclip(&box_pts, distance).expect("expand");
        assert_eq!(
            out,
            vec![
                (445.0, 364.0),
                (478.0, 364.0),
                (478.0, 395.0),
                (445.0, 395.0),
            ],
            "必须逐位等于 pyclipper 的输出"
        );
    }

    /// 输入坐标被截断成整数（pyclipper 的 IntPoint 是 cInt 强转）。
    #[test]
    fn unclip_truncates_input_coordinates() {
        // 角点 452.6 配 delta=7：pyclipper 输出 445（=452-7），不是 446
        let box_pts = [(452.6, 371.6), (471.6, 371.6), (471.6, 388.6), (452.6, 388.6)];
        let out = unclip(&box_pts, 7.0).expect("expand");
        assert_eq!(
            out,
            vec![
                (445.0, 364.0),
                (478.0, 364.0),
                (478.0, 395.0),
                (445.0, 395.0),
            ]
        );
    }

    /// 旋转框：斜接（miter）与 pyclipper 的 JT_ROUND 有已知的微小分歧。
    ///
    /// 圆角弧会切掉斜接尖，所以 pyclipper 的最小外接矩形略小。
    /// 实测（probe_unclip_truth.py）：100x30 旋转 30°、d=5，
    /// pyclipper minAreaRect = 107.9109 x 39.4424。
    /// 轴对齐框（绝大多数文本框）两者逐位一致，只有明显旋转时才差 ~1.5px。
    #[test]
    fn unclip_rotated_rect_diverges_from_pyclipper_by_at_most_2px() {
        let ang = 30f64.to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let base = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
        let rot: Vec<Pt> = base
            .iter()
            .map(|(x, y)| (x * c - y * s, x * s + y * c))
            .collect();
        let out = unclip(&rot, 5.0).expect("should expand");
        // 输出必须是整数
        for p in &out {
            assert_eq!(p.0.fract(), 0.0);
            assert_eq!(p.1.fract(), 0.0);
        }
        let (mr, _) = get_mini_boxes(&out).expect("rect");
        let w = dist(mr[0], mr[1]);
        let h = dist(mr[1], mr[2]);
        // pyclipper 真值 107.9109 x 39.4424；斜接偏大，但应在 2px 以内
        assert!((w - 107.9109).abs() <= 2.0, "长边 {w} 偏离 pyclipper 超过 2px");
        assert!((h - 39.4424).abs() <= 2.0, "短边 {h} 偏离 pyclipper 超过 2px");
    }

    #[test]
    fn min_area_rect_on_square() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let (box_pts, short_side) = get_mini_boxes(&pts).expect("rect");
        assert!(approx(short_side, 10.0), "short side {short_side}");
        assert_eq!(box_pts.len(), 4);
    }

    #[test]
    fn min_area_rect_on_rotated_rect() {
        let ang = 20f64.to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let base = [(0.0, 0.0), (120.0, 0.0), (120.0, 20.0), (0.0, 20.0)];
        let rot: Vec<Pt> = base
            .iter()
            .map(|(x, y)| (x * c - y * s, x * s + y * c))
            .collect();
        let (box_pts, short_side) = get_mini_boxes(&rot).expect("rect");
        assert!((short_side - 20.0).abs() < 0.01, "short side {short_side}");
        // 长边应为 120
        let long = dist(box_pts[0], box_pts[1]).max(dist(box_pts[0], box_pts[3]));
        assert!((long - 120.0).abs() < 0.01, "long side {long}");
    }

    #[test]
    fn order_points_wide_box() {
        // 宽扁文本框：顶点序错乱会让透视变换拿到乱序四边形
        let pts = [(67.0, 57.0), (67.0, 70.0), (428.0, 70.0), (428.0, 57.0)];
        let [tl, tr, br, bl] = order_points(pts);
        assert_eq!(tl, (67.0, 57.0));
        assert_eq!(tr, (428.0, 57.0));
        assert_eq!(br, (428.0, 70.0));
        assert_eq!(bl, (67.0, 70.0));
    }

    #[test]
    fn order_points_square() {
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let [tl, tr, br, bl] = order_points(pts);
        assert_eq!(tl, (0.0, 0.0));
        assert_eq!(tr, (10.0, 0.0));
        assert_eq!(br, (10.0, 10.0));
        assert_eq!(bl, (0.0, 10.0));
    }

    #[test]
    fn order_points_rotated() {
        let ang = 30f64.to_radians();
        let (c, s) = (ang.cos(), ang.sin());
        let base = [(0.0, 0.0), (100.0, 0.0), (100.0, 30.0), (0.0, 30.0)];
        let pts: [Pt; 4] = base.map(|(x, y)| (x * c - y * s, x * s + y * c));
        let [tl, tr, br, bl] = order_points(pts);
        assert!(approx(dist(tl, tr), 100.0), "top edge {}", dist(tl, tr));
        assert!(approx(dist(tr, br), 30.0), "right edge {}", dist(tr, br));
        assert!(approx(dist(br, bl), 100.0), "bottom edge {}", dist(br, bl));
        assert!(approx(dist(bl, tl), 30.0), "left edge {}", dist(bl, tl));
    }

    #[test]
    fn box_score_full_coverage() {
        // 概率图全 1，框覆盖整图 -> 均值 1
        let arr = ndarray::Array2::<f32>::ones((10, 20));
        let box_pts = [(0.0, 0.0), (19.0, 0.0), (19.0, 9.0), (0.0, 9.0)];
        assert!((box_score(&arr.view(), &box_pts) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn box_score_half() {
        // 左半 1 右半 0，框均分 -> 约 0.5
        let mut arr = ndarray::Array2::<f32>::zeros((10, 20));
        for y in 0..10 {
            for x in 0..10 {
                arr[[y, x]] = 1.0;
            }
        }
        let box_pts = [(0.0, 0.0), (19.0, 0.0), (19.0, 9.0), (0.0, 9.0)];
        let s = box_score(&arr.view(), &box_pts);
        assert!((s - 0.5).abs() < 0.05, "score {s}");
    }

    #[test]
    fn homography_identity() {
        let src = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let dst = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let h = homography(&src, &dst).expect("h");
        for i in 0..4 {
            let (x, y) = apply_h(&h, src[i].0, src[i].1);
            assert!((x - dst[i].0).abs() < 1e-6, "x {x}");
            assert!((y - dst[i].1).abs() < 1e-6, "y {y}");
        }
    }

    #[test]
    fn homography_scaling() {
        // 放大 2 倍
        let src = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let dst = [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)];
        let h = homography(&src, &dst).expect("h");
        let (x, y) = apply_h(&h, 5.0, 5.0);
        assert!((x - 10.0).abs() < 1e-6, "x {x}");
        assert!((y - 10.0).abs() < 1e-6, "y {y}");
    }

    #[test]
    fn round_half_even_matches_cvround() {
        // cvRound 平局取偶，不是远离零
        assert_eq!(round_half_even(0.5), 0.0);
        assert_eq!(round_half_even(1.5), 2.0);
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(3.5), 4.0);
        assert_eq!(round_half_even(-0.5), 0.0);
        assert_eq!(round_half_even(-1.5), -2.0);
        assert_eq!(round_half_even(64.5), 64.0);
        assert_eq!(round_half_even(65.5), 66.0);
        // 非平局照常就近
        assert_eq!(round_half_even(2.4), 2.0);
        assert_eq!(round_half_even(2.6), 3.0);
        assert_eq!(round_half_even(-2.4), -2.0);
    }

    /// 下面的期望值全部来自 `scripts/probe_warp4/5/6.py` 对 OpenCV 4.10 的实测输出，
    /// 不是推出来的——它们锁住「逐位复刻 warpPerspective」的语义。
    #[test]
    fn sample_bilinear_matches_opencv_probes() {
        // probe_warp4: 4x1 图，列值 0/60/120/180
        let mut img = image::RgbImage::new(4, 1);
        for x in 0..4u32 {
            img.put_pixel(x, 0, image::Rgb([(x * 60) as u8; 3]));
        }
        // sx=3.5: x0=3 界内、x1=4 越界 -> 0.5*180 + 0.5*0 = 90
        assert_eq!(sample_bilinear(&img, 3.5, 0.0).0, [90; 3]);
        // sx=2.5: 正常双线性 -> 0.5*120 + 0.5*180 = 150
        assert_eq!(sample_bilinear(&img, 2.5, 0.0).0, [150; 3]);

        // probe_warp6: 4x1 全 180 图
        let full = image::RgbImage::from_pixel(4, 1, image::Rgb([180; 3]));
        // sx=-0.5: x0=-1 越界、x1=0 界内 -> 0.5*0 + 0.5*180 = 90
        assert_eq!(sample_bilinear(&full, -0.5, 0.0).0, [90; 3]);
        // sx=-0.25: ax=3/4 -> 0.75*180 = 135
        assert_eq!(sample_bilinear(&full, -0.25, 0.0).0, [135; 3]);
        // sx=-1.5: 两个 tap 都越界 -> 0
        assert_eq!(sample_bilinear(&full, -1.5, 0.0).0, [0; 3]);

        // probe_warp5 平局案例: 列值 0/64/128/192, sx=2.015625
        // X = cvRound(2.015625*32) = cvRound(64.5) = 64（取偶）-> ax=0 -> p(2)=128
        // （若按 1/32 以外的精度会得到 129，按远离零舍入会得到 130）
        let mut img2 = image::RgbImage::new(4, 1);
        for x in 0..4u32 {
            img2.put_pixel(x, 0, image::Rgb([(x * 64) as u8; 3]));
        }
        assert_eq!(sample_bilinear(&img2, 2.015625, 0.0).0, [128; 3]);

        // probe_warp5 双向部分越界: 4x4 = arange(16), sx=sy=3.5
        // 只有 p(3,3)=15 在界内，权重 (1/2)*(1/2) -> 3.75 -> 舍入 4
        let mut img3 = image::RgbImage::new(4, 4);
        for y in 0..4u32 {
            for x in 0..4u32 {
                img3.put_pixel(x, y, image::Rgb([(y * 4 + x) as u8; 3]));
            }
        }
        assert_eq!(sample_bilinear(&img3, 3.5, 3.5).0, [4; 3]);
    }

    #[test]
    fn order_points_uses_yminusx_for_tr_bl() {
        // 倾斜文本框：TR 的 y-x 最小，BL 的 y-x 最大
        let pts = [(100.0, 50.0), (150.0, 30.0), (170.0, 80.0), (120.0, 100.0)];
        let [tl, tr, br, bl] = order_points(pts);
        assert_eq!(tl, (100.0, 50.0));
        assert_eq!(tr, (150.0, 30.0));
        assert_eq!(br, (170.0, 80.0));
        assert_eq!(bl, (120.0, 100.0));
    }
}
