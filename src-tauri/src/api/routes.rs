//! HTTP 路由与处理器。路由表 1:1 对应 `docs/PLAN.md` 第 6 节。
//!
//! 全部端点均已实现：
//! - 识别类：`/ocr/shibie`、`/ocr/zhaozi`、`/ocr/duohangjson`
//! - 找图类：`/ocr/tuzhaotu1|2`（传图）、`/ocr/jubingzhaotu1|2`（截窗口）、
//!   `/ocr/pingmuzhaotu1|2`（截屏幕）
//! - 状态：`/ocr/health`
//!
//! 找图三组的后缀语义：`1` = 精准（NCC ≥ 0.92），`2` = 容错（NCC ≥ 0.72）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use image::{DynamicImage, RgbImage};

use super::dto::{
    decode_base64_image, decode_image, err_resp, ApiError, ApiResp, Box4, CharDto, DanziData,
    DanziLineDto, DuohangData, FindData, FindHitDto, FindOptions, HealthDto, LineDto, OcrOptions,
    ShibieData, ZhaoziData, CODE_ENGINE_NOT_READY,
};
use super::state::AppState;
use crate::capture::{self, ScreenRegion};
use crate::matcher::{self, MatchOptions};
use crate::ocr::TextBox;

/// 请求体上限 32MB：截图级 PNG  compressed 一般几百 KB，留足余量。
const MAX_BODY: usize = 32 * 1024 * 1024;

/// `template_file` 允许的扩展名。限制扩展名是为了让这条「读本地路径」的通道
/// 只能拿图片，不能被当作任意文件读取器使用。
const ALLOWED_TPL_EXT: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp"];

/// 找图端点预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindMode {
    /// 精准：要求近乎逐像素一致（NCC ≥ 0.92）
    Exact,
    /// 容错：允许亮度/对比度漂移与轻微压缩噪声（NCC ≥ 0.72）
    Loose,
}

impl FindMode {
    pub(crate) fn default_min_score(self) -> f64 {
        match self {
            FindMode::Exact => 0.92,
            FindMode::Loose => 0.72,
        }
    }
}

/// 搜索图来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindSource {
    /// 调用方自己上传的图（`tuzhaotu*`）
    Image,
    /// 内部截屏幕（`pingmuzhaotu*`）
    Screen,
    /// 内部截指定窗口（`jubingzhaotu*`）
    Window,
}

impl FindSource {
    pub(crate) fn name(self) -> &'static str {
        match self {
            FindSource::Image => "image",
            FindSource::Screen => "screen",
            FindSource::Window => "window",
        }
    }

    /// 供 Tauri 命令层把前端字符串映射过来。
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "image" | "tu" => Some(FindSource::Image),
            "screen" | "pingmu" => Some(FindSource::Screen),
            "window" | "jubing" => Some(FindSource::Window),
            _ => None,
        }
    }
}

/// 构建路由。调用方需 `with_state` 注入 `Arc<AppState>`。
pub fn build_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ocr/shibie", post(shibie))
        .route("/ocr/zhaozi", post(zhaozi))
        .route("/ocr/duohangjson", post(duohangjson))
        .route("/ocr/danzijson", post(danzijson))
        .route("/ocr/tuzhaotu1", post(tuzhaotu1))
        .route("/ocr/tuzhaotu2", post(tuzhaotu2))
        .route("/ocr/jubingzhaotu1", post(jubingzhaotu1))
        .route("/ocr/jubingzhaotu2", post(jubingzhaotu2))
        .route("/ocr/pingmuzhaotu1", post(pingmuzhaotu1))
        .route("/ocr/pingmuzhaotu2", post(pingmuzhaotu2))
        .route("/ocr/health", get(health))
}

// ---------------------------------------------------------------- 入参解析

