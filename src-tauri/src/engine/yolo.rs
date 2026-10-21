//! YOLO 通用目标检测引擎（v8 优先）。
//!
//! 预处理与后处理严格对齐 ultralytics 的 predict 路径，验收方式是
//! **与本机 ultralytics 逐框比对**（`tests/yolo_parity.rs`）：
//!
//! 1. **letterbox**：`scale = min(640/w, 640/h)`，缩放用 **cv2.INTER_LINEAR**
//!    （这里直接复用 `ocr::preprocess::resize_bilinear`——它是 OpenCV 11bit 定点
//!    路径的逐位复刻，PP-OCR 精度基线就是靠它过的）；填充值按 [`YoloVersion::pad_value`]
//!    （v8=114）。ultralytics 里所有 `round()` 都是 Python 的银行家舍入，
//!    Rust 对应用 `round_ties_even()`，用 `f64::round()` 会在 .5 边界差一个像素；
//! 2. **输入**：RGB、`/255`、NCHW `[1,3,640,640]`（导出时 opset12 固定尺寸）；
//! 3. **输出布局按版本分**：v8/v11 是 `[1, 4+C, N]`（无 objectness，需转置）；
//!    v5/v7 是 `[1, N, 5+C]`（conf = obj × max_cls）。两种布局的偏移公式见
//!    [`decode`]，不能用同一套索引；
//! 4. **NMS**：conf 过滤(0.25) → 按 conf 降序（并列时先出现者在前，与
//!    torchvision 一致）→ 同类内贪心抑制（IoU 0.7，ultralytics 用类别偏移技巧
//!    实现的 class-aware NMS 与之等价）→ max_det 300 → 还原原图坐标 →
//!    clip 到 `[0, w] × [0, h]`。注意 NMS 在 **letterbox 坐标系**里做
//!    （ultralytics 也是），之后才缩放回去。

use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, RgbImage};
use ndarray::Array4;

use super::catalog;
use super::{
    Capabilities, DetectOptions, Detection, EngineSpec, EngineStatus, RecognizeOptions, TextLine,
    VisionEngine, VisionError, YoloVersion,
};
use crate::config::Device;
use crate::ocr::preprocess::resize_bilinear;

/// 模型输入边长。`scripts/fetch_yolo.ps1` 按 640 导出；换尺寸必须重新导出，
/// 否则固定形状的模型会直接推理失败。
pub const IMG_SIZE: u32 = 640;

/// ultralytics 默认 `max_det=300`。
const MAX_DET: usize = 300;

#[derive(Debug, Clone)]
pub struct YoloConfig {
    /// `models/detect/yolo/<version>`
    pub dir: PathBuf,
    pub version: YoloVersion,
    pub device: Device,
}

pub struct YoloEngine {
    sess: ort::session::Session,
    input: String,
    version: YoloVersion,
    /// 类别名，行号即 class_id；缺失时为空、名字回落 `class_{id}`
    classes: Vec<String>,
    cfg: YoloConfig,
    actual_device: Device,
}

impl YoloEngine {
    pub fn load(cfg: YoloConfig) -> Result<Self, VisionError> {
        let model_path = cfg.dir.join("model.onnx");
        if !model_path.exists() {
            return Err(VisionError::NotReady(format!(
                "缺少检测模型 {}（用 scripts/fetch_yolo.ps1 导出）",
                model_path.display()
            )));
        }
        let labels_path = cfg.dir.join("labels.txt");
        let classes = if labels_path.exists() {
            load_labels(&labels_path)
        } else {
            tracing::warn!(
                "{} 缺少 labels.txt，类别名将显示为 class_{{id}}",
                cfg.dir.display()
            );
            Vec::new()
        };

        let env = crate::ocr::ort_util::ort_env()
            .map_err(|e| VisionError::NotReady(format!("ORT 环境初始化失败: {e}")))?;
        let (sess, actual_device) = crate::ocr::ort_util::build_session_with_fallback(
            env,
            &model_path,
            cfg.device,
            [1, 3, IMG_SIZE as usize, IMG_SIZE as usize],
        )
        .map_err(|e| VisionError::NotReady(format!("检测模型加载失败: {e}")))?;
        let input = crate::ocr::ort_util::input_name(&sess);

        Ok(Self {
            sess,
            input,
            version: cfg.version,
            classes,
            cfg,
            actual_device,
        })
    }

    pub fn class_name(&self, id: usize) -> String {
        self.classes
            .get(id)
            .cloned()
            .unwrap_or_else(|| format!("class_{id}"))
    }

