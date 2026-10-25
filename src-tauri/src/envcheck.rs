//! 环境体检与一键修复（Windows）。
//!
//! # 主题：不会崩溃、但让功能静默不可用的那一类失败
//!
//! 程序崩溃有日志、有错误码，反倒容易查。真正难搞的是这类：
//! 进程活得好好的，**界面打不开 / 服务没起来 / 推理走了慢通道**，用户完全
//! 看不懂原因，只会觉得「这程序坏了」：
//!
//! - **没装 WebView2** → 窗口建不起来，只剩一个托盘图标。免安装 zip 没有
//!   安装器替用户引导装它（`bundle.active = false`，不走 NSIS/MSI 那条路）。
//! - **Win7 / Win8** → evergreen WebView2 只支持 Win10 / Server 2016+，
//!   在旧系统上装都没地方装。
//! - **端口被占用** → HTTP 服务 bind 失败，界面显示「服务已停止」但不说是
//!   谁占的、怎么办。
//!
//! # 做法：检测 → 人话说明 → 能自动修的给一键修复
//!
//! 「一键修复」的边界：**装 WebView2 是动用户系统组件，绝不静默装**。
//! 一律先弹原生消息框问过，用户点了「立即安装」才下载安装；没网就打开官网
//! 让用户自己下。每用户安装（`/silent /install`）不需要管理员权限。
//!
//! # 一个必须讲清的事实
//!
//! WebView2 挂了，**Rust 的 HTTP 服务照常跑**（8848 与 webview 毫无关系）。
//! 所有提示里都要带上这句——「界面打不开不影响你的脚本调用」，
//! 这跟产品定位（交付物是服务，界面只是控制面板）是一致的。

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use tauri::AppHandle;

/// evergreen WebView2 Runtime 的注册表键（相对路径，HKCU / HKLM 通用）。
///
/// 这是微软文档给的固定 GUID，`pv` 值就是 runtime 版本号。两个根键都要查，
/// 详见 [`webview2_version`] 的说明。
const WV2_KEY: &str =
    r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// 官方 Evergreen Bootstrapper（约 1.5MB），它会再联网下载 runtime 本体。
const WV2_BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// WebView2 官网下载页（自动安装失败 / 用户想自己装时的兜底）。
const WV2_HOME: &str = "https://developer.microsoft.com/zh-cn/microsoft-edge/webview2/";

/// 诊断开关：强制走「WebView2 缺失」分支。
///
/// 为什么需要：装了 WebView2 的机器上没法真把它卸了来验收引导流程
/// （卸了会影响系统里别的 Electron 程序）。设了这个变量，
/// `webview2_missing()` 恒为真，弹窗、托盘项、安装引导全部按缺失路径走。
/// 只影响提示与引导，**不会真的去装**——安装仍要点消息框里的按钮。
const FAKE_NO_WEBVIEW: &str = "OCR_FAKE_NO_WEBVIEW";

/// 正在下载/安装 WebView2。
///
/// 存在的唯一原因：托盘 tooltip 有个 2 秒一轮的状态播报循环（服务运行中 / 已停止），
/// 它会把安装进度文案**立刻覆盖回去**，用户就看不到「正在下载…」了。
/// 循环侧读这个标志，为真时跳过写 tooltip。
static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 供托盘状态循环查询：安装进行中就不要抢 tooltip。
pub fn is_installing() -> bool {
    INSTALLING.load(std::sync::atomic::Ordering::Relaxed)
}

/// 安装结束（含 panic 路径）自动复位标志。
struct InstallingGuard;

