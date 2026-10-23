use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// 注：托盘**不再**用 `tauri::menu::Menu`（系统原生菜单，样式改不了），
// 改成自己画的无边框窗口，见 `setup_tray`。
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, WindowEvent};

pub mod api;
/// 二维码 / 条码识别（rxing = ZXing 的纯 Rust 移植）。
pub mod barcode;
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
/// 版本号（唯一来源 `tauri.conf.json`，由 `build.rs` 注入）。
pub mod version;
/// 版本检查与自更新（免安装绿色版，不走官方 updater 插件）。
pub mod updater;

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

/// 当前生效的 WebView 附加参数（主窗口定，托盘窗口跟着用）。
///
/// 为什么托盘窗口也要管：主窗口要是切了软件合成，说明这台机器的 GPU 进程
/// 起不来（白屏）。托盘菜单是**另一个 WebView**，用默认参数一样会白，
/// 而且它小、没标题栏，白起来更像「程序坏了」而不是「渲染问题」。
static WEBVIEW_ARGS: Mutex<Option<String>> = Mutex::new(None);

fn set_webview_args(args: Option<String>) {
    *WEBVIEW_ARGS.lock().unwrap_or_else(|e| e.into_inner()) = args;
}

fn webview_args() -> Option<String> {
    WEBVIEW_ARGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

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

/// 自定义托盘菜单窗口的 label。
pub(crate) const TRAY_MENU_LABEL: &str = "tray";
/// 托盘菜单宽度（逻辑像素）。高度由内容决定，前端量完再回调改。
pub(crate) const TRAY_MENU_WIDTH: f64 = 272.0;

/// 托盘菜单的焦点是否「稳定地」拿到了。
///
/// 注意这里问的不是「有没有拿到过焦点」，而是「**稳不稳**」。
/// 前一版用的是「拿到过就算」，结果用户实测到的就是**菜单闪一下就没了**：
///
/// 点托盘图标时前台还在 explorer 手里，我们的窗口 show 出来之后
/// 焦点被抢回去，于是 `Focused(true)` 后面紧跟一个 `Focused(false)`，
/// 按「拿到过就算」的逻辑就直接收起了。窗口是对的、位置也是对的，
/// 只是活了几百毫秒 —— 用户看到的就是「点了什么都没发生」。
///
/// 所以改成：show 之后等一小会儿再问一次焦点还在不在，
/// **还在**才允许「失焦即收起」。拿不到焦点的机器上菜单就一直挂着，
/// 靠再点一次托盘 / Esc / 点菜单项来关 —— 菜单挂着只是不好看，
/// 菜单看不见才是真的坏。
static TRAY_FOCUS_STABLE: AtomicBool = AtomicBool::new(false);

/// 菜单弹出后等多久判定焦点是否稳定。
///
/// 太短会把「焦点还没排到」误判成「拿不到焦点」；太长则用户点别处时
/// 菜单会赖着不走。800ms 是实测下来两边都不难受的点。
const TRAY_FOCUS_SETTLE: Duration = Duration::from_millis(800);

/// 上次点击托盘图标的屏幕物理坐标。
///
/// 菜单是「贴着托盘图标弹出」的，而高度要等前端把内容渲染完才知道，
/// 所以锚点得留着，量完高度再重新定位一次 —— 否则菜单会朝上长到屏幕外面去。
static TRAY_ANCHOR: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// 托盘点击去重：同一次点击被重复投递时只认第一次。
///
/// 150ms 是刻意的短间隔 —— 人手连点两次（想用 toggle 收起菜单）间隔通常
/// 在 200ms 以上，不会误伤；而同一个物理点击被重复投递几乎都是紧挨着的。
fn tray_click_debounce() -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let now = Instant::now();
    let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *g {
        if now.duration_since(t) < Duration::from_millis(150) {
            return false;
        }
    }
    *g = Some(now);
    true
}