    /// 单图检测（不做 HTTP/DTO 层的事，给测试与 trait 共用）。
    pub fn detect_image(
        &mut self,
        img: &DynamicImage,
        opts: &DetectOptions,
    ) -> Result<Vec<Detection>, VisionError> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }

        let (tensor, lb) = letterbox(img, IMG_SIZE, self.version.pad_value());
        // 类别表先拷出来：`out` 活着期间持有 self.sess 的可变借用，
        // 那之后不能再走 &self 的方法（ddddocr 是直接取字段所以没这个坑）
        let classes = self.classes.clone();
        let out = self
            .sess
            .run(ort::inputs![
                self.input.as_str() => ort::value::Tensor::from_array(tensor)
                    .map_err(|e| VisionError::Inference(e.to_string()))?
            ])
            .map_err(|e| VisionError::Inference(format!("yolo 推理失败: {e}")))?;
        let p = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("yolo 输出读取失败: {e}")))?;

        let cands = decode(&p, self.version.has_objectness())
            .ok_or_else(|| VisionError::Inference(format!("yolo 输出维度不认识: {:?}", p.shape())))?;

        let kept = nms(cands, opts);

        // letterbox 坐标 -> 原图坐标 -> clip 到图内（ultralytics 的 clip_boxes
        // 是闭区间 [0, w]/[0, h]，不是 [0, w-1]）
        let sx = 1.0 / lb.scale;
        let mut dets = Vec::with_capacity(kept.len());
        for c in kept {
            let x1 = ((c.x1 as f64 - lb.pad_x) * sx).clamp(0.0, w as f64);
            let y1 = ((c.y1 as f64 - lb.pad_y) * sx).clamp(0.0, h as f64);
            let x2 = ((c.x2 as f64 - lb.pad_x) * sx).clamp(0.0, w as f64);
            let y2 = ((c.y2 as f64 - lb.pad_y) * sx).clamp(0.0, h as f64);
            let name = classes
                .get(c.class_id)
                .cloned()
                .unwrap_or_else(|| format!("class_{}", c.class_id));
            dets.push(Detection {
                points: [(x1, y1), (x2, y1), (x2, y2), (x1, y2)],
                class_id: c.class_id,
                class_name: name,
                score: c.score as f64,
            });
        }
        Ok(dets)
    }
}

impl VisionEngine for YoloEngine {
    fn spec(&self) -> EngineSpec {
        EngineSpec::yolo(self.version)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            line_boxes: false,
            char_boxes: false,
            rotation_cls: false,
            detect: true,
            classes: self.classes.clone(),
            batch: false,
        }
    }

    fn status(&self) -> EngineStatus {
        EngineStatus {
            spec: self.spec(),
            capabilities: self.capabilities(),
            available: true,
            actual_device: Some(self.actual_device),
            note: Some(format!(
                "{:?} 检测，输入 {}x{}，后处理对齐 ultralytics（conf=0.25/iou=0.7 缺省）",
                self.version, IMG_SIZE, IMG_SIZE
            )),
        }
    }

    /// 检测引擎不做文本识别。用 trait 的默认实现（`Unsupported`）还不够——
    /// 这里显式写出来是为了让报错信息带上「该用 /vision/detect」的指引。
    fn infer(
        &mut self,
        _img: &DynamicImage,
        _opts: &RecognizeOptions,
    ) -> Result<Vec<TextLine>, VisionError> {
        Err(VisionError::Unsupported {
            engine: self.spec().key(),
            capability: "recognize（检测引擎请走 /vision/detect）",
        })
    }

    fn detect(
        &mut self,
        img: &DynamicImage,
        opts: &DetectOptions,
    ) -> Result<Vec<Detection>, VisionError> {
        self.detect_image(img, opts)
    }
}

// ---------------------------------------------------------------------------
// letterbox：对齐 ultralytics `LetterBox(auto=False, center=True)`
// ---------------------------------------------------------------------------

/// letterbox 结果的还原参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LetterboxInfo {
    /// 缩放系数（原图 -> 输入图）
    pub scale: f64,
    /// 左/上填充像素
    pub pad_x: f64,
    pub pad_y: f64,
}

