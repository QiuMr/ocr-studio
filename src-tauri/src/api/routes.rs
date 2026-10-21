//! HTTP 路由与处理器。路由表 1:1 对应 `docs/PLAN.md` 第 6 节。
//!
//! 已实现：`/ocr/shibie`、`/ocr/zhaozi`、`/ocr/duohangjson`、`/ocr/health`。
//! 其余（找图/句柄/屏幕截图、单字 JSON）属于 M2 里程碑，统一返回
//! `code=501` + 明确说明，而不是假装成功。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::to_bytes;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use image::DynamicImage;

use super::dto::{
    decode_base64_image, decode_image, err_resp, ApiError, ApiResp, Box4, DuohangData, HealthDto,
    LineDto, OcrOptions, ShibieData, ZhaoziData, CODE_ENGINE_NOT_READY,
};
use super::state::AppState;
use crate::ocr::TextBox;

/// 请求体上限 32MB：截图级 PNG  compressed 一般几百 KB，留足余量。
const MAX_BODY: usize = 32 * 1024 * 1024;

/// 构建路由。调用方需 `with_state` 注入 `Arc<AppState>`。
pub fn build_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/ocr/shibie", post(shibie))
        .route("/ocr/zhaozi", post(zhaozi))
        .route("/ocr/duohangjson", post(duohangjson))
        .route("/ocr/danzijson", post(not_implemented_danzi))
        .route("/ocr/tuzhaotu1", post(not_implemented_findimg))
        .route("/ocr/tuzhaotu2", post(not_implemented_findimg))
        .route("/ocr/jubingzhaotu1", post(not_implemented_findimg))
        .route("/ocr/jubingzhaotu2", post(not_implemented_findimg))
        .route("/ocr/pingmuzhaotu1", post(not_implemented_findimg))
        .route("/ocr/pingmuzhaotu2", post(not_implemented_findimg))
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

// ---------------------------------------------------------------- M2 占位

async fn not_implemented_danzi() -> Response {
    ApiError::NotImplemented(
        "单字 JSON 需要字符级定位（CTC 帧对齐 + 逆透视映射），规划在 M2 实现".to_string(),
    )
    .into_response()
}

async fn not_implemented_findimg() -> Response {
    ApiError::NotImplemented(
        "找图类接口依赖屏幕/句柄截图与模板匹配，规划在 M2 实现".to_string(),
    )
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