/// 系统托盘：常驻入口。
///
/// 这是「API 工具」定位的直接体现——关掉窗口服务还得活着，用户必须有个地方
/// 能看状态、能停服务、能真正退出。
///
/// **菜单不用 `tauri::menu::Menu`**（2026-10-23 重做）：
/// 原生菜单由系统绘制，样式改不了 —— 白底黑字、无图标、字体跟着系统走，
/// 放在一个讲究排版的产品里显得很旧。改成点击托盘时弹出一个自己画的
/// 无边框窗口（见 `show_tray_menu`），外观跟主界面同一套。
///
/// 代价是要自己处理定位与失焦关闭，换来的是：能放图标、能放状态徽标、
/// 能放「有更新」高亮条目 —— 这些原生菜单一样都做不到。
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("OCR Studio · API 服务")
        // 不给图标时 Windows 上托盘图标是不可见的（tray-icon 不会自动回退到应用图标），
        // 必须显式把打包进 exe 的默认窗口图标给它。
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::AssetNotFound("没有默认窗口图标".into()))?,
        );

    builder = builder.on_tray_icon_event(|tray, event| {
        // 左键和右键都弹自定义菜单。
        //
        // 为什么右键也一样：原来右键弹的是系统原生菜单，一旦用户右键过，
        // 就会看到两套完全不同的菜单（一次现代的、一次老旧的）——
        // 那比一直用老菜单更出戏。
        match event {
            tauri::tray::TrayIconEvent::Click {
                position,
                button_state,
                ..
            } => {
                // **只在按键抬起时弹**。
                //
                // `Click` 事件按下和抬起各来一次。两次都处理的话，第二次会撞上
                // 「已经可见就收起」的 toggle 分支，菜单刚 show 出来就被自己
                // 关掉 —— 用户看到的就是「右键点一下瞬间闪没」。
                // 这一条跟焦点无关，前面按焦点排查被带偏过一次。
                if button_state != tauri::tray::MouseButtonState::Up {
                    return;
                }
                // 再挡一层重复投递：某些环境（远程桌面、部分输入法）下同一次
                // 点击会被送两次，效果跟上面一样是闪没。
                if !tray_click_debounce() {
                    return;
                }
                show_tray_menu(tray.app_handle(), position);
            }
            // 双击不单独处理：双击之前必然已经有过 Click(Up)，再弹一次会变成
            // toggle 收起，反而闪。
            _ => {}
        }
    });

    let tray = builder.build(app)?;
    spawn_tray_ticker(app.clone(), tray);
    Ok(())
}

/// 弹出（或复用）托盘菜单窗口，定位到托盘图标上方。
///
/// 入口统一走主线程（见下），真正的实现在 `show_tray_menu_inner`。
fn show_tray_menu(app: &tauri::AppHandle, pos: tauri::PhysicalPosition<f64>) {
    // 托盘的事件回调跑在**托盘自己的消息线程**上，不是主线程。
    // 在那个线程上建窗口、抢前台会失败：Windows 的前台锁要求调用线程本身
    // 有资格设置前台窗口，托盘线程没有。失败了窗口照样能 show 出来，
    // 但焦点拿不到 —— 这正是「菜单闪一下就没」的直接原因之一。
    let app2 = app.clone();
    if let Err(e) = app.run_on_main_thread(move || show_tray_menu_inner(&app2, pos)) {
        tracing::warn!("托盘菜单投递到主线程失败: {e}；退回当前线程");
        show_tray_menu_inner(app, pos);
    }
}

/// 菜单上次弹出的时刻。
///
/// 用途：`show_tray_menu` 里的 toggle 判断要区分「用户想关掉菜单」和
/// 「同一次物理点击被处理了两次」。后者会表现为菜单瞬间闪没，
/// 是本模块踩过两次的坑，所以宁可多一道时间闸。
static TRAY_SHOWN_AT: Mutex<Option<Instant>> = Mutex::new(None);

/// 托盘菜单：这个时间内不认为「再点一次 = 关闭」。
const TRAY_TOGGLE_GUARD: Duration = Duration::from_millis(400);