/// 原图 -> `[1,3,S,S]` 张量（RGB、/255、letterbox 填充）。
pub fn letterbox(img: &DynamicImage, size: u32, pad_value: u8) -> (Array4<f32>, LetterboxInfo) {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let size_f = size as f64;

    // Python: scale = min(size/w, size/h)
    let scale = (size_f / w as f64).min(size_f / h as f64);
    // Python round() 是银行家舍入
    let new_w = (w as f64 * scale).round_ties_even().max(1.0) as u32;
    let new_h = (h as f64 * scale).round_ties_even().max(1.0) as u32;

    let resized = resize_bilinear(&rgb, new_w, new_h);

    // auto=False：dw/dh 直接均分，不做 stride 取模
    let dw = (size - new_w) as f64 / 2.0;
    let dh = (size - new_h) as f64 / 2.0;
    // Python: top = round(dh - 0.1), bottom = round(dh + 0.1)（银行家舍入）。
    // right/bottom = left/top + new_w/new_h，overlay 只需要左上角。
    let left = (dw - 0.1).round_ties_even() as i32;
    let top = (dh - 0.1).round_ties_even() as i32;

    let mut canvas = RgbImage::from_pixel(size, size, image::Rgb([pad_value; 3]));
    image::imageops::overlay(
        &mut canvas,
        &resized,
        left.max(0) as i64,
        top.max(0) as i64,
    );

    let mut t = Array4::<f32>::zeros((1, 3, size as usize, size as usize));
    for (x, y, px) in canvas.enumerate_pixels() {
        let [r, g, b] = px.0;
        // ultralytics: img[..., ::-1].transpose(2,0,1) 即 BGR->RGB，再 /255
        t[[0, 0, y as usize, x as usize]] = r as f32 / 255.0;
        t[[0, 1, y as usize, x as usize]] = g as f32 / 255.0;
        t[[0, 2, y as usize, x as usize]] = b as f32 / 255.0;
    }

    (
        t,
        LetterboxInfo {
            scale,
            pad_x: left as f64,
            pad_y: top as f64,
        },
    )
}

// ---------------------------------------------------------------------------
// 输出解码：v8/v11 [1,4+C,N] 与 v5/v7 [1,N,5+C]
// ---------------------------------------------------------------------------

/// 解码后的候选框（letterbox 坐标系）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub class_id: usize,
    pub score: f32,
}

/// 按布局解码全部候选。
///
/// v8/v11：`[1, 4+C, N]`，值域已是概率（导出图带 sigmoid），偏移 `c*N + n`；
/// v5/v7：`[1, N, 5+C]`，`conf = obj * max_cls`，偏移 `n*(5+C) + ch`。
/// 返回 `None` 表示维度不认识（交给调用方报错，而不是静默给出垃圾框）。
pub fn decode(p: &ndarray::ArrayViewD<f32>, has_objectness: bool) -> Option<Vec<Candidate>> {
    let s = p.shape();
    let flat: &[f32] = p.as_slice()?; // 连续布局；ORT 输出总是连续的
    let cands = match (has_objectness, s) {
        // v8/v11
        (false, [1, ch, n]) => {
            let (ch, n) = (*ch, *n);
            if ch < 5 {
                return None;
            }
            let c = ch - 4;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let cx = flat[i];
                let cy = flat[n + i];
                let bw = flat[2 * n + i];
                let bh = flat[3 * n + i];
                // 类别分数
                let (best, score) = (0..c)
                    .map(|k| (k, flat[(4 + k) * n + i]))
                    .fold((0usize, f32::MIN), |(bi, bs), (k, v)| {
                        if v > bs {
                            (k, v)
                        } else {
                            (bi, bs)
                        }
                    });
                out.push(Candidate {
                    x1: cx - bw / 2.0,
                    y1: cy - bh / 2.0,
                    x2: cx + bw / 2.0,
                    y2: cy + bh / 2.0,
                    class_id: best,
                    score,
                });
            }
            out
        }
        // v5/v7
        (true, [1, n, ch]) => {
            let (n, ch) = (*n, *ch);
            if ch < 6 {
                return None;
            }
            let c = ch - 5;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let row = &flat[i * ch..(i + 1) * ch];
                let (cx, cy, bw, bh, obj) = (row[0], row[1], row[2], row[3], row[4]);
                let (best, cls) = (0..c)
                    .map(|k| (k, row[5 + k]))
                    .fold((0usize, f32::MIN), |(bi, bs), (k, v)| {
                        if v > bs {
                            (k, v)
                        } else {
                            (bi, bs)
                        }
                    });
                out.push(Candidate {
                    x1: cx - bw / 2.0,
                    y1: cy - bh / 2.0,
                    x2: cx + bw / 2.0,
                    y2: cy + bh / 2.0,
                    class_id: best,
                    score: obj * cls,
                });
            }
            out
        }
        _ => return None,
    };
    Some(cands)
}

// ---------------------------------------------------------------------------
// NMS：对齐 ultralytics non_max_suppression（agnostic=False）
// ---------------------------------------------------------------------------

