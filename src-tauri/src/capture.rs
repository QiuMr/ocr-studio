//! Windows 截图：屏幕区域、指定窗口、窗口枚举。
//!
//! 实现选型：
//! - **屏幕**用 `BitBlt(SRCCOPY | CAPTUREBLT)`：兼容性最好，能抓到分层窗口。
//! - **窗口**优先 `PrintWindow(PW_RENDERFULLCONTENT)`：对硬件加速/分层窗口有效；
//!   失败回退 `BitBlt(GetWindowDC(hwnd))`。
//! - 取像素统一走 `GetDIBits` 32bpp top-down，自己转 RGB。
//!   不依赖 `image` 的 Windows 后端，避免引入额外特性开关。
//!
//! 已知限制（诚实记录）：
//! - 多显示器 + 不同 DPI 时，坐标系统一按**主屏 DPI** 处理。混合 DPI 场景下
//!   `GetSystemMetrics(SM_XVIRTUALSCREEN)` 与实际物理像素会有偏差，需按
//!   per-monitor DPI 逐个换算（未实现）。
//! - 抓取受 UAC 保护的窗口会失败（提权窗口不在同一桌面合成层）。

use image::RgbImage;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetWindowDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
    DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GW_OWNER,
    IsWindowVisible, SetProcessDPIAware,
};