fn show_tray_menu_inner(app: &tauri::AppHandle, pos: tauri::PhysicalPosition<f64>) {
    *TRAY_ANCHOR
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some((pos.x, pos.y));

    let win = match app.get_webview_window(TRAY_MENU_LABEL) {
        Some(w) => w,
        None => {
            // 透明 + 无边框 + 置顶 + 不进任务栏：托盘菜单的全部外在要求。
            // shadow(true) 必须开，否则在浅色壁纸上边缘会糊掉看不出是个浮层。
            let b = tauri::WebviewWindowBuilder::new(
                app,
                TRAY_MENU_LABEL,
                tauri::WebviewUrl::App("index.html#tray".into()),
            )
            .title("OCR Studio")
            .inner_size(TRAY_MENU_WIDTH, 320.0)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(true)
            .visible(false);
            // 跟上主窗口的渲染参数：那台机器要是需要软件合成，这里也一样需要
            let b = match webview_args() {
                Some(a) if !a.trim().is_empty() => b.additional_browser_args(&a),
                _ => b,
            };
            match b.build() {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!("托盘菜单窗口创建失败: {e}");
                    return;
                }
            }
        }
    };
    // 已经开着就收起来：再点一次托盘 = 关闭。
    // 这条同时是「菜单拿不到焦点时怎么关掉」的兜底 —— 没有它，
    // 万一某台机器上菜单既没焦点也没收到失焦事件，就再也关不掉了。
    //
    // **刚弹出来的不算**：同一次点击被重复处理时，第二次会走到这里把菜单
    // 关掉，用户看到的就是「闪一下就没」。就算上面那两道闸（只认按键抬起、
    // 150ms 去重）都被绕过，这一道也能兜住。
    let just_shown = {
        let g = TRAY_SHOWN_AT.lock().unwrap_or_else(|e| e.into_inner());
        g.map(|t| t.elapsed() < TRAY_TOGGLE_GUARD).unwrap_or(false)
    };
    if win.is_visible().unwrap_or(false) && !just_shown {
        let _ = win.hide();
        return;
    }
    *TRAY_SHOWN_AT
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());

    // 每次弹出都重新判定焦点，别信上一次的结论
    TRAY_FOCUS_STABLE.store(false, Ordering::SeqCst);
    if let Err(e) = place_tray_menu(&win) {
        tracing::warn!("托盘菜单定位失败: {e}");
    }
    let _ = win.show();
    let _ = win.set_focus();
    // Tauri 的 set_focus 在托盘场景下常被前台锁挡掉，用 Win32 再抢一次。
    // 抢不到也不打紧 —— 下面会据此判定「焦点不稳定」，菜单改成手动关闭。
    force_foreground(&win);

    // 延迟判定焦点是否真的稳住了（判定逻辑见 TRAY_FOCUS_STABLE 的注释）
    let w = win.clone();
    std::thread::spawn(move || {
        std::thread::sleep(TRAY_FOCUS_SETTLE);
        // 期间用户可能已经把它关掉了（点了菜单项 / 又点了一次托盘）
        if !w.is_visible().unwrap_or(false) {
            return;
        }
        let stable = w.is_focused().unwrap_or(false);
        TRAY_FOCUS_STABLE.store(stable, Ordering::SeqCst);
        if stable {
            return;
        }
        tracing::warn!("托盘菜单没能稳定拿到前台焦点，改用轮询检测「点击菜单外部」关闭");
        spawn_click_outside_watch(w);
    });
}

