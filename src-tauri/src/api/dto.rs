//! HTTP 接口的请求/响应 DTO 与错误类型。
//!
//! 设计约定（见 `docs/PLAN.md` 第 6 节）：
//! - 全部接口返回统一信封 `{code, msg, data?}`，`code == 0` 表示成功。
//! - 图像入参支持两种形态：`application/json` 的 base64 字段，或
//!   `multipart/form-data` 的文件字段。默认 JSON base64（脚本党习惯）。

use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// 成功码。
pub const CODE_OK: i32 = 0;
/// 请求体/参数错误。
pub const CODE_BAD_REQUEST: i32 = 400;
/// 引擎未加载（模型缺失或加载失败）。
pub const CODE_ENGINE_NOT_READY: i32 = 503;
/// 已规划但尚未实现的功能。
pub const CODE_NOT_IMPLEMENTED: i32 = 501;
/// 路径不存在（打错接口名）。带最近似路径提示。
pub const CODE_NOT_FOUND: i32 = 404;
/// 路径对但方法不对（例如对 POST 接口发了 GET）。
pub const CODE_METHOD_NOT_ALLOWED: i32 = 405;

/// 统一响应信封。
#[derive(Debug, Serialize)]
pub struct ApiResp<T> {
    pub code: i32,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> ApiResp<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: CODE_OK,
            msg: "ok".to_string(),
            data: Some(data),
        }
    }
}

/// 构造错误响应（`data` 为 `None`）。
pub fn err_resp(code: i32, msg: impl Into<String>) -> ApiResp<()> {
    ApiResp {
        code,
        msg: msg.into(),
        data: None,
    }
}

/// 识别请求的公共参数。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OcrOptions {
    /// base64 图像，可带 `data:image/png;base64,` 前缀
    #[serde(default)]
    pub image: String,
    /// 模型档位：tiny / small / medium（缺省用当前配置）
    #[serde(default)]
    pub level: Option<String>,
    /// 设备：directml / cpu（缺省用当前配置）
    #[serde(default)]
    pub device: Option<String>,
    /// 是否启用方向分类（缺省用当前配置）
    #[serde(default)]
    pub use_cls: Option<bool>,
    /// 引擎：`ppocr:small` / `ddddocr:common` / `yolo`。
    ///
    /// 缺省 = 用当前 PP-OCR 配置（行为与历史版本完全一致）。
    /// 写 `ppocr:*` 也归到缺省路径，档位由 `level` 参数控制。
    #[serde(default)]
    pub engine: Option<String>,
    /// 找字关键词：普通子串，或 `/正则/` 形式
    #[serde(default)]
    pub keyword: Option<String>,
    /// 目标检测置信度下限（仅 `/vision/detect`）。缺省 0.25（ultralytics 默认）
    #[serde(default)]
    pub conf: Option<f32>,
    /// NMS IoU 阈值（仅 `/vision/detect`）。缺省 0.7
    #[serde(default)]
    pub iou: Option<f32>,
    /// 只保留这些 class_id（仅 `/vision/detect`）。空/缺省 = 全部
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
    /// `/vision/pipeline` 的检测引擎。缺省 `yolo`
    #[serde(default)]
    pub detect_engine: Option<String>,
    /// 只识别这块区域 `[x, y, w, h]`（原图像素坐标）。**仅 `/ocr/quyuocr` 会读**，
    /// 其它识别接口收到这个字段会忽略。
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 只找这些码制（**仅 `/vision/qrcode` 会读**），如 `["qr","code128"]`。
    /// 支持分组写法 `1d`（全部一维码）/ `2d`（全部二维码）。空/缺省 = 全找。
    #[serde(default)]
    pub barcode_formats: Option<Vec<String>>,
    /// 条码解码器是否多花点时间找（**仅 `/vision/qrcode` 会读**），缺省 true。
    /// 关掉更快，但一维码容易漏。
    #[serde(default)]
    pub try_harder: Option<bool>,
}

