//! DDDDOCR 引擎（common / old 两版）。
//!
//! 它是一个**单行分类器**：喂一张「一行字」的图，输出一串字。它自己不做行检测
//! （`common_det.onnx` 是 YOLOX 目标检测，训练目标是验证码字符块，切文档截图很糟），
//! 所以本引擎的行检测复用 PP-OCR 的 det 模型，识别用 ddddocr 的 rec 模型。
//!
//! 三条与 PP-OCR 不同的地方，全部来自 ddddocr 1.6.1 源码实测，不能凭直觉改：
//!
//! 1. **输入是灰度单通道** `[1,1,64,W]`，PP-OCR 是三通道 `[1,3,48,W]`；
//! 2. **预处理顺序是「先 resize 再转灰度」**，resize 用 LANCZOS、宽度是
//!    `int(w * 64/h)` 截断（不是四舍五入），像素只除 255、不做 mean/std 归一化；
//! 3. **输出是 logits，不是概率**（实测单帧 logits 和可为 -34，softmax 后才归 1），
//!    而且它的字典**把 blank 放在索引 0**（`charset[0] == ""`），与 PP-OCR 的
//!    「blank=0、字典从索引 1 开始」不是同一个约定。
//!
//! 第 3 点最容易出无声错误：如果直接把 ddddocr 的字典丢给 `ocr::rec::ctc_decode_spans`，
//! 它会当成 PP-OCR 的约定去取 `chars[idx-1]`，结果**整行文字整体错位一个字**，
//! 而且不会报错。所以这里单独写解码，并在测试里用真实图片锁住。

use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbImage};
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;

use super::catalog;
use super::{
    Capabilities, EngineSpec, EngineStatus, RecognizeOptions, TextLine, VisionEngine, VisionError,
};
use crate::config::{Device, ModelLevel};
use crate::ocr::detector::LineDetector;
use crate::ocr::geometry::WarpMap;
use crate::ocr::rec::CharSpan;
use crate::ocr::CharBox;

/// 识别输入高度。ddddocr 固定 64，且**不允许**改成 48（模型就是按 64 训的）。
pub const REC_H: u32 = 64;

/// 字典候选文件名。各家打包不一致，按顺序找第一个存在的。
const CHARSET_NAMES: [&str; 3] = ["charset.txt", "keys.txt", "dict.txt"];

/// 加载配置。
#[derive(Debug, Clone)]
pub struct DdddConfig {
    /// `models/ocr/ddddocr/<variant>`
    pub dir: PathBuf,
    /// 行检测用的 PP-OCR det 目录（`models/ocr/ppocr/<level>` 或旧路径）
    pub det_dir: PathBuf,
    pub device: Device,
}

pub struct DdddOcrEngine {
    rec: Session,
    rec_input: String,
    /// 完整字典，**含 blank 在 0 位**，长度等于模型类别数。
    chars: Vec<String>,
    det: LineDetector,
    cfg: DdddConfig,
    actual_device: Device,
    /// rec 与 det 设备不一致时留个说明（DirectML 只对其中一个生效的情况真实存在）
    device_note: Option<String>,
}

