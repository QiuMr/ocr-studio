//! 版本检查与自更新（免安装绿色版）。
//!
//! # 为什么不用 `tauri-plugin-updater`
//!
//! 官方插件只认**安装器**（NSIS / MSI）产出的、带签名的更新包。本项目
//! `bundle.active = false`，发的是免安装 zip，硬接官方 updater 只会在
//! 「找不到安装器」和「已是最新」之间反复。
//!
//! 绿色版反而更好做：换版本 = 整目录覆盖，没有注册表、没有服务、没有卸载项。
//! 用户数据（`config.json` / `prompt_refs/` / 日志）在 `%APPDATA%`，不在包里，
//! 覆盖时天然不受影响。
//!
//! # 更新流程（顺序有原因，别调换）
//!
//! 1. 下载 zip 到 `%TEMP%`，按 `latest.json` 里的大小和 sha256 校验。
//! 2. 用系统 `tar.exe` 解压（Win10+ 自带 bsdtar，认 zip）。**不引 zip crate**：
//!    解压只在更新时用一次，为它多编一个依赖不值。
//! 3. 把**当前 exe 复制一份**到 TEMP 当「更新助手」。正在运行的 exe 不能覆盖自己，
//!    而副本跑在 TEMP，所以安装目录里的 exe 从一开始就没有文件锁。
//! 4. 拉起助手（带 PID），主进程退出。
//! 5. 助手等旧进程真的消失 → 覆盖文件 → 启动新 exe。
//!
//! # 两个必须知道的坑
//!
//! - **助手必须在 `tauri::Builder` 之前分流**。它和主程序是同一个 exe，
//!   `tauri-plugin-single-instance` 会把它当成「第二个实例」直接转交并踢掉，
//!   表现为「点了更新，程序关掉就再也没起来」。
//! - **必须等进程真的退出**，不能只 sleep 几秒。`onnxruntime.dll` 由主进程加载，
//!   进程还在时覆盖 DLL 会 `ERROR_SHARING_VIOLATION`，更新到一半停住，
//!   目录处于半新半旧状态 —— 这是最难救的一种失败。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 更新源。指向 GitHub Release 的**固定资产路径**。
///
/// `releases/latest/download/<asset>` 永远指向最新一个非预发布 Release 的同名资产，
/// 所以只要发版时把 `latest.json` 一起传上去，这个 URL 一次写定、永不变更。
/// 换成别的托管（R2 / Pages）只需改这一行。
pub const FEED_URL: &str =
    "https://github.com/QiuMr/ocr-studio/releases/latest/download/latest.json";

/// 助手模式的命令行开关。
const APPLY_FLAG: &str = "--apply-update";

/// 等旧进程退出的上限。超过就放弃并提示用户手动更新，**不要在不确定的状态下覆盖**。
const WAIT_EXIT: Duration = Duration::from_secs(120);

/// 远端 `latest.json` 的结构。字段缺失一律 `default`，这样加字段不会让老版本报错。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LatestJson {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    pub url: String,
    /// 包大小（字节）。用来识别「下到一半断了」——zip 截断解压会报错，
    /// 但报错点很靠后，不如先比大小直观。
    #[serde(default)]
    pub size: u64,
    /// zip 的 sha256（小写十六进制）。缺了就跳过校验（不阻断更新）。
    #[serde(default)]
    pub sha256: Option<String>,
}

/// 检查结果。**网络失败不当错误抛**，装在 `error` 里返回 ——
/// 没网、源还没发版都是常态，弹红字只会让用户以为程序坏了。
#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    pub notes: String,
    pub url: String,
    pub size: u64,
    /// zip 的 sha256。**不发给前端**（界面用不上），只供下载校验。
    /// 前端要的是「有没有更新」，多一个长串只会让人以为要自己核对。
    #[serde(skip)]
    pub sha256: Option<String>,
    pub error: Option<String>,
}

/// 把 `curl` 式的半成品异常收敛成一句人话。
fn friendly(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "连不上更新服务器（超时）。检查网络后再试".to_string()
    } else if e.is_connect() {
        "连不上更新服务器。如果是公司网络，可能需要配代理".to_string()
    } else if e.is_decode() {
        "更新源返回的内容不是有效 JSON（latest.json 写坏了？）".to_string()
    } else {
        format!("检查更新失败：{e}")
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // 检查更新是启动时的旁路动作，绝不能因为网络慢把界面卡住
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(8))
        // 跟随 GitHub 的 302（releases/latest/download 本身就是重定向）
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{e}"))
}

