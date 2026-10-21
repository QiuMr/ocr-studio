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
use crate::api::state::AppState;
use crate::api::stop as stop_server;
use crate::config::{AppConfig, Device, ModelLevel};

/// 识别结果（前端 Playground 用）。
#[derive(Debug, Serialize)]
pub struct RecognizeOut {
    pub text: String,
    pub lines: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
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
        keyword: None,
    };
    state
        .ensure_engine(&opts)
        .await
        .map_err(|e| e.message())?;
    let boxes = state
        .run_recognize(img)
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
    cfg: AppConfig,
) -> Result<AppConfig, String> {
    if cfg.port == 0 {
        return err("端口不能为 0".to_string());
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

// 命令注册集中在 `lib.rs` 的 `invoke_handler` 中，用 `tauri::generate_handler!`
// 引用本模块的各个函数（宏接受路径，无需额外包装）。
