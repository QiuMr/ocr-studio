//! 模板匹配（找图）引擎。
//!
//! 算法：**灰度金字塔 + 最粗层 NCC 全扫 + 逐层 ±2 精化 + NMS**。
//!
//! 为什么粗筛也用 NCC 而不是 SQDIFF：
//! 一开始的设计是「SQDIFF 积分图粗筛 + NCC 精算」，理由是 SQDIFF 能 O(1) 算。
//! **这个前提是错的** —— SQDIFF 的 `ΣT² + ΣI² - 2ΣTI` 里 `ΣTI` 必须逐像素
//! 相乘，和 NCC 一样是 `O(tw*th)`。既然代价相同，就没必要用 SQDIFF：
//! 它对亮度/对比度漂移敏感（实测模板 ×0.8 后真位置的 SQDIFF 分数会掉出
//! top-64 候选，直接漏检），而 NCC 天然免疫。
//!
//! 所以省掉粗筛-精算的度量切换，全流程统一 NCC 分数语义，反而更短更快。
//!
//! 已知限制（诚实记录）：
//! - **只支持单一尺度**。模板与目标尺寸差 >5% 时分数会明显掉。多尺度搜索
//!   会成倍增加耗时，规划在后续版本加。
//! - **不支持旋转**。模板旋转后 NCC 直接崩（典型能从 0.99 掉到 0.3），
//!   需要旋转模板阵列，未实现。
//! - **方差为 0 的模板**（纯色块）NCC 无定义，本实现返回 0 分而不假阳。
//! - **高光截断会破坏亮度不变性**：若亮度调整把像素推到 255 饱和，线性关系
//!   不再成立，NCC 会真的掉分。这是数学事实，不是 bug。
//! - 同一位置若有完全相同的重复纹理（如平铺背景），NMS 按中心距离合并，
//!   只报一个点。

use std::cmp::Ordering;

use image::RgbImage;

/// 粗筛层最多保留的候选数。太大只增耗时，太小容易漏真解。
const COARSE_KEEP: usize = 64;
/// 逐层精化时每层保留的候选数。
const REFINE_KEEP: usize = 24;
/// 金字塔最多 4 层（1/16）。
const MAX_LEVEL: usize = 4;
/// 降采样后模板的最小边长。
///
/// 取 6 而不是 8：多降一层能把粗扫的位置数降 4 倍、模板面积降 4 倍，
/// 合计约 17 倍加速（1080p 实测粗扫 424ms -> 25ms）。代价是最粗层的模板只有
/// 8×6，判别力下降 —— 但粗筛只需要保证**真解还在候选集里**，位置精度由后续
/// 逐层精化负责，所以这个交换是划算的。
const MIN_TPL_SIDE: usize = 6;

/// 一次匹配的参数。
#[derive(Debug, Clone, Copy)]
pub struct MatchOptions {
    /// 最终 NCC 分数下限（`-1..=1`，实用区间 `0.6..=0.99`）。
    pub min_score: f64,
    /// 最多返回几个结果（按分数降序）。
    pub max_results: usize,
}

/// 模板掩码：只让掩码覆盖的像素参与 NCC。
///
/// 解决什么问题：模板里常常混着**会变的部分**——按钮上的文字、动态的背景、
/// 队友头像的位置。整块比像素时这些一变就失配。画个掩码把「稳定的部分」圈出来，
/// 只比那一块，命中率立刻上来。
///
/// 与「裁剪模板」的区别：裁剪会改变模板尺寸，也就改变了命中框的大小与中心点；
/// 掩码保持尺寸不变，只是不参与计算。前端画框、脚本算点击点都不用改。
#[derive(Debug, Clone)]
pub struct TemplateMask {
    w: usize,
    h: usize,
    /// 长度 = w×h，非 0 表示该像素参与匹配
    data: Vec<u8>,
}

impl TemplateMask {
    /// 从裸数据构造。长度必须等于 `w*h`。
    pub fn new(w: usize, h: usize, data: Vec<u8>) -> Result<Self, String> {
        if w == 0 || h == 0 {
            return Err("掩码尺寸为 0".to_string());
        }
        if data.len() != w * h {
            return Err(format!(
                "掩码数据长度 {} 与尺寸 {w}x{h} 不匹配",
                data.len()
            ));
        }
        Ok(Self { w, h, data })
    }

    /// 从灰度图构造：像素 **≥ threshold** 视为参与匹配（默认 128）。
    ///
    /// 约定「白 = 要比」而不是「黑 = 要比」：用户画掩码时习惯把关心的区域涂白，
    /// 白色更符合直觉；而且二值化后白=255 天然落在阈值之上。
    pub fn from_gray(img: &image::GrayImage, threshold: u8) -> Self {
        let (w, h) = img.dimensions();
        let data: Vec<u8> = img
            .as_raw()
            .iter()
            .map(|&v| u8::from(v >= threshold))
            .collect();
        Self {
            w: w as usize,
            h: h as usize,
            data,
        }
    }

