//! 键鼠注入：让 AI 从「看得见」变成「能下手」。
//!
//! 为什么需要这一层：找图/OCR 只能回答「它在哪」，用户真正要的是「帮我点它」。
//! 没有注入能力，AI 每次都得让用户自己动手，自动化闭环就断了。
//!
//! 三条实现约束（踩过才知道）：
//!
//! 1. **坐标必须是虚拟屏绝对坐标**，用 `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`
//!    再按「虚拟屏宽高 - 1」归一化到 0..65535。用主屏尺寸归一化的话，多显示器
//!    （尤其副屏在主屏左侧、原点为负）点下去会整体偏移一整屏。
//! 2. **注入的输入只会进「前台窗口」**。想操作某个具体窗口必须先 `focus_window`，
//!    而 `SetForegroundWindow` 受前台锁限制可能静默失败（返回 false），
//!    调用方要能看见这个失败，不能假装成功。
//! 3. **中文/emoji 走 `KEYEVENTF_UNICODE`**（一个 UTF-16 码元一对 down/up），
//!    不要试着用虚拟键码 —— 键盘布局里根本没有中文键。
//!
//! 安全边界：只做「模拟用户操作」，不读不写别的进程内存，不提权。

use std::time::Duration;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEINPUT, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSE_EVENT_FLAGS, VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN,
    VK_END, VK_ESCAPE, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN,
    VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

/// 按下与抬起之间的停顿（毫秒）。
///
/// 0 也能用，但很多界面（Electron、WPF）对「同一 tick 内 down/up」会判成拖拽而
/// 不是点击。20ms 在人类感知上仍是「瞬间」，却能让 99% 的控件正确响应。
const TAP_MS: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

impl Button {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "left" | "l" | "左" | "左键" => Some(Button::Left),
            "right" | "r" | "右" | "右键" => Some(Button::Right),
            "middle" | "m" | "中" | "中键" => Some(Button::Middle),
            _ => None,
        }
    }
}

