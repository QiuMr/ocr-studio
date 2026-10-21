//! 统一视觉引擎抽象层。
//!
//! 设计契约见 `docs/ENGINES.md`。本模块只做三件事：
//!
//! 1. 定义**引擎身份**（[`EngineKind`] / [`EngineSpec`]）——决定模型目录与加载方式；
//! 2. 定义**能力声明**（[`Capabilities`]）——调用方据此决定能调哪些接口，
//!    而不是靠「先调再捕获错误」来探测；
//! 3. 定义**统一入口**（[`VisionEngine`]）——PP-OCR / DDDDOCR / YOLO 各自实现它。
//!
//! 为什么现在（只有一种引擎时）就抽象：加第二、第三种引擎时，`if kind == ...` 分支
//! 会散落到路由、命令层、前端三处，而三档 PP-OCR 的精度对齐测试（`cargo test -- --ignored`）
//! 会因此无法定位「改 A 坏了 B」。先切层，后续每接一种引擎都只是新增一个 impl。
//!
//! 注意命名的两个坑（本模块刻意避开）：
//! - 不提供 `recognize` 方法名，用 [`VisionEngine::infer`]：`OcrEngine` 已有一个历史
//!   单参 `recognize`，同名会让「具体类型 vs `dyn VisionEngine`」解析结果不一致，排查成本极高。
//! - 不提供 `health` 方法名，用 [`VisionEngine::status`]：同理，且语义上 status 更宽
//!   （带能力声明与可用性），health 特指模型进程状态。

pub mod catalog;

use std::fmt;

use image::DynamicImage;

use crate::config::{Device, ModelLevel};
use crate::ocr::{CharBox, LineDetail, OcrEngine, Pt};

// ---------------------------------------------------------------------------
// 引擎身份
// ---------------------------------------------------------------------------

/// YOLO 版本。
///
/// 版本必须显式写进模型目录名，**不能靠猜**：v5/v7 与 v8/v11 的输出布局、letterbox
/// 填充值、有无 objectness 通道全都不同，猜错会让置信度整体偏移且框位错乱。
/// 下面两个方法把这三处差异编码在一处，避免后处理代码里再散落 `if version`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YoloVersion {
    V5,
    V7,
    V8,
    V11,
}

impl YoloVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            YoloVersion::V5 => "v5",
            YoloVersion::V7 => "v7",
            YoloVersion::V8 => "v8",
            YoloVersion::V11 => "v11",
        }
    }

    /// 接受 `v8` / `yolov8` / `yolo11` 等写法。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "v5" | "yolov5" => Some(YoloVersion::V5),
            "v7" | "yolov7" => Some(YoloVersion::V7),
            "v8" | "yolov8" => Some(YoloVersion::V8),
            "v11" | "yolov11" | "yolo11" => Some(YoloVersion::V11),
            _ => None,
        }
    }

    /// 输出是否带 objectness 通道。
    ///
    /// v5/v7 输出 `[1, N, 5+C]`（cx,cy,w,h,obj,cls...）；v8/v11 输出 `[1, 4+C, N]`
    /// （无 obj，且需要转置）。两者后处理不通用。
    pub fn has_objectness(self) -> bool {
        matches!(self, YoloVersion::V5 | YoloVersion::V7)
    }

    /// letterbox 填充值：v5 用 0（纯黑），v8+ 用 114（灰）。
    /// 填错会让置信度整体偏移 0.05 以上，肉眼看不出来但阈值一改就翻。
    pub fn pad_value(self) -> u8 {
        if self.has_objectness() {
            0
        } else {
            114
        }
    }
}

/// 引擎族。`variant`（档位/版本）不在这里，见 [`EngineSpec`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    PpOcr,
    DdddOcr,
    Yolo(YoloVersion),
}

impl EngineKind {
    pub fn family(self) -> &'static str {
        match self {
            EngineKind::PpOcr => "ppocr",
            EngineKind::DdddOcr => "ddddocr",
            EngineKind::Yolo(_) => "yolo",
        }
    }
}

/// 引擎完整标识：`<family>[:<variant>]`。
///
/// 例：`ppocr:small`、`ddddocr:common`、`yolo:v8`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineSpec {
    pub kind: EngineKind,
    /// ppocr 的档位（tiny/small/medium）或 ddddocr 的版本（common/old）。
    /// YOLO 的版本已编码进 `kind`，故恒为 `None`。
    pub variant: Option<String>,
}