/// 屏幕区域（虚拟屏幕坐标，单位 = 缩放后的像素）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRegion {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 虚拟屏幕的完整范围。
pub fn virtual_screen() -> ScreenRegion {
    use windows::Win32::UI::WindowsAndMessaging::{
        SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    unsafe {
        ScreenRegion {
            x: windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CXVIRTUALSCREEN),
            height: windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// 让进程感知 DPI，保证截图坐标与 `GetSystemMetrics` 一致。
///
/// 幂等且失败无害（Tauri 可能已通过 manifest 设为 PerMonitorV2）。
pub fn ensure_dpi_aware() {
    unsafe {
        let _ = SetProcessDPIAware();
    }
}

/// 内部辅助：从「源 DC」按偏移抓取区域。
///
/// 单独抽出来是因为屏幕（GetDC(NULL) + 偏移）与窗口回退（GetWindowDC + 0 偏移）
/// 两条路径只有取像素的方式不同。
fn grab_from_source_dc(
    src_dc: HDC,
    offset_x: i32,
    offset_y: i32,
    w: i32,
    h: i32,
    release_src: bool,
) -> Result<RgbImage, String> {
    unsafe {
        let hdc_mem = CreateCompatibleDC(src_dc);
        if hdc_mem.is_invalid() {
            if release_src {
                let _ = ReleaseDC(HWND::default(), src_dc);
            }
            return Err("CreateCompatibleDC 失败".to_string());
        }
        let hbmp = CreateCompatibleBitmap(src_dc, w, h);
        if hbmp.is_invalid() {
            let _ = DeleteDC(hdc_mem);
            if release_src {
                let _ = ReleaseDC(HWND::default(), src_dc);
            }
            return Err("CreateCompatibleBitmap 失败".to_string());
        }
        let hbmp_obj: HGDIOBJ = hbmp.into();
        let old = SelectObject(hdc_mem, hbmp_obj);
        let ok = BitBlt(
            hdc_mem,
            0,
            0,
            w,
            h,
            src_dc,
            offset_x,
            offset_y,
            SRCCOPY | CAPTUREBLT,
        );
        if ok.is_err() {
            let _ = SelectObject(hdc_mem, old);
            let _ = DeleteObject(hbmp_obj);
            let _ = DeleteDC(hdc_mem);
            if release_src {
                let _ = ReleaseDC(HWND::default(), src_dc);
            }
            return Err("BitBlt 失败".to_string());
        }

        let img = dib_from_mem_dc(hdc_mem, hbmp, w, h);
        let _ = SelectObject(hdc_mem, old);
        let _ = DeleteObject(hbmp_obj);
        let _ = DeleteDC(hdc_mem);
        if release_src {
            let _ = ReleaseDC(HWND::default(), src_dc);
        }
        img
    }
}

/// 从内存 DC + 其中的位图取出像素。
fn dib_from_mem_dc(
    hdc_mem: HDC,
    hbmp: windows::Win32::Graphics::Gdi::HBITMAP,
    w: i32,
    h: i32,
) -> Result<RgbImage, String> {
    let (w, h) = (w.max(0), h.max(0));
    if w == 0 || h == 0 {
        return Err("截图尺寸为 0".to_string());
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default(); 1],
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let got = unsafe {
        GetDIBits(
            hdc_mem,
            hbmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
            &bi as *const BITMAPINFO as *mut BITMAPINFO,
            DIB_RGB_COLORS,
        )
    };
    if got == 0 {
        return Err("GetDIBits 失败".to_string());
    }
    // 一次性构造 RGB 缓冲：1920×1080 就是 200 万像素，用 put_pixel 逐点写
    // 在 debug 构建下光这一步就要 200ms+（实测整条找图链路 427ms 的大头在这）。
    let n = (w as usize) * (h as usize);
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let s = i * 4;
        let d = i * 3;
        // 32bpp DIB 是 BGRA
        out[d] = buf[s + 2];
        out[d + 1] = buf[s + 1];
        out[d + 2] = buf[s];
    }
    RgbImage::from_raw(w as u32, h as u32, out)
        .ok_or_else(|| "构造 RGB 图像失败（尺寸与缓冲不匹配）".to_string())
}

/// 截取虚拟屏幕的指定区域。`None` 表示整个虚拟屏幕。
pub fn capture_region(region: Option<ScreenRegion>) -> Result<RgbImage, String> {
    let vs = virtual_screen();
    let r = region.unwrap_or(vs);
    // 区域不允许超出虚拟屏幕
    let w = r.width.min(vs.width + vs.x - r.x).min(vs.width);
    let h = r.height.min(vs.height + vs.y - r.y).min(vs.height);
    if w <= 0 || h <= 0 {
        return Err(format!(
            "区域无效: {r:?}，虚拟屏幕 {vs:?}"
        ));
    }
    unsafe {
        let src = GetDC(HWND::default());
        if src.is_invalid() {
            return Err("GetDC(NULL) 失败".to_string());
        }
        grab_from_source_dc(src, r.x, r.y, w, h, true)
    }
}

/// 截取指定窗口的客户区+非客户区（含标题栏）。
pub fn capture_window(hwnd: isize) -> Result<RgbImage, String> {
    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    let rect = window_rect(hwnd)?;
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return Err(format!("窗口尺寸无效: {w}x{h}"));
    }

    unsafe {
        let src = GetWindowDC(hwnd);
        if src.is_invalid() {
            return Err("GetWindowDC 失败".to_string());
        }
        // 先试 PrintWindow（对硬件加速窗口更可靠），失败再走 BitBlt
        let result = print_window(hwnd, src, w, h).and_then(|img| {
            // PrintWindow 有个经典的假成功：返回 TRUE 但位图全黑（典型于某些
            // 沙箱/远程桌面/独显合成窗口）。这种情况必须当失败处理，否则调用方
            // 会拿到一张黑图去做模板匹配，然后报"找不到"。
            if is_blank(&img) {
                Err("PrintWindow 返回全黑图（假成功）".to_string())
            } else {
                Ok(img)
            }
        }).or_else(|e| {
            tracing::debug!("PrintWindow 不可用({e})，回退 BitBlt");
            grab_from_source_dc(src, 0, 0, w, h, false)
        });
        let _ = ReleaseDC(hwnd, src);
        result
    }
}

/// `PrintWindow(PW_RENDERFULLCONTENT)` 路径。
fn print_window(
    hwnd: HWND,
    hdc_src: HDC,
    w: i32,
    h: i32,
) -> Result<RgbImage, String> {
    unsafe {
        let hdc_mem = CreateCompatibleDC(hdc_src);
        if hdc_mem.is_invalid() {
            return Err("CreateCompatibleDC 失败".to_string());
        }
        let hbmp = CreateCompatibleBitmap(hdc_src, w, h);
        if hbmp.is_invalid() {
            let _ = DeleteDC(hdc_mem);
            return Err("CreateCompatibleBitmap 失败".to_string());
        }
        let hbmp_obj: HGDIOBJ = hbmp.into();
        let old = SelectObject(hdc_mem, hbmp_obj);
        let ok = PrintWindow(hwnd, hdc_mem, PRINT_WINDOW_FLAGS(2 /* PW_RENDERFULLCONTENT */));
        if !ok.as_bool() {
            let _ = SelectObject(hdc_mem, old);
            let _ = DeleteObject(hbmp_obj);
            let _ = DeleteDC(hdc_mem);
            return Err("PrintWindow 返回 FALSE".to_string());
        }
        let img = dib_from_mem_dc(hdc_mem, hbmp, w, h);
        let _ = SelectObject(hdc_mem, old);
        let _ = DeleteObject(hbmp_obj);
        let _ = DeleteDC(hdc_mem);
        img
    }
}

/// 判断整张图是否接近全黑（PrintWindow 假成功的典型表现）。
///
/// 阈值取「99% 以上的像素亮度 < 8」：允许少量非黑内容（如阴影/边框），
/// 但真正的窗口画面不可能这么黑。
fn is_blank(img: &RgbImage) -> bool {
    let total = (img.width() as u64) * (img.height() as u64);
    if total == 0 {
        return true;
    }
    let mut dark = 0u64;
    for p in img.pixels() {
        if p[0] < 8 && p[1] < 8 && p[2] < 8 {
            dark += 1;
        }
    }
    dark * 100 >= total * 99
}

/// 可见窗口信息。
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    pub rect: ScreenRegion,
}

