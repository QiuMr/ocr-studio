//! HTTP 路由与处理器。路由表 1:1 对应 `docs/PLAN.md` 第 6 节。
//!
//! 全部端点均已实现：
//! - 识别类：`/ocr/shibie`、`/ocr/zhaozi`、`/ocr/duohangjson`
//! - 找图类（模板）：`/ocr/tuzhaotu1|2`（传图）、`/ocr/jubingzhaotu1|2`（截窗口）、
//!   `/ocr/pingmuzhaotu1|2`（截屏幕）
//! - 找图类（语义）：`/ocr/yuyizhaotu`（用检测模型直接找目标，不需要模板图）
//! - 状态：`/ocr/health`
//!
//! 找图三组的后缀语义：`1` = 精准（NCC ≥ 0.92），`2` = 容错（NCC ≥ 0.72）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::header::{self, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use image::{DynamicImage, RgbImage};

use super::dto::{
    decode_base64_image, decode_image, err_resp, ApiError, ApiResp, Box4, CharDto, DanziData,
    DanziLineDto, DetectData, DetectHitDto, DuohangData, FindData, FindHitDto, FindOptions,
    HealthDto, LineDto, OcrOptions, PipelineData, PipelineRegionDto, ShibieData, ZhaoziData,
    CODE_ENGINE_NOT_READY, CODE_METHOD_NOT_ALLOWED, CODE_NOT_FOUND, EngineItemDto, EngineListData,
};
use super::state::AppState;
use crate::capture::{self, ScreenRegion};
use crate::engine::TextLine;
use crate::matcher::{self, MatchOptions};
use crate::ocr::Device;

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
///
/// CORS 默认**关闭**：这些接口能读本机屏幕（`/ocr/pingmuzhaotu*`），
/// 默认放行任意网页等于把「截屏 + 识别」交给任何打开的网页去驱动。
/// 需要从浏览器页面调用时，去设置里显式打开。
pub fn build_router() -> Router<Arc<AppState>> {
    build_router_with(false)
}

/// 带 CORS 开关的路由构建。
pub fn build_router_with(allow_browser: bool) -> Router<Arc<AppState>> {
    let mut router = Router::new()
        .route(P_SHIBIE, post(shibie))
        .route(P_ZHAOZI, post(zhaozi))
        .route(P_DUOHANG, post(duohangjson))
        .route(P_DANZI, post(danzijson))
        .route(P_TU1, post(tuzhaotu1))
        .route(P_TU2, post(tuzhaotu2))
        .route(P_JU1, post(jubingzhaotu1))
        .route(P_JU2, post(jubingzhaotu2))
        .route(P_PINGMU1, post(pingmuzhaotu1))
        .route(P_PINGMU2, post(pingmuzhaotu2))
        .route(P_YUYI, post(yuyizhaotu))
        .route(P_HEALTH, get(health))
        .route(P_ENGINES, get(engine_list))
        .route(P_DETECT, post(vision_detect))
        .route(P_PIPELINE, post(vision_pipeline))
        // 兜底：先注册再套 layer，layer 才会把 fallback 一起包住
        .fallback(not_found);

    // 404/405 统一成 {code,msg} 信封。顺序有意义：后 layer 的在外层，
    // 所以 CORS 在 unify 外面——OPTIONS 预检要能在进入 unify 之前被处理。
    router = router.layer(axum::middleware::from_fn(unify_router_errors));
    if allow_browser {
        router = router.layer(axum::middleware::from_fn(cors_headers));
    }
    router
}

// ---------------------------------------------------------------- 路径表

