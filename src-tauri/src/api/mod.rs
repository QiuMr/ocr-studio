//! 本地 HTTP 接口模块。
//!
//! 拆分：`dto`（入参出参与错误）、`state`（引擎生命周期）、`routes`（路由与处理器）。
//! 路由表 1:1 复刻 `docs/PLAN.md` 第 6 节的 `/ocr/*` 接口。

pub mod dto;
pub mod routes;
pub mod state;

pub use dto::{ApiError, ApiResp, OcrOptions, CODE_NOT_IMPLEMENTED, CODE_OK};
pub use routes::{build_router, build_router_with};
pub use state::{resolve_models_root, workspace_root, AppState};

use std::sync::Arc;

use anyhow::Context;
use tauri::Manager;

use crate::config::AppConfig;
use crate::ocr::ensure_ort_dylib_path;

/// 启动 HTTP 服务，直到收到停机信号。
pub async fn serve(state: Arc<AppState>) -> anyhow::Result<()> {
    let port = state.port();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .with_context(|| format!("绑定 127.0.0.1:{port} 失败（端口被占用？）"))?;

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    {
        let mut guard = state.shutdown.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(tx);
    }

    tracing::info!("HTTP 服务已启动: http://127.0.0.1:{port}");
    let allow_browser = state
        .cfg
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .allow_browser;
    if allow_browser {
        tracing::info!("已允许浏览器页面跨域调用（CORS 打开）");
    }
    axum::serve(listener, build_router_with(allow_browser).with_state(state))
        .with_graceful_shutdown(async {
            let _ = rx.await;
        })
        .await
        .context("HTTP 服务运行异常")?;
    tracing::info!("HTTP 服务已停止");
    Ok(())
}

/// 请求停机（幂等）。
pub fn stop(state: &Arc<AppState>) {
    let tx = {
        let mut guard = state.shutdown.lock().unwrap_or_else(|e| e.into_inner());
        guard.take()
    };
    if let Some(tx) = tx {
        let _ = tx.send(());
    }
}

/// 供 Tauri `setup` 调用：解析配置与模型目录、后台加载引擎并启动 HTTP 服务。
///
/// 引擎加载失败不会让应用退出——此时接口返回 503，前端可引导用户检查模型。
/// 返回的 `Arc<AppState>` 建议由调用方 `app.manage(...)`，供 Tauri 命令复用。
pub fn start_for_tauri(app: &tauri::AppHandle) -> Arc<AppState> {
    // 必须在任何 ort API 之前指定 DLL 路径，否则可能误加载 system32 里的旧版
    if !ensure_ort_dylib_path(&workspace_root()) {
        tracing::warn!(
            "未找到 src-tauri/runtime/onnxruntime.dll；请先运行 scripts/copy_runtime.ps1"
        );
    }

    // app_data_dir() = Roaming\<identifier>；data_dir() 是 Roaming 根目录，别用错
    let data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| workspace_root().join(".ocr-studio-data"));
    let cfg = AppConfig::load(&data_dir);
    let models_root = resolve_models_root();
    tracing::info!("模型根目录: {}", models_root.display());
    tracing::info!(
        "配置: level={:?} device={:?} port={}",
        cfg.level,
        cfg.device,
        cfg.port
    );

    let state = Arc::new(AppState::with_data_dir(models_root, cfg, data_dir));

    // 引擎加载以秒计，放独立线程 + 独立 runtime，避免阻塞 UI 线程
    let st = Arc::clone(&state);
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!("创建 tokio runtime 失败: {e}");
                return;
            }
        };
        rt.block_on(async move {
            if let Err(e) = st.ensure_engine(&Default::default()).await {
                tracing::error!("引擎加载失败: {e}");
            }
            if let Err(e) = serve(st).await {
                tracing::error!("HTTP 服务异常退出: {e}");
            }
        });
    });

    state
}