    pub fn dimensions(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// 参与匹配的像素数。
    pub fn count(&self) -> usize {
        self.data.iter().filter(|&&v| v > 0).count()
    }

    /// 金字塔降采样：任一源像素被掩码 → 目标像素被掩码（宁多勿少）。
    ///
    /// 为什么取「或」而不是「与」：粗筛层只要保证**真解还在候选集里**，
    /// 多带进来几个无关像素只会略微拉低分数，而漏掉真解是直接漏检。
    fn downscale(&self) -> Self {
        let w = (self.w / 2).max(1);
        let h = (self.h / 2).max(1);
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            let y0 = (y * 2).min(self.h - 1);
            let y1 = (y0 + 1).min(self.h - 1);
            for x in 0..w {
                let x0 = (x * 2).min(self.w - 1);
                let x1 = (x0 + 1).min(self.w - 1);
                let any = self.data[y0 * self.w + x0] > 0
                    || self.data[y0 * self.w + x1] > 0
                    || self.data[y1 * self.w + x0] > 0
                    || self.data[y1 * self.w + x1] > 0;
                data[y * w + x] = u8::from(any);
            }
        }
        Self { w, h, data }
    }
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self {
            min_score: 0.8,
            max_results: 1,
        }
    }
}

/// 单个命中结果。坐标是**搜索图内的像素坐标**，屏幕绝对坐标由调用方加偏移。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchHit {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 零均值 NCC，`-1..=1`。1.0 = 逐像素完全一致。
    pub score: f64,
}

impl MatchHit {
    /// 命中框中心。
    pub fn center(&self) -> (f64, f64) {
        (
            self.x as f64 + self.width as f64 / 2.0,
            self.y as f64 + self.height as f64 / 2.0,
        )
    }
}

/// 在 `haystack` 中查找 `needle`，返回按分数降序的命中列表。
///
/// 找不到返回空 `Vec`；入参不可能成立（模板为空、模板比搜索图大）返回 `Err`。
pub fn find_template(
    haystack: &RgbImage,
    needle: &RgbImage,
    opts: &MatchOptions,
) -> Result<Vec<MatchHit>, String> {
    find_template_masked(haystack, needle, None, opts)
}