/// 拉取远端 `latest.json`。
pub fn fetch_latest() -> Result<LatestJson, String> {
    let resp = client()?
        .get(FEED_URL)
        .header("User-Agent", concat!("OCR-Studio/", env!("OCR_STUDIO_VERSION")))
        .send()
        .map_err(|e| friendly(&e))?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // 还没发过 Release 时就是这个，属正常状态而不是故障
        return Err("更新源还没有发布记录".to_string());
    }
    if !status.is_success() {
        return Err(format!("更新源返回 HTTP {status}"));
    }
    resp.json::<LatestJson>()
        .map_err(|e| format!("更新源返回的内容无法解析：{e}"))
}

/// 检查是否有新版本。**任何异常都收敛进返回值**，不 panic、不抛错。
pub fn check() -> UpdateCheck {
    let current = crate::version::VERSION.to_string();
    match fetch_latest() {
        Ok(l) => {
            let has = crate::version::cmp_version(&l.version, &current)
                == std::cmp::Ordering::Greater;
            UpdateCheck {
                current,
                latest: l.version,
                has_update: has,
                notes: l.notes,
                url: l.url,
                size: l.size,
                sha256: l.sha256,
                error: None,
            }
        }
        Err(e) => UpdateCheck {
            current: current.clone(),
            latest: current,
            has_update: false,
            notes: String::new(),
            url: String::new(),
            size: 0,
            sha256: None,
            error: Some(e),
        },
    }
}

/// 下载 + 解压 + 拉起助手。成功后调用方应立即退出进程。
///
/// `on_progress(已下载, 总大小)` 用于给界面推进度；总大小未知时为 0。
pub fn download_and_prepare<F>(info: &UpdateCheck, mut on_progress: F) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    if !info.has_update {
        return Err("已经是最新版本，不需要更新".to_string());
    }
    let stage_root = temp_root()?;
    let ver_dir = stage_root.join(&info.latest);

    // 清掉上一次失败的残留（解压目录 / 旧 zip）。残留本身不影响正确性，
    // 但会让「目录里一半旧文件」的排查变得很乱。
    let _ = std::fs::remove_dir_all(&ver_dir);
    std::fs::create_dir_all(&ver_dir).map_err(|e| format!("建更新暂存目录失败：{e}"))?;

    let zip_path = stage_root.join(format!("{}.zip", info.latest));
    download(&info.url, &zip_path, info.size, info.sha256.as_deref(), &mut on_progress)?;
    extract(&zip_path, &ver_dir)?;

    // 防呆：解压完连主 exe 都没有，说明包不对。宁可中止，也不要把目录覆盖成空的。
    if !ver_dir.join(EXE_NAME).exists() {
        return Err(format!(
            "更新包里没有 {EXE_NAME}，可能下错了资产。已中止，当前版本未受影响"
        ));
    }

    launch_helper(&ver_dir)
}

/// 主程序 exe 名。**带空格**（`productName` 决定），替换时要精确。
const EXE_NAME: &str = "OCR Studio.exe";

fn temp_root() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("ocr-studio-update");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建临时目录失败：{e}"))?;
    Ok(dir)
}

fn download<F>(
    url: &str,
    dst: &Path,
    expect_size: u64,
    expect_sha: Option<&str>,
    on_progress: &mut F,
) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    // `Sha256::new()` / `finalize()` 都是 Digest trait 的方法，得先进作用域
    use sha2::Digest as _;

    let mut resp = client()?
        .get(url)
        // 下载 200MB 不能沿用 15s 的总超时；这里只约束「多久没收到数据」
        .timeout(Duration::from_secs(600))
        .header("User-Agent", concat!("OCR-Studio/", env!("OCR_STUDIO_VERSION")))
        .send()
        .map_err(|e| friendly(&e))?;
    if !resp.status().is_success() {
        return Err(format!("下载更新包失败：HTTP {}", resp.status()));
    }
    let total = if expect_size > 0 {
        expect_size
    } else {
        resp.content_length().unwrap_or(0)
    };

    let mut file = std::fs::File::create(dst).map_err(|e| format!("写临时文件失败：{e}"))?;
    let mut hasher = sha2::Sha256::new();
    let mut done: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = std::io::Read::read(&mut resp, &mut buf)
            .map_err(|e| format!("下载中断：{e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("写入失败：{e}"))?;
        hasher.update(&buf[..n]);
        done += n as u64;
        on_progress(done, total);
    }
    drop(file);

    if expect_size > 0 && done != expect_size {
        let _ = std::fs::remove_file(dst);
        return Err(format!(
            "更新包大小不对（期望 {expect_size} 字节，实际 {done}）。可能是下载被截断，请重试"
        ));
    }
    if let Some(want) = expect_sha {
        let want = want.trim().to_ascii_lowercase();
        if !want.is_empty() {
            let got = format!("{:x}", hasher.finalize());
            if got != want {
                let _ = std::fs::remove_file(dst);
                return Err(
                    "更新包校验失败（sha256 不一致），已丢弃。可能是下载损坏或被篡改，请重试"
                        .to_string(),
                );
            }
        }
    }
    Ok(())
}