/// `/vision/qrcode` 出参：一个识别到的码。
#[derive(Debug, Serialize)]
pub struct BarcodeHitDto {
    /// 码里编的内容（原文，不做任何解析）
    pub text: String,
    /// 码制名，如 `QR_CODE` / `CODE_128`
    pub format: String,
    /// 定位点/角点，原图坐标。QR 一般给 3 个定位图案 + 1 个校正点
    pub points: Vec<[f32; 2]>,
    /// 定位点的轴对齐包围盒 `[x, y, w, h]`
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// `/vision/qrcode` 出参。
#[derive(Debug, Serialize)]
pub struct BarcodeData {
    /// 找到几个码
    pub count: usize,
    pub codes: Vec<BarcodeHitDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 四点框（原图坐标，顺序 左上/右上/右下/左下）。
pub type Box4 = [[f64; 2]; 4];

/// 单行识别结果。
#[derive(Debug, Clone, Serialize)]
pub struct LineDto {
    pub text: String,
    /// 字段名必须是 `box`（与原产品一致），而 `box` 是 Rust 保留字，用原始标识符。
    #[serde(rename = "box")]
    pub r#box: Box4,
    pub score: f64,
    /// 检测框得分
    pub det_score: f64,
    /// 是否被 cls 判定为 180° 旋转并已翻转
    pub rotated: bool,
}

/// `/ocr/shibie` 出参。
#[derive(Debug, Serialize)]
pub struct ShibieData {
    /// 全部文本按行用 `\n` 拼接（与原产品行为一致）
    pub text: String,
    pub boxes: Vec<Box4>,
    pub scores: Vec<f64>,
    pub lines: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/quyuocr` 出参：区域 OCR。
///
/// 与 `ShibieData` 的区别只有两点：多一个 `region`（实际生效的区域，已钳到图内），
/// 且 `width`/`height` 是**原图**尺寸——`boxes` 里的坐标已经平移回原图坐标系，
/// 调用方不用再做任何换算。
#[derive(Debug, Serialize)]
pub struct RegionOcrData {
    /// 实际生效的区域 `[x, y, w, h]`（请求值被钳到图内后的结果）
    pub region: [i32; 4],
    /// 区域内全部文本按行用 `\n` 拼接
    pub text: String,
    /// 命中行的框，坐标已在原图坐标系
    pub boxes: Vec<Box4>,
    pub scores: Vec<f64>,
    pub lines: Vec<LineDto>,
    /// 原图宽（不是区域宽）
    pub width: u32,
    /// 原图高（不是区域高）
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/zhaozi` 出参。
#[derive(Debug, Serialize)]
pub struct ZhaoziData {
    pub keyword: String,
    pub matched: usize,
    pub hits: Vec<LineDto>,
    pub elapsed_ms: u128,
}

/// `/ocr/duohangjson` 出参：多行数组。
#[derive(Debug, Serialize)]
pub struct DuohangData {
    pub rows: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 单字结果。`box` 是该字在原图中的四边形（左上/右上/右下/左下）。
#[derive(Debug, Serialize)]
pub struct CharDto {
    pub ch: String,
    pub r#box: Box4,
    pub score: f64,
    /// 行内序号，0 起
    pub index: usize,
    /// CTC 起止帧，排查对齐问题用
    pub t_start: usize,
    pub t_end: usize,
}

/// 单行结果（含逐字）。
#[derive(Debug, Serialize)]
pub struct DanziLineDto {
    pub text: String,
    pub r#box: Box4,
    pub score: f64,
    pub det_score: f64,
    pub rotated: bool,
    /// rec 总帧数。字框宽度由 `帧区间 / 总帧数` 决定
    pub frames: usize,
    pub chars: Vec<CharDto>,
}

/// `/ocr/danzijson` 出参。
#[derive(Debug, Serialize)]
pub struct DanziData {
    /// 所有行按阅读顺序拼成的纯文本
    pub text: String,
    pub lines: Vec<DanziLineDto>,
    /// 全部字符总数（含所有行），省得调用方自己数
    pub char_count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 找图请求参数（JSON 形态；multipart 形态用同名 form 字段）。
///
/// 模板找图（`tuzhaotu*` / `jubingzhaotu*` / `pingmuzhaotu*`）三种来源由**端点路径**
/// 决定，这里只做字段容器：
/// - `tuzhaotu*` 用 `image`（调用方自己传搜索图）
/// - `jubingzhaotu*` 用 `hwnd` 或 `title`（内部截窗口）
/// - `pingmuzhaotu*` 用 `region`（内部截屏幕，缺省全屏）
///
/// 语义找图（`yuyizhaotu`）只有一个端点，来源由 `source` 字段给出，
/// 阈值用 `conf` 而不是 `min_score`（前者是检测置信度、后者是模板相似度）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FindOptions {
    /// 搜索图 base64（`tuzhaotu*` 必填）
    #[serde(default)]
    pub image: Option<String>,
    /// 模板图 base64（与 `template_file` 二选一）
    #[serde(default)]
    pub template: Option<String>,
    /// 模板文件绝对路径（与 `template` 二选一，支持 png/jpg/bmp）
    #[serde(default)]
    pub template_file: Option<String>,
    /// 模板掩码 base64（可省略）。**白（≥ 阈值）= 参与匹配**，尺寸必须与模板逐像素一致
    #[serde(default)]
    pub mask: Option<String>,
    /// 掩码文件绝对路径（与 `mask` 二选一）
    #[serde(default)]
    pub mask_file: Option<String>,
    /// 掩码二值化阈值，缺省 128（像素 ≥ 阈值视为参与匹配）
    #[serde(default)]
    pub mask_threshold: Option<u8>,
    /// 窗口句柄（`jubingzhaotu*`）；缺省时回退 `title`
    #[serde(default)]
    pub hwnd: Option<i64>,
    /// 窗口标题，精确匹配优先，找不到再按子串匹配
    #[serde(default)]
    pub title: Option<String>,
    /// 搜索区域 `[x, y, w, h]`（屏幕绝对坐标），仅截屏幕时有效
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 分数下限（NCC），缺省按端点预设：精准 0.92 / 容错 0.75
    #[serde(default)]
    pub min_score: Option<f64>,
    /// 最多返回几个命中，默认 1
    #[serde(default)]
    pub max_results: Option<usize>,