/// 贪心 NMS：conf 过滤 -> 降序（稳定，并列时先出现者先）-> 同类内抑制。
///
/// ultralytics 用「框坐标加 `class_id × 7680` 偏移再调 torchvision NMS」实现
/// class-aware 抑制，结果与「同类内贪心」完全一致；这里直接写同类判断，
/// 免去大数偏移的精度戏法。
pub fn nms(cands: Vec<Candidate>, opts: &DetectOptions) -> Vec<Candidate> {
    // conf 过滤 + 类别过滤（class_filter 为空 = 全部类别）
    let mut kept: Vec<Candidate> = cands
        .into_iter()
        .filter(|c| {
            c.score >= opts.conf_thresh
                && (opts.class_filter.is_empty() || opts.class_filter.contains(&c.class_id))
        })
        .collect();
    // 降序；`sort_by` 是稳定排序，conf 并列时保持输出顺序（与 torchvision 一致）
    kept.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut result: Vec<Candidate> = Vec::with_capacity(kept.len().min(MAX_DET));
    'outer: for c in kept {
        if result.len() >= MAX_DET {
            break;
        }
        for r in &result {
            // 只在同类之间抑制；NMS 在 letterbox 坐标系里做（与 ultralytics 一致，
            // 平移缩放对 IoU 无影响，但坐标系必须统一）
            if r.class_id == c.class_id && iou(r, &c) > opts.iou_thresh {
                continue 'outer;
            }
        }
        result.push(c);
    }
    result
}