/// 用系统 `tar.exe` 解压。Win10 1803+ 的 bsdtar 认 zip（libarchive）。
///
/// `CREATE_NO_WINDOW` 必须加：主窗口是 GUI 子系统，spawn 控制台程序时
/// 不加这个标志会**弹出黑框**闪一下，用户会以为中毒了。
fn extract(zip: &Path, out: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let tar = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("System32")
        .join("tar.exe");
    if !tar.exists() {
        return Err(format!(
            "系统里找不到 {}，无法解压更新包。请手动下载更新",
            tar.display()
        ));
    }
    let out_s = out.to_string_lossy().to_string();
    let zip_s = zip.to_string_lossy().to_string();
    // 中文路径 + 空格：传参数组由系统负责加引号，不要自己拼字符串
    let st = std::process::Command::new(&tar)
        .args(["-xf", &zip_s, "-C", &out_s])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("解压失败：{e}"))?;
    if !st.success() {
        return Err(format!(
            "解压更新包失败（tar 退出码 {:?}）。请手动下载更新",
            st.code()
        ));
    }
    Ok(())
}

/// 复制当前 exe 到临时目录并拉起它，让它替我们在主进程退出后覆盖文件。
fn launch_helper(staging: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let cur = std::env::current_exe().map_err(|e| format!("取当前程序路径失败：{e}"))?;
    let install_dir = cur
        .parent()
        .ok_or_else(|| "取当前程序目录失败".to_string())?
        .to_path_buf();

    let stage_root = temp_root()?;
    // 先清掉旧助手（上上次更新留下的）。删不掉就算了，不影响这次。
    if let Ok(entries) = std::fs::read_dir(&stage_root) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("helper-") && name.ends_with(".exe") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let helper = stage_root.join(format!("helper-{}.exe", crate::version::VERSION));
    std::fs::copy(&cur, &helper).map_err(|e| {
        format!("无法准备更新助手：{e}（如果程序装在 Program Files，请换到有写权限的目录）")
    })?;

    let st = std::process::Command::new(&helper)
        .arg(APPLY_FLAG)
        .arg(staging)
        .arg(&install_dir)
        .arg(std::process::id().to_string())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("拉起更新助手失败：{e}"))?;
    tracing::info!(helper = %helper.display(), pid = st.id(), "更新助手已启动，主进程准备退出");
    Ok(())
}

/// 如果本进程是「更新助手」，执行替换并返回退出码；不是则返回 `None`。
///
/// **必须在 `tauri::Builder::default()` 之前调用** —— 否则单实例插件会
/// 把这个助手当成重复启动的第二个实例，直接转交控制权并退出。
pub fn run_apply_if_requested() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == APPLY_FLAG)?;
    let rest: Vec<&String> = args.iter().skip(pos + 1).collect();
    if rest.len() < 3 {
        eprintln!("{APPLY_FLAG} 参数不足：需要 <暂存目录> <安装目录> <旧进程PID>");
        return Some(2);
    }
    let (staging, install, pid) = (
        PathBuf::from(rest[0]),
        PathBuf::from(rest[1]),
        rest[2].parse::<u32>().unwrap_or(0),
    );
    Some(apply(staging, install, pid))
}

