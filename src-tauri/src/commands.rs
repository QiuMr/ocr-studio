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
            .unwrap_or_else(|| "yolo:v8".to_string()),
        ocr_engine: crate::engine::EngineSpec::ppocr(ocr_level).key(),
        count: text_count,
        regions: region_out,
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
    let mut engines = crate::engine::catalog::discover(&state.models_root);

    // 回填已加载实例的真实设备，前端才能看出「GPU 到底生效没有」
    if let Some(info) = state.health() {
        let key = crate::engine::EngineSpec::ppocr(info.level).key();
        for e in engines.iter_mut() {
            if e.spec.key() == key {
                e.actual_device = Some(info.actual_device);
            }
        }
    }

    EngineListOut {
        models_root: state.models_root.display().to_string(),
        default_engine: crate::engine::EngineSpec::ppocr(cfg.level).key(),
        engines: engines
            .iter()
            .map(crate::api::dto::EngineItemDto::from)
            .collect(),
    }
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
    cfg.webview_compat = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .webview_compat;
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

/// 启动 HTTP 服务（已在运行时直接返回端口）。
#[tauri::command]
pub async fn server_start(state: State<'_, Arc<AppState>>) -> Result<u16, String> {
    if state.is_running() {
        return Ok(state.port());
    }
    let port = state.port();
    let st = Arc::clone(&*state);
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
            hwnd: self.hwnd,
            title: self.title,
            region: self.region,
            min_score: self.min_score,
            max_results: self.max_results,
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
    let (hay, ox, oy) =
        routes::acquire_haystack(source, &opts, None).map_err(|e| e.message())?;

    let mopts = MatchOptions {
        min_score: opts.min_score.unwrap_or(mode.default_min_score()),
        max_results: opts.max_results.unwrap_or(1),
    };
    // 只有屏幕/窗口来源需要回传截图（图片来源调用方本来就有原图）
    let need_preview = source != FindSource::Image;
    let (hits, preview) = tokio::task::spawn_blocking(move || {
        let hits = matcher::find_template(&hay, &tpl, &mopts)?;
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

// 命令注册集中在 `lib.rs` 的 `invoke_handler` 中，用 `tauri::generate_handler!`
// 引用本模块的各个函数（宏接受路径，无需额外包装）。

// ---------------------------------------------------------------- 日志

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
