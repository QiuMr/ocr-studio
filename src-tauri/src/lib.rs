use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;

pub mod api;
pub mod capture;
pub mod commands;
pub mod config;
pub mod logbuf;
/// 模板匹配。文件名 `match.rs` 与 Rust 关键字冲突，模块名对外暴露为 `matcher`。
#[path = "match.rs"]
pub mod matcher;
pub mod ocr;

/// 前端是否已经跑起来并报到。
///
/// 用途：WebView2 的 GPU 进程在部分机器上启动即崩，Chromium 会打印
/// `GPU process isn't usable. Goodbye.` 并**终止浏览器进程**——宿主进程还活着，
/// 窗口却永远是白的，而且 **JS 一行都不会执行**。所以「前端有没有报到」
/// 正好是判断 WebView 是否可用的可靠信号。
static UI_READY: AtomicBool = AtomicBool::new(false);

/// 前端挂载后调用，标记界面已成功渲染。
#[tauri::command]
fn ui_ready() {
    UI_READY.store(true, Ordering::SeqCst);
}

/// 兼容渲染参数：只关合成，保留 GPU 进程做光栅化，比全量 `--disable-gpu` 轻。
const COMPAT_ARGS: &str = "--disable-gpu-compositing";

/// 简单文件日志 writer（`Arc<Mutex<File>>` 的 MakeWriter 包装）。
#[derive(Clone)]
struct LogFile(Arc<Mutex<std::fs::File>>);

impl std::io::Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut f) => f.write(buf),
            Err(_) => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut f) => f.flush(),
            Err(_) => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogFile {
    type Writer = LogFile;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 初始化日志：控制台 + 应用数据目录下的 `ocr-studio.log`。
///
/// release 是 GUI 子系统（无控制台），不落文件就等于**完全没有日志**——
/// 「双击后白屏」这类问题只能靠猜。日志文件是排查的唯一抓手。
fn init_logging(data_dir: &std::path::Path) {
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;

    let _ = std::fs::create_dir_all(data_dir);
    let log_path = data_dir.join("ocr-studio.log");
    // 超过 8MB 就截断重来，避免无限增长
    if let Ok(meta) = std::fs::metadata(&log_path) {
        if meta.len() > 8 * 1024 * 1024 {
            let _ = std::fs::remove_file(&log_path);
        }
    }

    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    // ort 在 INFO 级别会把图优化过程逐条打印（单次启动十万行级别），
    // 真正的错误会被淹掉。默认压到 warn，需要时用 RUST_LOG 覆盖。
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ort=warn"))
    };

    // 日志同时进：文件 / stderr / 环形缓冲（前端「运行日志」页）。
    // 用 registry 手动拼 layer，是为了能挂上自己的 BufferLayer。
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let buf_layer = logbuf::BufferLayer::new();

    match file {
        Some(f) => {
            let writer = LogFile(Arc::new(Mutex::new(f))).and(std::io::stderr);
            let fmt_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            tracing_subscriber::registry()
                .with(filter())
                .with(fmt_layer)
                .with(buf_layer)
                .init();
            tracing::info!("日志文件: {}", log_path.display());
        }
        None => {
            let fmt_layer = tracing_subscriber::fmt::layer();
            tracing_subscriber::registry()
                .with(filter())
                .with(fmt_layer)
                .with(buf_layer)
                .init();
        }
    }
    let _ = tracing_log::LogTracer::init();
}