/// 端点路径的**单一事实源**：`.route()` 与「你是不是想调 X」提示都从这里取，
/// 不可能只改一处。加路由时同时加到这个表里。
const P_SHIBIE: &str = "/ocr/shibie";
const P_ZHAOZI: &str = "/ocr/zhaozi";
const P_DUOHANG: &str = "/ocr/duohangjson";
const P_DANZI: &str = "/ocr/danzijson";
const P_TU1: &str = "/ocr/tuzhaotu1";
const P_TU2: &str = "/ocr/tuzhaotu2";
const P_JU1: &str = "/ocr/jubingzhaotu1";
const P_JU2: &str = "/ocr/jubingzhaotu2";
const P_PINGMU1: &str = "/ocr/pingmuzhaotu1";
const P_PINGMU2: &str = "/ocr/pingmuzhaotu2";
/// 语义找图：一个端点，来源由 body 里的 `source` 指定（模板找图才按来源分三组）
const P_YUYI: &str = "/ocr/yuyizhaotu";
const P_HEALTH: &str = "/ocr/health";
const P_ENGINES: &str = "/engine/list";
const P_DETECT: &str = "/vision/detect";
const P_PIPELINE: &str = "/vision/pipeline";

/// 全部端点（给 404 提示与自检用）。
pub const PATHS: &[&str] = &[
    P_SHIBIE, P_ZHAOZI, P_DUOHANG, P_DANZI, P_TU1, P_TU2, P_JU1, P_JU2, P_PINGMU1, P_PINGMU2, P_YUYI,
    P_HEALTH, P_ENGINES, P_DETECT, P_PIPELINE,
];

// ---------------------------------------------------------------- 兜底与 CORS

/// 默认 fallback。正常情况下会被 `unify_router_errors` 改写，保留它是为了
/// 中间件顺序被改动时也不至于返回空 body。
async fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(err_resp(CODE_NOT_FOUND, "没有这个接口")),
    )
        .into_response()
}

/// 把 axum 自动产生的空体 404/405 换成统一信封。
///
/// 为什么值得做：全站约定是 `{code,msg,data}`，但 axum 的路由层自己回的
/// 404/405 是**空 body**。小白照着文档把 `shibie` 打成 `shibei`，只会拿到
/// 一个空响应，完全不知道是路径写错了——这类问题靠猜最耗时间。
async fn unify_router_errors(req: Request, next: axum::middleware::Next) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let resp = next.run(req).await;

    if resp.status() == StatusCode::NOT_FOUND {
        let msg = match closest_path(&path) {
            Some(p) => format!("没有这个接口：{path}，你是不是想调 {p}？"),
            None => format!(
                "没有这个接口：{path}。可用接口见界面「接口」页，或 GET /engine/list"
            ),
        };
        return (StatusCode::NOT_FOUND, Json(err_resp(CODE_NOT_FOUND, msg))).into_response();
    }

    if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("POST")
            .to_string();
        let hint = if method == Method::OPTIONS {
            "（浏览器页面调用需要先在设置里打开「允许浏览器页面调用」）"
        } else {
            ""
        };
        let msg = format!("{path} 不支持 {method}，请改用 {allow}{hint}");
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            Json(err_resp(CODE_METHOD_NOT_ALLOWED, msg)),
        )
            .into_response();
    }
    resp
}

/// 给响应补 CORS 头；OPTIONS 预检直接短路返回 204。
///
/// 回显 Origin 而不是写死 `*`：将来若要支持 credentials，写死 `*` 会被浏览器拒绝。
async fn cors_headers(req: Request, next: axum::middleware::Next) -> Response {
    let origin = req.headers().get(header::ORIGIN).cloned();
    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        add_cors(&mut resp, origin);
        return resp;
    }
    let mut resp = next.run(req).await;
    add_cors(&mut resp, origin);
    resp
}

fn add_cors(resp: &mut Response, origin: Option<HeaderValue>) {
    let h = resp.headers_mut();
    if let Some(o) = origin {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
        h.insert(header::VARY, HeaderValue::from_static("Origin"));
    } else {
        h.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
}

/// 在已知路径里找最接近的一条（编辑距离 ≤ 3 才算），用于「你是不是想调 X」。
fn closest_path(input: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for p in PATHS {
        let d = edit_distance(input, p);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((p, d));
        }
    }
    best.filter(|(_, d)| *d <= 3).map(|(p, _)| p)
}

