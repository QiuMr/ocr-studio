use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

pub mod api;
pub mod capture;
pub mod commands;
pub mod config;
/// 多引擎抽象层（PP-OCR / DDDDOCR / YOLO）。设计契约见 `docs/ENGINES.md`。
pub mod engine;
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

/// 系统托盘：常驻入口。
///
/// 这是「API 工具」定位的直接体现——关掉窗口服务还得活着，用户必须有个地方
/// 能看状态、能停服务、能真正退出。菜单第一行是只读状态（不可点），
/// 由后台线程每 2 秒刷新一次，省得用户为了看一眼状态去开窗口。
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "服务状态读取中…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开界面", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", "启动服务", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "停止服务", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出 OCR Studio", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&status, &sep1, &open, &start, &stop, &sep2, &quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("OCR Studio · API 服务");
    // 不给图标时 Windows 上托盘图标是不可见的（tray-icon 不会自动回退到应用图标），
    // 必须显式把打包进 exe 的默认窗口图标给它。
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let tray = builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            "start" => {
                if let Some(state) = app.try_state::<Arc<api::AppState>>() {
                    if !state.is_running() {
                        tracing::info!("托盘：启动服务");
                        commands::start_server_background(&state);
                    }
                }
            }
            "stop" => {
                if let Some(state) = app.try_state::<Arc<api::AppState>>() {
                    tracing::info!("托盘：停止服务");
                    api::stop(&state);
                }
            }
            "quit" => {
                tracing::info!("托盘：退出应用");
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击弹出菜单（默认行为），双击直接开界面
            if let tauri::tray::TrayIconEvent::DoubleClick { .. } = event {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    spawn_tray_ticker(app.clone(), tray, menu);
    Ok(())
}

/// 每 2 秒把服务状态刷到托盘菜单首行与 tooltip。
///
/// 为什么要轮询而不是事件驱动：服务的启停入口有三个（界面按钮、托盘菜单、
/// 端口变更触发的自动重启），逐个挂回调容易漏。状态是内存里的一个原子读，
/// 2 秒一次的成本可以忽略。
fn spawn_tray_ticker(
    app: tauri::AppHandle,
    tray: tauri::tray::TrayIcon,
    menu: Menu<tauri::Wry>,
) {
    std::thread::spawn(move || {
        let mut last: Option<(bool, u16)> = None;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let (running, port) = match app.try_state::<Arc<api::AppState>>() {
                Some(state) => (state.is_running(), state.port()),
                None => (false, 0),
            };
            if last == Some((running, port)) {
                continue;
            }
            last = Some((running, port));

            let text = if running {
                format!("● 运行中 · 127.0.0.1:{port}")
            } else {
                "○ 已停止".to_string()
            };
            let tip = if running {
                format!("OCR Studio · 服务运行中 127.0.0.1:{port}")
            } else {
                "OCR Studio · 服务已停止".to_string()
            };
            // 菜单项文字/可用态在 Windows 上要走主线程更新
            let menu = menu.clone();
            let _ = app.run_on_main_thread(move || {
                if let Some(item) = menu.get("status").and_then(|i| i.as_menuitem().cloned()) {
                    let _ = item.set_text(text);
                }
                if let Some(item) = menu.get("start").and_then(|i| i.as_menuitem().cloned()) {
                    let _ = item.set_enabled(!running);
                }
                if let Some(item) = menu.get("stop").and_then(|i| i.as_menuitem().cloned()) {
                    let _ = item.set_enabled(running);
                }
            });
            let _ = tray.set_tooltip(Some(tip));
        }
    });
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
        // 单实例插件**必须是第一个**注册的插件（官方要求），否则初始化顺序会出错。
        // 场景：用户双击两次图标，或从托盘已隐藏时又点了一次 exe。
        // 没有它，第二个进程会因为 8848 已被占用而服务起不来，
        // 界面上显示「服务已停止」——用户会以为程序坏了（实测踩过）。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tracing::info!("检测到第二个实例，转交控制权给已运行的窗口");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        // 开机自启：拉起时带 `--tray`，直接缩在托盘里不弹窗，
        // 否则每次开机都在屏幕中间冒一个窗口。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--tray"]),
        ))
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
            setup_tray(app.handle())?;
            match &created {
                Ok(()) => tracing::info!("主窗口创建成功"),
                Err(e) => {
                    tracing::error!("兼容模式重建主窗口仍失败: {e}");
                    return Ok(());
                }
            }

            // 开机自启拉起时带 --tray：只留托盘，不弹窗
            if std::env::args().any(|a| a == "--tray") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                    tracing::info!("检测到 --tray：主窗口保持隐藏，仅托盘常驻");
                }
            }

            // 看门狗：等前端报到；超时说明 WebView 已死（JS 一次都没执行），
            // 切软件合成重建。只在没有外部指定参数且尚未处于兼容模式时尝试。
            //
            // ⚠️ 2026-10-22 又绕了一圈的教训（别再重复）：
            // 这条兜底在**本项目的 agent shell 里永远救不回来** —— 那个 shell 会让
            // WebView2 的 GPU 子进程必崩（见 docs/PROGRESS.md 0.1 节），连重建都起不来。
            // 那是**环境问题，不是产品缺陷**：同一个 exe 用资源管理器启动就正常。
            // 所以不要拿 agent shell 里的白屏去改代码，验收界面一律
            // `explorer.exe <exe>` 启动后再用 scripts/shot_window.py 抓图。
            //
            // build() 保持在子线程调用：Tauri 内部会把它派发到主线程并等待结果，
            // 子线程调用是正常路径；反过来在主线程直接调会死锁（已试过，更糟）。
            if forced.is_some() || compat_already {
                return Ok(());
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                // release 4 秒：白窗挂 10 秒足够让用户认定程序坏了；
                // dev 12 秒：vite 首次加载慢，给足。
                let budget_ms: u64 = if tauri::is_dev() { 12_000 } else { 4_000 };
                let mut waited = 0u64;
                while waited < budget_ms {
                    if UI_READY.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                    waited += 250;
                }
                tracing::error!(
                    "前端 {budget_ms} ms 内未报到，判定 WebView 渲染失败（GPU 进程崩溃），切换软件合成重建窗口"
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
            commands::vision_detect,
            commands::vision_pipeline,
            commands::get_health,
            commands::list_engines,
            commands::detect_models,
            commands::pick_detect_model,
            commands::add_detect_model,
            commands::remove_detect_model,
            commands::get_config,
            commands::save_config,
            commands::set_level,
            commands::set_device,
            commands::server_status,
            commands::server_start,
            commands::server_stop,
            commands::get_autostart,
            commands::set_autostart,
            commands::models_root,
            commands::find_image,
            commands::screen_preview,
            commands::list_windows,
            commands::get_logs,
            commands::read_image_file,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                tracing::info!("主窗口已隐藏，服务继续在托盘运行");
            }
        })
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