/// 带掩码的模板匹配。`mask` 为 `None` 时与 [`find_template`] 完全等价。
///
/// 掩码尺寸必须与模板一致——不一致时直接报错而不是自动缩放：掩码是「哪些像素
/// 算数」的逐像素约定，缩放会悄悄改变语义（插值出来的中间值到底算不算参与？），
/// 与其猜一个规则，不如让调用方给对尺寸。
pub fn find_template_masked(
    haystack: &RgbImage,
    needle: &RgbImage,
    mask: Option<&TemplateMask>,
    opts: &MatchOptions,
) -> Result<Vec<MatchHit>, String> {
    let (hw, hh) = haystack.dimensions();
    let (nw, nh) = needle.dimensions();

    if nw == 0 || nh == 0 {
        return Err("模板图为空".to_string());
    }
    if nw > hw || nh > hh {
        return Err(format!(
            "模板 {nw}x{nh} 大于搜索图 {hw}x{hh}"
        ));
    }
    if let Some(m) = mask {
        if (m.w, m.h) != (nw as usize, nh as usize) {
            return Err(format!(
                "掩码尺寸 {}x{} 与模板 {nw}x{nh} 不一致（掩码必须逐像素对应模板）",
                m.w, m.h
            ));
        }
        if m.count() < 4 {
            return Err(format!(
                "掩码只覆盖了 {} 个像素，太少无法匹配（至少 4 个）",
                m.count()
            ));
        }
    }

    let max_results = opts.max_results.clamp(1, 64);
    let min_score = opts.min_score.clamp(-1.0, 1.0);

    // ---- 建金字塔 ----
    // 先把各层的图全部降采样出来，再 move 进 Level —— 避免每层 clone 一份
    // 全尺寸像素（1080p 一层就是 2MB，白拷了 4 次）。
    let levels = pick_levels(hw as usize, hh as usize, nw as usize, nh as usize);
    let mut gs: Vec<Gray> = Vec::with_capacity(levels + 1);
    gs.push(Gray::from_rgb(haystack));
    for i in 1..=levels {
        let next = gs[i - 1].downscale();
        gs.push(next);
    }
    let mut ts: Vec<Gray> = Vec::with_capacity(levels + 1);
    ts.push(Gray::from_rgb(needle));
    for i in 1..=levels {
        let next = ts[i - 1].downscale();
        ts.push(next);
    }
    // 掩码也跟着降采样，保证每层与模板同尺寸
    let mut ms: Vec<Option<TemplateMask>> = Vec::with_capacity(levels + 1);
    ms.push(mask.cloned());
    for i in 1..=levels {
        ms.push(ms[i - 1].as_ref().map(|m| m.downscale()));
    }
    let mut pyr: Vec<Level> = Vec::with_capacity(levels + 1);
    for ((g, t), m) in gs.into_iter().zip(ts).zip(ms) {
        if t.w > g.w || t.h > g.h {
            return Err("模板在降采样层超出搜索图（不应发生，疑似尺寸计算错误）".to_string());
        }
        pyr.push(Level::new(g, t, m));
    }

    // ---- 最粗层 NCC 全扫 ----
    let top = &pyr[levels];
    let mut cands = top.coarse_scan(COARSE_KEEP);

    // ---- 逐层向下精化 ----
    for lv in (0..levels).rev() {
        let cur = &pyr[lv];
        let mut next: Vec<(usize, usize, f64)> = Vec::with_capacity(cands.len() * 25);
        for &(cx, cy, _) in &cands {
            let bx = (cx * 2) as i32;
            let by = (cy * 2) as i32;
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let x = bx + dx;
                    let y = by + dy;
                    if x < 0 || y < 0 {
                        continue;
                    }
                    let (x, y) = (x as usize, y as usize);
                    if x > cur.max_x() || y > cur.max_y() {
                        continue;
                    }
                    let s = cur.ncc_at(x, y);
                    next.push((x, y, s));
                }
            }
        }
        if next.is_empty() {
            // 上一层候选全部落空：理论上不会，兜底用本层重新粗扫一次
            next = cur.coarse_scan(COARSE_KEEP);
        }
        next.sort_by(desc);
        next.truncate(REFINE_KEEP);
        cands = next;
    }

    // ---- 过滤 + NMS ----
    let base = &pyr[0];
    let mut hits: Vec<MatchHit> = Vec::new();
    for &(x, y, s) in &cands {
        if s < min_score {
            continue;
        }
        let hit = MatchHit {
            x: x as u32,
            y: y as u32,
            width: nw,
            height: nh,
            score: s,
        };
        if hits.iter().any(|h| overlaps(h, &hit)) {
            continue;
        }
        hits.push(hit);
        if hits.len() >= max_results {
            break;
        }
    }
    // 用最终层（原图）重算一次分数，避免上层近似带来的误差
    for h in &mut hits {
        h.score = base.ncc_at(h.x as usize, h.y as usize);
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
    Ok(hits)
}

fn desc(a: &(usize, usize, f64), b: &(usize, usize, f64)) -> Ordering {
    b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal)
}

/// 两个命中框是否算同一个目标（中心距离 < 半边长）。
fn overlaps(a: &MatchHit, b: &MatchHit) -> bool {
    let (ax, ay) = a.center();
    let (bx, by) = b.center();
    let min_side = a.width.min(a.height) as f64;
    let thr = (min_side * 0.5).max(4.0);
    let dx = ax - bx;
    let dy = ay - by;
    dx * dx + dy * dy < thr * thr
}

/// 选金字塔层数：既要让最粗层的位置数降下来，又不能把小模板糊没。
fn pick_levels(hw: usize, hh: usize, tw: usize, th: usize) -> usize {
    let mut lv = 0usize;
    while lv < MAX_LEVEL {
        let next = lv + 1;
        if (hw >> next) < 64 || (hh >> next) < 64 {
            break;
        }
        if (tw >> next) < MIN_TPL_SIDE || (th >> next) < MIN_TPL_SIDE {
            break;
        }
        lv = next;
    }
    lv
}

// ---------------------------------------------------------------- 灰度与金字塔

#[derive(Debug, Clone)]
struct Gray {
    w: usize,
    h: usize,
    data: Vec<u8>,
}