impl EngineSpec {
    pub fn ppocr(level: ModelLevel) -> Self {
        Self {
            kind: EngineKind::PpOcr,
            variant: Some(level.dir_name().to_string()),
        }
    }

    pub fn ddddocr(variant: &str) -> Self {
        Self {
            kind: EngineKind::DdddOcr,
            variant: Some(variant.to_string()),
        }
    }

    pub fn yolo(version: YoloVersion) -> Self {
        Self {
            kind: EngineKind::Yolo(version),
            variant: None,
        }
    }

    /// 解析 `"ppocr:small"` / `"ddddocr"` / `"yolo:v8"`。
    ///
    /// 缺省 variant 时取各族默认值（ppocr=small、ddddocr=common）。
    /// 但 `"yolo"` 不带版本直接报错——不能猜，见 [`YoloVersion`] 的说明。
    pub fn parse(s: &str) -> Result<Self, VisionError> {
        let raw = s.trim();
        if raw.is_empty() {
            return Err(VisionError::BadRequest("引擎名不能为空".to_string()));
        }
        let (family, variant) = match raw.split_once(':') {
            Some((f, v)) => (f.trim().to_ascii_lowercase(), Some(v.trim())),
            None => (raw.to_ascii_lowercase(), None),
        };
        let variant = variant.filter(|v| !v.is_empty());

        match family.as_str() {
            "ppocr" | "pp-ocr" | "paddleocr" => {
                let level = match variant {
                    None => ModelLevel::default(),
                    Some(v) => ModelLevel::from_str(v).ok_or_else(|| {
                        VisionError::UnknownEngine(format!(
                            "未知的 ppocr 档位 `{v}`（可选 tiny / small / medium）"
                        ))
                    })?,
                };
                Ok(Self::ppocr(level))
            }
            "ddddocr" | "dddd" => {
                let v = variant.unwrap_or("common");
                // 目前只规划 common / old 两版，但仍允许任意目录名，
                // 由 catalog 判定是否真的存在，避免这里成为白名单瓶颈。
                Ok(Self::ddddocr(v))
            }
            "yolo" => {
                let v = variant.ok_or_else(|| {
                    VisionError::UnknownEngine(
                        "yolo 必须带版本，例如 `yolo:v8`（可选 v5 / v7 / v8 / v11）".to_string(),
                    )
                })?;
                let ver = YoloVersion::parse(v).ok_or_else(|| {
                    VisionError::UnknownEngine(format!(
                        "未知的 yolo 版本 `{v}`（可选 v5 / v7 / v8 / v11）"
                    ))
                })?;
                Ok(Self::yolo(ver))
            }
            other => Err(VisionError::UnknownEngine(format!(
                "未知引擎 `{other}`（可选 ppocr / ddddocr / yolo）"
            ))),
        }
    }

    /// 规范化字符串，兼作缓存键。`yolo` 的版本在 kind 里，故形如 `yolo:v8`。
    pub fn key(&self) -> String {
        match (&self.kind, &self.variant) {
            (EngineKind::Yolo(v), _) => format!("yolo:{}", v.as_str()),
            (k, Some(v)) => format!("{}:{}", k.family(), v),
            (k, None) => k.family().to_string(),
        }
    }
}

impl fmt::Display for EngineSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

// ---------------------------------------------------------------------------
// 能力与结果
// ---------------------------------------------------------------------------

/// 能力声明。调用方（前端 / 路由）先看这里，再决定调什么。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// 能返回整行文本框 + 文本
    pub line_boxes: bool,
    /// 能返回单字框
    pub char_boxes: bool,
    /// 带 180° 方向分类
    pub rotation_cls: bool,
    /// 能做通用目标检测
    pub detect: bool,
    /// 检测的类别名。空 = 未提供 labels 或不做检测
    pub classes: Vec<String>,
    /// 支持批量推理
    pub batch: bool,
}

/// 统一识别选项。
#[derive(Debug, Clone, Default)]
pub struct RecognizeOptions {
    /// 是否计算逐字框。
    ///
    /// 对 PP-OCR 是**免费**的（CTC 帧区间直接给出，不额外推理）；
    /// 对 DDDDOCR 只能按字符数等分行宽**估计**，语义与 PP-OCR 不同，
    /// 前端需要注明，不能当作精确字框用。
    pub want_chars: bool,
}