/// 助手主逻辑：等旧进程退出 → 覆盖 → 拉起新版。
fn apply(staging: PathBuf, install: PathBuf, old_pid: u32) -> i32 {
    tracing::info!(
        staging = %staging.display(),
        install = %install.display(),
        old_pid,
        "更新助手开始工作"
    );
    if !wait_pid_gone(old_pid, WAIT_EXIT) {
        // 关键决策：宁可失败也不要在旧进程还活着时覆盖。
        // DLL 被占用时覆盖会得到半个新半个旧的目录，比不更新难救得多。
        tracing::error!("等待旧进程退出超时，已放弃覆盖（目录保持原样）");
        return 3;
    }
    if let Err(e) = copy_tree(&staging, &install) {
        tracing::error!("覆盖文件失败: {e}");
        return 4;
    }
    // 清掉解压目录与 zip（失败无所谓，TEMP 会被系统回收）。
    // 注意只删 `<ver>/` 与 `<ver>.zip`，**不能删父目录** —— 助手自己就住在里面。
    let _ = std::fs::remove_dir_all(&staging);
    if let (Some(parent), Some(name)) = (staging.parent(), staging.file_name()) {
        let _ = std::fs::remove_file(parent.join(format!("{}.zip", name.to_string_lossy())));
    }

    let exe = install.join(EXE_NAME);
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => {
            tracing::info!("新版本已启动: {}", exe.display());
            0
        }
        Err(e) => {
            tracing::error!("新版本启动失败（文件已替换成功，请手动双击）: {e}");
            5
        }
    }
}

/// 轮询 PID 是否消失。用 `OpenProcess` 而不是 tasklist：
/// tasklist 会起控制台进程、要解析输出，还要处理中文本地化。
fn wait_pid_gone(pid: u32, limit: Duration) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return true;
    }
    let start = Instant::now();
    loop {
        // SAFETY: 只查询不修改；handle 一定成对关闭。
        let alive = unsafe {
            match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                // 打不开通常是「进程已退出」（PID 不存在）
                Err(_) => false,
            }
        };
        if !alive {
            // 进程刚消失时 DLL 句柄可能还没完全释放，多给一小会儿
            std::thread::sleep(Duration::from_millis(400));
            return true;
        }
        if start.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 递归覆盖复制。目标已存在就替换（`fs::copy` 会覆盖）。
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            // 目标可能是只读（zip 解出来的旧文件），先去掉只读属性
            if let Ok(md) = std::fs::metadata(&to) {
                let mut perm = md.permissions();
                if perm.readonly() {
                    #[allow(clippy::permissions_set_readonly_false)]
                    perm.set_readonly(false);
                    let _ = std::fs::set_permissions(&to, perm);
                }
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_json_parses_minimal() {
        let raw = r#"{"version":"0.2.0","url":"https://x/y.zip"}"#;
        let l: LatestJson = serde_json::from_str(raw).unwrap();
        assert_eq!(l.version, "0.2.0");
        assert_eq!(l.notes, "");
        assert_eq!(l.size, 0);
        assert!(l.sha256.is_none());
    }

    #[test]
    fn latest_json_parses_full() {
        let raw = r#"{"version":"0.2.0","notes":"修了找图","url":"https://x/y.zip",
                      "size":12345,"sha256":"ABCDEF"}"#;
        let l: LatestJson = serde_json::from_str(raw).unwrap();
        assert_eq!(l.notes, "修了找图");
        assert_eq!(l.size, 12345);
        assert_eq!(l.sha256.as_deref(), Some("ABCDEF"));
    }

    #[test]
    fn check_reports_no_update_for_same_version() {
        // 用当前版本伪造一份，确认「相同版本不算更新」这条判定没写反
        let raw = format!(
            r#"{{"version":"{}","url":"https://x/y.zip"}}"#,
            crate::version::VERSION
        );
        let l: LatestJson = serde_json::from_str(&raw).unwrap();
        assert_ne!(
            crate::version::cmp_version(&l.version, crate::version::VERSION),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn copy_tree_overwrites_and_creates_nested() {
        let root = std::env::temp_dir().join("ocr-studio-copytest");
        let _ = std::fs::remove_dir_all(&root);
        let (src, dst) = (root.join("src"), root.join("dst"));
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"new").unwrap();
        std::fs::write(src.join("sub/b.txt"), b"nested").unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("a.txt"), b"old").unwrap();
        copy_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "new");
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/b.txt")).unwrap(),
            "nested"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