impl Gray {
    fn from_rgb(img: &RgbImage) -> Self {
        let (w, h) = img.dimensions();
        // 用 chunks_exact 而不是 pixels() 迭代器：后者每像素要经过
        // Iterator::next + Pixel trait 分派，2M 像素（1080p）下差距很明显。
        let raw = img.as_raw();
        let mut data = Vec::with_capacity((w * h) as usize);
        for p in raw.chunks_exact(3) {
            // 定点 luma：0.299R + 0.587G + 0.114B ≈ (77R + 150G + 29B) >> 8
            let y = (77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32) >> 8;
            data.push(y as u8);
        }
        Self {
            w: w as usize,
            h: h as usize,
            data,
        }
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.w + x]
    }

    /// 2×2 box 均值降采样。奇数尺寸向下取整，末列/末行重复边缘。
    fn downscale(&self) -> Self {
        let w = (self.w / 2).max(1);
        let h = (self.h / 2).max(1);
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            let y0 = (y * 2).min(self.h - 1);
            let y1 = (y0 + 1).min(self.h - 1);
            for x in 0..w {
                let x0 = (x * 2).min(self.w - 1);
                let x1 = (x0 + 1).min(self.w - 1);
                let s = self.at(x0, y0) as u32
                    + self.at(x1, y0) as u32
                    + self.at(x0, y1) as u32
                    + self.at(x1, y1) as u32;
                data[y * w + x] = ((s + 2) >> 2) as u8;
            }
        }
        Self { w, h, data }
    }
}

/// 前缀和积分图，用于 O(1) 取窗口的 `ΣI` 与 `ΣI²`。
struct Integral {
    w: usize,
    s: Vec<u64>,
    sq: Vec<u64>,
}

impl Integral {
    fn new(g: &Gray) -> Self {
        let (w, h) = (g.w, g.h);
        let stride = w + 1;
        let mut s = vec![0u64; stride * (h + 1)];
        let mut sq = vec![0u64; stride * (h + 1)];
        for y in 0..h {
            let mut rs = 0u64;
            let mut rq = 0u64;
            for x in 0..w {
                let v = g.at(x, y) as u64;
                rs += v;
                rq += v * v;
                s[(y + 1) * stride + (x + 1)] = s[y * stride + (x + 1)] + rs;
                sq[(y + 1) * stride + (x + 1)] = sq[y * stride + (x + 1)] + rq;
            }
        }
        Self { w, s, sq }
    }

    /// 返回 `[x, x+w) × [y, y+h)` 的 `(ΣI, ΣI²)`。调用方保证范围合法。
    #[inline]
    fn rect(&self, x: usize, y: usize, w: usize, h: usize) -> (u64, u64) {
        let stride = self.w + 1;
        let (x0, y0, x1, y1) = (x, y, x + w, y + h);
        let si = |a: usize, b: usize| (b * stride + a) as usize;
        let sum = self.s[si(x1, y1)] + self.s[si(x0, y0)]
            - self.s[si(x0, y1)]
            - self.s[si(x1, y0)];
        let sq = self.sq[si(x1, y1)] + self.sq[si(x0, y0)]
            - self.sq[si(x0, y1)]
            - self.sq[si(x1, y0)];
        (sum, sq)
    }
}

/// 掩码在某一层展开后的统计量：参与匹配的像素列表 + 模板侧的均值/方差原料。
struct MaskLevel {
    /// 掩码像素在模板内的 `(ty, tx)` 偏移
    offs: Vec<(usize, usize)>,
    /// 参与匹配的像素数
    n: f64,
    /// `Σ T·M`
    t_sum: f64,
    /// `Σ T²·M`
    t_sq: f64,
}

/// 一层金字塔：搜索图 + 它的积分图 + 模板 + 模板的统计量。
struct Level {
    g: Gray,
    integ: Integral,
    t: Gray,
    t_sum: f64,
    t_var: f64,
    /// 掩码（与 `t` 同尺寸）。`None` = 全部像素参与，走积分图的快路径
    mask: Option<MaskLevel>,
}

impl Level {
    fn new(g: Gray, t: Gray, mask: Option<TemplateMask>) -> Self {
        let integ = Integral::new(&g);
        let mut t_sum = 0u64;
        let mut t_sq = 0u64;
        for &v in &t.data {
            let v = v as u64;
            t_sum += v;
            t_sq += v * v;
        }
        let n = (t.w * t.h) as f64;
        let t_sum_f = t_sum as f64;
        // 方差用「平方和 - 和的平方/n」一次性算完，不留 t_sq 字段
        let t_var = t_sq as f64 - t_sum_f * t_sum_f / n;

        let mask = mask.and_then(|m| {
            // 掩码覆盖全部像素时没必要走慢路径，直接当没有掩码
            if m.count() >= t.w * t.h {
                return None;
            }
            let mut offs: Vec<(usize, usize)> = Vec::with_capacity(t.w * t.h);
            for ty in 0..t.h {
                let row = ty * t.w;
                for tx in 0..t.w {
                    if m.data[row + tx] > 0 {
                        offs.push((ty, tx));
                    }
                }
            }
            let mut mt_sum = 0f64;
            let mut mt_sq = 0f64;
            for &(ty, tx) in &offs {
                let v = t.data[ty * t.w + tx] as f64;
                mt_sum += v;
                mt_sq += v * v;
            }
            Some(MaskLevel {
                n: offs.len() as f64,
                offs,
                t_sum: mt_sum,
                t_sq: mt_sq,
            })
        });

        Self {
            g,
            integ,
            t,
            t_sum: t_sum_f,
            t_var,
            mask,
        }
    }