/// 从请求里取出图像与参数。支持 JSON base64 与 multipart 两种形态。
async fn parse_recognize(req: Request) -> Result<(DynamicImage, OcrOptions), ApiError> {
    let is_multipart = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start().starts_with("multipart/form-data"))
        .unwrap_or(false);

    if is_multipart {
        parse_multipart(req).await
    } else {
        parse_json(req).await
    }
}

async fn parse_json(req: Request) -> Result<(DynamicImage, OcrOptions), ApiError> {
    let bytes = to_bytes(req.into_body(), MAX_BODY)
        .await
        .map_err(|e| ApiError::BadRequest(format!("读取请求体失败: {e}")))?;
    let opts: OcrOptions = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::BadRequest(format!("JSON 解析失败: {e}")))?;
    if opts.image.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "缺少 image 字段（base64 字符串）".to_string(),
        ));
    }
    let raw = decode_base64_image(&opts.image).map_err(ApiError::BadRequest)?;
    let img = decode_image(&raw).map_err(ApiError::BadRequest)?;
    Ok((img, opts))
}

async fn parse_multipart(req: Request) -> Result<(DynamicImage, OcrOptions), ApiError> {
    let mut mp = Multipart::from_request(req, &())
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart 解析失败: {e}")))?;

    let mut image_bytes: Option<Vec<u8>> = None;
    let mut fields: HashMap<String, String> = HashMap::new();

    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart 字段读取失败: {e}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        if name == "image" || name == "file" {
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("multipart 数据读取失败: {e}")))?;
            image_bytes = Some(data.to_vec());
        } else {
            let txt = field
                .text()
                .await
                .map_err(|e| ApiError::BadRequest(format!("multipart 字段 {name} 读取失败: {e}")))?;
            fields.insert(name, txt);
        }
    }

    let bytes = image_bytes
        .ok_or_else(|| ApiError::BadRequest("multipart 中缺少 image（或 file）文件字段".to_string()))?;
    let img = decode_image(&bytes).map_err(ApiError::BadRequest)?;

    let opts = OcrOptions {
        image: String::new(),
        level: fields.get("level").cloned(),
        device: fields.get("device").cloned(),
        use_cls: fields.get("use_cls").and_then(|s| s.parse().ok()),
        keyword: fields.get("keyword").cloned(),
    };
    Ok((img, opts))
}

// ---------------------------------------------------------------- 处理器

fn to_dto(b: &TextBox) -> LineDto {
    LineDto {
        text: b.text.clone(),
        r#box: [
            [b.points[0].0, b.points[0].1],
            [b.points[1].0, b.points[1].1],
            [b.points[2].0, b.points[2].1],
            [b.points[3].0, b.points[3].1],
        ],
        score: b.score,
        det_score: b.det_score,
        rotated: b.rotated,
    }
}