impl DdddOcrEngine {
    /// 按目录加载。模型缺失时返回 `VisionError::NotReady` 并写清缺哪个文件。
    pub fn load(cfg: DdddConfig) -> Result<Self, VisionError> {
        let rec_path = cfg.dir.join("rec.onnx");
        if !rec_path.exists() {
            return Err(VisionError::NotReady(format!(
                "缺少识别模型 {}",
                rec_path.display()
            )));
        }
        let charset_path =
            CHARSET_NAMES
                .iter()
                .map(|n| cfg.dir.join(n))
                .find(|p| p.exists())
                .ok_or_else(|| {
                    VisionError::NotReady(format!(
                        "{} 下找不到字典（charset.txt / keys.txt / dict.txt）",
                        cfg.dir.display()
                    ))
                })?;
        let chars = load_charset(&charset_path)
            .map_err(|e| VisionError::NotReady(format!("字典读取失败: {e}")))?;

        let det_path = cfg.det_dir.join("det.onnx");
        if !det_path.exists() {
            return Err(VisionError::NotReady(format!(
                "ddddocr 只做单行识别，行检测需要 PP-OCR 的 {}",
                det_path.display()
            )));
        }

        let env = crate::ocr::ort_util::ort_env()
            .map_err(|e| VisionError::NotReady(format!("ORT 环境初始化失败: {e}")))?;
        // rec 输入是 [1,1,64,W]：宽是动态的，预热用 64 宽就够
        let (rec, actual_device) =
            crate::ocr::ort_util::build_session_with_fallback(env, &rec_path, cfg.device, [1, 1, 64, 64])
                .map_err(|e| VisionError::NotReady(format!("识别模型加载失败: {e}")))?;
        let rec_input = crate::ocr::ort_util::input_name(&rec);

        let det = LineDetector::load(&det_path, cfg.device)
            .map_err(|e| VisionError::NotReady(format!("行检测模型加载失败: {e}")))?;

        let device_note = if det.actual_device() != actual_device {
            Some(format!(
                "识别在 {:?}、行检测在 {:?}",
                actual_device,
                det.actual_device()
            ))
        } else {
            None
        };

        Ok(Self {
            rec,
            rec_input,
            chars,
            det,
            cfg,
            actual_device,
            device_note,
        })
    }

    pub fn charset_len(&self) -> usize {
        self.chars.len()
    }

    /// 对一行裁剪图做识别（不做行检测）。给测试与「整图当一行」的场景用。
    pub fn infer_line(
        &mut self,
        line: &DynamicImage,
    ) -> Result<(String, f64, Vec<CharSpan>, usize), VisionError> {
        let tensor = preprocess_gray(line);
        let out = self
            .rec
            .run(ort::inputs![
                self.rec_input.as_str() => Tensor::from_array(tensor)
                    .map_err(|e| VisionError::Inference(e.to_string()))?
            ])
            .map_err(|e| VisionError::Inference(format!("ddddocr rec 推理失败: {e}")))?;
        let p = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("rec 输出读取失败: {e}")))?;
        let (_, t, c) = layout_of(&p)
            .ok_or_else(|| VisionError::Inference("rec 输出维度不是 [T,C] / [T,1,C] / [1,T,C]".to_string()))?;
        let flat = flatten(&p)
            .ok_or_else(|| VisionError::Inference("rec 输出无法展平".to_string()))?;
        let (text, score, spans) = decode_logits(&flat, t, c, &self.chars);
        Ok((text, score, spans, t))
    }
}