    /// 可放置模板的最大左上角坐标。
    #[inline]
    fn max_x(&self) -> usize {
        self.g.w.saturating_sub(self.t.w)
    }

    #[inline]
    fn max_y(&self) -> usize {
        self.g.h.saturating_sub(self.t.h)
    }

    /// `Σ T·I`，唯一需要逐像素循环的量 —— 也是整个算法的热点。
    ///
    /// 用切片 zip 而不是 `data[i]` 索引：索引版每个元素都要做一次边界检查 +
    /// 坐标乘法，debug 构建下这一项能占粗扫一半时间。
    #[inline]
    fn dot(&self, x: usize, y: usize) -> u64 {
        let tw = self.t.w;
        let mut acc = 0u64;
        for ty in 0..self.t.h {
            let s = (y + ty) * self.g.w + x;
            let t = ty * tw;
            let src = &self.g.data[s..s + tw];
            let tpl = &self.t.data[t..t + tw];
            for (a, b) in src.iter().zip(tpl.iter()) {
                acc += (*a as u64) * (*b as u64);
            }
        }
        acc
    }

    /// 零均值 NCC，值域 `-1..=1`。有掩码时只统计掩码内的像素。
    #[inline]
    fn ncc_at(&self, x: usize, y: usize) -> f64 {
        match &self.mask {
            None => self.ncc_plain(x, y),
            Some(m) => self.ncc_masked(x, y, m),
        }
    }

    /// 掩码版 NCC：只对掩码内的像素求 `ΣI / ΣI² / ΣT·I`。
    ///
    /// 为什么不能用积分图：积分图只能 O(1) 取**矩形**窗口的和，而掩码是任意形状。
    /// 所以这里退回逐像素累加（一次循环里同时算三个量）。好在金字塔把真正需要
    /// 全扫的只有最粗一层（1080p 约 8000 个位置），代价可以接受。
    #[inline]
    fn ncc_masked(&self, x: usize, y: usize, m: &MaskLevel) -> f64 {
        if m.n < 2.0 {
            return 0.0;
        }
        let tw = self.t.w;
        let gw = self.g.w;
        let base = y * gw + x;
        let mut i_sum = 0f64;
        let mut i_sq = 0f64;
        let mut dot = 0f64;
        for &(ty, tx) in &m.offs {
            let iv = self.g.data[base + ty * gw + tx] as f64;
            let tv = self.t.data[ty * tw + tx] as f64;
            i_sum += iv;
            i_sq += iv * iv;
            dot += iv * tv;
        }
        // 掩码内模板是纯色 → NCC 无定义，宁可漏不可假阳
        let t_var = m.t_sq - m.t_sum * m.t_sum / m.n;
        if t_var < 1e-6 {
            return 0.0;
        }
        let i_var = i_sq - i_sum * i_sum / m.n;
        if i_var < 1e-6 {
            return 0.0;
        }
        let num = dot - m.t_sum * i_sum / m.n;
        let denom = (t_var * i_var).sqrt();
        if denom < 1e-9 {
            return 0.0;
        }
        (num / denom).clamp(-1.0, 1.0)
    }

    /// 无掩码的 NCC 快路径：`ΣI` / `ΣI²` 走积分图 O(1)，只有 `ΣT·I` 需要逐像素。
    ///
    /// 模板方差为 0（纯色）时 NCC 无定义，返回 0.0 —— 宁可漏，不可假阳。
    #[inline]
    fn ncc_plain(&self, x: usize, y: usize) -> f64 {
        if self.t_var < 1e-6 {
            return 0.0;
        }
        let n = (self.t.w * self.t.h) as f64;
        let (sum_i, sq_i) = self.integ.rect(x, y, self.t.w, self.t.h);
        let sum_i = sum_i as f64;
        let var_i = sq_i as f64 - sum_i * sum_i / n;
        if var_i < 1e-6 {
            return 0.0;
        }
        let dot = self.dot(x, y) as f64;
        let num = dot - self.t_sum * sum_i / n;
        let denom = (self.t_var * var_i).sqrt();
        if denom < 1e-9 {
            return 0.0;
        }
        (num / denom).clamp(-1.0, 1.0)
    }