/// 焦点**不稳**时的兜底：轮询鼠标键，点在菜单外面就收起。
///
/// 为什么非得有它：菜单拿不到前台焦点的时候，既收不到 `Focused(false)`，
/// 前端也收不到 Esc 的 keydown（键盘消息只发给前台窗口）。这样最直觉的
/// 两条退路都断了，只剩「再点一次托盘图标」——用户不知道这条隐藏规则，
/// 只会觉得菜单卡死了。
///
/// 用轮询而不是 `SetWindowsHookEx` 全局钩子：钩子要跑消息循环，会跟 Tauri
/// 的事件循环抢主线程；`GetAsyncKeyState` 读的是物理按键状态，
/// 不依赖本窗口是否在前台 —— 正好适合这个场景。
fn spawn_click_outside_watch(win: tauri::WebviewWindow) {
    std::thread::spawn(move || {
        // 先等用户把刚才点托盘那一下松开，否则开局的按键会被当成「点外面」
        for _ in 0..12 {
            if !mouse_down() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        loop {
            std::thread::sleep(Duration::from_millis(60));
            // 后来又拿到焦点了：交回正常的失焦逻辑，别让两套机制打架
            if win.is_focused().unwrap_or(false) {
                TRAY_FOCUS_STABLE.store(true, Ordering::SeqCst);
                return;
            }
            if !win.is_visible().unwrap_or(false) {
                return;
            }
            if mouse_down() && !cursor_in_window(&win) {
                let _ = win.hide();
                return;
            }
        }
    });
}

/// 鼠标左键或右键当前是否按着。
#[cfg(windows)]
fn mouse_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    unsafe {
        // 0x8000 = 这一刻正处于按下状态
        let l = GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0;
        let r = GetAsyncKeyState(VK_RBUTTON.0 as i32) as u16 & 0x8000 != 0;
        l || r
    }
}

#[cfg(not(windows))]
fn mouse_down() -> bool {
    false
}

/// 光标此刻是否在窗口矩形内（物理像素）。
#[cfg(windows)]
fn cursor_in_window(win: &tauri::WebviewWindow) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut p = POINT::default();
    unsafe {
        // 注意是 Result 不是 BOOL：0.58 起这类函数把 GetLastError 包进了返回值
        if GetCursorPos(&mut p).is_err() {
            return true; // 拿不到坐标就当在窗口内，宁可不关也别误关
        }
    }
    let Ok(pos) = win.outer_position() else { return true };
    let Ok(size) = win.outer_size() else { return true };
    let (x, y) = (p.x, p.y);
    x >= pos.x
        && x <= pos.x + size.width as i32
        && y >= pos.y
        && y <= pos.y + size.height as i32
}

#[cfg(not(windows))]
fn cursor_in_window(_win: &tauri::WebviewWindow) -> bool {
    true
}

/// 把托盘菜单窗口推到前台（Windows）。
///
/// `WebviewWindow::set_focus()` 走的是 Chromium 自己的激活路径，在托盘场景里
/// 经常被系统的前台锁挡掉：窗口显示出来了，但 `GetForegroundWindow()` 还不是它，
/// 于是紧接着就被 explorer 收走焦点 —— 用户看到就是「闪一下就没」。
///
/// 这里补一次 Win32 的 `SetForegroundWindow`；失败再退到
/// `AttachThreadInput` 的老办法（把自己挂到前台线程上再抢）。
#[cfg(windows)]
fn force_foreground(win: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    let Ok(raw) = win.hwnd() else { return };
    // tauri 暴露的 HWND 来自它自己锁定的 windows 版本，跟本项目 Cargo.toml 里
    // 声明的可能不是同一个类型（直接传会给 `Param<HWND>` 报 trait 不满足）。
    // 两者都是 `*mut c_void` 的新类型包装，用裸指针重新包一层即可。
    let hwnd = HWND(raw.0);
    unsafe {
        if !SetForegroundWindow(hwnd).as_bool() {
            // 抢不到前台不算错误：下面会判定「焦点不稳定」，
            // 菜单改成靠「点击外部」关闭，而不是让它闪没。
            tracing::warn!("SetForegroundWindow 未生效，托盘菜单可能拿不到前台焦点");
        }
    }
}

#[cfg(not(windows))]
fn force_foreground(_win: &tauri::WebviewWindow) {}

