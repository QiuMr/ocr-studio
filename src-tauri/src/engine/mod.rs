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
pub mod ddddocr;
pub mod prompt;
pub mod prompt_refs;
pub mod table;
pub mod yolo;

use std::fmt;
use std::path::Path;

use image::DynamicImage;

use crate::config::{Device, ModelLevel};
use crate::ocr::{CharBox, EngineConfig, LineDetail, OcrEngine, Pt};

// ---------------------------------------------------------------------------
// 引擎身份
// ---------------------------------------------------------------------------

/// 引擎族。`variant`（档位/版本）不在这里，见 [`EngineSpec`]。
///
/// **YOLO 不按版本分家**（2026-10-22 用户要求）：目标检测只作为**一类**引擎，
/// 版本差异（输出布局、有无 objectness、输入边长）在加载时从 ONNX 的
/// 输入输出形状**自动探测**，见 [`yolo::YoloLayout`]。
///
/// 为什么不让用户选版本：他手上只有一个 `model.onnx`，版本号既不是他填的，
/// 也决定不了怎么后处理——真正决定后处理的是那个文件的输出形状。而且白名单
/// 必然落后（v10 / v12 / v26 已经在用了，v3 的多 head 又是另一套结构）。
/// 按形状探测能覆盖「结构差不多」的所有版本，且模型换了不用改配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineKind {
    PpOcr,
    DdddOcr,
    Yolo,
}

impl EngineKind {
    pub fn family(self) -> &'static str {
        match self {
            EngineKind::PpOcr => "ppocr",
            EngineKind::DdddOcr => "ddddocr",
            EngineKind::Yolo => "yolo",
        }
    }
}

/// 引擎完整标识：`<family>[:<variant>]`。
///
/// 例：`ppocr:small`、`ddddocr:common`、`yolo`、`yolo:车牌`。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineSpec {
    pub kind: EngineKind,
    /// ppocr 的档位（tiny/small/medium）、ddddocr 的版本（common/old），
    /// 或**目标检测的模型 id**（模型文件名，见 [`catalog::list_yolo_models`]）。
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

    /// 默认目标检测模型（目录扫描结果里的第一个；出厂目录为空，所以由用户决定）。
    pub fn yolo() -> Self {
        Self {
            kind: EngineKind::Yolo,
            variant: None,
        }
    }

    /// 指定某一个检测模型（`variant` 是模型 id = 文件名，不含 `.onnx`）。
    pub fn yolo_model(id: &str) -> Self {
        Self {
            kind: EngineKind::Yolo,
            variant: Some(id.to_string()),
        }
    }

    /// 解析 `"ppocr:small"` / `"ddddocr"` / `"yolo"` / `"yolo:车牌"`。
    ///
    /// 缺省 variant 时取各族默认值（ppocr=small、ddddocr=common、yolo=默认模型）。
    /// `"detect"` 是 `"yolo"` 的别名；`"yolo:v8"` 这类**旧写法继续接受**，
    /// 版本部分被忽略、落回默认模型（只校验形状像不像版本号，避免把拼写错误咽下去）。
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
            "yolo" | "detect" => match variant {
                // 旧的 `yolo:v8`：版本号已被淘汰，落回默认模型
                Some(v) if looks_like_yolo_version(v) => Ok(Self::yolo()),
                // 其余当模型 id：`yolo:车牌` 选那个模型，找不到由目录扫描报错
                Some(v) => Ok(Self::yolo_model(v)),
                None => Ok(Self::yolo()),
            },
            other => Err(VisionError::UnknownEngine(format!(
                "未知引擎 `{other}`（可选 ppocr / ddddocr / yolo）"
            ))),
        }
    }

    /// 规范化字符串，兼作缓存键。
    pub fn key(&self) -> String {
        match (&self.kind, &self.variant) {
            (k, Some(v)) => format!("{}:{}", k.family(), v),
            (k, None) => k.family().to_string(),
        }
    }
}

/// `v8` / `yolov8` / `v26` 这类写法算版本号；`abc` / `latest` 不算。
///
/// 只用于**校验旧写法的拼写**——版本值本身被丢弃。放行明显笔误会让
/// 「写错了也能跑」变成习惯，最后没人知道 `yolo:v10` 和 `yolo:v100` 有区别。
fn looks_like_yolo_version(s: &str) -> bool {
    let s = s.trim();
    let digits = s
        .strip_prefix("yolov")
        .or_else(|| s.strip_prefix("yolo"))
        .or_else(|| s.strip_prefix('v'))
        .unwrap_or("");
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
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
    /// rec 输出的总帧数。逐字框宽度 = 帧区间比例 × 行宽，留出来便于排查
    pub frames: usize,
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
            frames: l.frames,
            chars: l.chars,
        }
    }
}