    // ---- 以下字段只有语义找图（`/ocr/yuyizhaotu`）会读 ----
    /// 去哪找：`image`（缺省）/ `screen` / `window`
    #[serde(default)]
    pub source: Option<String>,
    /// 用哪个检测模型：`yolo`（缺省，= 目录里第一个）/ `yolo:<模型id>`
    #[serde(default)]
    pub engine: Option<String>,
    /// 检测置信度下限，缺省 0.25。给了 `min_score` 时按它兜底
    #[serde(default)]
    pub conf: Option<f32>,
    /// 只保留这些 class_id；空/缺省 = 全部类别
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
}

/// 单个命中。
///
/// 模板找图与语义找图**共用这一个结构**，脚本切模式不用改解析代码。
#[derive(Debug, Clone, Serialize)]
pub struct FindHitDto {
    /// 左上角（屏幕绝对坐标；`source=image` 为搜索图内坐标）
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub center_x: f64,
    pub center_y: f64,
    /// 模板找图是零均值 NCC（`-1..=1`）；语义找图是检测置信度（`0..=1`）
    pub score: f64,
    /// 语义找图才有：命中的类别 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<usize>,
    /// 语义找图才有：类别名（模型没写类名时回落 `class_{id}`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
}

/// 找图出参。
///
/// 同时提供扁平字段（`x`/`y`/`score`）与数组（`matches`）：
/// 老脚本习惯直接读 `data.x` / `data.y`，新脚本可用 `matches` 处理多目标。
#[derive(Debug, Serialize)]
pub struct FindData {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center_y: Option<f64>,
    /// 模板尺寸（未命中时也返回，便于调用方算点击偏移）。
    /// 语义找图没有模板，这里给**首个命中框的尺寸**，未命中为 0。
    pub width: u32,
    pub height: u32,
    /// 命中数量
    pub matched: usize,
    pub matches: Vec<FindHitDto>,
    /// 搜索图来源：`image` / `screen` / `window`
    pub source: String,
    /// 搜索图左上角在屏幕上的坐标。前端要用它把绝对坐标换算回图内坐标
    /// （`source=image` 时恒为 0）。
    pub origin_x: i32,
    pub origin_y: i32,
    /// 语义找图才有：实际使用的检测引擎（`yolo` / `yolo:<id>`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub elapsed_ms: u128,
}

/// `/vision/detect` 的单条检测结果。
#[derive(Debug, Clone, Serialize)]
pub struct DetectHitDto {
    /// 轴对齐框转四点（左上/右上/右下/左下），前端只写一套画框逻辑
    pub r#box: Box4,
    pub class_id: usize,
    /// labels.txt 缺失时回落 `class_{id}`
    pub class_name: String,
    pub score: f64,
}

/// `/vision/detect` 出参。
#[derive(Debug, Serialize)]
pub struct DetectData {
    /// 实际使用的引擎（缺省请求 = 当前 PP-OCR，检测只会失败，这里回请求值）
    pub engine: String,
    pub detections: Vec<DetectHitDto>,
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/vision/pipeline` 的单区域结果：一个检测目标 + 框内识别出的文字。
#[derive(Debug, Clone, Serialize)]
pub struct PipelineRegionDto {
    pub r#box: Box4,
    pub class_id: usize,
    pub class_name: String,
    /// 检测置信度
    pub score: f64,
    /// 框内各行文本按阅读顺序拼接（行间 `\n`）
    pub text: String,
    /// 框内识别出的行（坐标已平移回原图）
    pub lines: Vec<LineDto>,
}

/// `/vision/pipeline` 出参。
#[derive(Debug, Serialize)]
pub struct PipelineData {
    /// 检测引擎标识
    pub detect_engine: String,
    /// 识别引擎标识
    pub ocr_engine: String,
    pub regions: Vec<PipelineRegionDto>,
    /// 检出且成功识别的区域数（识别失败的区域不在其中）
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/health` 出参。
#[derive(Debug, Serialize)]
pub struct HealthDto {
    pub status: String,
    /// 引擎标识：`ppocr:small` / `ddddocr:common`。前端据此显示「正在用哪个引擎」。
    pub engine: String,
    pub model: String,
    pub device: String,
    /// 实际生效设备（DirectML 不可用时会是 cpu）
    pub actual_device: String,
    pub det_loaded: bool,
    pub rec_loaded: bool,
    pub cls_loaded: bool,
    pub dict_size: usize,
    pub port: u16,
    pub uptime_sec: u64,
}

/// 能力声明。前端据此决定显示哪些面板——**未实现能力的入口直接隐藏**，
/// 不要显示成灰色禁用按钮（小白看到灰按钮只会困惑，不知道为什么点不了）。
#[derive(Debug, Serialize)]
pub struct CapabilitiesDto {
    pub line_boxes: bool,
    pub char_boxes: bool,
    pub rotation_cls: bool,
    pub detect: bool,
    /// 检测类别名。空 = 不做检测，或没给 labels.txt
    pub classes: Vec<String>,
    pub batch: bool,
}

impl From<&crate::engine::Capabilities> for CapabilitiesDto {
    fn from(c: &crate::engine::Capabilities) -> Self {
        Self {
            line_boxes: c.line_boxes,
            char_boxes: c.char_boxes,
            rotation_cls: c.rotation_cls,
            detect: c.detect,
            classes: c.classes.clone(),
            batch: c.batch,
        }
    }
}

/// `/engine/list` 的一条引擎记录。
#[derive(Debug, Serialize)]
pub struct EngineItemDto {
    /// 规范化引擎名，如 `ppocr:small`、`yolo`
    pub name: String,
    /// 引擎族：ppocr / ddddocr / yolo
    pub family: String,
    /// ppocr 的档位或 ddddocr 的版本；YOLO 为 null（版本已含在 name 里）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub capabilities: CapabilitiesDto,
    /// 模型文件齐全、可以加载。**不代表已在内存里**
    pub available: bool,
    /// 实际生效设备。只有当前已加载的那个引擎才有值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_device: Option<String>,
    /// 不可用的原因或补充说明
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<&crate::engine::EngineStatus> for EngineItemDto {
    fn from(s: &crate::engine::EngineStatus) -> Self {
        Self {
            name: s.spec.key(),
            family: s.spec.kind.family().to_string(),
            variant: s.spec.variant.clone(),
            capabilities: CapabilitiesDto::from(&s.capabilities),
            available: s.available,
            actual_device: s
                .actual_device
                .map(|d| format!("{d:?}").to_lowercase()),
            note: s.note.clone(),
        }
    }
}

/// `/engine/list` 出参。
#[derive(Debug, Serialize)]
pub struct EngineListData {
    pub models_root: String,
    /// 当前配置的默认引擎
    pub default_engine: String,
    pub engines: Vec<EngineItemDto>,
}

/// 解析 multipart 表单得到的字段集合。
pub type FormFields = HashMap<String, String>;

/// 从 base64 字符串解码图像字节。
///
/// 容忍三种常见形态：纯 base64、带 `data:...;base64,` 前缀、含换行/空白的 base64。
pub fn decode_base64_image(s: &str) -> Result<Vec<u8>, String> {
    let b64 = match s.find(',') {
        // data URL 前缀只可能出现在开头，取最后一个逗号之后的内容
        Some(i) if s[..i].contains("base64") => &s[i + 1..],
        _ => s,
    };
    use base64::Engine;
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// 用 `image` crate 解码内存中的图像字节。
pub fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    image::load_from_memory(bytes).map_err(|e| format!("图像解码失败: {e}"))
}

/// 接口错误。统一转成 `{code, msg}` JSON，并带上合适的 HTTP 状态码。
#[derive(Debug)]
pub enum ApiError {
    /// 入参问题（缺字段、base64 坏、图像坏、正则坏）
    BadRequest(String),
    /// 引擎未加载：模型文件缺失或加载失败
    EngineNotReady(String),
    /// 引擎不支持该能力（code 1002，见 docs/ENGINES.md 4.3）。
    /// 调用方本应先看 `/engine/list` 的 capabilities，走到这里说明漏判了。
    Unsupported(String),
    /// 未知引擎名（code 1003）
    UnknownEngine(String),
    /// 内部错误（推理失败等）
    Internal(String),
    /// 已规划但尚未实现
    NotImplemented(String),
}

impl ApiError {
    pub fn code(&self) -> i32 {
        match self {
            ApiError::BadRequest(_) => CODE_BAD_REQUEST,
            ApiError::EngineNotReady(_) => CODE_ENGINE_NOT_READY,
            ApiError::Unsupported(_) => 1002,
            ApiError::UnknownEngine(_) => 1003,
            ApiError::Internal(_) => 500,
            ApiError::NotImplemented(_) => CODE_NOT_IMPLEMENTED,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ApiError::BadRequest(m)
            | ApiError::EngineNotReady(m)
            | ApiError::Unsupported(m)
            | ApiError::UnknownEngine(m)
            | ApiError::Internal(m)
            | ApiError::NotImplemented(m) => m.clone(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ApiError {}

impl From<ApiError> for Response {
    fn from(e: ApiError) -> Self {
        use axum::http::StatusCode;
        let status = match &e {
            ApiError::BadRequest(_)
            | ApiError::Unsupported(_)
            | ApiError::UnknownEngine(_) => StatusCode::BAD_REQUEST,
            ApiError::EngineNotReady(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
        };
        (status, axum::Json(err_resp(e.code(), e.message()))).into_response()
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.into()
    }
}