/// 标准 Levenshtein 距离（两条短路径，O(n·m) 足够）。
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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
        engine: fields.get("engine").cloned(),
        keyword: fields.get("keyword").cloned(),
        conf: fields.get("conf").and_then(|s| s.trim().parse().ok()),
        iou: fields.get("iou").and_then(|s| s.trim().parse().ok()),
        class_filter: fields
            .get("class_filter")
            .and_then(|s| parse_id_list(s)),
        detect_engine: fields.get("detect_engine").cloned(),
    };
    Ok((img, opts))
}

/// multipart 里 `class_filter` 写成逗号分隔的 id 列表（如 `0,5,7`）。
fn parse_id_list(s: &str) -> Option<Vec<usize>> {
    let ids: Vec<usize> = s
        .split(',')
        .map(|p| p.trim().parse::<usize>())
        .collect::<Result<_, _>>()
        .ok()?;
    Some(ids)
}

// ---------------------------------------------------------------- 处理器

fn to_dto(b: &TextLine) -> LineDto {
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

    let boxes = match state.recognize_lines(&opts, img).await {
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

    let boxes = match state.recognize_lines(&opts, img).await {
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

    let boxes = match state.recognize_lines(&opts, img).await {
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
///
/// PP-OCR 还没加载、但用户正在用 ddddocr 时，不能报「引擎未加载」——
/// 那会让用户以为整个服务挂了。这种情况下按额外引擎的实际状态回一份。
async fn health(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match state.health() {
        Some(info) => Json(ApiResp::ok(HealthDto {
            status: "ok".to_string(),
            engine: crate::engine::EngineSpec::ppocr(info.level).key(),
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
        None => {
            if let Some((spec, dev, _caps)) = state.alt_loaded() {
                let dev = dev.unwrap_or(Device::Cpu);
                return Json(ApiResp::ok(HealthDto {
                    status: "ok".to_string(),
                    engine: spec.key(),
                    model: spec.variant.clone().unwrap_or_default(),
                    device: format!("{:?}", dev).to_lowercase(),
                    actual_device: format!("{:?}", dev).to_lowercase(),
                    det_loaded: true,
                    rec_loaded: true,
                    cls_loaded: false,
                    // ddddocr 的字典长度没在这一层缓存，回 0 而不是瞎猜
                    dict_size: 0,
                    port: cfg.port,
                    uptime_sec: state.started_at.elapsed().as_secs(),
                }))
                .into_response();
            }
            Json(err_resp(
                CODE_ENGINE_NOT_READY,
                "引擎未加载：请检查 models 目录与 onnxruntime.dll",
            ))
            .into_response()
        }
    }
}

// ---------------------------------------------------------------- 引擎列表

/// 列出本机可用的引擎与能力。
///
/// **只读文件系统，不加载模型**：加载一次 det+rec 要几百毫秒到几秒、还吃显存，
/// 拿来做探测完全不合适。所以这里只能回答「这个引擎的模型齐不齐」，
/// 「能不能真跑起来」要加载后才知道（`/ocr/health` 的 `actual_device` 才是真相）。
///
/// 已加载的实例会把 `actual_device` 回填到对应条目上，前端才能显示
/// 「当前正在用 GPU」——否则用户改完设备重启，界面上看不出到底生效没有。
async fn engine_list(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let engines = state.engine_status_list();

    Json(ApiResp::ok(EngineListData {
        models_root: state.models_root.display().to_string(),
        default_engine: crate::engine::EngineSpec::ppocr(cfg.level).key(),
        engines: engines.iter().map(EngineItemDto::from).collect(),
    }))
    .into_response()
}

// ---------------------------------------------------------------- 目标检测

/// POST /vision/detect —— 通用目标检测（引擎名 `yolo`）。
///
/// 与 `/ocr/*` 共用引擎槽：OCR 引擎没有检测能力，会返回 code 1002；
/// `engine` 参数缺省时按 PP-OCR 处理（同样 1002），想检测必须显式写
/// `engine: "yolo"`。
async fn vision_detect(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (w, h) = (img.width(), img.height());

    // 阈值合法性：NaN/负数直接 400，超 1 的 conf/iou 没有意义也拦下
    let conf = opts.conf.unwrap_or(0.25);
    let iou = opts.iou.unwrap_or(0.7);
    if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
        return ApiError::BadRequest(format!("conf 必须在 0..1，收到 {conf}")).into_response();
    }
    if !iou.is_finite() || !(0.0..=1.0).contains(&iou) {
        return ApiError::BadRequest(format!("iou 必须在 0..1，收到 {iou}")).into_response();
    }

    let detect = crate::engine::DetectOptions {
        conf_thresh: conf,
        iou_thresh: iou,
        class_filter: opts.class_filter.clone().unwrap_or_default(),
    };
    // 出参里的 engine 标识：**回显规范化后的引擎名**，不是原请求串。
    // 否则调用方传 `yolo:v8`（旧写法）时，响应里也写 `yolo:v8`，看起来像
    // 版本号还有意义；实际生效的是默认模型。解析失败时原样回，交给下游报 1003。
    let engine_label = match opts.engine.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => crate::engine::EngineSpec::parse(e)
            .map(|s| s.key())
            .unwrap_or_else(|_| e.to_string()),
        None => {
            let level = state
                .cfg
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .level;
            crate::engine::EngineSpec::ppocr(level).key()
        }
    };

    let dets = match state.detect_objects(&opts, img, detect).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let detections: Vec<DetectHitDto> = dets
        .iter()
        .map(|d| DetectHitDto {
            r#box: [
                [d.points[0].0, d.points[0].1],
                [d.points[1].0, d.points[1].1],
                [d.points[2].0, d.points[2].1],
                [d.points[3].0, d.points[3].1],
            ],
            class_id: d.class_id,
            class_name: d.class_name.clone(),
            score: d.score,
        })
        .collect();

    Json(ApiResp::ok(DetectData {
        engine: engine_label,
        count: detections.len(),
        detections,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
}

/// POST /vision/pipeline —— 检测 + 识别串联：检测引擎找区域，PP-OCR 框内认字。
///
/// `detect_engine` 缺省 `yolo`；`engine` 字段若指向 yolo 也会被用作检测引擎。
/// 用 yolov8n+coco 时检出的是人/车而不是文字块——**这是预期**，pipeline 面向
/// 自训检测模型（见 docs/ENGINES.md 6.3）。
async fn vision_pipeline(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let (img, opts) = match parse_recognize(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let (w, h) = (img.width(), img.height());

    let conf = opts.conf.unwrap_or(0.25);
    let iou = opts.iou.unwrap_or(0.7);
    if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
        return ApiError::BadRequest(format!("conf 必须在 0..1，收到 {conf}")).into_response();
    }
    if !iou.is_finite() || !(0.0..=1.0).contains(&iou) {
        return ApiError::BadRequest(format!("iou 必须在 0..1，收到 {iou}")).into_response();
    }
    let detect = crate::engine::DetectOptions {
        conf_thresh: conf,
        iou_thresh: iou,
        class_filter: opts.class_filter.clone().unwrap_or_default(),
    };

    // 识别引擎标识：PP-OCR 当前档位（engine 字段指向 ppocr:* 也归到这）
    let ocr_level = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .level;
    let ocr_label = crate::engine::EngineSpec::ppocr(ocr_level).key();
    let detect_label = opts
        .detect_engine
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            opts.engine.clone().filter(|s| {
                let t = s.trim().to_ascii_lowercase();
                t.starts_with("yolo") || t.starts_with("detect")
            })
        })
        .unwrap_or_else(|| "yolo".to_string());

    let regions = match state.run_pipeline(&opts, img, detect).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    let mut text_count = 0usize;
    let region_dtos: Vec<PipelineRegionDto> = regions
        .iter()
        .map(|(d, lines)| {
            text_count += lines.len();
            PipelineRegionDto {
                r#box: [
                    [d.points[0].0, d.points[0].1],
                    [d.points[1].0, d.points[1].1],
                    [d.points[2].0, d.points[2].1],
                    [d.points[3].0, d.points[3].1],
                ],
                class_id: d.class_id,
                class_name: d.class_name.clone(),
                score: d.score,
                text: lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"),
                // 行坐标在裁剪图坐标系，平移回原图（检测框是轴对齐的，直接加偏移）
                lines: lines
                    .iter()
                    .map(|l| LineDto {
                        text: l.text.clone(),
                        r#box: [
                            [l.points[0].0 + d.points[0].0, l.points[0].1 + d.points[0].1],
                            [l.points[1].0 + d.points[0].0, l.points[1].1 + d.points[0].1],
                            [l.points[2].0 + d.points[0].0, l.points[2].1 + d.points[0].1],
                            [l.points[3].0 + d.points[0].0, l.points[3].1 + d.points[0].1],
                        ],
                        score: l.score,
                        det_score: l.det_score,
                        rotated: l.rotated,
                    })
                    .collect(),
            }
        })
        .collect();

    Json(ApiResp::ok(PipelineData {
        detect_engine: detect_label,
        ocr_engine: ocr_label,
        count: text_count,
        regions: region_dtos,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    }))
    .into_response()
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

/// POST /ocr/yuyizhaotu —— 语义找图：用目标检测模型直接找目标，**不需要模板图**。
///
/// 与模板找图（`tuzhaotu*` / `pingmuzhaotu*` / `jubingzhaotu*`）的取舍：
/// - 模板找图要求「长得一模一样」——图标换个皮肤、按钮按下态就失配；
///   语义找图不挑外观，认得「这是个按钮」就行（YOLOE-26 这类开放词汇模型尤其如此）。
/// - 代价：只认得模型见过的类别。YOLOE-26 的提示在导出 ONNX 时就烘焙进权重了，
///   导出后改不了类别，换目标要重新导一个模型（`scripts/fetch_yoloe.py`）。
///
/// **出参与模板找图完全一致**（`found`/`x`/`y`/`center_x`/`center_y`/`score`/
/// `matches`/`origin_x`/`origin_y`），额外多出 `engine` 与命中里的
/// `class_id`/`class_name`。已有脚本把端点路径一换就能用。
async fn yuyizhaotu(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let t0 = Instant::now();
    let parsed = match parse_find(req).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let opts = parsed.opts;

    // 来源：body 里的 source（缺省 image——不需要窗口句柄，最不容易失败）
    let source = match opts
        .source
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => match FindSource::parse(s) {
            Some(s) => s,
            None => {
                return ApiError::BadRequest(format!(
                    "未知 source {s:?}（可选 image / screen / window）"
                ))
                .into_response()
            }
        },
        None => FindSource::Image,
    };

    // conf 是主字段；给了 min_score 就当别名用（脚本从模板找图切过来时常常只改路径）
    let conf = opts.conf.or(opts.min_score.map(|v| v as f32)).unwrap_or(0.25);
    if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
        return ApiError::BadRequest(format!("conf 必须在 0..1，收到 {conf}")).into_response();
    }

    let (hay, off_x, off_y) =
        match acquire_haystack(source, &opts, parsed.image_bytes.as_deref()) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };

    // 语义找图必须走检测引擎。缺省 `yolo` = 目录里排第一的那个检测模型，
    // 传 OCR 引擎名会得到 1002（不支持该能力），错误提示由引擎层给出。
    let engine_raw = opts
        .engine
        .clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "yolo".to_string());
    // 回显规范化后的引擎名，理由同 /vision/detect：`yolo:v8` 这类旧写法
    // 实际生效的是默认模型，原样回显会让人以为版本号还有意义。
    let engine_label = crate::engine::EngineSpec::parse(&engine_raw)
        .map(|s| s.key())
        .unwrap_or_else(|_| engine_raw.clone());

    let detect = crate::engine::DetectOptions {
        conf_thresh: conf,
        // 语义找图不暴露 iou：NMS-free 端到端模型（v10/v26/yoloe）根本不做 NMS，
        // 暴露一个只在部分模型上生效的参数，比不暴露更容易让人困惑。
        iou_thresh: 0.7,
        class_filter: opts.class_filter.clone().unwrap_or_default(),
    };
    let ocr_opts = OcrOptions {
        image: String::new(),
        level: None,
        device: None,
        use_cls: None,
        engine: Some(engine_raw),
        keyword: None,
        conf: Some(conf),
        iou: None,
        class_filter: opts.class_filter.clone(),
        detect_engine: None,
    };

    let dets = match state
        .detect_objects(&ocr_opts, DynamicImage::ImageRgb8(hay), detect)
        .await
    {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };

    Json(ApiResp::ok(to_find_data_from_dets(
        &dets,
        (off_x, off_y),
        source,
        Some(engine_label),
        opts.max_results.unwrap_or(1),
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
                class_id: None,
                class_name: None,
            }
        })
        .collect();

    // 模板尺寸在未命中时也要给（调用方靠它算点击偏移），所以显式传进来
    assemble_find_data(matches, (tw, th), offset, source, None, t0)
}