/// 统一识别结果：一行文本。
#[derive(Debug, Clone)]
pub struct TextLine {
    /// 四点，原图坐标，顺序 `[左上, 右上, 右下, 左下]`
    pub points: [Pt; 4],
    pub text: String,
    /// 识别置信度
    pub score: f64,
    /// 检测框得分
    pub det_score: f64,
    /// 是否被方向分类判定为 180° 并已翻转
    pub rotated: bool,
    /// 逐字框；`RecognizeOptions::want_chars = false` 时为空
    pub chars: Vec<CharBox>,
}

impl From<LineDetail> for TextLine {
    fn from(l: LineDetail) -> Self {
        Self {
            points: l.points,
            text: l.text,
            score: l.score,
            det_score: l.det_score,
            rotated: l.rotated,
            chars: l.chars,
        }
    }
}

/// 统一检测结果：一个目标。
#[derive(Debug, Clone)]
pub struct Detection {
    /// 四点框，原图坐标，顺序 `[左上, 右上, 右下, 左下]`。
    /// YOLO 的轴对齐框转四点是为了让前端只写一套画框逻辑。
    pub points: [Pt; 4],
    pub class_id: usize,
    pub class_name: String,
    pub score: f64,
}

/// 检测选项。
#[derive(Debug, Clone)]
pub struct DetectOptions {
    /// 置信度下限
    pub conf_thresh: f32,
    /// NMS 的 IoU 阈值
    pub iou_thresh: f32,
    /// 只保留这些类别（空 = 全部）
    pub class_filter: Vec<usize>,
}

impl Default for DetectOptions {
    fn default() -> Self {
        // 与 ultralytics 默认值对齐，便于逐框比对
        Self {
            conf_thresh: 0.25,
            iou_thresh: 0.7,
            class_filter: Vec::new(),
        }
    }
}

/// 引擎运行状态（`/engine/list` 与前端设置页用）。
#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub spec: EngineSpec,
    pub capabilities: Capabilities,
    /// 模型文件齐全、可以加载。**不代表已在内存里**——内存中的实例由
    /// `AppState` / 注册表持有，见 `docs/ENGINES.md` 4.2 的 LRU 说明。
    pub available: bool,
    /// 实际生效设备（未加载时为 `None`，因为降级结果要推理预热才知道）
    pub actual_device: Option<Device>,
    /// 不可用的原因，或可用时的补充说明（如「缺少 cls.onnx，方向分类已禁用」）
    pub note: Option<String>,
}

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum VisionError {
    /// 参数非法（引擎名解析失败等）
    BadRequest(String),
    /// 引擎不支持该能力。**调用方本应通过 `capabilities()` 提前判断**，
    /// 走到这里说明前端/路由漏判了。
    Unsupported { engine: String, capability: &'static str },
    /// 未知引擎名
    UnknownEngine(String),
    /// 模型缺失或加载失败
    NotReady(String),
    /// 推理失败
    Inference(String),
}

impl fmt::Display for VisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VisionError::BadRequest(m) => write!(f, "{m}"),
            VisionError::Unsupported { engine, capability } => {
                write!(f, "引擎 {engine} 不支持 {capability}")
            }
            VisionError::UnknownEngine(m) => write!(f, "{m}"),
            VisionError::NotReady(m) => write!(f, "引擎未就绪: {m}"),
            VisionError::Inference(m) => write!(f, "推理失败: {m}"),
        }
    }
}

impl std::error::Error for VisionError {}

// ---------------------------------------------------------------------------
// 统一入口
// ---------------------------------------------------------------------------

/// 所有视觉引擎的统一入口。
///
/// 只有 `Send` 约束：实例总被 `Mutex` 包着用（推理是阻塞式的，要 `spawn_blocking`），
/// 不需要 `Sync`。
pub trait VisionEngine: Send {
    /// 引擎标识（含档位/版本）。
    fn spec(&self) -> EngineSpec;

    /// 能力声明。
    fn capabilities(&self) -> Capabilities;

    /// 运行状态。
    fn status(&self) -> EngineStatus;

