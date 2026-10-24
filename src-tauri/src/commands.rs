//! Tauri invoke 命令层：给前端 Dashboard / Playground 直接调用。
//!
//! 与 HTTP 接口的分工：
//! - 前端界面走这里（少一跳、类型安全、无需自己拼 base64 传输开销）
//! - 外部脚本/易语言/按键精灵走 `api::routes` 的 `/ocr/*`（PLAN 第 6 节）
//! 两条路最终都汇到同一个 `AppState`，不会各加载一份模型。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::State;

use crate::api::dto::{decode_base64_image, decode_image, HealthDto, LineDto, OcrOptions};
use crate::api::routes::{self, FindMode, FindSource};
use crate::api::state::AppState;
use crate::api::stop as stop_server;
use crate::capture;
use crate::config::{AppConfig, Device, ModelLevel};
use crate::matcher::{self, MatchOptions};

/// 识别结果（前端 Playground 用）。
#[derive(Debug, Serialize)]
pub struct RecognizeOut {
    pub text: String,
    pub lines: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 引擎列表（前端设置页用）。字段与 `/engine/list` 的出参保持一致，
/// 前端两套通道（IPC / HTTP）可以共用同一个 TS 类型。
#[derive(Debug, Serialize)]
pub struct EngineListOut {
    pub models_root: String,
    pub default_engine: String,
    pub engines: Vec<crate::api::dto::EngineItemDto>,
}

fn err<T>(msg: String) -> Result<T, String> {
    Err(msg)
}

/// 识别一张图片。`image` 为 base64（可带 data URL 前缀）。
#[tauri::command]
pub async fn ocr_recognize(
    state: State<'_, Arc<AppState>>,
    image: String,
    use_cls: Option<bool>,
    engine: Option<String>,
) -> Result<RecognizeOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let opts = OcrOptions {
        image,
        level: None,
        device: None,
        use_cls,
        engine,
        keyword: None,
        conf: None,
        iou: None,
        class_filter: None,
        detect_engine: None,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let boxes = state
        .recognize_lines(&opts, img)
        .await
        .map_err(|e| e.message())?;

    let text = boxes
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let lines = boxes
        .iter()
        .map(|b| LineDto {
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
        })
        .collect();

    Ok(RecognizeOut {
        text,
        lines,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------- 区域 OCR

/// 区域 OCR 结果（与 HTTP `/ocr/quyuocr` 结构一致）。
#[derive(Debug, Serialize)]
pub struct RegionOcrOut {
    /// 实际生效的区域 `[x, y, w, h]`（请求值钳到图内后的结果）
    pub region: [i32; 4],
    /// 区域内全部文本按行用 `\n` 拼接
    pub text: String,
    /// 命中行的框，坐标已在**原图**坐标系
    pub lines: Vec<LineDto>,
    /// 原图宽（不是区域宽）
    pub width: u32,
    /// 原图高（不是区域高）
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 区域 OCR：只认指定矩形里的字，返回的框坐标自动平移回原图。
///
/// 与 HTTP `/ocr/quyuocr` 等价。调用方自己裁剪再识别也能做到，但要多一次
/// 编码/解码，还得自己平移坐标（多显示器、负原点时容易算错）。
#[tauri::command]
pub async fn ocr_quyuocr(
    state: State<'_, Arc<AppState>>,
    image: String,
    region: [i32; 4],
    use_cls: Option<bool>,
    engine: Option<String>,
) -> Result<RegionOcrOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let [rx, ry, rw, rh] = region;
    let Some([x0, y0, cw, ch]) = routes::clamp_region(rx, ry, rw, rh, w, h) else {
        return Err(format!(
            "region [{rx},{ry},{rw},{rh}] 与图片（{w}x{h}）没有有效交集"
        ));
    };

    let opts = OcrOptions {
        image,
        level: None,
        device: None,
        use_cls,
        engine,
        keyword: None,
        conf: None,
        iou: None,
        class_filter: None,
        detect_engine: None,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let crop = img.crop_imm(x0 as u32, y0 as u32, cw as u32, ch as u32);
    let mut boxes = state
        .recognize_lines(&opts, crop)
        .await
        .map_err(|e| e.message())?;
    // 坐标平移回原图（轴对齐裁剪，直接加偏移）
    for b in &mut boxes {
        for p in &mut b.points {
            p.0 += x0 as f64;
            p.1 += y0 as f64;
        }
    }

    let text = boxes
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let lines = boxes.iter().map(|b| to_line_dto(b)).collect();

    Ok(RegionOcrOut {
        region: [x0, y0, cw, ch],
        text,
        lines,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

/// `TextLine` → `LineDto`。commands.rs 里有多处同样的映射，统一到这里。
fn to_line_dto(b: &crate::engine::TextLine) -> LineDto {
    LineDto {
        text: b.text.clone(),
        r#box: box4(&b.points),
        score: b.score,
        det_score: b.det_score,
        rotated: b.rotated,
    }
}

// ---------------------------------------------------------------- 目标检测

/// 单条检测（前端测试台「目标检测」视图用）。
#[derive(Debug, Serialize)]
pub struct DetectHitOut {
    pub r#box: [[f64; 2]; 4],
    pub class_id: usize,
    pub class_name: String,
    pub score: f64,
}

/// 检测结果（与 HTTP `/vision/detect` 结构一致）。
#[derive(Debug, Serialize)]
pub struct DetectOut {
    pub engine: String,
    pub detections: Vec<DetectHitOut>,
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 通用目标检测：前端测试台走这条（少一跳 base64）。
#[tauri::command]
pub async fn vision_detect(
    state: State<'_, Arc<AppState>>,
    image: String,
    engine: Option<String>,
    conf: Option<f32>,
    iou: Option<f32>,
    class_filter: Option<Vec<usize>>,
) -> Result<DetectOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let engine_label = engine
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let level = state
                .cfg
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .level;
            crate::engine::EngineSpec::ppocr(level).key()
        });

    let opts = OcrOptions {
        image,
        level: None,
        device: None,
        use_cls: None,
        engine,
        keyword: None,
        conf,
        iou,
        class_filter,
        detect_engine: None,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let detect = crate::engine::DetectOptions {
        conf_thresh: opts.conf.unwrap_or(0.25),
        iou_thresh: opts.iou.unwrap_or(0.7),
        class_filter: opts.class_filter.clone().unwrap_or_default(),
    };
    let dets = state
        .detect_objects(&opts, img, detect)
        .await
        .map_err(|e| e.message())?;

    let detections: Vec<DetectHitOut> = dets
        .iter()
        .map(|d| DetectHitOut {
            r#box: box4(&d.points),
            class_id: d.class_id,
            class_name: d.class_name.clone(),
            score: d.score,
        })
        .collect();

    Ok(DetectOut {
        engine: engine_label,
        count: detections.len(),
        detections,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------- pipeline

/// 单个区域（检测框 + 框内文字），与 HTTP `/vision/pipeline` 一致。
#[derive(Debug, Serialize)]
pub struct PipelineRegionOut {
    pub r#box: [[f64; 2]; 4],
    pub class_id: usize,
    pub class_name: String,
    pub score: f64,
    pub text: String,
    pub lines: Vec<LineDto>,
}

#[derive(Debug, Serialize)]
pub struct PipelineOut {
    pub detect_engine: String,
    pub ocr_engine: String,
    pub regions: Vec<PipelineRegionOut>,
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 检测 + 识别串联（前端测试台「框内识字」用）。
#[tauri::command]
pub async fn vision_pipeline(
    state: State<'_, Arc<AppState>>,
    image: String,
    detect_engine: Option<String>,
    conf: Option<f32>,
    iou: Option<f32>,
    class_filter: Option<Vec<usize>>,
) -> Result<PipelineOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let opts = OcrOptions {
        image,
        level: None,
        device: None,
        use_cls: None,
        engine: None,
        keyword: None,
        conf,
        iou,
        class_filter,
        detect_engine,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let detect = crate::engine::DetectOptions {
        conf_thresh: opts.conf.unwrap_or(0.25),
        iou_thresh: opts.iou.unwrap_or(0.7),
        class_filter: opts.class_filter.clone().unwrap_or_default(),
    };
    let ocr_level = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .level;

    let regions = state
        .run_pipeline(&opts, img, detect)
        .await
        .map_err(|e| e.message())?;

    let mut text_count = 0usize;
    let region_out: Vec<PipelineRegionOut> = regions
        .iter()
        .map(|(d, lines)| {
            text_count += lines.len();
            PipelineRegionOut {
                r#box: box4(&d.points),
                class_id: d.class_id,
                class_name: d.class_name.clone(),
                score: d.score,
                text: lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
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

    Ok(PipelineOut {
        detect_engine: opts
            .detect_engine
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "yolo".to_string()),
        ocr_engine: crate::engine::EngineSpec::ppocr(ocr_level).key(),
        count: text_count,
        regions: region_out,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------- 扫码

/// 一个识别到的码（与 HTTP `/vision/qrcode` 同构）。
#[derive(Debug, Serialize)]
pub struct BarcodeHitOut {
    pub text: String,
    pub format: String,
    pub points: Vec<[f32; 2]>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Serialize)]
pub struct BarcodeOut {
    pub count: usize,
    pub codes: Vec<BarcodeHitOut>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 表格识别：与 HTTP 的 `/ocr/biaoge` 等价。
///
/// 两条通道必须给出一模一样的结果 —— 界面试跑和脚本联调看到的不一样是最难查的
/// 一类问题，所以核心逻辑都在 `engine::table::recognize_page` 里，这里只做取参
/// 和出参转换。
///
/// 文字识别失败**不让整个调用失败**：结构和坐标已经拿到了，没文字的表格
/// 对「先看结构对不对」这个阶段仍然有用。
#[tauri::command]
pub async fn ocr_biaoge(
    state: State<'_, Arc<AppState>>,
    image: String,
    mode: Option<String>,
    with_text: Option<bool>,
    conf: Option<f32>,
) -> Result<crate::api::dto::TableData, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let Some(tmode) = crate::engine::table::Mode::parse(mode.as_deref().unwrap_or("auto")) else {
        return err("mode 只能是 auto / structure / detect".to_string());
    };
    let c = conf.unwrap_or(0.5).clamp(0.05, 0.95);
    let want_text = with_text.unwrap_or(true);

    let root = state.models_root.clone();
    let page = img.clone();
    let (fallback, mut hits) = tokio::task::spawn_blocking(move || {
        crate::engine::table::recognize_page(&root, &page, Device::Directml, tmode, c)
    })
    .await
    .map_err(|e| format!("表格识别任务异常终止：{e}"))?
    .map_err(|e| e.to_string())?;

    if want_text && hits.iter().any(|h| h.result.is_some()) {
        // 空白 opts = 用当前的 PP-OCR 配置
        let opts = OcrOptions {
            image: String::new(),
            ..Default::default()
        };
        match state.recognize_lines(&opts, img).await {
            Ok(lines) => routes::fill_table_text(&mut hits, &lines),
            Err(e) => tracing::warn!("表格文字识别失败，结构与坐标照常返回: {e:?}"),
        }
    }

    let tables = routes::to_table_dtos(hits);
    Ok(crate::api::dto::TableData {
        mode: if fallback { "structure" } else { tmode.as_str() }.to_string(),
        fallback,
        count: tables.len(),
        tables,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

/// 二维码 / 条码识别：与 HTTP 的 `/vision/qrcode` 等价。
///
/// 与 OCR / 检测引擎都无关，纯 CPU；所以引擎没就绪也能用。
#[tauri::command]
pub async fn vision_qrcode(
    image: String,
    barcode_formats: Option<Vec<String>>,
    try_harder: Option<bool>,
) -> Result<BarcodeOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?.to_rgb8();
    let (w, h) = img.dimensions();

    let bopts = crate::barcode::BarcodeOptions {
        formats: barcode_formats.unwrap_or_default(),
        try_harder: try_harder.unwrap_or(true),
    };
    let codes = tokio::task::spawn_blocking(move || {
        crate::barcode::decode_barcodes(&img, &bopts)
    })
    .await
    .map_err(|e| format!("条码识别任务异常终止: {e}"))??;

    let items: Vec<BarcodeHitOut> = codes
        .iter()
        .map(|c| BarcodeHitOut {
            text: c.text.clone(),
            format: c.format.clone(),
            points: c.points.iter().map(|&(x, y)| [x, y]).collect(),
            x: c.bounds[0],
            y: c.bounds[1],
            width: c.bounds[2],
            height: c.bounds[3],
        })
        .collect();

    Ok(BarcodeOut {
        count: items.len(),
        codes: items,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

// ---------------------------------------------------------------- 单字 JSON

/// 单个字的框与置信度。
#[derive(Debug, Serialize)]
pub struct CharOut {
    pub ch: String,
    /// 原图四点，顺序 `[左上, 右上, 右下, 左下]`
    pub r#box: [[f64; 2]; 4],
    pub score: f64,
    pub index: usize,
    pub t_start: usize,
    pub t_end: usize,
}

/// 单行（含逐字）。
#[derive(Debug, Serialize)]
pub struct DanziLineOut {
    pub text: String,
    pub r#box: [[f64; 2]; 4],
    pub score: f64,
    pub det_score: f64,
    pub rotated: bool,
    pub frames: usize,
    pub chars: Vec<CharOut>,
}

/// 单字级识别结果（与 HTTP `/ocr/danzijson` 结构一致）。
#[derive(Debug, Serialize)]
pub struct DanziOut {
    pub text: String,
    pub lines: Vec<DanziLineOut>,
    pub char_count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

fn box4(p: &[crate::ocr::geometry::Pt; 4]) -> [[f64; 2]; 4] {
    [[p[0].0, p[0].1], [p[1].0, p[1].1], [p[2].0, p[2].1], [p[3].0, p[3].1]]
}

/// 逐字识别：在整行结果之外，额外给出每个字在原图中的四边形。
#[tauri::command]
pub async fn ocr_danzijson(
    state: State<'_, Arc<AppState>>,
    image: String,
    use_cls: Option<bool>,
    engine: Option<String>,
) -> Result<DanziOut, String> {
    let t0 = Instant::now();
    let bytes = decode_base64_image(&image)?;
    let img = decode_image(&bytes)?;
    let (w, h) = (img.width(), img.height());

    let opts = OcrOptions {
        image,
        level: None,
        device: None,
        use_cls,
        engine,
        keyword: None,
        conf: None,
        iou: None,
        class_filter: None,
        detect_engine: None,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let details = state
        .recognize_details(&opts, img)
        .await
        .map_err(|e| e.message())?;

    let mut char_count = 0usize;
    let lines: Vec<DanziLineOut> = details
        .into_iter()
        .map(|l| {
            char_count += l.chars.len();
            DanziLineOut {
                text: l.text,
                r#box: box4(&l.points),
                score: l.score,
                det_score: l.det_score,
                rotated: l.rotated,
                frames: l.frames,
                chars: l
                    .chars
                    .into_iter()
                    .map(|c| CharOut {
                        ch: c.ch,
                        r#box: box4(&c.points),
                        score: c.score,
                        index: c.index,
                        t_start: c.t_start,
                        t_end: c.t_end,
                    })
                    .collect(),
            }
        })
        .collect();

    let text = lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    Ok(DanziOut {
        text,
        lines,
        char_count,
        width: w,
        height: h,
        elapsed_ms: t0.elapsed().as_millis(),
    })
}

/// 引擎健康状态。
#[tauri::command]
pub fn get_health(state: State<'_, Arc<AppState>>) -> HealthDto {
    health_dto(&state)
}

/// 由引擎状态构造 HealthDto（引擎未加载时 status 为 `no_model`）。
fn health_dto(state: &AppState) -> HealthDto {
    let cfg = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match state.health() {
        Some(info) => HealthDto {
            version: crate::version::VERSION.to_string(),
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
        },
        None => HealthDto {
            version: crate::version::VERSION.to_string(),
            status: "no_model".to_string(),
            engine: crate::engine::EngineSpec::ppocr(cfg.level).key(),
            model: format!("{:?}", cfg.level).to_lowercase(),
            device: format!("{:?}", cfg.device).to_lowercase(),
            actual_device: "-".to_string(),
            det_loaded: false,
            rec_loaded: false,
            cls_loaded: false,
            dict_size: 0,
            port: cfg.port,
            uptime_sec: state.started_at.elapsed().as_secs(),
        },
    }
}

/// 列出可用引擎与能力（前端设置页 / 测试台用）。
///
/// 与 `/engine/list` 同一份逻辑：只扫目录，不加载模型。
#[tauri::command]
pub fn list_engines(state: State<'_, Arc<AppState>>) -> EngineListOut {
    let cfg = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let engines = state.engine_status_list();

    EngineListOut {
        models_root: state.models_root.display().to_string(),
        default_engine: crate::engine::EngineSpec::ppocr(cfg.level).key(),
        engines: engines
            .iter()
            .map(crate::api::dto::EngineItemDto::from)
            .collect(),
    }
}

/// 设置页「模型」卡片：提示找图（YOLOE）资产的安装状态。
///
/// 与 `list_engines` 的区别：那份表只扫 **OCR 引擎**（ppocr/ddddoc/yolo），
/// 提示找图的两件套走另一套 `discover_assets`，不在这里露出来，
/// 设置页就只能显示「引擎清单」而说不清找图模型装没装。
/// 只扫目录、不加载模型 —— 与引擎清单同一纪律。
#[derive(Debug, Serialize)]
pub struct PromptAssetsOut {
    /// 检测器在不在（整套功能可用的判据）
    pub detector: bool,
    /// 视觉提示编码器：图找图和「按名字找」都走它
    pub vpe: bool,
    /// 资产目录约定（缺东西时告诉用户该放哪）
    pub dir: String,
}

#[tauri::command]
pub fn prompt_assets_status(state: State<'_, Arc<AppState>>) -> PromptAssetsOut {
    use crate::engine::prompt::discover_assets;
    let dir = state.models_root.join("yoloe").display().to_string();
    let Some(a) = discover_assets(&state.models_root) else {
        return PromptAssetsOut {
            detector: false,
            vpe: false,
            dir,
        };
    };
    PromptAssetsOut {
        detector: true,
        vpe: a.vpe.is_some(),
        dir,
    }
}
// ---------------------------------------------------------------- 检测模型

/// 设置页「目标检测模型」卡片的一条记录。
#[derive(Debug, Serialize)]
pub struct DetectModelOut {
    /// 模型 id（`yolo:<id>` 的后半段，也是下拉里显示的名字）
    pub id: String,
    /// 下拉/接口里用的完整引擎名
    pub engine: String,
    /// onnx 完整路径
    pub path: String,
    /// 文件名（界面上短一点好显示）
    pub file_name: String,
    /// true = 用户从任意目录加进来的（配置里记着路径），false = 在 models 目录里
    pub external: bool,
}

fn detect_models_out(state: &AppState) -> Vec<DetectModelOut> {
    let cfg = state.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
    crate::engine::catalog::list_yolo_models(&state.models_root, &cfg.detect_models)
        .into_iter()
        .enumerate()
        .map(|(i, m)| DetectModelOut {
            // 第一项 = 默认模型，引擎名与 /engine/list 对齐成 `yolo`（不带 id）
            engine: if i == 0 {
                crate::engine::EngineSpec::yolo().key()
            } else {
                crate::engine::EngineSpec::yolo_model(&m.id).key()
            },
            file_name: m
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string(),
            path: m.path.display().to_string(),
            id: m.id,
            external: m.external,
        })
        .collect()
}

/// 列出所有可用的检测模型（models 目录里的 + 用户自己加的）。
///
/// 与 `/engine/list` 里 `family == "yolo"` 的条目是同一批，但这里带**路径**，
/// 设置页要显示「这个模型到底加载的是哪个文件」。
#[tauri::command]
pub fn detect_models(state: State<'_, Arc<AppState>>) -> Vec<DetectModelOut> {
    detect_models_out(&state)
}

/// 弹系统文件选择框挑一个 onnx；选中就记进配置（**不复制文件**）。
///
/// 取消选择时静默返回当前列表，前端不需要区分「取消」和「成功」。
#[tauri::command]
pub async fn pick_detect_model(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<DetectModelOut>, String> {
    // 模态对话框会阻塞线程，必须离开 async 线程池
    let picked = tauri::async_runtime::spawn_blocking(pick_onnx_dialog)
        .await
        .map_err(|e| format!("文件选择框异常: {e}"))?;
    if let Some(path) = picked {
        add_detect_model_path(&state, &path)?;
    }
    Ok(detect_models_out(&state))
}

/// 直接按路径加一个模型（拖拽进来的文件也走这里）。
#[tauri::command]
pub fn add_detect_model(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Vec<DetectModelOut>, String> {
    add_detect_model_path(&state, &path)?;
    Ok(detect_models_out(&state))
}

/// 从配置里移除一个外部模型（只删记录，不动磁盘上的文件）。
#[tauri::command]
pub fn remove_detect_model(
    state: State<'_, Arc<AppState>>,
    path: String,
) -> Result<Vec<DetectModelOut>, String> {
    let want = canonical_or_raw(&path);
    let snapshot = {
        let mut guard = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        guard
            .detect_models
            .retain(|p| canonical_or_raw(p) != want);
        guard.clone()
    };
    snapshot
        .save(&state.data_dir)
        .map_err(|e| format!("保存配置失败: {e}"))?;
    Ok(detect_models_out(&state))
}

fn canonical_or_raw(p: &str) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| std::path::PathBuf::from(p))
}

fn add_detect_model_path(state: &AppState, path: &str) -> Result<(), String> {
    let p = std::fs::canonicalize(path).map_err(|e| format!("打不开这个文件：{path}（{e}）"))?;
    if !p.is_file() {
        return Err(format!("不是一个文件：{}", p.display()));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext != "onnx" {
        return Err(format!(
            "只支持 .onnx 模型文件，选到的是 .{ext}（.pt 请先用 yolo export 导出）"
        ));
    }
    let snapshot = {
        let mut guard = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        // 同一份文件不重复加（用户可能既放在 models 目录又手动选过一次）
        if guard
            .detect_models
            .iter()
            .any(|x| canonical_or_raw(x) == p)
        {
            return Ok(());
        }
        guard.detect_models.push(p.display().to_string());
        guard.clone()
    };
    snapshot
        .save(&state.data_dir)
        .map_err(|e| format!("保存配置失败: {e}"))
}

/// 系统文件选择框（只挑 .onnx）。
///
/// 用 `GetOpenFileNameW` 而不是引入 tauri-plugin-dialog：这台机器上只要一个
/// 「选模型」的场景，多一个插件（前端依赖 + 权限声明 + 打包体积）不划算。
/// owner 取前台窗口——按钮是用户点的，此刻前台就是主窗口。
#[cfg(windows)]
fn pick_onnx_dialog() -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
        OPEN_FILENAME_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // 过滤器是「名称\0通配\0名称\0通配\0\0」，末尾两个 0 结束
    let filter: Vec<u16> = "ONNX 模型 (*.onnx)\0*.onnx\0所有文件 (*.*)\0*.*\0"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title = wide("选择目标检测模型");
    let def_ext = wide("onnx");
    let mut file = vec![0u16; 2048];

    let mut ofn = OPENFILENAMEW::default();
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = unsafe { GetForegroundWindow() };
    ofn.lpstrFilter = PCWSTR::from_raw(filter.as_ptr());
    ofn.lpstrFile = PWSTR::from_raw(file.as_mut_ptr());
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrTitle = PCWSTR::from_raw(title.as_ptr());
    ofn.lpstrDefExt = PCWSTR::from_raw(def_ext.as_ptr());
    ofn.Flags = OPEN_FILENAME_FLAGS(
        OFN_FILEMUSTEXIST.0 | OFN_PATHMUSTEXIST.0 | OFN_NOCHANGEDIR.0,
    );

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return None; // 用户取消
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(String::from_utf16_lossy(&file[..len]))
}

#[cfg(not(windows))]
fn pick_onnx_dialog() -> Option<String> {
    None
}

/// 当前配置。
#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> AppConfig {
    state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 保存配置并热生效：端口变更会重启服务，档位/设备变更会重载引擎。
#[tauri::command]
pub async fn save_config(
    state: State<'_, Arc<AppState>>,
    mut cfg: AppConfig,
) -> Result<AppConfig, String> {
    if cfg.port == 0 {
        return err("端口不能为 0".to_string());
    }
    // webview_compat 由后端自己维护（白屏看门狗自动置位），前端不感知也不该覆盖它
    // detect_models 同理：设置页用专用命令增删，前端保存配置时带的是旧快照，
    // 若让它覆盖，刚「添加模型」的记录会被下一次保存冲掉
    {
        let cur = state.cfg.read().unwrap_or_else(|e| e.into_inner());
        cfg.webview_compat = cur.webview_compat;
        cfg.detect_models = cur.detect_models.clone();
    }
    cfg.save(&state.data_dir).map_err(|e| format!("保存配置失败: {e}"))?;

    let old = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    {
        let mut guard = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        *guard = cfg.clone();
    }

    // 档位/设备变更 -> 重载引擎
    if cfg.level != old.level || cfg.device != old.device {
        state
            .load_engine_blocking(Some(cfg.level), Some(cfg.device), None)
            .map_err(|e| e)?;
    }
    // 端口变更 -> 重启服务
    if cfg.port != old.port {
        stop_server(&state);
        server_start(state).await?;
    }
    Ok(cfg)
}

/// 切换模型档位（重载引擎）。
#[tauri::command]
pub async fn set_level(
    state: State<'_, Arc<AppState>>,
    level: String,
) -> Result<HealthDto, String> {
    let level = ModelLevel::from_str(&level).ok_or_else(|| format!("未知档位: {level}"))?;
    state.reload_engine(Some(level), None, None).await?;
    {
        let mut guard = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        guard.level = level;
    }
    Ok(health_dto(&state))
}

/// 切换推理设备（重载引擎）。
#[tauri::command]
pub async fn set_device(
    state: State<'_, Arc<AppState>>,
    device: String,
) -> Result<HealthDto, String> {
    let device = Device::from_str(&device).ok_or_else(|| format!("未知设备: {device}"))?;
    state.reload_engine(None, Some(device), None).await?;
    {
        let mut guard = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        guard.device = device;
    }
    Ok(health_dto(&state))
}

/// HTTP 服务是否在跑。
#[tauri::command]
pub fn server_status(state: State<'_, Arc<AppState>>) -> bool {
    state.is_running()
}

/// 在后台启动 HTTP 服务。托盘菜单和 Tauri 命令共用这段逻辑。
pub fn start_server_background(state: &Arc<AppState>) {
    let st = Arc::clone(state);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("创建 runtime 失败: {e}");
                return;
            }
        };
        rt.block_on(async move {
            if let Err(e) = crate::api::serve(st).await {
                tracing::error!("HTTP 服务退出: {e}");
            }
        });
    });
}

/// 启动 HTTP 服务（已在运行时直接返回端口）。
#[tauri::command]
pub async fn server_start(state: State<'_, Arc<AppState>>) -> Result<u16, String> {
    if state.is_running() {
        return Ok(state.port());
    }
    let port = state.port();
    start_server_background(&state);

    // 等监听真正就绪（serve 里绑定成功后才登记 shutdown）
    for _ in 0..100 {
        if state.is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if !state.is_running() {
        return err(format!("服务启动失败：127.0.0.1:{port} 可能被占用"));
    }
    Ok(port)
}

/// 停止 HTTP 服务。
#[tauri::command]
pub fn server_stop(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    stop_server(&state);
    Ok(())
}

/// 模型根目录（供前端展示）。
#[tauri::command]
pub fn models_root(state: State<'_, Arc<AppState>>) -> String {
    state.models_root.display().to_string()
}

// ---------------------------------------------------------------- 开机自启

/// 开机自启当前是否开启。
///
/// 状态只存在操作系统里（注册表 Run 项），**不写进 config.json**：
/// 两处都存就会出现「配置文件说开着、系统里其实没有」的分裂状态，
/// 用户还得自己判断哪个是真的。
#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch()
        .is_enabled()
        .map_err(|e| format!("读取开机自启状态失败: {e}"))
}

/// 打开/关闭开机自启，返回操作后的真实状态。
#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let r = if enabled { al.enable() } else { al.disable() };
    r.map_err(|e| {
        format!(
            "{}失败: {e}",
            if enabled { "开启开机自启" } else { "关闭开机自启" }
        )
    })?;
    // 回读而不是直接返回 enabled：注册表可能因权限被拒，如实反映才可信
    al.is_enabled()
        .map_err(|e| format!("回读开机自启状态失败: {e}"))
}

// ---------------------------------------------------------------- 找图

/// 前端发来的找图参数。字段与 HTTP 的 `FindOptions` 一一对应，额外多一个
/// `source`（来源）和 `mode`（精准/容错）—— 这两个在 HTTP 侧是由路径决定的。
#[derive(Debug, serde::Deserialize)]
pub struct FindArgs {
    /// `image` / `screen` / `window`
    pub source: String,
    /// `exact`（默认）或 `loose`
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub template_file: Option<String>,
    /// 模板掩码 base64（可省略）。**白（≥ 阈值）= 参与匹配**
    #[serde(default)]
    pub mask: Option<String>,
    /// 掩码文件绝对路径（与 `mask` 二选一）
    #[serde(default)]
    pub mask_file: Option<String>,
    /// 掩码二值化阈值，缺省 128
    #[serde(default)]
    pub mask_threshold: Option<u8>,
    #[serde(default)]
    pub hwnd: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    #[serde(default)]
    pub min_score: Option<f64>,
    #[serde(default)]
    pub max_results: Option<usize>,
}

impl FindArgs {
    fn into_options(self) -> crate::api::dto::FindOptions {
        crate::api::dto::FindOptions {
            image: self.image,
            template: self.template,
            template_file: self.template_file,
            mask: self.mask,
            mask_file: self.mask_file,
            mask_threshold: self.mask_threshold,
            hwnd: self.hwnd,
            title: self.title,
            region: self.region,
            min_score: self.min_score,
            max_results: self.max_results,
            // 语义找图专有字段由 `find_semantic` 单独填，模板找图这条路径不用
            ..Default::default()
        }
    }
}

/// 可见窗口信息（前端选择「截哪个窗口」用）。
#[derive(Debug, Serialize)]
pub struct WindowDto {
    pub hwnd: i64,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 截图预览结果。带上区域原点，前端才能把屏幕绝对坐标换算成图内坐标。
#[derive(Debug, Serialize)]
pub struct ScreenPreview {
    /// PNG 的 base64
    pub image: String,
    /// 截图区域在屏幕上的左上角
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 找图结果 + 本次实际用于匹配的搜索图。
///
/// 为什么要回传搜索图：屏幕是活的，若前端先单独截一张图当背景、再调找图，
/// 两次截屏之间画面可能已经变了，命中框会画在过时的背景上。由同一次匹配
/// 返回它自己看到的像素，框与背景必然对齐。
#[derive(Debug, Serialize)]
pub struct FindOut {
    #[serde(flatten)]
    pub data: crate::api::dto::FindData,
    /// 搜索图的 PNG base64；`source=image` 时不返回（调用方本来就有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// 枚举可见窗口。
#[tauri::command]
pub fn list_windows() -> Vec<WindowDto> {
    capture::list_windows()
        .into_iter()
        .map(|w| WindowDto {
            hwnd: w.hwnd as i64,
            title: w.title,
            x: w.rect.x,
            y: w.rect.y,
            width: w.rect.width,
            height: w.rect.height,
        })
        .collect()
}

/// 截一张屏幕预览图，返回 PNG 的 base64（前端做预览用）。
#[tauri::command]
pub async fn screen_preview(region: Option<[i32; 4]>) -> Result<ScreenPreview, String> {
    let region = region.map(|[x, y, w, h]| capture::ScreenRegion {
        x,
        y,
        width: w,
        height: h,
    });
    let img = tokio::task::spawn_blocking(move || capture::capture_region(region))
        .await
        .map_err(|e| format!("截图任务异常: {e}"))?
        .map_err(|e| format!("截图失败: {e}"))?;
    let (x, y) = match region {
        Some(r) => (r.x, r.y),
        None => {
            let vs = capture::virtual_screen();
            (vs.x, vs.y)
        }
    };
    let (width, height) = img.dimensions();
    Ok(ScreenPreview {
        image: rgb_to_png_b64(&img)?,
        x,
        y,
        width,
        height,
    })
}

fn rgb_to_png_b64(img: &image::RgbImage) -> Result<String, String> {
    use base64::Engine as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("PNG 编码失败: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

/// 找图：与 HTTP 的 `/ocr/tuzhaotu*|jubingzhaotu*|pingmuzhaotu*` 等价，
/// 由 `source` 选择搜索图来源，`mode` 选择阈值预设。
#[tauri::command]
pub async fn find_image(args: FindArgs) -> Result<FindOut, String> {
    let t0 = Instant::now();
    let source = FindSource::parse(&args.source)
        .ok_or_else(|| format!("未知来源 {:?}（可选 image/screen/window）", args.source))?;
    let mode = match args.mode.as_deref().map(str::trim) {
        Some("loose") | Some("rongcuo") | Some("容错") => FindMode::Loose,
        _ => FindMode::Exact,
    };
    let opts = args.into_options();

    // 模板必须先校验（错误提示优先级最高，和 HTTP 层保持一致）
    let tpl = routes::load_template(&opts, None).map_err(|e| e.message())?;
    let (tw, th) = tpl.dimensions();
    let mask = routes::load_mask(&opts, None).map_err(|e| e.message())?;
    let (hay, ox, oy) =
        routes::acquire_haystack(source, &opts, None).map_err(|e| e.message())?;

    let mopts = MatchOptions {
        min_score: opts.min_score.unwrap_or(mode.default_min_score()),
        max_results: opts.max_results.unwrap_or(1),
    };
    // 只有屏幕/窗口来源需要回传截图（图片来源调用方本来就有原图）
    let need_preview = source != FindSource::Image;
    let (hits, preview) = tokio::task::spawn_blocking(move || {
        let hits = matcher::find_template_masked(&hay, &tpl, mask.as_ref(), &mopts)?;
        let preview = if need_preview {
            Some(rgb_to_png_b64(&hay)?)
        } else {
            None
        };
        Ok::<_, String>((hits, preview))
    })
    .await
    .map_err(|e| format!("匹配任务异常终止: {e}"))??;

    Ok(FindOut {
        data: routes::to_find_data(&hits, (ox, oy), (tw, th), source, t0),
        preview,
    })
}

/// 语义找图参数。与 `FindArgs` 的差别：**没有模板相关字段**（语义找图不用模板），
/// 多了检测相关的 `engine` / `conf` / `class_filter`。
#[derive(Debug, serde::Deserialize)]
pub struct SemanticFindArgs {
    /// `image` / `screen` / `window`
    pub source: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub hwnd: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 用哪个检测模型：`yolo`（缺省 = 目录里第一个）/ `yolo:<模型id>`
    #[serde(default)]
    pub engine: Option<String>,
    /// 检测置信度下限，缺省 0.25
    #[serde(default)]
    pub conf: Option<f32>,
    /// 只保留这些 class_id；空 = 全部
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
    /// 最多返回几个命中，默认 1
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// 语义找图：与 HTTP 的 `/ocr/yuyizhaotu` 等价，出参与 `find_image` 同构。
///
/// 用检测模型（YOLOE-26 这类）直接找目标，不需要准备模板图。代价是只认得
/// 模型见过的类别；换目标要换模型，不是换模板。
#[tauri::command]
pub async fn find_semantic(
    state: State<'_, Arc<AppState>>,
    args: SemanticFindArgs,
) -> Result<FindOut, String> {
    let t0 = Instant::now();
    let source = FindSource::parse(&args.source)
        .ok_or_else(|| format!("未知来源 {:?}（可选 image/screen/window）", args.source))?;

    let conf = args.conf.unwrap_or(0.25);
    if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
        return Err(format!("conf 必须在 0..1，收到 {conf}"));
    }
    let class_filter = args.class_filter.clone().unwrap_or_default();

    let opts = crate::api::dto::FindOptions {
        image: args.image,
        hwnd: args.hwnd,
        title: args.title,
        region: args.region,
        conf: Some(conf),
        class_filter: Some(class_filter.clone()),
        ..Default::default()
    };
    let (hay, ox, oy) = routes::acquire_haystack(source, &opts, None).map_err(|e| e.message())?;

    // 缺省 `yolo` = 目录里排第一的检测模型；回显规范化后的引擎名（理由同
    // `/vision/detect`：`yolo:v8` 这类旧写法实际生效的是默认模型）。
    let engine_raw = args
        .engine
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "yolo".to_string());
    let engine_label = crate::engine::EngineSpec::parse(&engine_raw)
        .map(|s| s.key())
        .unwrap_or_else(|_| engine_raw.clone());

    let ocr_opts = OcrOptions {
        image: String::new(),
        level: None,
        device: None,
        use_cls: None,
        engine: Some(engine_raw),
        keyword: None,
        conf: Some(conf),
        iou: None,
        class_filter: Some(class_filter.clone()),
        detect_engine: None,
        region: None,
        barcode_formats: None,
        try_harder: None,
        mode: None,
        with_text: None,
    };
    let detect = crate::engine::DetectOptions {
        conf_thresh: conf,
        // 语义找图不暴露 iou：NMS-free 端到端模型（v10/v26/yoloe）不做 NMS，
        // 暴露一个只在部分模型上生效的参数比不暴露更让人困惑。
        iou_thresh: 0.7,
        class_filter,
    };

    // 只有屏幕/窗口来源需要回传截图（图片来源调用方本来就有原图）
    let need_preview = source != FindSource::Image;
    let preview_src = if need_preview { Some(hay.clone()) } else { None };

    let dets = state
        .detect_objects(&ocr_opts, image::DynamicImage::ImageRgb8(hay), detect)
        .await
        .map_err(|e| e.message())?;

    let preview = match preview_src {
        Some(img) => Some(rgb_to_png_b64(&img)?),
        None => None,
    };

    Ok(FindOut {
        data: routes::to_find_data_from_dets(
            &dets,
            (ox, oy),
            source,
            Some(engine_label),
            args.max_results.unwrap_or(1),
            t0,
        ),
        preview,
    })
}

/// 运行时提示找图参数。与 `SemanticFindArgs` 的差别：语义找图的目标是**模型固化的
/// 类别**，这里的目标由调用方**当场给**——给图（`ref_image` + `ref_boxes`）或给词
/// （`words`，教过的名字），换目标只换参数、不换模型。
#[derive(Debug, serde::Deserialize)]
pub struct PromptFindArgs {
    /// `image` / `screen` / `window`
    pub source: String,
    /// `visual`（图找图，缺省）或 `ref`（按教过的名字找）
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub hwnd: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 参考图（PNG/JPG 的 base64），`mode=visual` 必填
    #[serde(default)]
    pub ref_image: Option<String>,
    /// 参考框 `[x1,y1,x2,y2]`（原图像素坐标），`mode=visual` 必填，可多个
    #[serde(default)]
    pub ref_boxes: Option<Vec<[f32; 4]>>,
    /// 每个参考框归属的「槽」；同槽的多个框取**并集**当作一个提示。缺省全 0
    #[serde(default)]
    pub ref_classes: Option<Vec<usize>>,
    /// 槽对应的名字（仅用于回显，不影响匹配）。缺省用「目标1/目标2…」
    #[serde(default)]
    pub ref_names: Option<Vec<String>>,
    /// 名字列表，`mode=ref` 必填（必须先在参考库里教过）
    #[serde(default)]
    pub words: Option<Vec<String>>,
    #[serde(default)]
    pub conf: Option<f32>,
    /// 只保留这些槽（`class_filter` 在这里是**提示槽下标**，不是模型类别）
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
    #[serde(default)]
    pub max_results: Option<usize>,
}

/// 运行时提示找图：与 HTTP 的 `/vision/prompt` 等价，出参与 `find_image` 同构。
///
/// 与 `find_semantic` 的区别：那个用**烘焙过提示**的模型（换目标要重导 ONNX），
/// 这个把提示当**额外输入**（换目标只换参数）。代价是要多带两个编码器
/// （视觉 12MB + 文本 250MB 级），首次调用才加载。
#[tauri::command]
pub async fn find_prompt(
    state: State<'_, Arc<AppState>>,
    args: PromptFindArgs,
) -> Result<FindOut, String> {
    let t0 = Instant::now();
    let source = FindSource::parse(&args.source)
        .ok_or_else(|| format!("未知来源 {:?}（可选 image/screen/window）", args.source))?;
    let mode = args
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("visual")
        .to_ascii_lowercase();

    let conf = args.conf.unwrap_or(0.25);
    if !conf.is_finite() || !(0.0..=1.0).contains(&conf) {
        return Err(format!("conf 必须在 0..1，收到 {conf}"));
    }

    // 提示先组好再取目标图：提示参数错的时候不该先去截一次屏
    let input = match mode.as_str() {
        "visual" => {
            let b64 = args
                .ref_image
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| "mode=visual 需要参考图 ref_image（base64）".to_string())?;
            let raw = crate::api::dto::decode_base64_image(b64)?;
            let reference = crate::api::dto::decode_image(&raw)?;

            let boxes = args.ref_boxes.clone().unwrap_or_default();
            if boxes.is_empty() {
                return Err("mode=visual 需要至少一个参考框 ref_boxes".to_string());
            }
            for b in &boxes {
                if !b.iter().all(|v| v.is_finite()) {
                    return Err(format!("参考框 {b:?} 含非有限值"));
                }
                if b[2] <= b[0] || b[3] <= b[1] {
                    return Err(format!("参考框 {b:?} 宽或高不为正（要 x2>x1 且 y2>y1）"));
                }
            }
            let classes = match args.ref_classes.clone() {
                Some(v) if !v.is_empty() => {
                    if v.len() != boxes.len() {
                        return Err(format!(
                            "ref_classes 有 {} 项，ref_boxes 有 {} 个框，数量要一致",
                            v.len(),
                            boxes.len()
                        ));
                    }
                    v
                }
                _ => vec![0usize; boxes.len()],
            };
            crate::engine::prompt::PromptInput::Visual {
                reference,
                boxes,
                classes,
                names: args.ref_names.clone().unwrap_or_default(),
            }
        }
        "ref" => {
            let names: Vec<String> = args
                .words
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|w| w.trim().to_string())
                .filter(|w| !w.is_empty())
                .collect();
            if names.is_empty() {
                return Err(
                    "mode=ref 需要 words（教过的名字，至少一个；先用「教它认」教一次）".to_string(),
                );
            }
            crate::engine::prompt::PromptInput::Refs { names }
        }
        "text" => {
            return Err(
                "mode=text（按词描述找图）已移除：在界面元素上的分数只有 0.1~0.2，等于噪声。\
                 改用 mode=ref，按「教过的名字」找"
                    .to_string(),
            )
        }
        other => return Err(format!("未知 mode {other:?}（可选 visual / ref）")),
    };

    let find_opts = crate::api::dto::FindOptions {
        image: args.image.clone(),
        hwnd: args.hwnd,
        title: args.title.clone(),
        region: args.region,
        ..Default::default()
    };
    let (hay, ox, oy) =
        routes::acquire_haystack(source, &find_opts, None).map_err(|e| e.message())?;

    let detect = crate::engine::DetectOptions {
        conf_thresh: conf,
        // 端到端头不做 NMS，暴露 iou 只会让人以为它生效了（同语义找图）
        iou_thresh: 0.7,
        class_filter: args.class_filter.clone().unwrap_or_default(),
    };

    // 只有屏幕/窗口来源需要回传截图（图片来源调用方本来就有原图）
    let need_preview = source != FindSource::Image;
    let preview_src = if need_preview { Some(hay.clone()) } else { None };

    let dets = state
        .prompt_find(input, image::DynamicImage::ImageRgb8(hay), detect)
        .await
        .map_err(|e| e.message())?;

    let preview = match preview_src {
        Some(img) => Some(rgb_to_png_b64(&img)?),
        None => None,
    };

    Ok(FindOut {
        data: routes::to_find_data_from_dets(
            &dets,
            (ox, oy),
            source,
            Some("yoloe-prompt".to_string()),
            args.max_results.unwrap_or(8),
            t0,
        ),
        preview,
    })
}

// 命令注册集中在 `lib.rs` 的 `invoke_handler` 中，用 `tauri::generate_handler!`
// 引用本模块的各个函数（宏接受路径，无需额外包装）。

// ---------------------------------------------------------------- 日志

// ---------------------------------------------------------------- AI 操控

/// AI 接口的状态（给设置页面板用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AiStatusOut {
    pub enabled: bool,
    pub port: u16,
    pub base_url: String,
    /// 连接码。面板上要能复制、能重置。
    pub token: String,
    /// 「复制给 AI 的提示词」，整段粘给本机 AI 就能用。
    pub prompt: String,
    /// 服务当前是否在监听（接口开着但服务没起的话，AI 也调不通）。
    pub running: bool,
}

#[tauri::command]
pub fn ai_status(state: State<'_, Arc<AppState>>) -> AiStatusOut {
    let (enabled, token, port) = {
        let cfg = state.cfg.read().unwrap_or_else(|e| e.into_inner());
        (cfg.ai_enabled, cfg.ai_token.clone(), cfg.port)
    };
    let base = format!("http://127.0.0.1:{port}");
    AiStatusOut {
        enabled,
        port,
        base_url: base.clone(),
        token: token.clone(),
        prompt: crate::api::ai::prompt(&base, &token),
        running: state.is_running(),
    }
}

/// 开关 AI 接口。它只是让 `/ai/v1/*` 要不要拒绝请求，不用重启服务。
#[tauri::command]
pub fn ai_set_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> AiStatusOut {
    {
        let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        cfg.ai_enabled = enabled;
        if let Err(e) = cfg.save(&state.data_dir) {
            tracing::warn!("AI 开关没能落盘：{e}");
        }
    }
    // 重置开关时清掉幂等缓存，免得上一轮的 request_id 串味
    if !enabled {
        let mut idem = state.ai_idem.lock().unwrap_or_else(|e| e.into_inner());
        idem.cache.clear();
        idem.order.clear();
        idem.inflight.clear();
    }
    tracing::info!("AI 操控接口已{}", if enabled { "开启" } else { "关闭" });
    ai_status(state)
}

/// 重置连接码：旧码立刻失效。
#[tauri::command]
pub fn ai_reset_token(state: State<'_, Arc<AppState>>) -> AiStatusOut {
    {
        let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
        cfg.ai_token = crate::api::ai::new_ai_token();
        if let Err(e) = cfg.save(&state.data_dir) {
            tracing::warn!("AI 连接码没能落盘：{e}");
        }
        // 同上：换码就当新一轮，幂等缓存清掉
        let mut idem = state.ai_idem.lock().unwrap_or_else(|e| e.into_inner());
        idem.cache.clear();
        idem.order.clear();
        idem.inflight.clear();
    }
    tracing::info!("AI 操控接口的连接码已重置");
    ai_status(state)
}

/// 中文参考库里已经教过的目标（给设置页展示，删不掉的东西用户会疑惑）。
#[tauri::command]
pub fn ai_targets(state: State<'_, Arc<AppState>>) -> Vec<String> {
    let refs = state.refs.lock().unwrap_or_else(|e| e.into_inner());
    refs.list().iter().map(|e| e.name.clone()).collect()
}

/// 最近的后端日志（环形缓冲快照）。
///
/// 前端进「运行日志」页时先调这个补历史，之后靠 `log-lines` 事件增量接收。
#[tauri::command]
pub fn get_logs() -> Vec<crate::logbuf::LogLine> {
    crate::logbuf::snapshot()
}

// ---------------------------------------------------------------- 文件

/// 读取本地图片并统一转成 PNG 的 base64。
///
/// 拖拽进入窗口时，Tauri 只把**路径**交给我们（DOM 拿不到 File 对象），
/// 而前端需要 base64 才能做缩略图和发给识别接口。这里顺手统一成 PNG，
/// 前端就不用按扩展名猜 MIME。
#[tauri::command]
pub async fn read_image_file(path: String) -> Result<String, String> {
    /// 单张图的硬上限。再大基本是误拖（比如把整个文件夹里的 RAW 扔进来）。
    const MAX_BYTES: u64 = 64 * 1024 * 1024;

    let p = std::path::PathBuf::from(&path);
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const ALLOWED: [&str; 6] = ["png", "jpg", "jpeg", "bmp", "webp", "gif"];
    if !ALLOWED.contains(&ext.as_str()) {
        return err(format!("不支持的文件类型: .{ext}（支持 png/jpg/bmp/webp/gif）"));
    }

    let meta = tokio::fs::metadata(&p)
        .await
        .map_err(|e| format!("读不到文件: {e}"))?;
    if meta.len() > MAX_BYTES {
        return err(format!(
            "文件太大（{} MB），上限 64 MB",
            meta.len() / (1024 * 1024)
        ));
    }

    let bytes = tokio::fs::read(&p)
        .await
        .map_err(|e| format!("读取失败: {e}"))?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("不是有效的图片: {e}"))?;

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("转 PNG 失败: {e}"))?;

    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(buf.into_inner()))
}

// ─────────────────────────── 版本与更新 ───────────────────────────

/// 检查是否有新版本。
///
/// 网络不通 / 更新源还没发版都**不算错误**：原因放在返回值的 `error` 字段，
/// `has_update` 为 false。启动时自动检查要**静默**（没网不该弹红字），
/// 只有用户主动点「检查更新」时才把 `error` 显示出来。
#[tauri::command]
pub async fn update_check() -> crate::updater::UpdateCheck {
    match tauri::async_runtime::spawn_blocking(crate::updater::check).await {
        Ok(v) => v,
        Err(e) => crate::updater::UpdateCheck {
            current: crate::version::VERSION.to_string(),
            latest: crate::version::VERSION.to_string(),
            has_update: false,
            notes: String::new(),
            url: String::new(),
            size: 0,
            sha256: None,
            error: Some(format!("检查更新任务异常：{e}")),
        },
    }
}

/// 下载并安装更新。成功后**本进程退出**，由临时目录里的助手替换文件并拉起新版。
///
/// 为什么这里再查一次、而不是信前端传来的 url：前端那份可能是几小时前查的，
/// 期间可能已经发了更新的版本；「下载哪个包」必须与服务端当前指针一致，
/// 否则会装出一个夹在中间的版本。
#[tauri::command]
pub async fn update_install(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Emitter as _;

    let info = tauri::async_runtime::spawn_blocking(crate::updater::check)
        .await
        .map_err(|e| format!("检查更新任务异常：{e}"))?;
    if let Some(e) = info.error.clone() {
        return Err(e);
    }
    if !info.has_update {
        return Err(format!("已经是最新版本 v{}", info.current));
    }

    let app2 = app.clone();
    let mut last_sent: u64 = 0;
    let res = tauri::async_runtime::spawn_blocking(move || {
        crate::updater::download_and_prepare(&info, move |done, total| {
            // 256KB 一个 chunk，224MB 会回调近千次。按 1.5MB 节流，
            // 否则事件队列本身会拖慢下载（界面还会跟着卡）。
            if done - last_sent >= 1_500_000 || (total > 0 && done == total) {
                last_sent = done;
                let _ = app2.emit(
                    "update-progress",
                    serde_json::json!({ "done": done, "total": total }),
                );
            }
        })
    })
    .await
    .map_err(|e| format!("更新任务异常：{e}"))?;

    res?;
    // 助手已就位。给前端一点时间把「正在重启」画出来，然后让位。
    std::thread::sleep(Duration::from_millis(600));
    app.exit(0);
    Ok(())
}

// ─────────────────────────── 托盘菜单 ───────────────────────────

/// 托盘菜单的点选动作。
///
/// 做成一个命令而不是四个，是因为这些动作的共同点比差异更重要：
/// 都是「点了就该立刻收起菜单」的一次性入口，分开写每个都要重复一遍
/// show/hide/focus 的样板，而且以后加一项要动两处（命令表 + 前端）。
#[tauri::command]
pub fn tray_action(app: tauri::AppHandle, action: String) -> Result<(), String> {
    // get_webview_window / try_state 都来自 Manager trait
    use tauri::Manager as _;

    // 不管哪种动作，菜单都得先收起来 —— 留着它会抢焦点，主窗口出不来。
    if let Some(w) = app.get_webview_window(crate::TRAY_MENU_LABEL) {
        let _ = w.hide();
    }

    match action.as_str() {
        "open" => {
            if let Some(w) = app.get_webview_window("main") {
                w.show().map_err(|e| e.to_string())?;
                let _ = w.unminimize();
                w.set_focus().map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        "start" => {
            let Some(state) = app.try_state::<Arc<AppState>>() else {
                return Err("服务状态还没初始化".to_string());
            };
            if state.is_running() {
                return Ok(());
            }
            tracing::info!("托盘：启动服务");
            start_server_background(&state);
            Ok(())
        }
        "stop" => {
            let Some(state) = app.try_state::<Arc<AppState>>() else {
                return Err("服务状态还没初始化".to_string());
            };
            tracing::info!("托盘：停止服务");
            stop_server(&state);
            Ok(())
        }
        "quit" => {
            tracing::info!("托盘：退出应用");
            app.exit(0);
            Ok(())
        }
        other => Err(format!("未知托盘动作：{other}")),
    }
}

/// 托盘菜单渲染完，告诉后端实际高度，让它重新贴到托盘图标上。
///
/// 为什么需要这一步：菜单里有没有「有更新」条目会差一行的高度，
/// 而窗口是**先显示、再渲染**的 —— 首次显示时后端只能按估算高度定位，
/// 不回调一次就会歪（菜单底部离托盘图标一大截，或者顶出屏幕外）。
#[tauri::command]
pub fn tray_menu_height(app: tauri::AppHandle, height: f64) -> Result<(), String> {
    use tauri::Manager as _;

    let Some(w) = app.get_webview_window(crate::TRAY_MENU_LABEL) else {
        return Ok(());
    };
    let scale = w.scale_factor().unwrap_or(1.0);
    // ceil：逻辑像素换成物理像素时会被截掉小数，底部就少一截
    // （表现是「退出」那一行下面被切掉一点）。
    let h = (height / scale).ceil().clamp(120.0, 640.0);
    w.set_size(tauri::LogicalSize::new(crate::TRAY_MENU_WIDTH, h))
        .map_err(|e| e.to_string())?;
    crate::place_tray_menu(&w).map_err(|e| e.to_string())
}

// ─────────────────────────── 模型库（按需下载） ───────────────────────────

/// 列出可选模型及其安装状态。
///
/// `force` = true 时绕过清单缓存，强制联网拉取（用户点「刷新」）。
#[tauri::command]
pub async fn models_catalog(
    state: State<'_, Arc<AppState>>,
    force: Option<bool>,
) -> Result<Vec<crate::modellib::ModelStatus>, String> {
    // 先拷出来再进阻塞线程：State 的借用不能跨 await
    let models_root = state.models_root.clone();
    let data_dir = state.data_dir.clone();
    let force = force.unwrap_or(false);

    tauri::async_runtime::spawn_blocking(move || {
        let (catalog, online) = crate::modellib::load_catalog(&data_dir, force);
        crate::modellib::statuses(&models_root, &catalog, online)
    })
    .await
    .map_err(|e| format!("读取模型清单异常：{e}"))
}

/// 下载并安装一个可选模型。进度通过 `models-progress` 事件推送。
#[tauri::command]
pub async fn models_download(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    let models_root = state.models_root.clone();
    let data_dir = state.data_dir.clone();

    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter as _;

        let (catalog, _) = crate::modellib::load_catalog(&data_dir, false);
        let entry = crate::modellib::find(&catalog, &id)
            .ok_or_else(|| format!("清单里没有「{id}」这个模型，点「刷新」试试"))?
            .clone();

        let app2 = app.clone();
        let id2 = id.clone();
        let mut last_sent: u64 = 0;
        crate::modellib::install(&models_root, &data_dir, &entry, move |done, total, phase| {
            // 下载阶段按 1MB 节流：256KB 一个 chunk，133MB 会回调五百多次，
            // 不节流的话事件队列本身就成了瓶颈（界面反而卡）。
            // 其它阶段（校验/解压/完成）次数很少，直接发。
            if phase == "download" {
                if done.saturating_sub(last_sent) < 1_000_000 && done != total {
                    return;
                }
                last_sent = done;
            }
            let _ = app2.emit(
                "models-progress",
                serde_json::json!({
                    "id": id2,
                    "done": done,
                    "total": total,
                    "phase": phase,
                }),
            );
        })
    })
    .await
    .map_err(|e| format!("下载任务异常：{e}"))?
}

/// 从本地文件手动安装（网络受限时的活路）。
#[tauri::command]
pub async fn models_import(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    path: String,
) -> Result<(), String> {
    let models_root = state.models_root.clone();
    let data_dir = state.data_dir.clone();

    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Emitter as _;

        let (catalog, _) = crate::modellib::load_catalog(&data_dir, false);
        let entry = crate::modellib::find(&catalog, &id)
            .ok_or_else(|| format!("清单里没有「{id}」这个模型"))?
            .clone();

        let app2 = app.clone();
        let id2 = id.clone();
        crate::modellib::import_zip(
            &models_root,
            &entry,
            std::path::Path::new(&path),
            move |_done, _total, phase| {
                let _ = app2.emit(
                    "models-progress",
                    serde_json::json!({ "id": id2, "done": 0, "total": 0, "phase": phase }),
                );
            },
        )
    })
    .await
    .map_err(|e| format!("导入任务异常：{e}"))?
}

/// 卸载一个可选模型（只删它的目录）。
#[tauri::command]
pub async fn models_remove(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<(), String> {
    let models_root = state.models_root.clone();
    let data_dir = state.data_dir.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let (catalog, _) = crate::modellib::load_catalog(&data_dir, false);
        let entry = crate::modellib::find(&catalog, &id)
            .ok_or_else(|| format!("清单里没有「{id}」这个模型"))?;
        crate::modellib::remove(&models_root, entry)
    })
    .await
    .map_err(|e| format!("卸载任务异常：{e}"))?
}

/// 模型目录的绝对路径（前端「打开目录」按钮用）。
#[tauri::command]
pub fn models_dir(state: State<'_, Arc<AppState>>) -> String {
    state.models_root.display().to_string()
}