/// 把检测结果组装成**与模板找图同构**的出参（语义找图用）。
///
/// 坐标换算、扁平字段、`origin_x/origin_y` 全部与 `to_find_data` 走同一条路，
/// 已有脚本把端点路径一换即可，不用改解析代码。
pub(crate) fn to_find_data_from_dets(
    dets: &[crate::engine::Detection],
    offset: (i32, i32),
    source: FindSource,
    engine: Option<String>,
    max_results: usize,
    t0: Instant,
) -> FindData {
    let (off_x, off_y) = offset;
    let mut matches: Vec<FindHitDto> = dets
        .iter()
        .map(|d| {
            // 检测框是轴对齐的：p0 = 左上，p2 = 右下
            let (x0, y0) = (d.points[0].0, d.points[0].1);
            let (x1, y1) = (d.points[2].0, d.points[2].1);
            FindHitDto {
                x: x0.round() as i32 + off_x,
                y: y0.round() as i32 + off_y,
                width: (x1 - x0).round().max(0.0) as u32,
                height: (y1 - y0).round().max(0.0) as u32,
                center_x: (x0 + x1) / 2.0 + off_x as f64,
                center_y: (y0 + y1) / 2.0 + off_y as f64,
                score: d.score,
                class_id: Some(d.class_id),
                class_name: Some(d.class_name.clone()),
            }
        })
        .collect();

    // 置信度高的在前——脚本读 `x`/`y` 拿到的应该是「最像的那个」
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(max_results.clamp(1, MAX_DET_RESULTS));

    // 没有模板，用首个命中框的尺寸充当 width/height（未命中为 0）
    let size = matches
        .first()
        .map(|h| (h.width, h.height))
        .unwrap_or((0, 0));
    assemble_find_data(matches, size, offset, source, engine, t0)
}