fn sleep_ms(ms: u64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// 屏幕坐标 → 归一化绝对坐标（0..65535）。
fn to_absolute(x: i32, y: i32) -> (i32, i32) {
    let vs = crate::capture::virtual_screen();
    // 减 1 是 Windows 的历史行为：归一化区间是 [0, 65535]，映射到 (0..宽-1)。
    let w = (vs.width - 1).max(1) as f64;
    let h = (vs.height - 1).max(1) as f64;
    let nx = ((x - vs.x) as f64 / w * 65535.0).round() as i32;
    let ny = ((y - vs.y) as f64 / h * 65535.0).round() as i32;
    (nx.clamp(0, 65535), ny.clamp(0, 65535))
}

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 发一批输入事件。被系统拒绝（UIPI/桌面隔离）时给能听懂的错。
fn send(inputs: &[INPUT]) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if n as usize != inputs.len() {
        return Err(format!(
            "系统拒绝了这次输入（只接受 {n}/{} 个事件）。\
             常见原因：目标窗口以管理员身份运行（提权窗口收不到普通进程的输入），\
             或锁屏/安全桌面。",
            inputs.len()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 鼠标
// ---------------------------------------------------------------------------

/// 把光标移到屏幕坐标。
pub fn move_to(x: i32, y: i32) -> Result<(), String> {
    let (nx, ny) = to_absolute(x, y);
    send(&[mouse_input(
        nx,
        ny,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )])
}

/// 在屏幕坐标点一下。`count = 2` 是双击。
pub fn click(x: i32, y: i32, button: Button, count: u32) -> Result<(), String> {
    move_to(x, y)?;
    // 先让界面收到 hover（下拉菜单/悬停高亮靠它展开），再按
    sleep_ms(30);

    let (down, up) = match button {
        Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let times = count.clamp(1, 3);
    for i in 0..times {
        send(&[mouse_input(0, 0, 0, down)])?;
        sleep_ms(TAP_MS);
        send(&[mouse_input(0, 0, 0, up)])?;
        if i + 1 < times {
            // 双击间隔要短于系统 DoubleClickTime（默认 500ms），否则会变成两次单击
            sleep_ms(60);
        }
    }
    Ok(())
}

/// 从一点拖到另一点（按下 → 分步移动 → 抬起）。
///
/// 分步移动不能省：很多控件靠 `WM_MOUSEMOVE` 的连续性判断拖拽，
/// 直接「起点按下、终点抬起」会被当成一次点击。
pub fn drag(from: (i32, i32), to: (i32, i32), button: Button) -> Result<(), String> {
    let (down, up) = match button {
        Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    move_to(from.0, from.1)?;
    sleep_ms(30);
    send(&[mouse_input(0, 0, 0, down)])?;

    const STEPS: i32 = 12;
    for i in 1..=STEPS {
        let x = from.0 + (to.0 - from.0) * i / STEPS;
        let y = from.1 + (to.1 - from.1) * i / STEPS;
        let (nx, ny) = to_absolute(x, y);
        send(&[mouse_input(
            nx,
            ny,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )])?;
        sleep_ms(12);
    }
    sleep_ms(40);
    send(&[mouse_input(0, 0, 0, up)])
}

/// 滚轮。`delta > 0` 向上，一格 = 120。
pub fn scroll(x: i32, y: i32, delta: i32) -> Result<(), String> {
    move_to(x, y)?;
    sleep_ms(20);
    // mouseData 是 u32，向下滚要给负数的补码
    let data = delta.clamp(-12000, 12000) as u32;
    send(&[mouse_input(0, 0, data, MOUSEEVENTF_WHEEL)])
}

/// 当前前台窗口句柄（0 表示没有）。
pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// 当前光标位置（屏幕坐标）。`scroll` 不传坐标时用它，避免要求调用方先知道鼠标在哪。
pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    // 失败（极少见，桌面切换瞬间）就退回 0,0 —— 调用方还有 region 兜底
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}

/// 把窗口提到前台（最小化的先还原）。
///
/// `SetForegroundWindow` 受前台锁限制会**静默失败**（返回 false）—— 系统不允许
/// 后台进程随便抢焦点。这里如实返回错误，让调用方改用「点任务栏 / 用点击坐标」
/// 之类的替代方案，而不是以为前置成功了。
pub fn focus_window(hwnd: isize) -> Result<bool, String> {
    if hwnd == 0 {
        return Err("窗口句柄是 0，先调 list_windows 拿 hwnd 或 title".to_string());
    }
    let h = HWND(hwnd as *mut std::ffi::c_void);
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
            sleep_ms(120);
        }
        let ok = SetForegroundWindow(h).as_bool();
        Ok(ok)
    }
}

// ---------------------------------------------------------------------------
// 键盘
// ---------------------------------------------------------------------------

/// 打一段文本（支持中文/emoji，走 Unicode 注入）。
pub fn type_text(text: &str) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    // 一次最多塞这么多码元，避免超大字符串把 SendInput 的批次撑爆
    const CHUNK: usize = 64;
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(CHUNK) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for &u in chunk {
            // 代理对要连续发；中间不插停顿，否则会被拆成两个乱码字符
            inputs.push(key_input(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE));
            inputs.push(key_input(
                VIRTUAL_KEY(0),
                u,
                KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
            ));
        }
        send(&inputs)?;
        sleep_ms(8);
    }
    Ok(())
}

/// 名字 → 虚拟键。
fn vk_of(name: &str) -> Option<VIRTUAL_KEY> {
    let n = name.trim().to_ascii_lowercase();
    let vk = match n.as_str() {
        "ctrl" | "control" => VK_CONTROL,
        "shift" => VK_SHIFT,
        "alt" | "menu" => VK_MENU,
        "win" | "super" | "cmd" => VK_LWIN,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "space" | "空格" => VK_SPACE,
        "backspace" | "back" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        _ => {
            // F1..F12
            if let Some(rest) = n.strip_prefix('f') {
                if let Ok(i) = rest.parse::<u16>() {
                    if (1..=12).contains(&i) {
                        // VK_F1 = 0x70
                        return Some(VIRTUAL_KEY(0x70 + i - 1));
                    }
                }
            }
            // 单字符：字母/数字/常见符号按 ASCII 大写取值（虚拟键码与大小写无关）
            let mut it = n.chars();
            let c = it.next()?;
            if it.next().is_some() {
                return None;
            }
            let upper = c.to_ascii_uppercase() as u32;
            if (0x30..=0x5A).contains(&upper) {
                return Some(VIRTUAL_KEY(upper as u16));
            }
            return None;
        }
    };
    Some(vk)
}

/// 按组合键，如 `enter`、`ctrl+s`、`ctrl+shift+esc`、`alt+f4`、`win+r`。
///
/// 修饰键先按下、最后逆序抬起：反了会把 `ctrl+shift+esc` 变成「按着 esc 再按 shift」，
/// 触发的是完全不同的快捷键。
pub fn press_keys(combo: &str) -> Result<(), String> {
    let parts: Vec<&str> = combo
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("按键是空的，写法如 \"enter\" 或 \"ctrl+s\"".to_string());
    }
    let mut keys = Vec::with_capacity(parts.len());
    for p in &parts {
        let vk = vk_of(p).ok_or_else(|| {
            format!(
                "不认识按键 {p:?}。可用：ctrl / shift / alt / win / enter / tab / esc / \
                 space / backspace / delete / insert / home / end / pageup / pagedown / \
                 up / down / left / right / f1..f12 / 单个字母数字。\
                 要输入整段文字（含中文）请用 type_text。"
            )
        })?;
        keys.push(vk);
    }

    // 按下：按书写顺序；抬起：逆序
    let mut down = Vec::with_capacity(keys.len());
    for vk in &keys {
        down.push(key_input(*vk, 0, KEYBD_EVENT_FLAGS(0)));
    }
    send(&down)?;
    sleep_ms(30);
    let mut up = Vec::with_capacity(keys.len());
    for vk in keys.iter().rev() {
        up.push(key_input(*vk, 0, KEYEVENTF_KEYUP));
    }
    send(&up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_table_covers_common_keys() {
        assert_eq!(vk_of("ctrl"), Some(VK_CONTROL));
        assert_eq!(vk_of("Enter"), Some(VK_RETURN));
        assert_eq!(vk_of("esc"), Some(VK_ESCAPE));
        assert_eq!(vk_of("f12"), Some(VIRTUAL_KEY(0x7B)));
        assert_eq!(vk_of("a"), Some(VIRTUAL_KEY(0x41)));
        assert_eq!(vk_of("7"), Some(VIRTUAL_KEY(0x37)));
        // 不认识的必须返回 None，不能瞎猜一个键（会变成往用户窗口里乱打字）
        assert_eq!(vk_of("hyper"), None);
        assert_eq!(vk_of("f13"), None);
    }

    #[test]
    fn button_parse_is_lenient() {
        assert_eq!(Button::parse(""), Some(Button::Left));
        assert_eq!(Button::parse("Right"), Some(Button::Right));
        assert_eq!(Button::parse("中键"), Some(Button::Middle));
        assert_eq!(Button::parse("side"), None);
    }

    /// 空输入必须直接返回成功：SendInput 传空切片在部分系统上行为未定义。
    #[test]
    fn empty_batches_are_noops() {
        assert!(send(&[]).is_ok());
        assert!(type_text("").is_ok());
    }
}
