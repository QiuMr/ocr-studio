//! 目标检测引擎：**一个引擎吃所有「结构差不多」的 YOLO 导出**。
//!
//! 2026-10-22 用户要求「不应该指定版本，目标检测就专门一类」。所以这里不再有
//! `YoloVersion`：模型结构在**加载时从 ONNX 的输入输出形状探测**，能覆盖
//! v5 / v6 / v7 / v8 / v10 / v11 / v12 / v26 这类单输出 head 的导出。
//!
//! 探测的三件事（见 [`YoloLayout`]）：
//! - **输出布局**：`[1, C, N]`（anchor-free，v8 系）还是 `[1, N, C]`（anchor-based，v5 系）。
//!   判据是哪一维更小——通道数（几十到几百）永远远小于候选数（几千到几万）；
//! - **有无 objectness**：`C == 5 + 类别数` 则有（`score = obj × cls`），
//!   `C == 4 + 类别数` 则无（`score = cls`）。类别数来自 `labels.txt`；
//!   拿不到 labels 时用候选数对锚点网格（`8400` vs `25200`）兜底判断；
//! - **输入边长**：读模型的静态输入维度，动态维度回落到 640。
//!
//! 不支持的（会**明确报错**，不静默给垃圾框）：
//! - v3 / v4 原始三尺度多输出 head（需要按 anchor 解码三个 feature map，另一套逻辑）；
//! - 已内置 NMS 的端到端导出（如 YOLOv10 的 `[1, 300, 6]`）——候选数对不上锚点网格。
//!
//! 预处理与后处理严格对齐 ultralytics 的 predict 路径，验收方式是
//! **与本机 ultralytics 逐框比对**（`tests/yolo_parity.rs`，IoU=1.0000 / conf 差=0）：
//!
//! 1. **letterbox**：`scale = min(S/w, S/h)`，缩放用 **cv2.INTER_LINEAR**
//!    （这里直接复用 `ocr::preprocess::resize_bilinear`——它是 OpenCV 11bit 定点
//!    路径的逐位复刻，PP-OCR 精度基线就是靠它过的）；上下 `round(dh±0.1)` 均分，
//!    `round()` 是 Python 的银行家舍入，Rust 对应用 `round_ties_even()`；
//! 2. **填充值统一 114**：ultralytics 全系和 yolov5 官方 `letterbox` 的默认色都是
//!    `(114,114,114)`。早期版本把 v5 写成 0 是错的，别改回去；
//! 3. **输入**：RGB、`/255`、NCHW；
//! 4. **NMS**：conf 过滤(0.25) → 按 conf 降序（并列时先出现者在前，与 torchvision
//!    一致）→ 同类内贪心抑制（IoU 0.7）→ max_det 300 → 还原原图坐标 →
//!    clip 到 `[0, w] × [0, h]`。NMS 在 **letterbox 坐标系**里做（ultralytics 也是），
//!    之后才缩放回去。

use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView, RgbImage};
use ndarray::Array4;
use ort::value::ValueType;

use super::catalog;
use super::{
    Capabilities, DetectOptions, Detection, EngineSpec, EngineStatus, RecognizeOptions, TextLine,
    VisionEngine, VisionError,
};
use crate::config::Device;
use crate::ocr::preprocess::resize_bilinear;

/// 动态输入（或读不到维度）时的默认边长。
pub const DEFAULT_SIZE: u32 = 640;

/// ultralytics 默认 `max_det=300`。
const MAX_DET: usize = 300;

/// letterbox 填充值。全系统一 114，见模块文档第 2 条。
const PAD_VALUE: u8 = 114;

// ---------------------------------------------------------------------------
// 结构探测
// ---------------------------------------------------------------------------

/// 输出张量的排布方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputLayout {
    /// `[1, C, N]`：v8 / v10 / v11 / v12 / v26 等 anchor-free 系
    ChannelsFirst,
    /// `[1, N, C]`：v5 / v6 / v7 等 anchor-based 系
    AnchorsFirst,
}

impl OutputLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputLayout::ChannelsFirst => "C×N",
            OutputLayout::AnchorsFirst => "N×C",
        }
    }
}