/// 语义找图最多返回几个命中。比模板找图（上限 64）小一档：
/// 检测模型一屏最多也就检出几十个目标，再多的返回量只会让脚本难处理。
const MAX_DET_RESULTS: usize = 64;

/// 命中列表 → 出参。扁平字段统一取**第一个**命中，两条组装路径共用这一段。
fn assemble_find_data(
    matches: Vec<FindHitDto>,
    (width, height): (u32, u32),
    offset: (i32, i32),
    source: FindSource,
    engine: Option<String>,
    t0: Instant,
) -> FindData {
    let first = matches.first().cloned();
    FindData {
        found: !matches.is_empty(),
        x: first.as_ref().map(|h| h.x),
        y: first.as_ref().map(|h| h.y),
        score: first.as_ref().map(|h| h.score),
        center_x: first.as_ref().map(|h| h.center_x),
        center_y: first.as_ref().map(|h| h.center_y),
        width,
        height,
        matched: matches.len(),
        matches,
        source: source.name().to_string(),
        // 上面各坐标已经是「图内坐标 + offset」的屏幕绝对坐标；这里再单独回填
        // 一份偏移原值：前端要用它把绝对坐标换回图内坐标（多屏时原点可能是负数）。
        origin_x: offset.0,
        origin_y: offset.1,
        engine,
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
                        // 语义找图专有字段
                        "source" => opts.source = Some(txt),
                        "engine" => opts.engine = Some(txt),
                        "conf" => opts.conf = txt.trim().parse().ok(),
                        "class_filter" => opts.class_filter = parse_id_list(&txt),
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

    let details = match state.recognize_details(&opts, img).await {
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


#[cfg(test)]
mod tests {
    use super::*;

    /// 路径表就是路由表的镜像：数量对不上说明有人加了路由忘了登记，
    /// 那样 404 提示会指错地方。
    #[test]
    fn paths_table_covers_all_endpoints() {
        assert_eq!(PATHS.len(), 15, "端点数量变了就要同步 PATHS");
        for p in PATHS {
            assert!(p.starts_with('/'), "路径必须以 / 开头: {p}");
        }
        let mut uniq = PATHS.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), PATHS.len(), "PATHS 里有重复项");
    }

    /// 打错一个字母要能猜回来（这是 404 提示的全部价值）。
    #[test]
    fn closest_path_suggests_typo() {
        assert_eq!(closest_path("/ocr/shibei"), Some("/ocr/shibie"));
        assert_eq!(closest_path("/ocr/zhaotu1"), Some("/ocr/tuzhaotu1"));
        assert_eq!(closest_path("/ocr/yuyizhaot"), Some("/ocr/yuyizhaotu"));
        // 完全不相干就别乱猜，返回 None 让提示退化成「看接口页」
        assert_eq!(closest_path("/favicon.ico"), None);
        assert_eq!(closest_path("/"), None);
    }

    fn det(points: [(f64, f64); 4], class_id: usize, score: f64) -> crate::engine::Detection {
        crate::engine::Detection {
            points,
            class_id,
            class_name: "button".to_string(),
            score,
        }
    }

    /// 语义找图出参与模板找图**同构**：脚本换路径就能用，不该出现字段差异。
    #[test]
    fn semantic_find_shares_template_find_shape() {
        let dets = vec![
            det([(10.0, 20.0), (110.0, 20.0), (110.0, 70.0), (10.0, 70.0)], 3, 0.4),
            det(
                [(200.0, 300.0), (300.0, 300.0), (300.0, 360.0), (200.0, 360.0)],
                3,
                0.9,
            ),
        ];
        let d = to_find_data_from_dets(
            &dets,
            (100, 50),
            FindSource::Window,
            Some("yolo".to_string()),
            1,
            Instant::now(),
        );
        assert!(d.found);
        // 置信度高的排前面，扁平字段取它（脚本读 data.x 拿到的应该是「最像的」）
        assert_eq!(d.matched, 1);
        assert_eq!(d.x, Some(300));
        assert_eq!(d.y, Some(350));
        assert_eq!(d.score, Some(0.9));
        assert_eq!(d.center_x, Some(350.0));
        assert_eq!(d.center_y, Some(380.0));
        // 没有模板，width/height 用首个命中框的尺寸
        assert_eq!((d.width, d.height), (100, 60));
        // 偏移是「图内坐标 → 屏幕绝对坐标」的换算量，前端靠它反算
        assert_eq!((d.origin_x, d.origin_y), (100, 50));
        assert_eq!(d.source, "window");
        assert_eq!(d.engine.as_deref(), Some("yolo"));
        assert_eq!(d.matches[0].class_name.as_deref(), Some("button"));
    }

    /// 未命中不能 panic，尺寸给 0（语义找图没有模板可回填）。
    #[test]
    fn semantic_find_empty_gives_zero_size() {
        let d = to_find_data_from_dets(
            &[],
            (0, 0),
            FindSource::Image,
            Some("yolo".to_string()),
            5,
            Instant::now(),
        );
        assert!(!d.found);
        assert_eq!(d.matched, 0);
        assert_eq!((d.width, d.height), (0, 0));
        assert!(d.x.is_none() && d.center_x.is_none() && d.score.is_none());
        assert_eq!(d.source, "image");
    }

    /// max_results 会被夹到上限，别让脚本要求 10 万条就真返回 10 万条。
    #[test]
    fn semantic_find_clamps_max_results() {
        let dets: Vec<_> = (0..3)
            .map(|i| det([(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], i, 0.5))
            .collect();
        let d = to_find_data_from_dets(&dets, (0, 0), FindSource::Image, None, 0, Instant::now());
        // 0 → 夹到 1
        assert_eq!(d.matched, 1);
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("shibie", "shibei"), 2);
    }
}