fn build_main_window(app: &tauri::AppHandle, extra_args: Option<&str>) -> tauri::Result<()> {
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("OCR Studio")
    .inner_size(1100.0, 720.0)
    .min_inner_size(900.0, 600.0)
    .decorations(false);

    if let Some(args) = extra_args.filter(|s| !s.trim().is_empty()) {
        tracing::warn!("WebView 附加参数: {args}");
        builder = builder.additional_browser_args(args);
    }

    builder.build().map(|_| ())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 注意用 app_data_dir 而不是 data_dir：
            // data_dir() 返回的是 Roaming 根目录，config.json / 日志会直接落到
            // C:\Users\xxx\AppData\Roaming\ 下，污染用户目录还可能和别的程序撞名。
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            init_logging(&data_dir);
            // 日志转发线程：把缓冲区里的新日志批量推给前端（日志页用）
            logbuf::spawn_forwarder(app.handle().clone());
            tracing::info!("OCR Studio 启动，数据目录: {}", data_dir.display());

            // 诊断开关：跳过 OCR 核心（引擎 + HTTP 服务），用于隔离白屏问题
            if std::env::var_os("OCR_NO_CORE").is_some() {
                tracing::warn!("OCR_NO_CORE 已设置：跳过引擎与 HTTP 服务启动");
            } else {
                let state = api::start_for_tauri(app.handle());
                app.manage(state);
            }

            let cfg = config::AppConfig::load(&data_dir);
            // 环境变量优先级最高（诊断用），其次是自动/用户置上的兼容模式
            let env_args = std::env::var("OCR_WEBVIEW_ARGS").ok();
            let forced = env_args.as_deref().filter(|s| !s.trim().is_empty());
            let compat_already = cfg.webview_compat;

            let first_args = match (forced, compat_already) {
                (Some(a), _) => Some(a.to_string()),
                (None, true) => Some(COMPAT_ARGS.to_string()),
                _ => None,
            };

            let mut created = build_main_window(app.handle(), first_args.as_deref());
            if let Err(e) = &created {
                // 建窗就失败（多见于 WebView2 环境创建不起来）：直接换兼容参数再试一次，
                // 而不是让事件循环因为没有窗口而立刻退出。
                tracing::error!("主窗口创建失败: {e}；改用兼容渲染重试");
                std::thread::sleep(Duration::from_millis(300));
                created = build_main_window(app.handle(), Some(COMPAT_ARGS));
                if created.is_ok() {
                    persist_compat(&data_dir);
                }
            }
            match &created {
                Ok(()) => tracing::info!("主窗口创建成功"),
                Err(e) => {
                    tracing::error!("兼容模式重建主窗口仍失败: {e}");
                    return Ok(());
                }
            }

            // 看门狗：等前端报到；超时说明 WebView 已死（JS 一次都没执行），
            // 切软件合成重建。只在没有外部指定参数且尚未处于兼容模式时尝试。
            if forced.is_some() || compat_already {
                return Ok(());
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for _ in 0..40 {
                    if UI_READY.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                tracing::error!(
                    "前端 10 秒内未报到，判定 WebView 渲染失败（GPU 进程崩溃），切换软件合成重建窗口"
                );

                if let Some(old) = handle.get_webview_window("main") {
                    let _ = old.destroy();
                }
                std::thread::sleep(Duration::from_millis(400));

                match build_main_window(&handle, Some(COMPAT_ARGS)) {
                    Ok(()) => {
                        tracing::warn!("已切换到兼容渲染模式（{COMPAT_ARGS}）");
                        persist_compat(&data_dir);
                    }
                    Err(e) => tracing::error!("兼容模式重建窗口仍失败: {e}"),
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ui_ready,
            commands::ocr_recognize,
            commands::ocr_danzijson,
            commands::get_health,
            commands::get_config,
            commands::save_config,
            commands::set_level,
            commands::set_device,
            commands::server_status,
            commands::server_start,
            commands::server_stop,
            commands::models_root,
            commands::find_image,
            commands::screen_preview,
            commands::list_windows,
            commands::get_logs,
            commands::read_image_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 把兼容模式写进配置，下次启动直接走软件合成，不再先白屏一次。
fn persist_compat(data_dir: &std::path::Path) {
    let mut c = config::AppConfig::load(data_dir);
    if c.webview_compat {
        return;
    }
    c.webview_compat = true;
    match c.save(data_dir) {
        Ok(()) => tracing::warn!("兼容渲染模式已写入配置（webview_compat=true）"),
        Err(e) => tracing::warn!("兼容模式写入配置失败: {e}"),
    }
}