    /// 全扫一遍，返回按 NCC 降序的前 `keep` 个位置。
    fn coarse_scan(&self, keep: usize) -> Vec<(usize, usize, f64)> {
        let (mx, my) = (self.max_x(), self.max_y());
        let mut all = Vec::with_capacity((mx + 1) * (my + 1));
        for y in 0..=my {
            for x in 0..=mx {
                all.push((x, y, self.ncc_at(x, y)));
            }
        }
        all.sort_by(desc);
        all.truncate(keep);
        all
    }
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// 造一张有纹理的假「屏幕」：渐变底 + 伪随机色块，避免平坦区域让 NCC 退化。
    fn make_scene(w: u32, h: u32) -> RgbImage {
        let mut img = RgbImage::new(w, h);
        let mut seed = 0x1234_5678u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        for y in 0..h {
            for x in 0..w {
                let base = ((x * 255 / w.max(1)) as u32 + (y * 128 / h.max(1)) as u32) % 256;
                let noise = (next() % 40) as u32;
                let v = ((base + noise) % 256) as u8;
                img.put_pixel(x, y, Rgb([v, v.wrapping_add(30), v.wrapping_add(70)]));
            }
        }
        img
    }

    /// 在场景上贴一个高对比度的「按钮」，返回它的左上角。
    fn paste_button(img: &mut RgbImage, x: u32, y: u32, bw: u32, bh: u32) {
        for dy in 0..bh {
            for dx in 0..bw {
                let border = dx == 0 || dy == 0 || dx == bw - 1 || dy == bh - 1;
                let c = if border {
                    Rgb([20u8, 20, 120])
                } else if (dx + dy) % 7 == 0 {
                    Rgb([240, 240, 30])
                } else if dx < bw / 2 {
                    Rgb([200, 40, 40])
                } else {
                    Rgb([40, 190, 90])
                };
                img.put_pixel(x + dx, y + dy, c);
            }
        }
    }

    #[test]
    fn finds_pasted_region_at_exact_position() {
        let mut scene = make_scene(480, 320);
        paste_button(&mut scene, 137, 89, 64, 48);
        // 精确裁剪出按钮区域当模板
        let tpl = image::imageops::crop_imm(&scene, 137, 89, 64, 48).to_image();

        let hits = find_template(&scene, &tpl, &MatchOptions { min_score: 0.95, max_results: 4 })
            .expect("匹配应成功");
        assert!(!hits.is_empty(), "应当至少命中一次");
        let h = hits[0];
        assert_eq!((h.x, h.y), (137, 89), "应命中原始位置，实际 {:?}", (h.x, h.y));
        assert!(h.score > 0.999, "完全一致的裁剪 NCC 应≈1，实际 {}", h.score);
    }

    #[test]
    fn finds_after_brightness_shift() {
        // NCC 对亮度线性变换免疫：模板整体 ×0.8 仍应命中。
        // 注意必须用**不饱和**的变换 —— 若加常数把高光推到 255，截断会破坏线性
        // 关系，NCC 会真的掉分（这不是实现 bug，是用例选错了变换）。
        let mut scene = make_scene(400, 300);
        paste_button(&mut scene, 200, 150, 56, 40);
        let tpl = image::imageops::crop_imm(&scene, 200, 150, 56, 40).to_image();
        let mut scaled = tpl.clone();
        for p in scaled.pixels_mut() {
            for c in p.0.iter_mut() {
                *c = ((*c as f32) * 0.8).round().min(255.0) as u8;
            }
        }

        let hits = find_template(&scene, &scaled, &MatchOptions { min_score: 0.98, max_results: 1 })
            .expect("匹配应成功");
        assert!(!hits.is_empty(), "亮度缩放后仍应命中");
        assert_eq!((hits[0].x, hits[0].y), (200, 150));
        assert!(hits[0].score > 0.99, "线性缩放下 NCC 应≈1，实际 {}", hits[0].score);
    }

    #[test]
    fn returns_empty_when_template_absent() {
        let scene = make_scene(320, 240);
        let mut other = make_scene(48, 48);
        // 造一个和场景完全无关的模板（纯随机的反向色）
        for p in other.pixels_mut() {
            *p = Rgb([255 - p[0], 255 - p[1], 255 - p[2]]);
        }
        let hits = find_template(&scene, &other, &MatchOptions { min_score: 0.95, max_results: 4 })
            .expect("匹配应成功");
        assert!(hits.is_empty(), "不存在的模板不应命中，实际 {}", hits.len());
    }

    #[test]
    fn flat_template_never_false_positives() {
        let scene = make_scene(200, 200);
        let flat = RgbImage::from_pixel(20, 20, Rgb([128u8, 128, 128]));
        let hits = find_template(&scene, &flat, &MatchOptions { min_score: 0.1, max_results: 4 })
            .expect("匹配应成功");
        assert!(hits.is_empty(), "纯色模板 NCC 无定义，必须不命中");
    }