/// 加载时探测出的模型结构。取代了原来的 `YoloVersion` 枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YoloLayout {
    pub layout: OutputLayout,
    /// 是否有 objectness 通道（v5 系有，v8 系没有）
    pub has_objectness: bool,
    /// 模型输入边长（静态维度读出来的；非方形时取长边）
    pub input_size: u32,
    /// 候选框数量 N
    pub anchors: usize,
    /// 每个候选的通道数 C
    pub channels: usize,
}

impl YoloLayout {
    /// 给日志/状态页看的一句话。
    pub fn describe(&self) -> String {
        format!(
            "输出 [1,{c},{n}] / {} 布局 / {} / {}x{s} 输入",
            self.layout.as_str(),
            if self.has_objectness {
                "带 objectness"
            } else {
                "无 objectness"
            },
            self.input_size,
            c = self.channels,
            n = self.anchors,
            s = self.input_size,
        )
    }

    /// anchor-free 的锚点总数（640 输入 → 80²+40²+20² = 8400）。
    fn grid(size: u32) -> usize {
        [8u32, 16, 32]
            .iter()
            .map(|s| {
                let g = (size / s) as usize;
                g * g
            })
            .sum()
    }
}

/// 从输出形状 + 类别数推断结构。
///
/// 返回 `Err(说明)` 表示这个模型不支持——调用方应把它转成用户能看懂的
/// `NotReady`，**不要**猜一个布局硬跑（错误布局的解码不会崩，只会给出
/// 一堆看着像框的垃圾，这种 bug 最难查）。
pub fn infer_layout(
    out_dims: &[i64],
    classes: usize,
    input_size: u32,
) -> Result<YoloLayout, String> {
    if out_dims.len() != 3 {
        return Err(format!(
            "输出维度 {out_dims:?} 不是 `[1, C, N]` 三段式。\
             v3/v4 原始导出是三尺度多输出 head（3 个输出张量），需要按 anchor 逐个解码，\
             暂不支持；请用 ultralytics 导出单输出模型（yolo export format=onnx opset=12）"
        ));
    }
    if out_dims[0] != 1 {
        return Err(format!("输出 batch 维不是 1（{out_dims:?}），只支持单图推理"));
    }
    let (a, b) = (out_dims[1], out_dims[2]);
    if a <= 0 || b <= 0 {
        return Err(format!(
            "输出含动态维度（{out_dims:?}），无法确定候选数；导出时请固定 imgsz"
        ));
    }
    let (a, b) = (a as usize, b as usize);

    // 通道数永远远小于候选数，所以小的那一维就是通道
    let (layout, channels, anchors) = if b > a {
        (OutputLayout::ChannelsFirst, a, b)
    } else {
        (OutputLayout::AnchorsFirst, b, a)
    };
    if channels < 5 {
        return Err(format!(
            "输出通道只有 {channels}，装不下 4 个坐标 + 类别分。\
             输出形状 {out_dims:?} 可能是别的东西"
        ));
    }

    let grid = YoloLayout::grid(input_size);
    // 端到端导出（自带 NMS）的候选数远小于锚点网格，只可能是几百
    if anchors != grid && anchors != grid * 3 && anchors < grid {
        return Err(format!(
            "输出候选数 N={anchors} 对不上 {input_size} 输入的锚点网格（{grid} 或 {}）。\
             这通常意味着模型已经内置了 NMS（如 YOLOv10 的 `[1,300,6]` 端到端导出）；\
             请导出 raw head 版本（ultralytics: `yolo export ... nms=False`）",
            grid * 3
        ));
    }

    // 有无 objectness：类别数已知时是硬判据，否则用锚点数兜底
    let has_objectness = if classes > 0 && channels == 4 + classes {
        false
    } else if classes > 0 && channels == 5 + classes {
        true
    } else if anchors == grid {
        // anchor-free 网格数 → 无 objectness
        false
    } else if anchors == grid * 3 {
        // 3 个 anchor × 网格数 → anchor-based，有 objectness
        true
    } else {
        // 两套都对不上：按「无 objectness」走（v8 系更常见），但要说出来
        tracing::warn!(
            "检测模型输出 [1,{channels},{anchors}] 与类别数 {classes}、锚点网格 {grid} 都对不上，\
             先按无 objectness 处理；如果框的置信度整体偏低，八成是猜错了"
        );
        false
    };

    if classes > 0 {
        let expect = if has_objectness {
            5 + classes
        } else {
            4 + classes
        };
        if channels != expect {
            tracing::warn!(
                "检测模型输出通道 {channels} 与「{classes} 个类别 + {}」应有的 {expect} 不一致，\
                 类别名可能错位",
                if has_objectness { "objectness" } else { "4 个坐标" }
            );
        }
    }

    Ok(YoloLayout {
        layout,
        has_objectness,
        input_size,
        anchors,
        channels,
    })
}