/// POST /ocr/shibie —— 整图识别，返回拼接文本 + 框 + 分数。
async fn shibie(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (w, h) = (img.width(), img.height());

    if let Err(e) = state.ensure_engine(&opts).await {
        return e.into_response();
    }
    let boxes = match state.run_recognize(img).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let text = boxes
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let box_list: Vec<Box4> = boxes.iter().map(|b| to_dto(b).r#box).collect();
    let scores: Vec<f64> = boxes.iter().map(|b| b.score).collect();
    let lines: Vec<LineDto> = boxes.iter().map(to_dto).collect();

    Json(ApiResp::ok(ShibieData {
        text,
        boxes: box_list,
        scores,
        lines,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
}

/// POST /ocr/zhaozi —— 识别后按关键词/正则筛选命中行。
async fn zhaozi(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let keyword = opts.keyword.clone().unwrap_or_default();
    if keyword.trim().is_empty() {
        return ApiError::BadRequest("缺少 keyword 字段".to_string()).into_response();
    }
    let matcher = match build_matcher(&keyword) {
        Ok(m) => m,
        Err(e) => return e.into_response(),
    };

    if let Err(e) = state.ensure_engine(&opts).await {
        return e.into_response();
    }
    let boxes = match state.run_recognize(img).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let hits: Vec<LineDto> = boxes
        .iter()
        .filter(|b| matcher.is_match(&b.text))
        .map(to_dto)
        .collect();

    Json(ApiResp::ok(ZhaoziData {
        keyword,
        matched: hits.len(),
        hits,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
}

/// POST /ocr/duohangjson —— 多行 JSON，`[{text, box, score}, ...]`。
async fn duohangjson(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (w, h) = (img.width(), img.height());

    if let Err(e) = state.ensure_engine(&opts).await {
        return e.into_response();
    }
    let boxes = match state.run_recognize(img).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    Json(ApiResp::ok(DuohangData {
        rows: boxes.iter().map(to_dto).collect(),
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
}

/// GET /ocr/health —— 服务与引擎状态。
async fn health(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match state.health() {
        Some(info) => Json(ApiResp::ok(HealthDto {
            status: "ok".to_string(),
            model: format!("{:?}", info.level).to_lowercase(),
            device: format!("{:?}", info.requested_device).to_lowercase(),
            actual_device: format!("{:?}", info.actual_device).to_lowercase(),
            det_loaded: info.det_loaded,
            rec_loaded: info.rec_loaded,
            cls_loaded: info.cls_loaded,
            dict_size: info.dict_size,
            port: cfg.port,
            uptime_sec: state.started_at.elapsed().as_secs(),
        }))
        .into_response(),
        None => Json(err_resp(
            CODE_ENGINE_NOT_READY,
            "引擎未加载：请检查 models 目录与 onnxruntime.dll",
        ))
        .into_response(),
    }
}

// ---------------------------------------------------------------- 找图

async fn tuzhaotu1(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Image, FindMode::Exact, req).await
}
async fn tuzhaotu2(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Image, FindMode::Loose, req).await
}
async fn jubingzhaotu1(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Window, FindMode::Exact, req).await
}
async fn jubingzhaotu2(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Window, FindMode::Loose, req).await
}
async fn pingmuzhaotu1(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Screen, FindMode::Exact, req).await
}
async fn pingmuzhaotu2(State(state): State<Arc<AppState>>, req: Request) -> Response {
    find_image(state, FindSource::Screen, FindMode::Loose, req).await
}

/// 找图统一入口。
///
/// `state` 在本实现里没用到（匹配是纯 CPU 计算，不碰 OCR 引擎），保留参数是为了
/// 未来加「结果缓存 / 并发限流」时不用改路由签名。
async fn find_image(
    _state: Arc<AppState>,
    source: FindSource,
    mode: FindMode,
    req: Request,
) -> Response {
    let t0 = Instant::now();
    let mut parsed = match parse_find(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    // 先解析模板（找图的核心参数，报错优先级最高），再取搜索图。
    // `take()` 之后 parsed 仍可整体借用，只是少了 template_bytes 字段。
    let tpl = match load_template(&parsed.opts, parsed.template_bytes.take()) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (tw, th) = tpl.dimensions();

    let (hay, off_x, off_y) = match acquire_haystack(source, &parsed.opts, parsed.image_bytes.as_deref())
    {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    // ---- 匹配（CPU 密集，挪出 async 线程）----
    let mopts = MatchOptions {
        min_score: parsed.opts.min_score.unwrap_or(mode.default_min_score()),
        max_results: parsed.opts.max_results.unwrap_or(1),
    };
    let matched = tokio::task::spawn_blocking(move || matcher::find_template(&hay, &tpl, &mopts))
        .await
        .unwrap_or_else(|e| Err(format!("匹配任务异常终止: {e}")));

    let hits = match matched {
        Ok(v) => v,
        Err(e) => return ApiError::BadRequest(e).into_response(),
    };

    Json(ApiResp::ok(to_find_data(
        &hits,
        (off_x, off_y),
        (tw, th),
        source,
        t0,
    )))
    .into_response()
}

/// 把匹配结果组装成出参。HTTP 层与 Tauri 命令层共用，避免两处坐标换算逻辑漂移。
pub(crate) fn to_find_data(
    hits: &[matcher::MatchHit],
    offset: (i32, i32),
    (tw, th): (u32, u32),
    source: FindSource,
    t0: Instant,
) -> FindData {
    let (off_x, off_y) = offset;
    let matches: Vec<FindHitDto> = hits
        .iter()
        .map(|h| {
            let (cx, cy) = h.center();
            FindHitDto {
                x: h.x as i32 + off_x,
                y: h.y as i32 + off_y,
                width: h.width,
                height: h.height,
                center_x: cx + off_x as f64,
                center_y: cy + off_y as f64,
                score: h.score,
            }
        })
        .collect();

    let first = matches.first().cloned();
    FindData {
        found: !matches.is_empty(),
        x: first.as_ref().map(|h| h.x),
        y: first.as_ref().map(|h| h.y),
        score: first.as_ref().map(|h| h.score),
        center_x: first.as_ref().map(|h| h.center_x),
        center_y: first.as_ref().map(|h| h.center_y),
        width: tw,
        height: th,
        matched: matches.len(),
        matches,
        source: source.name().to_string(),
        origin_x: off_x,
        origin_y: off_y,
        elapsed_ms: t0.elapsed().as_millis(),
    }
}

struct ParsedFind {
    opts: FindOptions,
    /// multipart 里直接传的搜索图字节
    image_bytes: Option<Vec<u8>>,
    /// multipart 里直接传的模板图字节
    template_bytes: Option<Vec<u8>>,
}

async fn parse_find(req: Request) -> Result<ParsedFind, ApiError> {
    let is_multipart = req
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_start().starts_with("multipart/form-data"))
        .unwrap_or(false);

    if is_multipart {
        let mut mp = Multipart::from_request(req, &())
            .await
            .map_err(|e| ApiError::BadRequest(format!("multipart 解析失败: {e}")))?;
        let mut opts = FindOptions::default();
        let mut image_bytes = None;
        let mut template_bytes = None;

        while let Some(field) = mp
            .next_field()
            .await
            .map_err(|e| ApiError::BadRequest(format!("multipart 字段读取失败: {e}")))?
        {
            let name = field.name().unwrap_or_default().to_string();
            match name.as_str() {
                "image" | "file" => {
                    let data = field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("读取 image 失败: {e}")))?;
                    image_bytes = Some(data.to_vec());
                }
                "template" => {
                    let data = field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(format!("读取 template 失败: {e}")))?;
                    template_bytes = Some(data.to_vec());
                }
                _ => {
                    let txt = field.text().await.map_err(|e| {
                        ApiError::BadRequest(format!("读取字段 {name} 失败: {e}"))
                    })?;
                    match name.as_str() {
                        "template_file" => opts.template_file = Some(txt),
                        "title" => opts.title = Some(txt),
                        "hwnd" => opts.hwnd = txt.trim().parse().ok(),
                        "min_score" => opts.min_score = txt.trim().parse().ok(),
                        "max_results" => opts.max_results = txt.trim().parse().ok(),
                        "region" => opts.region = parse_region_str(&txt),
                        _ => {}
                    }
                }
            }
        }
        Ok(ParsedFind {
            opts,
            image_bytes,
            template_bytes,
        })
    } else {
        let bytes = to_bytes(req.into_body(), MAX_BODY)
            .await
            .map_err(|e| ApiError::BadRequest(format!("读取请求体失败: {e}")))?;
        let opts: FindOptions = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::BadRequest(format!("JSON 解析失败: {e}")))?;
        Ok(ParsedFind {
            opts,
            image_bytes: None,
            template_bytes: None,
        })
    }
}

/// multipart 里 `region` 字段写成 `x,y,w,h`。
fn parse_region_str(s: &str) -> Option<[i32; 4]> {
    let parts: Vec<i32> = s
        .split(',')
        .map(|p| p.trim().parse::<i32>())
        .collect::<Result<_, _>>()
        .ok()?;
    if parts.len() == 4 {
        Some([parts[0], parts[1], parts[2], parts[3]])
    } else {
        None
    }
}

/// 解析模板：multipart 文件 > base64 字段 > 本地路径。
pub(crate) fn load_template(opts: &FindOptions, raw: Option<Vec<u8>>) -> Result<RgbImage, ApiError> {
    let img = if let Some(bytes) = raw {
        decode_image(&bytes).map_err(ApiError::BadRequest)?
    } else if let Some(b64) = opts.template.as_deref().filter(|s| !s.trim().is_empty()) {
        let bytes = decode_base64_image(b64).map_err(ApiError::BadRequest)?;
        decode_image(&bytes).map_err(ApiError::BadRequest)?
    } else if let Some(path) = opts.template_file.as_deref().filter(|s| !s.trim().is_empty()) {
        decode_template_file(path).map_err(ApiError::BadRequest)?
    } else {
        return Err(ApiError::BadRequest(
            "缺少模板图：请提供 template（base64）或 template_file（本地路径）".to_string(),
        ));
    };
    Ok(img.to_rgb8())
}

/// 从本地路径读模板图。只允许图片扩展名，且限制 16MB。
fn decode_template_file(path: &str) -> Result<DynamicImage, String> {
    let p = std::path::Path::new(path);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !ALLOWED_TPL_EXT.contains(&ext.as_str()) {
        return Err(format!(
            "template_file 只接受 {} 扩展名，收到 {:?}",
            ALLOWED_TPL_EXT.join("/"),
            ext
        ));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("读取模板文件失败({path}): {e}"))?;
    if !meta.is_file() {
        return Err(format!("template_file 不是文件: {path}"));
    }
    if meta.len() > 16 * 1024 * 1024 {
        return Err(format!(
            "template_file 超过 16MB 上限（{} 字节）",
            meta.len()
        ));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("读取模板文件失败({path}): {e}"))?;
    decode_image(&bytes)
}

/// 拿到搜索图，同时返回它在屏幕坐标系里的左上角偏移（用于把命中坐标换算成
/// 屏幕绝对坐标）。`tuzhaotu*` 传的是图本身，偏移恒为 0。
///
/// `image_bytes` 是 multipart 形态直接上传的图；JSON 形态传 `None`，走
/// `opts.image` 的 base64。
pub(crate) fn acquire_haystack(
    source: FindSource,
    opts: &FindOptions,
    image_bytes: Option<&[u8]>,
) -> Result<(RgbImage, i32, i32), ApiError> {
    match source {
        FindSource::Image => {
            let img = if let Some(bytes) = image_bytes {
                decode_image(bytes).map_err(ApiError::BadRequest)?
            } else if let Some(b64) = opts.image.as_deref().filter(|s| !s.trim().is_empty()) {
                let bytes = decode_base64_image(b64).map_err(ApiError::BadRequest)?;
                decode_image(&bytes).map_err(ApiError::BadRequest)?
            } else {
                return Err(ApiError::BadRequest(
                    "缺少搜索图：请提供 image（base64）".to_string(),
                ));
            };
            Ok((img.to_rgb8(), 0, 0))
        }
        FindSource::Screen => {
            let (region, ox, oy) = match opts.region {
                Some([x, y, w, h]) => {
                    if w <= 0 || h <= 0 {
                        return Err(ApiError::BadRequest(format!(
                            "region 宽高必须为正，收到 {w}x{h}"
                        )));
                    }
                    (Some(ScreenRegion { x, y, width: w, height: h }), x, y)
                }
                None => {
                    let vs = capture::virtual_screen();
                    (None, vs.x, vs.y)
                }
            };
            let img = capture::capture_region(region).map_err(ApiError::Internal)?;
            Ok((img, ox, oy))
        }
        FindSource::Window => {
            let hwnd = resolve_hwnd(opts)?;
            let region = capture::window_region(hwnd).map_err(ApiError::Internal)?;
            let img = capture::capture_window(hwnd).map_err(ApiError::Internal)?;
            Ok((img, region.x, region.y))
        }
    }
}

/// 句柄优先，其次按标题（精确 → 子串）。
fn resolve_hwnd(opts: &FindOptions) -> Result<isize, ApiError> {
    if let Some(h) = opts.hwnd {
        if h == 0 {
            return Err(ApiError::BadRequest("hwnd 不能为 0".to_string()));
        }
        return Ok(h as isize);
    }
    if let Some(title) = opts.title.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Some(h) = capture::find_window(title) {
            return Ok(h);
        }
        if let Some(h) = capture::find_window_contains(title) {
            return Ok(h);
        }
        return Err(ApiError::BadRequest(format!(
            "找不到标题为 {title:?} 的可见窗口"
        )));
    }
    Err(ApiError::BadRequest(
        "截窗口找图需要 hwnd 或 title".to_string(),
    ))
}

// ---------------------------------------------------------------- 单字定位

fn pt(p: crate::ocr::geometry::Pt) -> [f64; 2] {
    [p.0, p.1]
}

/// POST /ocr/danzijson —— 逐字返回文本框。
///
/// 用途：需要按字定位的场景（验证码、逐字点击、排版分析）。整行识别接口只给
/// 行的四边形，拿不到「每个字在哪」。
///
/// 精度边界：字框是**用 CTC 帧区间按比例切分行宽**算出来的，不是对每个字单独
/// 检测。所以字间距不均、行内混排不同字号时，边界会有几个像素误差；被空格
/// 隔开的两段字之间的空隙可能被算进前一个字的框。
async fn danzijson(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (w, h) = (img.width(), img.height());

    if let Err(e) = state.ensure_engine(&opts).await {
        return e.into_response();
    }
    let details = match state.run_recognize_detailed(img).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let mut char_count = 0usize;
    let mut text_lines: Vec<String> = Vec::with_capacity(details.len());
    let lines: Vec<DanziLineDto> = details
        .into_iter()
        .map(|l| {
            char_count += l.chars.len();
            text_lines.push(l.text.clone());
            DanziLineDto {
                text: l.text,
                r#box: [
                    pt(l.points[0]),
                    pt(l.points[1]),
                    pt(l.points[2]),
                    pt(l.points[3]),
                ],
                score: l.score,
                det_score: l.det_score,
                rotated: l.rotated,
                frames: l.frames,
                chars: l
                    .chars
                    .into_iter()
                    .map(|c| CharDto {
                        ch: c.ch,
                        r#box: [
                            pt(c.points[0]),
                            pt(c.points[1]),
                            pt(c.points[2]),
                            pt(c.points[3]),
                        ],
                        score: c.score,
                        index: c.index,
                        t_start: c.t_start,
                        t_end: c.t_end,
                    })
                    .collect(),
            }
        })
        .collect();

    Json(ApiResp::ok(DanziData {
        text: text_lines.join("\n"),
        lines,
        char_count,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
}
// ---------------------------------------------------------------- 关键词匹配

enum Matcher {
    Sub(String),
    Re(regex::Regex),
}

impl Matcher {
    fn is_match(&self, s: &str) -> bool {
        match self {
            Matcher::Sub(k) => s.contains(k.as_str()),
            Matcher::Re(r) => r.is_match(s),
        }
    }
}

/// `/正则/` 形式按正则匹配，其余按普通子串匹配。
fn build_matcher(keyword: &str) -> Result<Matcher, ApiError> {
    let k = keyword.trim();
    if k.len() >= 2 && k.starts_with('/') && k.ends_with('/') {
        let pat = &k[1..k.len() - 1];
        regex::Regex::new(pat)
            .map(Matcher::Re)
            .map_err(|e| ApiError::BadRequest(format!("正则表达式无效: {e}")))
    } else {
        Ok(Matcher::Sub(k.to_string()))
    }
}