impl InstallingGuard {
    fn acquire() -> Self {
        INSTALLING.store(true, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for InstallingGuard {
    fn drop(&mut self) {
        INSTALLING.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// 检测
// ---------------------------------------------------------------------------

/// 读 evergreen WebView2 Runtime 的版本号；没装返回 `None`。
///
/// **必须 HKCU 与 HKLM 都查**：每用户安装只写 HKCU；但 Win11 出厂预装、
/// 企业批量部署、以及被别的程序带安装的走的是**每机器安装**，只写
/// `HKLM\SOFTWARE\WOW6432Node\...`。只看 HKCU 会把这些机器误判成「没装」，
/// 白白弹一次卸载都卸不掉的安装引导。
pub fn webview2_version() -> Option<String> {
    if std::env::var_os(FAKE_NO_WEBVIEW).is_some() {
        return None;
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        read_pv(HKEY_CURRENT_USER).or_else(|| read_pv(HKEY_LOCAL_MACHINE))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 从指定根键读 WebView2 客户端项的 `pv` 值。
///
/// 路径里写死 `WOW6432Node`：EdgeUpdate 的客户端项固定登记在 32 位视图下，
/// 64 位进程若走默认视图（`KEY_WOW64_64KEY`）会读不到。
#[cfg(windows)]
fn read_pv(root: windows::Win32::System::Registry::HKEY) -> Option<String> {
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    let subkey: Vec<u16> = WV2_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = "pv".encode_utf16().chain(std::iter::once(0)).collect();
    // 版本串最长 "1.0.x.y"，给 64 个 u16 足够；RegGetValueW 要的是字节数
    let mut buf = vec![0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            root,
            windows::core::PCWSTR(subkey.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if ok.is_ok() && size >= 2 {
        let len = (size as usize / 2).saturating_sub(1); // 去掉结尾的 \0
        let s = String::from_utf16_lossy(&buf[..len]);
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// WebView2 是否缺失（供托盘高亮、设置页展示、安装引导判断）。
pub fn webview2_missing() -> bool {
    webview2_version().is_none()
}

/// 是否是 Win10 / Server 2016 或更新。
///
/// 必须用 `RtlGetVersion` 而不是 `GetVersionEx`：后者从 Win8.1 起受
/// manifest 兼容开关影响，没写 manifest 的进程在 Win10/11 上会被告知
/// 版本是 6.2（Win8），照它判断会把新系统误判成旧系统。
pub fn is_win10_or_newer() -> bool {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct OsVersionInfoW {
            dw_os_version_info_size: u32,
            dw_major_version: u32,
            dw_minor_version: u32,
            dw_build_number: u32,
            dw_platform_id: u32,
            sz_csd_version: [u16; 128],
        }
        #[link(name = "ntdll")]
        extern "system" {
            fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
        }
        let mut info: OsVersionInfoW = unsafe { std::mem::zeroed() };
        info.dw_os_version_info_size = std::mem::size_of::<OsVersionInfoW>() as u32;
        let status = unsafe { RtlGetVersion(&mut info) };
        status == 0 && info.dw_major_version >= 10
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// 查端口被谁占用，返回 `(PID, 进程名)`。
///
/// 用 `GetExtendedTcpTable` 而不是起 `netstat -ano` 解析文本：不起外部进程、
/// 不受 PATH 影响、没有本地化输出的解析问题。进程名走
/// `QueryFullProcessImageNameW`，拿不到名字就返回 PID 占位。
pub fn port_owner(port: u16) -> Option<(u32, String)> {
    #[cfg(windows)]
    {
        use windows::Win32::NetworkManagement::IpHelper::{
            GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_CLASS, TCP_TABLE_OWNER_PID_ALL,
        };
        use windows::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION,
        };

        #[repr(C)]
        struct MibTcpTableOwnerPid {
            num_entries: u32,
            table: [MIB_TCPROW_OWNER_PID; 1],
        }

        // 先问需要多大，再正式拉一遍
        let mut size: u32 = 0;
        const AF_INET: u32 = 2;
        let want = TCP_TABLE_CLASS(TCP_TABLE_OWNER_PID_ALL.0);
        let _ = unsafe { GetExtendedTcpTable(None, &mut size, false, AF_INET, want, 0) };
        if size == 0 || size > 8 * 1024 * 1024 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let r = unsafe {
            GetExtendedTcpTable(
                Some(buf.as_mut_ptr() as *mut _),
                &mut size,
                false,
                AF_INET,
                want,
                0,
            )
        };
        if r != 0 {
            return None;
        }
        let rows = unsafe {
            let t = &*(buf.as_ptr() as *const MibTcpTableOwnerPid);
            std::slice::from_raw_parts(
                &t.table[0] as *const MIB_TCPROW_OWNER_PID,
                t.num_entries as usize,
            )
        };
        // dwLocalPort 文档说「网络字节序」，实际低 16 位是 {hi, lo} 顺序存放。
        // 两种解释都试一遍，避免微软文档与实现不一致时静默漏判。
        for row in rows {
            let lo = (row.dwLocalPort & 0xFFFF) as u16;
            if lo != port && lo.swap_bytes() != port {
                continue;
            }
            let pid = row.dwOwningPid;
            let name = unsafe {
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    Ok(h) => {
                        let mut buf2 = vec![0u16; 260];
                        let mut len = buf2.len() as u32;
                        let ok = QueryFullProcessImageNameW(
                            h,
                            PROCESS_NAME_FORMAT(0),
                            windows::core::PWSTR(buf2.as_mut_ptr()),
                            &mut len,
                        );
                        let _ = windows::Win32::Foundation::CloseHandle(h);
                        if ok.is_ok() {
                            String::from_utf16_lossy(&buf2[..len as usize])
                                .rsplit('\\')
                                .next()
                                .unwrap_or("")
                                .to_string()
                        } else {
                            String::new()
                        }
                    }
                    Err(_) => String::new(),
                }
            };
            return Some((pid, name));
        }
        None
    }
    #[cfg(not(windows))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// 一键安装引导
// ---------------------------------------------------------------------------

/// 用系统默认浏览器打开网页（走 tauri-plugin-opener，不自己拼 ShellExecute）。
fn open_url(app: &AppHandle, url: &str) {
    use tauri_plugin_opener::OpenerExt as _;
    if let Err(e) = app.opener().open_url(url, None::<&str>) {
        tracing::warn!("打开网页失败 {url}: {e}");
    }
}

/// 托盘 tooltip 播报安装进度。
///
/// 为什么不弹进度框：原生 MessageBox 没有进度形态，自己画又要等前端起来
/// （前端这时候可能正好是坏的）。托盘 tooltip 是现成的、不依赖 WebView 的通道。
fn tray_tip(app: &AppHandle, tip: &str) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(tip));
    }
}

/// 弹一次原生消息框，返回用户选了的按钮 ID。
fn message_box(text: &str, caption: &str, style: u32) -> u32 {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_SYSTEMMODAL};
        use windows::core::PCWSTR;
        let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let c: Vec<u16> = caption.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(t.as_ptr()),
                PCWSTR(c.as_ptr()),
                windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE(
                    style | MB_ICONWARNING.0 | MB_SYSTEMMODAL.0,
                ),
            )
            .0 as u32
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (text, caption, style);
        2 // IDCANCEL
    }
}

/// 主窗口建不起来时调用：说明原因 + 给修复入口。
///
/// 两种情况分开处理，文案完全不同：
/// - 压根没装 WebView2 → 引导安装（本函数主路径）
/// - 装了但建窗仍失败（版本太老 / GPU 进程崩）→ 引导去官网更新，
///   顺带提醒可以用兼容渲染模式重开
///
/// **不阻塞调用线程之外的东西**：消息框是模态的，但此刻界面本来就还没起来，
/// 托盘也已经建好（调用点在 `setup_tray` 之后），用户随时能从托盘退出。
pub fn offer_webview2_repair(app: &AppHandle) {
    if !is_win10_or_newer() {
        offer_os_too_old(app);
        return;
    }
    if !webview2_missing() {
        offer_webview2_update(app);
        return;
    }

    let msg = "OCR Studio 的界面打不开了：这台电脑没有安装 Microsoft Edge WebView2 运行时，\
        程序的窗口没法创建。\n\n\
        不用担心你的服务——127.0.0.1:8848 接口照常在跑，脚本调用不受影响，\
        受影响的只是这个控制界面。\n\n\
        【是】立即自动安装（下载约 1.5MB 组件，装完重启程序即可，不需要管理员权限）\n\
        【否】打开官网，我自己下载安装\n\
        【取消】暂不处理（托盘图标里随时可以再找回来）";
    let pick = message_box(msg, "OCR Studio · 缺少界面组件", 3 /* MB_YESNOCANCEL */);
    match pick {
        6 => install_webview2_blocking(app),  // IDYES
        7 => open_url(app, WV2_HOME),          // IDNO
        _ => tracing::info!("用户选择暂不安装 WebView2"),
    }
}

/// 装了 WebView2 但窗口还是建不起来：多半是版本太老。
fn offer_webview2_update(app: &AppHandle) {
    let ver = webview2_version().unwrap_or_else(|| "未知版本".into());
    let msg = format!(
        "OCR Studio 的界面打不开了：已检测到 WebView2 运行时（版本 {ver}），\
        但它可能太旧或显卡进程起不来。\n\n\
        试试这两种办法：\n\
        1. 去官网把 WebView2 运行时更新到最新版，然后重启本程序；\n\
        2. 更新后仍然白屏的话，重启时程序会自动换兼容渲染模式再试一次。\n\n\
        【是】打开官网更新页面\n【否】暂不处理"
    );
    let pick = message_box(&msg, "OCR Studio · 界面组件异常", 4 /* MB_YESNO */);
    if pick == 6 {
        open_url(app, WV2_HOME);
    }
}

/// Win7 / Win8：evergreen WebView2 根本不支持，说清楚比瞎试强。
fn offer_os_too_old(app: &AppHandle) {
    let msg = "OCR Studio 的界面需要 Microsoft Edge WebView2 运行时，\
        而它只支持 Windows 10 / Server 2016 及更新版本——当前系统太旧，装不了。\n\n\
        接口服务不受系统界面限制，但本程序的可视化界面无法在旧系统上运行。\n\n\
        【是】查看系统要求说明\n【否】暂不处理";
    let pick = message_box(msg, "OCR Studio · 系统版本不受支持", 4 /* MB_YESNO */);
    if pick == 6 {
        open_url(app, WV2_HOME);
    }
}

/// 托盘菜单的「安装 WebView2」入口：用户当时选了「暂不」，从这里找回。
pub fn tray_install_webview2(app: &AppHandle) {
    if !is_win10_or_newer() {
        offer_os_too_old(app);
        return;
    }
    install_webview2_blocking(app);
}

/// 下载引导器并静默安装，全程用托盘 tooltip 播报，最后弹结果框。
fn install_webview2_blocking(app: &AppHandle) {
    let _guard = InstallingGuard::acquire();
    tray_tip(app, "正在下载 WebView2 安装组件…");
    let bootstrapper = match download_bootstrapper() {
        Ok(p) => p,
        Err(e) => {
            tray_tip(app, "OCR Studio");
            let msg = format!(
                "自动安装没成功：下载安装组件失败（{e}）。\n\n\
                可能是网络不通。已为你打开官网下载页，也可以手动下载 \
                「Evergreen Bootstrapper」运行。\n\n【是】打开官网\n【否】暂不处理"
            );
            if message_box(&msg, "OCR Studio · 安装失败", 4) == 6 {
                open_url(app, WV2_HOME);
            }
            return;
        }
    };

    tray_tip(app, "正在安装 WebView2 运行时…");
    let inst = Command::new(&bootstrapper)
        .args(["/silent", "/install"])
        .status();
    let _ = std::fs::remove_file(&bootstrapper);
    // 先复位成中性文案：状态轮询循环会在 2 秒内覆盖成「服务运行中/已停止」
    tray_tip(app, "OCR Studio");

    match inst {
        Ok(s) if s.success() => {
            let msg = "WebView2 运行时安装完成。\n\n\
                界面组件要重启程序后才会生效。\n\n\
                【是】立即重启 OCR Studio\n【否】稍后自己重启";
            if message_box(msg, "OCR Studio · 安装完成", 4) == 6 {
                restart_self();
            }
        }
        Ok(s) => {
            let msg = format!(
                "安装器跑了但返回了退出码 {}，自动安装没有成功。\n\n\
                已为你打开官网下载页，手动装一下即可。\n\n【是】打开官网\n【否】暂不处理",
                s.code().unwrap_or(-1)
            );
            if message_box(&msg, "OCR Studio · 安装失败", 4) == 6 {
                open_url(app, WV2_HOME);
            }
        }
        Err(e) => {
            let msg = format!(
                "启动安装器失败（{e}）。\n\n已为你打开官网下载页。\n\n【是】打开官网\n【否】暂不处理"
            );
            if message_box(&msg, "OCR Studio · 安装失败", 4) == 6 {
                open_url(app, WV2_HOME);
            }
        }
    }
}

/// 下载官方引导器到 `%TEMP%`。文件小（1.5MB），一把下完再落盘。
fn download_bootstrapper() -> Result<PathBuf, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    let mut resp = client
        .get(WV2_BOOTSTRAPPER_URL)
        .send()
        .map_err(|e| format!("网络请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("服务器返回 HTTP {}", resp.status()));
    }
    let dst = std::env::temp_dir().join("ocr-studio-webview2-bootstrapper.exe");
    {
        let mut f = std::fs::File::create(&dst).map_err(|e| format!("创建临时文件失败: {e}"))?;
        // 流式拷贝：引导器小，但别假设它永远小
        let _ = resp.copy_to(&mut f).map_err(|e| format!("写入失败: {e}"))?;
        f.flush().ok();
    }
    // 下到的东西要能跑：小于 100KB 基本可以断定是错误页而不是 exe
    let len = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    if len < 100 * 1024 {
        let _ = std::fs::remove_file(&dst);
        return Err(format!("下载到的文件异常（只有 {len} 字节）"));
    }
    Ok(dst)
}

/// 拉起一个自己的新进程然后退出。装完 WebView2 只有这条路能让它生效。
fn restart_self() {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = Command::new(&exe).spawn() {
                tracing::error!("重启失败（{e}），请手动双击程序");
                return;
            }
            std::process::exit(0);
        }
        Err(e) => tracing::error!("取不到自身路径，无法自动重启: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端口占用的字节序最容易写错（`dwLocalPort` 的说法与实现不一致）：
    /// 自己开一个监听端口，回查必须命中**自己的 PID**。写错字节序这条必红。
    #[test]
    fn port_owner_finds_our_own_listener() {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind 临时端口");
        let port = l.local_addr().expect("local_addr").port();
        let (pid, _name) = port_owner(port).expect("自建监听端口应能查到占用者");
        assert_eq!(pid, std::process::id(), "PID 对不上，多半是端口字节序比错了");
        drop(l);
    }

    /// 没人用的端口要返回 None —— 否则「端口被占用」的报错会指向空气。
    #[test]
    fn port_owner_none_for_released_port() {
        let port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            l.local_addr().expect("local_addr").port()
        };
        // 这里已释放；极短时间内被别的进程抢走的概率可忽略，
        // 万一撞上，本测试只会偶发失败一次，不会掩盖真 bug。
        assert!(port_owner(port).is_none(), "端口 {port} 已释放却仍报占用");
    }

    #[test]
    fn running_windows_is_win10_or_newer() {
        assert!(is_win10_or_newer(), "开发机至少是 Win10；这条挂了说明 RtlGetVersion 读错了");
    }

    /// 装了就要读出像样的版本号（形如 `128.0.2739.42`），没装就 None。
    /// 两种结果都合法，但**不能是空串或乱码**。
    #[test]
    fn webview2_version_is_wellformed() {
        match webview2_version() {
            None => {}
            Some(v) => {
                assert!(!v.trim().is_empty(), "版本号是空串");
                let first = v.chars().next().unwrap();
                assert!(first.is_ascii_digit(), "版本号首字符不是数字: {v}");
                assert!(v.contains('.'), "版本号里没有点: {v}");
            }
        }
        // missing 与 version 必须一致，否则托盘高亮项和引导会打架
        assert_eq!(webview2_missing(), webview2_version().is_none());
    }

    #[test]
    fn installing_flag_defaults_to_false() {
        assert!(!is_installing());
        {
            let _g = InstallingGuard::acquire();
            assert!(is_installing());
        }
        assert!(!is_installing(), "守卫析构必须复位标志");
    }
}