    #[test]
    fn nms_reports_distinct_instances() {
        let mut scene = make_scene(600, 300);
        paste_button(&mut scene, 40, 40, 40, 40);
        paste_button(&mut scene, 300, 180, 40, 40);
        let tpl = image::imageops::crop_imm(&scene, 40, 40, 40, 40).to_image();

        let hits = find_template(&scene, &tpl, &MatchOptions { min_score: 0.9, max_results: 8 })
            .expect("匹配应成功");
        assert!(hits.len() >= 2, "两个相同按钮应报至少 2 个，实际 {}", hits.len());
        // 两个结果不应重叠
        let (a, b) = (hits[0].center(), hits[1].center());
        let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(d > 40.0, "NMS 后两点应相距较远，实际 {d}");
    }

    /// 掩码的核心价值：模板里混着「会变的部分」（按钮上的文字、动态背景）时，
    /// 整块比像素会失配，把变化区域遮掉就又能命中。
    #[test]
    fn mask_ignores_changing_pixels() {
        let mut scene = make_scene(400, 300);
        paste_button(&mut scene, 200, 150, 64, 48);
        let tpl = image::imageops::crop_imm(&scene, 200, 150, 64, 48).to_image();

        // 模拟「模板中心一块内容变了」（比如按钮文字换了）：把中间 20×12 涂成纯白。
        let mut changed = tpl.clone();
        for dy in 0..12 {
            for dx in 0..20 {
                changed.put_pixel(22 + dx, 18 + dy, Rgb([255u8, 255, 255]));
            }
        }

        // 不遮罩：变化的块让整块失配，应当命中不了。
        let without = find_template(&scene, &changed, &MatchOptions { min_score: 0.9, max_results: 4 })
            .expect("匹配应成功");
        assert!(without.is_empty(), "未遮罩时应因动态块失配，实际命中 {} 个", without.len());

        // 遮住变化块：只比稳定的边框，应当命中且分数接近 1。
        let mut md = vec![1u8; 64 * 48];
        for dy in 0..12 {
            for dx in 0..20 {
                md[(18 + dy) * 64 + (22 + dx)] = 0;
            }
        }
        let mask = TemplateMask::new(64, 48, md).expect("掩码应合法");
        let hits = find_template_masked(
            &scene,
            &changed,
            Some(&mask),
            &MatchOptions { min_score: 0.9, max_results: 4 },
        )
        .expect("匹配应成功");
        assert!(!hits.is_empty(), "遮罩后应当命中");
        assert_eq!((hits[0].x, hits[0].y), (200, 150), "遮罩命中位置应正确");
        assert!(hits[0].score > 0.99, "稳定区域完全一致，NCC 应≈1，实际 {}", hits[0].score);
    }

    /// 掩码尺寸与模板不一致必须直接报错，而不是自动缩放（缩放会悄悄改变语义）。
    #[test]
    fn mask_size_mismatch_is_error() {
        let scene = make_scene(200, 200);
        let tpl = image::imageops::crop_imm(&scene, 10, 10, 32, 32).to_image();
        let mask = TemplateMask::new(16, 16, vec![1u8; 16 * 16]).expect("掩码应合法");
        let err = find_template_masked(&scene, &tpl, Some(&mask), &MatchOptions::default())
            .expect_err("尺寸不一致应报错");
        assert!(err.contains("不一致"), "错误信息应说明尺寸不一致，实际：{err}");
    }

    /// 掩码只圈了不到 4 个像素 → 统计量无意义，直接报错。
    #[test]
    fn mask_too_sparse_is_error() {
        let scene = make_scene(200, 200);
        let tpl = image::imageops::crop_imm(&scene, 10, 10, 32, 32).to_image();
        let mut md = vec![0u8; 32 * 32];
        md[0] = 1;
        md[1] = 1;
        let mask = TemplateMask::new(32, 32, md).expect("掩码应合法");
        let err = find_template_masked(&scene, &tpl, Some(&mask), &MatchOptions::default())
            .expect_err("掩码像素过少应报错");
        assert!(err.contains("太少"), "错误信息应说明像素过少，实际：{err}");
    }

    /// 掩码覆盖全部像素时应当与无掩码走同一条快路径、结果一致。
    #[test]
    fn full_mask_equals_no_mask() {
        let mut scene = make_scene(360, 260);
        paste_button(&mut scene, 120, 80, 48, 36);
        let tpl = image::imageops::crop_imm(&scene, 120, 80, 48, 36).to_image();
        let opts = MatchOptions { min_score: 0.9, max_results: 4 };
        let plain = find_template(&scene, &tpl, &opts).expect("匹配应成功");
        let mask = TemplateMask::new(48, 36, vec![1u8; 48 * 36]).expect("掩码应合法");
        let masked = find_template_masked(&scene, &tpl, Some(&mask), &opts).expect("匹配应成功");
        assert_eq!(plain, masked, "全覆盖掩码应与无掩码结果完全一致");
    }