impl VisionEngine for DdddOcrEngine {
    fn spec(&self) -> EngineSpec {
        let variant = self
            .cfg
            .dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "common".to_string());
        EngineSpec::ddddocr(&variant)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // 行框来自共用的 PP-OCR det，逐字框来自 rec 的 CTC 帧区间
            line_boxes: true,
            char_boxes: true,
            // ddddocr 没有方向分类模型
            rotation_cls: false,
            detect: false,
            classes: Vec::new(),
            batch: false,
        }
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            spec: self.spec(),
            capabilities: self.capabilities(),
            available: true,
            actual_device: Some(self.actual_device),
            note: Some(match &self.device_note {
                Some(n) => format!("行检测共用 PP-OCR det；{n}"),
                None => "行检测共用 PP-OCR det".to_string(),
            }),
        }
    }

    fn infer(
        &mut self,
        img: &DynamicImage,
        opts: &RecognizeOptions,
    ) -> Result<Vec<TextLine>, VisionError> {
        if img.width() == 0 || img.height() == 0 {
            return Ok(Vec::new());
        }
        let boxes = self
            .det
            .detect(img)
            .map_err(|e| VisionError::Inference(format!("行检测失败: {e}")))?;

        let mut out = Vec::with_capacity(boxes.len());
        for b in boxes {
            let Some(map) = WarpMap::new(b.points) else {
                continue;
            };
            let Some(crop) = map.warp(img) else {
                continue;
            };
            let (out_w, out_h) = map.size();
            if out_w < 2 || out_h < 2 {
                continue;
            }

            let tensor = preprocess_gray(&crop);
            let t_pre = std::time::Instant::now();
            let o = match self.rec.run(ort::inputs![
                self.rec_input.as_str() => Tensor::from_array(tensor)
                    .map_err(|e| VisionError::Inference(e.to_string()))?
            ]) {
                Ok(o) => o,
                Err(e) => {
                    // 单行失败不该让整张图 500：记一条日志继续下一行
                    tracing::warn!("ddddocr 单行推理失败，跳过：{e}");
                    continue;
                }
            };
            let d_run = t_pre.elapsed();
            let p = o[0]
                .try_extract_array::<f32>()
                .map_err(|e| VisionError::Inference(format!("rec 输出读取失败: {e}")))?;
            let Some((_, frames, c)) = layout_of(&p) else {
                continue;
            };
            let Some(flat) = flatten(&p) else {
                continue;
            };
            let t_dec = std::time::Instant::now();
            let (text, score, spans) = decode_logits(&flat, frames, c, &self.chars);

            // 调试钩子：OCR_DEBUG_DDDD=1 时按行打印三段耗时。
            // 用它区分「模型本身慢」和「预处理/解码慢」——本轮就是靠它确认
            // 主要成本在 rec 推理（54MB、8210 类），而不是 softmax 或 resize。
            if std::env::var("OCR_DEBUG_DDDD").is_ok() {
                eprintln!(
                    "[ddddocr] crop={}x{} frames={} run={:?} decode={:?} text={:?}",
                    crop.width(),
                    crop.height(),
                    frames,
                    d_run,
                    t_dec.elapsed(),
                    text
                );
            }

            let mut chars = Vec::new();
            if opts.want_chars && frames > 0 {
                let w_max = (out_w - 1) as f64;
                let h_max = (out_h - 1) as f64;
                for s in &spans {
                    let mut xa = s.t_start as f64 / frames as f64 * w_max;
                    let mut xb = (s.t_end + 1) as f64 / frames as f64 * w_max;
                    if xb - xa < 0.5 {
                        let mid = (xa + xb) / 2.0;
                        xa = (mid - 0.25).max(0.0);
                        xb = (mid + 0.25).min(w_max);
                    }
                    let tl = map.to_source(xa, 0.0);
                    let tr = map.to_source(xb, 0.0);
                    let br = map.to_source(xb, h_max);
                    let bl = map.to_source(xa, h_max);
                    chars.push(CharBox {
                        ch: s.ch.clone(),
                        points: [tl, tr, br, bl],
                        score: s.score,
                        index: s.index,
                        t_start: s.t_start,
                        t_end: s.t_end,
                    });
                }
            }

            out.push(TextLine {
                points: b.points,
                text,
                score,
                det_score: b.score,
                // ddddocr 没有方向分类
                rotated: false,
                frames,
                chars,
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 预处理：复刻 ddddocr 的 `_preprocess_image`
// ---------------------------------------------------------------------------

/// 一行图 -> `[1,1,64,W]` 灰度张量（值域 0..1）。
///
/// 顺序严格按参考实现：**先 resize 再转灰度**。反过来（先灰度再 resize）
/// 也是线性算子，但插值权重作用在不同通道上，结果会有 1~2 个灰阶的差；
/// 对分类器通常不影响 argmax，可一旦影响就是「某个字识别错」这种难查的问题，
/// 所以不省这一步。
pub fn preprocess_gray(img: &DynamicImage) -> Array4<f32> {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w == 0 || h == 0 {
        return Array4::<f32>::zeros((1, 1, REC_H as usize, 1));
    }
    // 参考实现是 int(w * 64 / h)，Python 的 int() 向零截断
    let target_w = ((w as f64) * (REC_H as f64) / (h as f64)).floor().max(1.0) as u32;

    let resized = resize_lanczos(&rgb, target_w, REC_H);

    let mut t = Array4::<f32>::zeros((1, 1, REC_H as usize, target_w as usize));
    for (x, y, px) in resized.enumerate_pixels() {
        let [r, g, b] = px.0;
        t[[0, 0, y as usize, x as usize]] = luma_601(r, g, b) as f32 / 255.0;
    }
    t
}

/// PIL 的 `Image.resize(..., LANCZOS)`。
///
/// 用的是 `image` crate 的 Lanczos3（support=3 的窗函数 sinc），与 PIL 的
/// `LANCZOS` 同名同阶。两者都是定点近似，逐像素可能有 ±1 的差，
/// 实测对该模型的 argmax 无影响（见 `tests/ddddocr_parity.rs` 的期望值）。
fn resize_lanczos(src: &RgbImage, dst_w: u32, dst_h: u32) -> RgbImage {
    if src.width() == dst_w && src.height() == dst_h {
        return src.clone();
    }
    image::imageops::resize(src, dst_w, dst_h, image::imageops::FilterType::Lanczos3)
}

/// PIL `convert('L')` 用的就是 ITU-R 601-2 亮度公式：
/// `L = 0.299R + 0.587G + 0.114B`，按整数四舍五入。
///
/// 注意 `image` crate 的 `to_luma8()` 用的是 Rec.709 系数（0.2126/0.7152/0.0722），
/// 绿色权重低得多，白底黑字的边缘灰阶会整体偏暗，别混用。
fn luma_601(r: u8, g: u8, b: u8) -> u8 {
    let v = 299 * r as u32 + 587 * g as u32 + 114 * b as u32;
    ((v + 500) / 1000).min(255) as u8
}

// ---------------------------------------------------------------------------
// 解码：logits + 「blank 在字典里」的约定
// ---------------------------------------------------------------------------

/// rec 输出的内存布局。
///
/// ddddocr 的实际输出是 `[T, 1, C]`（PP-OCR 是 `[1, T, C]`）。两种都认，
/// 判定依据是「哪一维等于 1」，而不是硬编码——改模型时布局变了不该直接错位。
///
/// 三种布局**展平后是同一个公式** `offset = t*C + c`（被忽略的那一维长度恒为 1），
/// 所以解码时不需要按布局分支，只按 `[T, C]` 的连续切片线性访问。
/// 这一点很关键：早先用 `p[[t, 0, c]]` 动态索引，每帧每类都要建一次 IxDyn，
/// 98 帧 × 8210 类把单行解码拖到 **300ms**（推理本身只要 29ms）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// `[T, C]`
    Tc,
    /// `[T, 1, C]`（ddddocr 实测布局）
    T1c,
    /// `[1, T, C]`（PP-OCR 布局）
    OneTc,
}

fn layout_of(p: &ndarray::ArrayViewD<f32>) -> Option<(Layout, usize, usize)> {
    let s = p.shape();
    match s.len() {
        2 => Some((Layout::Tc, s[0], s[1])),
        3 if s[1] == 1 => Some((Layout::T1c, s[0], s[2])),
        3 if s[0] == 1 => Some((Layout::OneTc, s[1], s[2])),
        _ => None,
    }
}

/// 取得按 `[T, C]` 排布的连续数据。
///
/// 标准布局时零拷贝；非标准布局退化成一次按逻辑顺序的拷贝（`iter()` 就是逻辑顺序，
/// 所以不存在「顺序错了但看着能跑」的风险）。
fn flatten<'a>(p: &'a ndarray::ArrayViewD<'a, f32>) -> Option<std::borrow::Cow<'a, [f32]>> {
    match p.as_slice() {
        Some(s) => Some(std::borrow::Cow::Borrowed(s)),
        None => Some(std::borrow::Cow::Owned(p.iter().copied().collect())),
    }
}

/// CTC 解码（logits 版，blank 在 `chars[0]`）。
///
/// 与 `ocr::rec::ctc_decode_spans` 的两点差异：
/// - 这里的模型输出是 **logits**，置信度要现算 softmax（只算被采纳的帧，见下）；
/// - 这里的 blank 就是 `chars[0]`，字符映射是 `chars[idx]` 而不是 `chars[idx-1]`。
///
/// 置信度只对「被采纳的帧」求 softmax：`p = 1 / Σ_j exp(v_j - v_max)`。
/// 全量 softmax 是 `T×8210` 次 exp（一行十几万次），而真正需要的往往只有几十帧，
/// 差着一个数量级。数学上这是精确的，不是近似——argmax 的概率就是 max 项的 softmax 值。
fn decode_logits(
    flat: &[f32],
    t: usize,
    c: usize,
    chars: &[String],
) -> (String, f64, Vec<CharSpan>) {
    if t == 0 || c == 0 || flat.len() < t * c {
        return (String::new(), 0.0, Vec::new());
    }

    let mut spans: Vec<CharSpan> = Vec::new();
    let mut prev = usize::MAX;

    for i in 0..t {
        let row = &flat[i * c..(i + 1) * c];
        let (idx, max) = argmax(row);

        if idx == 0 {
            prev = 0;
            continue;
        }
        if idx == prev {
            if let Some(last) = spans.last_mut() {
                last.t_end = i;
            }
            continue;
        }
        if idx < chars.len() {
            spans.push(CharSpan {
                ch: chars[idx].clone(),
                index: spans.len(),
                t_start: i,
                t_end: i,
                score: softmax_at(row, idx, max) as f64,
            });
        }
        prev = idx;
    }

    let text: String = spans.iter().map(|s| s.ch.as_str()).collect();
    let score = if spans.is_empty() {
        0.0
    } else {
        spans.iter().map(|s| s.score).sum::<f64>() / spans.len() as f64
    };
    (text, score, spans)
}

fn argmax(row: &[f32]) -> (usize, f32) {
    let mut bi = 0usize;
    let mut bv = f32::NEG_INFINITY;
    for (j, v) in row.iter().enumerate() {
        if *v > bv {
            bv = *v;
            bi = j;
        }
    }
    (bi, bv)
}

/// 第 `idx` 类在该帧的 softmax 概率。只算这一帧，不做整表。
fn softmax_at(row: &[f32], idx: usize, max: f32) -> f32 {
    let mut sum = 0f32;
    for v in row {
        sum += (*v - max).exp();
    }
    if sum <= 0.0 {
        return 0.0;
    }
    // exp(v_idx - max) 就是分子；若 idx 恰好是 max，分子为 1
    ((row[idx] - max).exp() / sum).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// 字典
// ---------------------------------------------------------------------------

/// 读字典。**不过滤空行**——第 0 行就是 blank，过滤掉会让整行文字错位一个字。
///
/// 只去掉行尾的 `\r`（Windows 换行）与文件头的 BOM。
pub fn load_charset(path: &Path) -> Result<Vec<String>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let chars: Vec<String> = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    // 文件以换行结尾时会多出一个空元素，去掉它（但保留开头那个 blank）
    let chars = match chars.last() {
        Some(l) if l.is_empty() && chars.len() > 1 => chars[..chars.len() - 1].to_vec(),
        _ => chars,
    };
    if chars.len() < 2 {
        return Err(format!("字典为空或只有 blank: {}", path.display()));
    }
    if !chars[0].is_empty() {
        // 不是致命错误，但几乎肯定拿错文件了
        tracing::warn!(
            "{} 的首行不是空字符串，blank 可能不在索引 0",
            path.display()
        );
    }
    Ok(chars)
}

/// 便捷函数：按 `models_root` 与版本名构造引擎（给测试与命令层用）。
pub fn engine_from_root(
    models_root: &Path,
    variant: &str,
    det_level: ModelLevel,
    device: Device,
) -> Result<DdddOcrEngine, VisionError> {
    DdddOcrEngine::load(DdddConfig {
        dir: catalog::ddddocr_dir(models_root, variant),
        det_dir: catalog::ppocr_dir(models_root, det_level),
        device,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn img(w: u32, h: u32, color: [u8; 3]) -> DynamicImage {
        let mut rgb = RgbImage::new(w, h);
        for p in rgb.pixels_mut() {
            *p = Rgb(color);
        }
        DynamicImage::ImageRgb8(rgb)
    }

    #[test]
    fn preprocess_shape_uses_floor_of_ratio() {
        // 379×31 -> 宽度 floor(379*64/31) = floor(782.45) = 782
        let t = preprocess_gray(&img(379, 31, [255, 255, 255]));
        assert_eq!(t.shape(), &[1, 1, 64, 782]);
        // 纯白图 -> 全 1.0
        assert!((t[[0, 0, 0, 0]] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn preprocess_keeps_extreme_aspect_alive() {
        // 极宽图：int(1 * 64/1000) = 0 -> 必须兜到 1，否则张量宽度 0 直接炸
        let t = preprocess_gray(&img(1, 1000, [10, 20, 30]));
        assert_eq!(t.shape()[3], 1);
    }

    #[test]
    fn luma_uses_itu_r_601() {
        // 纯绿 255 的正确亮度是 0.587*255 ≈ 150（Rec.709 会给 182）
        assert_eq!(luma_601(0, 255, 0), 150);
        assert_eq!(luma_601(255, 255, 255), 255);
        assert_eq!(luma_601(0, 0, 0), 0);
    }

    #[test]
    fn charset_keeps_leading_blank_and_drops_trailing_newline() {
        let dir = std::env::temp_dir().join(format!("dddd_cs_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("charset.txt");
        std::fs::write(&p, "\u{feff}\nA\r\n \r\nB\n").unwrap();
        let cs = load_charset(&p).unwrap();
        // ["", "A", " ", "B"]：blank 在 0、空格是合法字符、结尾换行不多出一项
        assert_eq!(cs, vec!["", "A", " ", "B"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_maps_index_zero_as_blank_not_as_char() {
        // 4 类：0=blank, 1='a', 2='b', 3='c'
        let chars: Vec<String> = ["", "a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let mut logits = ndarray::Array2::<f32>::zeros((5, 4));
        for (t, i) in [1usize, 1, 0, 2, 2].iter().enumerate() {
            logits[[t, *i]] = 10.0;
        }
        let view = logits.view().into_dyn();
        let flat = flatten(&view).unwrap();
        let (text, score, spans) = decode_logits(&flat, 5, 4, &chars);
        // 索引 1 必须映射成 'a'（若按 PP-OCR 约定会错位成 blank 下标外的东西）
        assert_eq!(text, "ab");
        assert_eq!(spans.len(), 2);
        assert!(score > 0.9);
    }

    #[test]
    fn decode_accepts_t1c_and_1tc_layouts() {
        // ddddocr 的真实布局 [T,1,C] 与 PP-OCR 的 [1,T,C] 必须得到同样结果
        let chars: Vec<String> = ["", "a", "b"].iter().map(|s| s.to_string()).collect();
        for one_first in [false, true] {
            if one_first {
                let mut logits = ndarray::Array3::<f32>::zeros((1, 3, 3));
                logits[[0, 0, 1]] = 5.0;
                logits[[0, 1, 0]] = 5.0;
                logits[[0, 2, 2]] = 5.0;
                let view = logits.view().into_dyn();
                let (lay, t, c) = layout_of(&view).unwrap();
                assert_eq!(lay, Layout::OneTc);
                let flat = flatten(&view).unwrap();
                assert_eq!(decode_logits(&flat, t, c, &chars).0, "ab");
            } else {
                let mut logits = ndarray::Array3::<f32>::zeros((3, 1, 3));
                logits[[0, 0, 1]] = 5.0;
                logits[[1, 0, 0]] = 5.0;
                logits[[2, 0, 2]] = 5.0;
                let view = logits.view().into_dyn();
                let (lay, t, c) = layout_of(&view).unwrap();
                assert_eq!(lay, Layout::T1c);
                let flat = flatten(&view).unwrap();
                assert_eq!(decode_logits(&flat, t, c, &chars).0, "ab");
            }
        }
    }

    #[test]
    fn softmax_at_is_a_real_probability() {
        // 均匀 logits -> 概率 1/3
        let row = [1.0f32, 1.0, 1.0];
        let p = softmax_at(&row, 1, 1.0);
        assert!((p - 1.0 / 3.0).abs() < 1e-6);
    }
}