    /// 识别：整行文本 + 可选逐字框。
    ///
    /// 名字用 `infer` 而非 `recognize`，理由见模块文档。
    fn infer(
        &mut self,
        img: &DynamicImage,
        opts: &RecognizeOptions,
    ) -> Result<Vec<TextLine>, VisionError>;

    /// 通用目标检测。默认不支持——OCR 类引擎不实现即可，
    /// 不要去 `impl` 里写个 `Err`，否则能力声明和实现容易长期不一致。
    fn detect(
        &mut self,
        _img: &DynamicImage,
        _opts: &DetectOptions,
    ) -> Result<Vec<Detection>, VisionError> {
        Err(VisionError::Unsupported {
            engine: self.spec().key(),
            capability: "detect",
        })
    }
}

// ---------------------------------------------------------------------------
// PP-OCR 的实现（M3.0：只是把现有引擎包一层，行为零变化）
// ---------------------------------------------------------------------------

impl VisionEngine for OcrEngine {
    fn spec(&self) -> EngineSpec {
        EngineSpec::ppocr(self.config().level)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            line_boxes: true,
            // PP-OCR 的逐字框来自 CTC 帧区间，与整行识别同一次推理，无额外开销
            char_boxes: true,
            rotation_cls: self.health().cls_loaded,
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
            actual_device: Some(self.actual_device()),
            note: None,
        }
    }

    fn infer(
        &mut self,
        img: &DynamicImage,
        opts: &RecognizeOptions,
    ) -> Result<Vec<TextLine>, VisionError> {
        let lines = self
            .recognize_detailed(img)
            .map_err(|e| VisionError::Inference(e.to_string()))?;
        Ok(lines
            .into_iter()
            .map(|mut l| {
                if !opts.want_chars {
                    l.chars.clear();
                }
                TextLine::from(l)
            })
            .collect())
    }
}

/// 编译期确认引擎能进 `Mutex` 并被 `spawn_blocking` 搬到阻塞线程。
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<OcrEngine>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_engine_spec_variants() {
        assert_eq!(
            EngineSpec::parse("ppocr:small").unwrap(),
            EngineSpec::ppocr(ModelLevel::Small)
        );
        // 缺省档位取默认（small）
        assert_eq!(
            EngineSpec::parse("ppocr").unwrap(),
            EngineSpec::ppocr(ModelLevel::default())
        );
        assert_eq!(
            EngineSpec::parse("DDDDOCR").unwrap(),
            EngineSpec::ddddocr("common")
        );
        assert_eq!(
            EngineSpec::parse("yolo:v8").unwrap(),
            EngineSpec::yolo(YoloVersion::V8)
        );
        assert_eq!(
            EngineSpec::parse("yolo:yolo11").unwrap(),
            EngineSpec::yolo(YoloVersion::V11)
        );
    }

    #[test]
    fn parse_engine_spec_rejects_ambiguous() {
        // 不带版本的 yolo 必须报错，不能猜
        assert!(matches!(
            EngineSpec::parse("yolo"),
            Err(VisionError::UnknownEngine(_))
        ));
        assert!(matches!(
            EngineSpec::parse("yolo:v9"),
            Err(VisionError::UnknownEngine(_))
        ));
        assert!(matches!(
            EngineSpec::parse("ppocr:huge"),
            Err(VisionError::UnknownEngine(_))
        ));
        assert!(matches!(
            EngineSpec::parse("  "),
            Err(VisionError::BadRequest(_))
        ));
    }

    #[test]
    fn engine_spec_key_roundtrip() {
        for s in ["ppocr:small", "ddddocr:common", "yolo:v8", "yolo:v5"] {
            let spec = EngineSpec::parse(s).unwrap();
            assert_eq!(spec.key(), s);
            // key 能被再解析回同一个 spec
            assert_eq!(EngineSpec::parse(&spec.key()).unwrap(), spec);
        }
    }

    #[test]
    fn yolo_version_encodes_the_known_pitfalls() {
        // v5/v7 有 objectness、letterbox 用 0；v8/v11 反之
        assert!(YoloVersion::V5.has_objectness());
        assert!(YoloVersion::V7.has_objectness());
        assert!(!YoloVersion::V8.has_objectness());
        assert!(!YoloVersion::V11.has_objectness());
        assert_eq!(YoloVersion::V5.pad_value(), 0);
        assert_eq!(YoloVersion::V8.pad_value(), 114);
    }
}