/// 两框 IoU（xyxy）。
fn iou(a: &Candidate, b: &Candidate) -> f32 {
    let ix1 = a.x1.max(b.x1);
    let iy1 = a.y1.max(b.y1);
    let ix2 = a.x2.min(b.x2);
    let iy2 = a.y2.min(b.y2);
    let iw = (ix2 - ix1).max(0.0);
    let ih = (iy2 - iy1).max(0.0);
    let inter = iw * ih;
    let area = |c: &Candidate| (c.x2 - c.x1) * (c.y2 - c.y1);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

// ---------------------------------------------------------------------------
// labels
// ---------------------------------------------------------------------------

/// 读 labels.txt：每行一个类名，行号即 class_id。允许尾随空行。
pub fn load_labels(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let mut labels: Vec<String> = content
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    while labels.last().is_some_and(|l| l.is_empty()) {
        labels.pop();
    }
    labels
}

/// 便捷构造：按 `models_root` 与版本加载。
pub fn engine_from_root(
    models_root: &Path,
    version: YoloVersion,
    device: Device,
) -> Result<YoloEngine, VisionError> {
    YoloEngine::load(YoloConfig {
        dir: catalog::yolo_dir(models_root, version),
        version,
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
    fn letterbox_square_input_is_identity() {
        let (t, lb) = letterbox(&img(640, 640, [1, 2, 3]), 640, 114);
        assert_eq!(t.shape(), &[1, 3, 640, 640]);
        assert_eq!(lb, LetterboxInfo { scale: 1.0, pad_x: 0.0, pad_y: 0.0 });
        assert_eq!(t[[0, 0, 0, 0]], 1.0 / 255.0);
    }

    #[test]
    fn letterbox_wide_image_pads_top_and_bottom() {
        // 100x50 -> scale 6.4 -> 640x320，上下各填 160
        let (t, lb) = letterbox(&img(100, 50, [200, 100, 50]), 640, 114);
        assert_eq!(t.shape(), &[1, 3, 640, 640]);
        assert!((lb.scale - 6.4).abs() < 1e-9);
        assert_eq!(lb.pad_y, 160.0);
        // 内容区首行应是源图首行（颜色 200/255）
        assert!((t[[0, 0, 160, 320]] - 200.0 / 255.0).abs() < 2.0 / 255.0);
        // 填充区必须是 114
        assert_eq!(t[[0, 0, 0, 0]], 114.0 / 255.0);
        assert_eq!(t[[0, 2, 639, 639]], 114.0 / 255.0);
    }

    #[test]
    fn letterbox_uses_bankers_rounding_like_python() {
        // 3x1 -> scale = 640/3，new_h = round(213.333) = 213，
        // dh = 427/2 = 213.5 -> top = round_ties_even(213.4) = 213（普通 round 也是 213）
        // 关键用例是下面这个：dh - 0.1 = 213.4 恒非整，但构造 0.5 边界验证:
        // 2x1 -> scale=320, new=(640,320)? h*scale = 320 -> dw=0, dh=320/2=160
        // 选 h 使 dh+0.1 落在 .5：h=5 -> scale=128, new_h=640? 直接验证 21x10:
        // 10 高 -> scale=64, new_h=640。改用 11 宽 10 高：scale=640/11=58.18..
        // 这些整数边界都用 round_ties_even 与 Python round 等价，逐值锁住：
        let (_, lb) = letterbox(&img(3, 1, [0, 0, 0]), 640, 114);
        // new_w = round(3*640/3)=640, new_h = round(640/3)=213（.333）
        // dw=0, dh=(640-213)/2=213.5 -> top=round(213.4)=213, bottom=round(213.6)=214
        assert_eq!(lb.pad_y, 213.0);
        let (_, lb2) = letterbox(&img(1, 3, [0, 0, 0]), 640, 114);
        assert_eq!(lb2.pad_x, 213.0);
    }

    #[test]
    fn decode_v8_transposed_layout() {
        // [1, 4+2, 3]：3 个候选，2 类
        let mut a = ndarray::Array3::<f32>::zeros((1, 6, 3));
        // 候选 0：cx=10,cy=20,w=4,h=6，类别 1 得分 0.9
        a[[0, 0, 0]] = 10.0;
        a[[0, 1, 0]] = 20.0;
        a[[0, 2, 0]] = 4.0;
        a[[0, 3, 0]] = 6.0;
        a[[0, 5, 0]] = 0.9;
        // 候选 1：类别 0 得分 0.5
        a[[0, 4, 1]] = 0.5;
        // 候选 2：低分
        a[[0, 4, 2]] = 0.1;
        let view = a.view().into_dyn();
        let cands = decode(&view, false).unwrap();
        assert_eq!(cands.len(), 3);
        assert_eq!(cands[0].class_id, 1);
        assert!((cands[0].score - 0.9).abs() < 1e-6);
        assert!((cands[0].x1 - 8.0).abs() < 1e-6);
        assert!((cands[0].y2 - 23.0).abs() < 1e-6);
        assert_eq!(cands[1].class_id, 0);
    }

    #[test]
    fn decode_v5_objectness_multiplies_class_score() {
        // [1, 1, 5+2]
        let mut a = ndarray::Array3::<f32>::zeros((1, 1, 7));
        for (i, v) in [10.0f32, 20.0, 4.0, 6.0, 0.8].iter().enumerate() {
            a[[0, 0, i]] = *v;
        }
        a[[0, 0, 6]] = 0.5; // 类别 1
        let view = a.view().into_dyn();
        let cands = decode(&view, true).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].class_id, 1);
        // conf = obj * cls = 0.8 * 0.5
        assert!((cands[0].score - 0.4).abs() < 1e-6);
    }

    fn cand(x1: f32, y1: f32, x2: f32, y2: f32, cls: usize, score: f32) -> Candidate {
        Candidate { x1, y1, x2, y2, class_id: cls, score }
    }

    #[test]
    fn nms_suppresses_same_class_only() {
        // A 与 B 高度重叠（同类，B 被抑制）；C 与 A 重叠但异类（保留）
        let a = cand(0.0, 0.0, 10.0, 10.0, 0, 0.9);
        // B：inter = 9.5^2 = 90.25, union = 200 - 90.25 = 109.75, IoU ≈ 0.822 -> 被抑制
        let b = cand(0.5, 0.5, 10.5, 10.5, 0, 0.8);
        let c = cand(0.0, 0.0, 10.0, 10.0, 1, 0.7);
        let d = cand(100.0, 100.0, 110.0, 110.0, 0, 0.6);
        let out = nms(vec![d, b, a, c], &DetectOptions::default());
        // 顺序按分数降序：a(0.9), b(0.8, 被抑制), c(0.7, 异类保留), d(0.6)
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].score, 0.9);
        assert_eq!(out[1].class_id, 1);
        assert_eq!(out[2].score, 0.6);
    }

    #[test]
    fn nms_respects_conf_and_class_filter() {
        let a = cand(0.0, 0.0, 10.0, 10.0, 0, 0.2); // 低于 0.25
        let b = cand(0.0, 0.0, 10.0, 10.0, 1, 0.9); // 类别不在过滤表
        let c = cand(0.0, 0.0, 10.0, 10.0, 2, 0.8);
        let opts = DetectOptions {
            conf_thresh: 0.25,
            iou_thresh: 0.7,
            class_filter: vec![2],
        };
        let out = nms(vec![a, b, c], &opts);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].class_id, 2);
    }

    #[test]
    fn labels_drop_trailing_blank_lines_but_keep_interior() {
        let dir = std::env::temp_dir().join(format!("yolo_lb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("labels.txt");
        std::fs::write(&p, "\u{feff}person\n\ncar\n\n").unwrap();
        assert_eq!(load_labels(&p), vec!["person", "", "car"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