/// 把菜单窗口贴到托盘图标上方。放不下就翻到下方。
pub(crate) fn place_tray_menu(win: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some((ax, ay)) = *TRAY_ANCHOR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
    else {
        return Ok(());
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let size = win.outer_size().unwrap_or(tauri::PhysicalSize::new(
        (TRAY_MENU_WIDTH * scale) as u32,
        (320.0 * scale) as u32,
    ));
    let mut x = ax - size.width as f64 / 2.0;
    let mut y = ay - size.height as f64 - 6.0 * scale;
    // 上方放不下（托盘在屏幕顶部、或任务栏在顶部）就翻到下方
    if y < 4.0 * scale {
        y = ay + 6.0 * scale;
    }
    // 夹回屏幕内，别让菜单跑出左右边缘
    x = x.max(4.0 * scale);
    win.set_position(tauri::PhysicalPosition::new(x, y))
}

/// 每 2 秒把服务状态刷给托盘窗口与 tooltip。
///
/// 为什么要轮询而不是事件驱动：服务的启停入口有三个（界面按钮、托盘菜单、
/// 端口变更触发的自动重启），逐个挂回调容易漏。状态是内存里的一个原子读，
/// 2 秒一次的成本可以忽略。
///
/// 状态**不再写进菜单项文字**（那是原生菜单时代的做法）：现在推一个
/// `tray-status` 事件，托盘窗口订阅后自己渲染。菜单窗口没打开时 emit 失败，
/// 忽略即可 —— 下次打开会拿到最新状态。
fn spawn_tray_ticker(app: tauri::AppHandle, tray: tauri::tray::TrayIcon) {
    std::thread::spawn(move || {
        use tauri::Emitter as _;
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

            let tip = if running {
                format!("OCR Studio · 服务运行中 127.0.0.1:{port}")
            } else {
                "OCR Studio · 服务已停止".to_string()
            };
            let _ = app.emit(
                "tray-status",
                serde_json::json!({ "running": running, "port": port }),
            );
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
    // 更新助手分流，**必须在 Builder 之前**。
    //
    // 助手和主程序是同一个 exe（只是从 TEMP 副本启动、命令行带了参数），
    // 如果先注册了 single-instance 插件，它会被当成「用户又双击了一次」，
    // 转交控制权后立刻退出 —— 表现是「点了更新，程序关掉就再也没起来」。
    if let Some(code) = updater::run_apply_if_requested() {
        std::process::exit(code);
    }

    tauri::Builder::default()
        // 单实例插件**必须是第一个**注册的插件（官方要求），否则初始化顺序会出错。
        // 场景：用户双击两次图标，或从托盘已隐藏时又点了一次 exe。
        // 没有它，第二个进程会因为 8848 已被占用而服务起不来，
        // 界面上显示「服务已停止」——用户会以为程序坏了（实测踩过）。
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            tracing::info!("检测到第二个实例，转交控制权给已运行的窗口");
            // 诊断开关：第二个实例带 `--debug-tray` 时，让**已经跑着的这个实例**
            // 把托盘菜单弹出来。
            //
            // 为什么要绕这一下：Win11 默认把托盘图标折叠进任务栏的「^」里，
            // 脚本点击很难命中（实测点了三次都没打开那个面板），而菜单的样式、
            // 高度、失焦行为必须真跑起来才看得见。又因为 explorer 用的是登录时
            // 的环境，新设的环境变量传不进它启动的进程，所以只能走命令行参数。
            if argv.iter().any(|a| a == "--debug-tray") {
                show_tray_menu(app, tauri::PhysicalPosition::new(1400.0, 820.0));
                // **必须在这里返回**：下面那三行会把焦点抢给主窗口，
                // 菜单刚 show 出来就被收掉。实测踩到过两次——第一次是
                // 菜单直接 focus 不到（visible 一直 0），第二次是弹出来
                // 又立刻失焦。真实点击托盘不走这里，但调试路径得靠它抓图。
                return;
            }
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
            set_webview_args(first_args.clone());

            let mut created = build_main_window(app.handle(), first_args.as_deref());
            if let Err(e) = &created {
                // 建窗就失败（多见于 WebView2 环境创建不起来）：直接换兼容参数再试一次，
                // 而不是让事件循环因为没有窗口而立刻退出。
                tracing::error!("主窗口创建失败: {e}；改用兼容渲染重试");
                std::thread::sleep(Duration::from_millis(300));
                created = build_main_window(app.handle(), Some(COMPAT_ARGS));
                set_webview_args(Some(COMPAT_ARGS.to_string()));
                if created.is_ok() {
                    persist_compat(&data_dir);
                }
            }
            setup_tray(app.handle())?;

            // 诊断开关：启动后直接把托盘菜单弹出来。
            //
            // 为什么需要它：Win11 默认把托盘图标折叠进「^」里，脚本点击很难命中
            // （实测点了三次都没弹开那个面板），而菜单的样式、高度、失焦行为
            // 又必须真跑起来才看得见。跟 `OCR_NO_CORE` / `OCR_WEBVIEW_ARGS`
            // 是同一类诊断开关，只在这一个环境变量存在时生效。
            if std::env::var_os("OCR_DEBUG_TRAY").is_some() {
                let h = app.handle().clone();
                std::thread::spawn(move || {
                    // 等 WebView 起来再弹，否则抓图只能看到空白
                    std::thread::sleep(Duration::from_millis(1800));
                    show_tray_menu(&h, tauri::PhysicalPosition::new(1400.0, 820.0));
                });
            }
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
                        set_webview_args(Some(COMPAT_ARGS.to_string()));
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
            commands::ocr_quyuocr,
            commands::vision_detect,
            commands::vision_pipeline,
            commands::vision_qrcode,
            commands::get_health,
            commands::list_engines,
            commands::detect_models,
            commands::prompt_assets_status,
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
            commands::find_semantic,
            commands::find_prompt,
            commands::screen_preview,
            commands::list_windows,
            commands::get_logs,
            commands::read_image_file,
            commands::ai_status,
            commands::ai_set_enabled,
            commands::ai_reset_token,
            commands::ai_targets,
            commands::update_check,
            commands::update_install,
            commands::tray_action,
            commands::tray_menu_height,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                tracing::info!("主窗口已隐藏，服务继续在托盘运行");
            }
            // 托盘菜单：一点别处就收起来。
            //
            // 为什么用失焦而不是「鼠标移出去」：托盘菜单常常弹在屏幕右下角，
            // 鼠标一动就消失会导致用户来不及点到下面的「退出」。失焦更符合
            // 系统菜单的手感（点菜单项、点别处都会失焦、Esc 由前端处理）。
            //
            // **必须先看有没有拿到过焦点**：`show()` 之后 Windows 会先抛一个
            // 假的 `Focused(false)`，照单全收的话菜单永远弹不出来
            // （窗口建好了、位置也对，就是 visible 一直是 0）。
            if let WindowEvent::Focused(got) = event {
                if window.label() == TRAY_MENU_LABEL {
                    // 只有「焦点已经稳定拿到」之后，失焦才收起。
                    //
                    // 拿不到焦点的机器（前台锁、远程桌面、某些输入法/安全软件环境）
                    // 上菜单会一直挂着，靠 Esc / 点菜单项 / 再点一次托盘关掉。
                    // 宁可让它赖着不走，也不能让它闪没了 —— 后者用户没法用。
                    if !*got && TRAY_FOCUS_STABLE.load(Ordering::SeqCst) {
                        let _ = window.hide();
                    }
                } else if *got {
                    // 主窗口被激活（用户去点界面了）：菜单别挡在上面。
                    // 这条同时是「菜单拿不到焦点、不会自动收起」时的兜底退路。
                    if let Some(w) = window.get_webview_window(TRAY_MENU_LABEL) {
                        let _ = w.hide();
                    }
                }
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