/// 读模型第一个输入的边长（静态维度）。读不到就用 [`DEFAULT_SIZE`]。
fn probe_input_size(sess: &ort::session::Session) -> u32 {
    let Some(outlet) = sess.inputs().first() else {
        return DEFAULT_SIZE;
    };
    if let ValueType::Tensor { shape, .. } = outlet.dtype() {
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() == 4 {
            let (h, w) = (dims[2], dims[3]);
            if h > 0 && w > 0 {
                // 非方形导出取长边，letterbox 的边长取它
                return h.max(w) as u32;
            }
        }
    }
    DEFAULT_SIZE
}

/// 读第一个输出的形状。多输出时由调用方先拦掉。
fn probe_output_dims(sess: &ort::session::Session) -> Vec<i64> {
    match sess.outputs().first().map(|o| o.dtype()) {
        Some(ValueType::Tensor { shape, .. }) => shape.iter().copied().collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct YoloConfig {
    /// 实际放 `model.onnx` 的目录（`detect/yolo/` 或兼容的 `detect/yolo/<任意名>/`）
    pub dir: PathBuf,
    pub device: Device,
}

pub struct YoloEngine {
    sess: ort::session::Session,
    input: String,
    layout: YoloLayout,
    /// 类别名，行号即 class_id；缺失时为空、名字回落 `class_{id}`
    classes: Vec<String>,
    dir: PathBuf,
    actual_device: Device,
}

impl YoloEngine {
    pub fn load(cfg: YoloConfig) -> Result<Self, VisionError> {
        let model_path = cfg.dir.join("model.onnx");
        if !model_path.exists() {
            return Err(VisionError::NotReady(format!(
                "缺少检测模型 {}（用 scripts/fetch_yolo.ps1 导出，或自己放一个 model.onnx）",
                model_path.display()
            )));
        }
        let classes = {
            let p = cfg.dir.join("labels.txt");
            if p.exists() {
                load_labels(&p)
            } else {
                tracing::warn!(
                    "{} 缺少 labels.txt，类别名将显示为 class_{{id}}（结构探测也会少一条判据）",
                    cfg.dir.display()
                );
                Vec::new()
            }
        };

        let env = crate::ocr::ort_util::ort_env()
            .map_err(|e| VisionError::NotReady(format!("ORT 环境初始化失败: {e}")))?;
        // 这里不能用 build_session_with_fallback：预热张量的形状必须取自模型本身，
        // 而那个形状只有建完会话才读得到。
        let mut sess = crate::ocr::ort_util::build_session(env, &model_path, cfg.device)
            .map_err(|e| VisionError::NotReady(format!("检测模型加载失败: {e}")))?;
        let input = crate::ocr::ort_util::input_name(&sess);
        let size = probe_input_size(&sess);
        let shape = [1, 3, size as usize, size as usize];

        // 注意 `directml_available()`：这份 DLL 不带 DML 时，就算请求了 DirectML
        // 也必须按 CPU 处理，否则会把「跑在 CPU 上」标成 GPU（见 ort_util 的说明）
        let dml_ok = cfg.device == Device::Directml && crate::ocr::ort_util::directml_available();
        let (mut sess, actual_device) = if dml_ok {
            match crate::ocr::ort_util::warmup(&mut sess, &input, shape) {
                Ok(()) => (sess, Device::Directml),
                Err(e) => {
                    tracing::warn!(
                        "{} 在 DirectML 上预热失败，降级 CPU: {e}",
                        model_path.display()
                    );
                    let mut cpu = crate::ocr::ort_util::build_session(env, &model_path, Device::Cpu)
                        .map_err(|e| VisionError::NotReady(format!("检测模型加载失败: {e}")))?;
                    crate::ocr::ort_util::warmup(&mut cpu, &input, shape).map_err(|e| {
                        VisionError::NotReady(format!("检测模型推理预热失败: {e}"))
                    })?;
                    (cpu, Device::Cpu)
                }
            }
        } else {
            crate::ocr::ort_util::warmup(&mut sess, &input, shape)
                .map_err(|e| VisionError::NotReady(format!("检测模型推理预热失败: {e}")))?;
            (sess, Device::Cpu)
        };

        let n_out = sess.outputs().len();
        if n_out != 1 {
            return Err(VisionError::NotReady(format!(
                "{} 有 {n_out} 个输出张量。多输出 head（v3/v4 原始导出）暂不支持，\
                 请用 ultralytics 导出单输出 ONNX",
                model_path.display()
            )));
        }

        let layout = infer_layout(&probe_output_dims(&sess), classes.len(), size)
            .map_err(|why| VisionError::NotReady(format!("{} 不是可用的检测模型：{why}", model_path.display())))?;

        tracing::info!(
            "检测模型已加载: {} · {} · 类别 {} 个 · 设备 {:?}",
            model_path.display(),
            layout.describe(),
            classes.len(),
            actual_device
        );

        Ok(Self {
            sess,
            input,
            layout,
            classes,
            dir: cfg.dir,
            actual_device,
        })
    }

    pub fn layout(&self) -> YoloLayout {
        self.layout
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

        let size = self.layout.input_size;
        let (tensor, lb) = letterbox(img, size, PAD_VALUE);
        // 类别表与布局先拷出来：`out` 活着期间持有 self.sess 的可变借用，
        // 那之后不能再走 &self 的方法
        let classes = self.classes.clone();
        let layout = self.layout;
        let out = self
            .sess
            .run(ort::inputs![
                self.input.as_str() => ort::value::Tensor::from_array(tensor)
                    .map_err(|e| VisionError::Inference(e.to_string()))?
            ])
            .map_err(|e| VisionError::Inference(format!("检测推理失败: {e}")))?;
        let p = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("检测输出读取失败: {e}")))?;

        let cands = decode(&p, layout)
            .ok_or_else(|| VisionError::Inference(format!("检测输出维度变了: {:?}", p.shape())))?;

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
        EngineSpec::yolo()
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
                "自动识别 {} · 类别 {} 个 · 后处理对齐 ultralytics（conf=0.25/iou=0.7 缺省）",
                self.layout.describe(),
                self.classes.len()
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
// 输出解码：C×N 与 N×C 两套偏移公式
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

/// 按探测出的布局解码全部候选。
///
/// - `ChannelsFirst`：`[1, C, N]`，值域已是概率，偏移 `c*N + n`；
/// - `AnchorsFirst` + objectness：`[1, N, 5+C]`，`conf = obj * max_cls`，偏移 `n*C + ch`；
/// - `AnchorsFirst` 无 objectness：`[1, N, 4+C]`，`conf = max_cls`。
///
/// 返回 `None` 表示维度与探测结果不一致（交给调用方报错，而不是静默给出垃圾框）。
pub fn decode(p: &ndarray::ArrayViewD<f32>, layout: YoloLayout) -> Option<Vec<Candidate>> {
    let s = p.shape();
    let n = layout.anchors;
    let ch = layout.channels;
    let expect = match layout.layout {
        OutputLayout::ChannelsFirst => [1, ch, n],
        OutputLayout::AnchorsFirst => [1, n, ch],
    };
    if s != expect {
        return None;
    }
    let flat: &[f32] = p.as_slice()?; // 连续布局；ORT 输出总是连续的

    let mut out = Vec::with_capacity(n);
    match layout.layout {
        OutputLayout::ChannelsFirst => {
            let c = ch - 4;
            for i in 0..n {
                let (best, score) = argmax(c, |k| flat[(4 + k) * n + i]);
                let (cx, cy) = (flat[i], flat[n + i]);
                let (bw, bh) = (flat[2 * n + i], flat[3 * n + i]);
                out.push(Candidate {
                    x1: cx - bw / 2.0,
                    y1: cy - bh / 2.0,
                    x2: cx + bw / 2.0,
                    y2: cy + bh / 2.0,
                    class_id: best,
                    score,
                });
            }
        }
        OutputLayout::AnchorsFirst => {
            let base = if layout.has_objectness { 5 } else { 4 };
            let c = ch - base;
            for i in 0..n {
                let row = &flat[i * ch..(i + 1) * ch];
                let (cx, cy, bw, bh) = (row[0], row[1], row[2], row[3]);
                let (best, cls) = argmax(c, |k| row[base + k]);
                out.push(Candidate {
                    x1: cx - bw / 2.0,
                    y1: cy - bh / 2.0,
                    x2: cx + bw / 2.0,
                    y2: cy + bh / 2.0,
                    class_id: best,
                    score: if layout.has_objectness {
                        row[4] * cls
                    } else {
                        cls
                    },
                });
            }
        }
    }
    Some(out)
}

/// 在 `0..len` 上取最大值的下标与值（并列时取先出现的，与 numpy argmax 一致）。
fn argmax(len: usize, at: impl Fn(usize) -> f32) -> (usize, f32) {
    let mut bi = 0usize;
    let mut bs = f32::MIN;
    for k in 0..len {
        let v = at(k);
        if v > bs {
            bs = v;
            bi = k;
        }
    }
    (bi, bs)
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

/// 便捷构造：在 `models_root` 下找检测模型并加载。
///
/// 目录下探逻辑在 [`catalog::yolo_model_dir`]，兼容旧的 `detect/yolo/v8/` 结构。
pub fn engine_from_root(models_root: &Path, device: Device) -> Result<YoloEngine, VisionError> {
    let dir = catalog::yolo_model_dir(models_root).ok_or_else(|| {
        VisionError::NotReady(format!(
            "没有找到检测模型。把 ONNX 放到 {}\\model.onnx（配套 labels.txt 放在同目录），\
             或者跑 scripts/fetch_yolo.ps1",
            catalog::yolo_dir(models_root).display()
        ))
    })?;
    YoloEngine::load(YoloConfig { dir, device })
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

    // ---------------------------------------------------------- letterbox

    #[test]
    fn letterbox_square_input_is_identity() {
        let (t, lb) = letterbox(&img(640, 640, [1, 2, 3]), 640, PAD_VALUE);
        assert_eq!(t.shape(), &[1, 3, 640, 640]);
        assert_eq!(lb, LetterboxInfo { scale: 1.0, pad_x: 0.0, pad_y: 0.0 });
        assert_eq!(t[[0, 0, 0, 0]], 1.0 / 255.0);
    }

    #[test]
    fn letterbox_wide_image_pads_top_and_bottom() {
        // 100x50 -> scale 6.4 -> 640x320，上下各填 160
        let (t, lb) = letterbox(&img(100, 50, [200, 100, 50]), 640, PAD_VALUE);
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
        // 3x1 -> new_w=640, new_h=round(640/3)=213，dh=(640-213)/2=213.5
        // top = round(213.4) = 213（普通 round 也是 213，但 .5 边界必须银行家舍入）
        let (_, lb) = letterbox(&img(3, 1, [0, 0, 0]), 640, PAD_VALUE);
        assert_eq!(lb.pad_y, 213.0);
        let (_, lb2) = letterbox(&img(1, 3, [0, 0, 0]), 640, PAD_VALUE);
        assert_eq!(lb2.pad_x, 213.0);
    }

    // ------------------------------------------------------- 结构自动探测

    #[test]
    fn infer_layout_v8_channels_first() {
        // YOLOv8n 导出：84 = 4 + 80 类，8400 = 80²+40²+20²
        let l = infer_layout(&[1, 84, 8400], 80, 640).unwrap();
        assert_eq!(l.layout, OutputLayout::ChannelsFirst);
        assert!(!l.has_objectness);
        assert_eq!((l.channels, l.anchors), (84, 8400));
    }

    #[test]
    fn infer_layout_v5_anchors_first_with_objectness() {
        // YOLOv5s 导出：85 = 5 + 80 类，25200 = 3 × 8400
        let l = infer_layout(&[1, 25200, 85], 80, 640).unwrap();
        assert_eq!(l.layout, OutputLayout::AnchorsFirst);
        assert!(l.has_objectness);
        assert_eq!((l.channels, l.anchors), (85, 25200));
    }

    #[test]
    fn infer_layout_works_without_labels() {
        // 没有 labels.txt 时靠锚点网格判断：8400 = anchor-free，25200 = anchored
        let free = infer_layout(&[1, 84, 8400], 0, 640).unwrap();
        assert!(!free.has_objectness);
        let anchored = infer_layout(&[1, 25200, 85], 0, 640).unwrap();
        assert!(anchored.has_objectness);
    }

    #[test]
    fn infer_layout_reads_input_size_from_model() {
        // 320 输入的 v8n：40²+20²+10² = 2100
        let l = infer_layout(&[1, 84, 2100], 80, 320).unwrap();
        assert_eq!(l.input_size, 320);
        assert_eq!(l.anchors, 2100);
    }

    #[test]
    fn infer_layout_rejects_end_to_end_nms_output() {
        // YOLOv10 端到端导出 [1, 300, 6]：候选数远小于网格
        let e = infer_layout(&[1, 300, 6], 80, 640).unwrap_err();
        assert!(e.contains("NMS"), "{e}");
    }

    #[test]
    fn infer_layout_rejects_multi_output_head() {
        let e = infer_layout(&[], 80, 640).unwrap_err();
        assert!(e.contains("v3"), "{e}");
        // 四维（带额外 batch 维）同样拦掉
        let e2 = infer_layout(&[1, 1, 84, 8400], 80, 640).unwrap_err();
        assert!(e2.contains("三段式"), "{e2}");
    }

    #[test]
    fn infer_layout_flags_dynamic_dims() {
        let e = infer_layout(&[1, 84, -1], 80, 640).unwrap_err();
        assert!(e.contains("动态"), "{e}");
    }

    // ------------------------------------------------------------- decode

    #[test]
    fn decode_v8_transposed_layout() {
        // [1, 4+2, 3]：3 个候选，2 类
        let mut a = ndarray::Array3::<f32>::zeros((1, 6, 3));
        a[[0, 0, 0]] = 10.0;
        a[[0, 1, 0]] = 20.0;
        a[[0, 2, 0]] = 4.0;
        a[[0, 3, 0]] = 6.0;
        a[[0, 5, 0]] = 0.9;
        a[[0, 4, 1]] = 0.5;
        a[[0, 4, 2]] = 0.1;
        let layout = YoloLayout {
            layout: OutputLayout::ChannelsFirst,
            has_objectness: false,
            input_size: 640,
            anchors: 3,
            channels: 6,
        };
        let view = a.view().into_dyn();
        let cands = decode(&view, layout).unwrap();
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
        let layout = YoloLayout {
            layout: OutputLayout::AnchorsFirst,
            has_objectness: true,
            input_size: 640,
            anchors: 1,
            channels: 7,
        };
        let view = a.view().into_dyn();
        let cands = decode(&view, layout).unwrap();
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].class_id, 1);
        // conf = obj * cls = 0.8 * 0.5
        assert!((cands[0].score - 0.4).abs() < 1e-6);
    }

    #[test]
    fn decode_anchors_first_without_objectness() {
        // 少数导出是 [1, N, 4+C]：不要给类别分乘 objectness
        let mut a = ndarray::Array3::<f32>::zeros((1, 1, 6));
        a[[0, 0, 2]] = 4.0;
        a[[0, 0, 3]] = 6.0;
        a[[0, 0, 5]] = 0.7;
        let layout = YoloLayout {
            layout: OutputLayout::AnchorsFirst,
            has_objectness: false,
            input_size: 640,
            anchors: 1,
            channels: 6,
        };
        let cands = decode(&a.view().into_dyn(), layout).unwrap();
        assert!((cands[0].score - 0.7).abs() < 1e-6);
        assert_eq!(cands[0].class_id, 1);
    }

    // ---------------------------------------------------------------- NMS

    fn cand(x1: f32, y1: f32, x2: f32, y2: f32, cls: usize, score: f32) -> Candidate {
        Candidate { x1, y1, x2, y2, class_id: cls, score }
    }

    #[test]
    fn nms_suppresses_same_class_only() {
        let a = cand(0.0, 0.0, 10.0, 10.0, 0, 0.9);
        // inter = 9.5² = 90.25, union = 200 - 90.25 = 109.75, IoU ≈ 0.822 -> 被抑制
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