impl From<TextLine> for LineDetail {
    fn from(l: TextLine) -> Self {
        Self {
            points: l.points,
            text: l.text,
            score: l.score,
            det_score: l.det_score,
            rotated: l.rotated,
            frames: l.frames,
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
    assert_send::<ddddocr::DdddOcrEngine>();
    assert_send::<yolo::YoloEngine>();
};

/// 按 [`EngineSpec`] 加载引擎。
///
/// - `ppocr:*` 走既有的 [`OcrEngine::load`]，**一个字节都没改**，精度基线不受影响；
/// - `ddddocr:*` 走 [`ddddocr::DdddOcrEngine`]，行检测借 `default_level` 档的 PP-OCR det；
/// - `yolo[:<模型 id>]` 走 [`yolo::YoloEngine`]，检测能力经 `/vision/detect` 暴露。
///   模型结构（输出布局 / objectness / 输入边长）与类名全部在加载时从 ONNX 里读，
///   不需要版本号、也不需要手写 labels.txt。
///
/// `yolo_external` 是配置里「添加模型」记下的外部 onnx 路径——模型可以由用户指向任意
/// 位置，`<模型 id>` 只在「目录扫描结果 + 这些外部路径」合并后的清单里查找。
///
/// 返回 `Box<dyn VisionEngine>` 而不是枚举：新增引擎时只需加一个分支，
/// 调用方（状态层 / 路由）不用改 match。
pub fn load_engine(
    spec: &EngineSpec,
    models_root: &Path,
    default_level: ModelLevel,
    device: Device,
    use_cls: bool,
    yolo_external: &[String],
) -> Result<Box<dyn VisionEngine>, VisionError> {
    match &spec.kind {
        EngineKind::PpOcr => {
            // 档位缺省取调用方给的默认档（而不是硬编码 small）
            let level = spec
                .variant
                .as_deref()
                .and_then(ModelLevel::from_str)
                .unwrap_or(default_level);
            let eng = OcrEngine::load(EngineConfig {
                models_root: models_root.to_path_buf(),
                level,
                device,
                use_cls,
                ..Default::default()
            })
            .map_err(|e| VisionError::NotReady(format!("PP-OCR 模型加载失败: {e}")))?;
            Ok(Box::new(eng))
        }
        EngineKind::DdddOcr => {
            let variant = spec.variant.as_deref().unwrap_or("common");
            Ok(Box::new(ddddocr::engine_from_root(
                models_root,
                variant,
                default_level,
                device,
            )?))
        }
        EngineKind::Yolo => Ok(Box::new(yolo::engine_from_root(
            models_root,
            yolo_external,
            spec.variant.as_deref(),
            device,
        )?)),
    }
}

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
        assert_eq!(EngineSpec::parse("yolo").unwrap(), EngineSpec::yolo());
        assert_eq!(EngineSpec::parse("detect").unwrap(), EngineSpec::yolo());
    }

    #[test]
    fn yolo_model_id_selects_that_model() {
        // `yolo:<id>` 选具体模型（用户自己的模型名，任意字符串）。
        // 注意 key 归一成 `yolo:*`——`detect` 只是 family 的别名。
        for (raw, id) in [
            ("yolo:车牌", "车牌"),
            ("yolo:yolov8n", "yolov8n"),
            ("detect:my_model", "my_model"),
        ] {
            let spec = EngineSpec::parse(raw).unwrap_or_else(|e| panic!("{raw} 应可解析: {e}"));
            assert_eq!(spec, EngineSpec::yolo_model(id), "{raw}");
            assert_eq!(spec.key(), format!("yolo:{id}"), "{raw}");
        }
    }

    #[test]
    fn yolo_accepts_legacy_versioned_names_but_ignores_version() {
        // 旧调用方写 yolo:v8 不能突然报错；版本被丢弃，key 归一成 yolo
        for s in ["yolo:v8", "yolo:yolov5", "detect:v11", "yolo:v26", "yolo:v3"] {
            let spec = EngineSpec::parse(s).unwrap_or_else(|e| panic!("{s} 应可解析: {e}"));
            assert_eq!(spec, EngineSpec::yolo(), "{s}");
            assert_eq!(spec.key(), "yolo", "{s}");
        }
    }

    #[test]
    fn parse_engine_spec_rejects_ambiguous() {
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
        for s in ["ppocr:small", "ddddocr:common", "yolo", "yolo:车牌"] {
            let spec = EngineSpec::parse(s).unwrap();
            assert_eq!(spec.key(), s);
            // key 能被再解析回同一个 spec
            assert_eq!(EngineSpec::parse(&spec.key()).unwrap(), spec);
        }
    }
}