/// 窗口完整矩形（含标题栏与边框），屏幕坐标。
///
/// 必须与 `GetWindowDC` / `PrintWindow` 的坐标系配对 —— 这两个 API 的原点是
/// **窗口左上角**（含非客户区），而不是客户区左上角。早先用
/// `ClientToScreen` + `GetClientRect` 取尺寸，配 `GetWindowDC` 截图会右侧/底部
/// 缺一块（尺寸按客户区算，内容按窗口算）。改用 `GetWindowRect` 后自洽。
fn window_rect(hwnd: HWND) -> Result<RECT, String> {
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return Err("GetWindowRect 失败".to_string());
        }
        if r.right <= r.left || r.bottom <= r.top {
            return Err(format!(
                "窗口矩形无效: ({}, {})-({}, {})",
                r.left, r.top, r.right, r.bottom
            ));
        }
        Ok(r)
    }
}

/// 供 HTTP 层换算坐标用：窗口在屏幕上的位置与尺寸。
pub fn window_region(hwnd: isize) -> Result<ScreenRegion, String> {
    let h = HWND(hwnd as *mut std::ffi::c_void);
    let r = window_rect(h)?;
    Ok(ScreenRegion {
        x: r.left,
        y: r.top,
        width: r.right - r.left,
        height: r.bottom - r.top,
    })
}

/// 枚举所有可见且有标题的顶层窗口（不含工具窗口/子窗口）。
pub fn list_windows() -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = Vec::new();

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<WindowInfo>);
        if IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER).unwrap_or_default().0.is_null()
        {
            let len = GetWindowTextLengthW(hwnd);
            if len > 0 {
                let mut buf = vec![0u16; (len + 1) as usize];
                let n = GetWindowTextW(hwnd, &mut buf);
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if !title.trim().is_empty() {
                    if let Ok(rect) = window_rect(hwnd) {
                        out.push(WindowInfo {
                            hwnd: hwnd.0 as isize,
                            title,
                            rect: ScreenRegion {
                                x: rect.left,
                                y: rect.top,
                                width: rect.right - rect.left,
                                height: rect.bottom - rect.top,
                            },
                        });
                    }
                }
            }
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// 按标题精确查找窗口（找不到返回 None；多个同名取第一个）。
pub fn find_window(title: &str) -> Option<isize> {
    list_windows()
        .into_iter()
        .find(|w| w.title == title)
        .map(|w| w.hwnd)
}

/// 按标题子串模糊查找。
pub fn find_window_contains(needle: &str) -> Option<isize> {
    list_windows()
        .into_iter()
        .find(|w| w.title.contains(needle))
        .map(|w| w.hwnd)
}