    /// 掩码降采样必须取「或」（宁多勿少），否则粗筛层可能漏掉真解。
    #[test]
    fn mask_downscale_keeps_any_set_pixel() {
        // 4×4，只有右下角一个像素被标记。
        let mut md = vec![0u8; 16];
        md[3 * 4 + 3] = 1;
        let m = TemplateMask::new(4, 4, md).expect("掩码应合法");
        let d = m.downscale();
        assert_eq!(d.dimensions(), (2, 2));
        // 该像素落在 (1,1) 的 2×2 块里，取「或」后 (1,1) 必须被标记。
        assert_eq!(d.data[1 * 2 + 1], 1, "取「或」后应保留被标记像素");
        assert_eq!(d.count(), 1, "只有那一块被标记");
    }

    #[test]
    fn rejects_oversized_template() {
        let scene = make_scene(50, 50);
        let tpl = make_scene(100, 100);
        assert!(find_template(&scene, &tpl, &MatchOptions::default()).is_err());
    }

    #[test]
    fn different_levels_still_locate() {
        let mut scene = make_scene(1280, 720);
        paste_button(&mut scene, 613, 401, 48, 36);
        let tpl = image::imageops::crop_imm(&scene, 613, 401, 48, 36).to_image();
        let hits = find_template(&scene, &tpl, &MatchOptions { min_score: 0.95, max_results: 1 })
            .expect("匹配应成功");
        assert!(!hits.is_empty());
        // 金字塔逐层 ×2 + ±2 搜索，允许 ±1 像素误差
        assert!(
            (hits[0].x as i64 - 613).abs() <= 1 && (hits[0].y as i64 - 401).abs() <= 1,
            "位置误差过大: {:?}",
            (hits[0].x, hits[0].y)
        );
    }

    /// 1080p 全屏找 64×64 的分段耗时。不是断言测试，是性能观测：
    /// 跑 `cargo test --lib bench_find_1080p -- --ignored --nocapture` 看输出。
    /// debug 构建的数不能代表发布性能，但能看出哪一段是瓶颈。
    #[test]
    #[ignore = "性能观测，仅手动运行"]
    fn bench_find_1080p() {
        use std::time::Instant;
        let mut scene = make_scene(1920, 1080);
        paste_button(&mut scene, 613, 401, 64, 48);
        let tpl = image::imageops::crop_imm(&scene, 613, 401, 64, 48).to_image();

        let t0 = Instant::now();
        let g = Gray::from_rgb(&scene);
        let gt = Instant::now();
        let n = (g.w * g.h) as f64;
        let _ = (n, gt.duration_since(t0));

        let lv = pick_levels(g.w, g.h, 64, 48);
        let t1 = Instant::now();
        let mut pyr = Vec::new();
        let mut a = g;
        let mut b = Gray::from_rgb(&tpl);
        for i in 0..=lv {
            if i > 0 {
                a = a.downscale();
                b = b.downscale();
            }
            // bench 里 clone 是有意保留的：真实实现已改为 move，这里的数比线上偏高
            pyr.push(Level::new(a.clone(), b.clone(), None));
        }
        let t2 = Instant::now();

        let mut cands = pyr[lv].coarse_scan(COARSE_KEEP);
        let t3 = Instant::now();
        for level in (0..lv).rev() {
            let cur = &pyr[level];
            let mut next = Vec::new();
            for &(cx, cy, _) in &cands {
                for dy in -2..=2i32 {
                    for dx in -2..=2i32 {
                        let x = (cx * 2) as i32 + dx;
                        let y = (cy * 2) as i32 + dy;
                        if x < 0 || y < 0 {
                            continue;
                        }
                        let (x, y) = (x as usize, y as usize);
                        if x > cur.max_x() || y > cur.max_y() {
                            continue;
                        }
                        next.push((x, y, cur.ncc_at(x, y)));
                    }
                }
            }
            next.sort_by(desc);
            next.truncate(REFINE_KEEP);
            cands = next;
        }
        let t4 = Instant::now();

        println!("\n=== bench: 1920x1080 找 64x48，金字塔 {lv} 层 ===");
        println!("灰阶化   {:>8.1} ms", (gt - t0).as_secs_f64() * 1e3);
        println!("建金字塔 {:>8.1} ms", (t2 - t1).as_secs_f64() * 1e3);
        println!("粗扫     {:>8.1} ms", (t3 - t2).as_secs_f64() * 1e3);
        println!("逐层精化 {:>8.1} ms", (t4 - t3).as_secs_f64() * 1e3);
        println!("合计     {:>8.1} ms", (t4 - t0).as_secs_f64() * 1e3);
        assert!(!cands.is_empty());
    }
}
