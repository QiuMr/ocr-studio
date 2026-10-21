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
    /// 找字关键词：普通子串，或 `/正则/` 形式
    #[serde(default)]
    pub keyword: Option<String>,
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

/// `/ocr/health` 出参。
#[derive(Debug, Serialize)]
pub struct HealthDto {
    pub status: String,
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
            ApiError::Internal(_) => 500,
            ApiError::NotImplemented(_) => CODE_NOT_IMPLEMENTED,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ApiError::BadRequest(m)
            | ApiError::EngineNotReady(m)
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
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
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
