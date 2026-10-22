"""抓 OCR Studio 窗口自身画面（不依赖窗口是否在最前）。

为什么不用「激活 + 截屏」：SetForegroundWindow 受 Windows 前台锁限制，别的窗口
刚抢过焦点时会**静默失败**，mss 抓到的就是压在 App 上面的浏览器画面。实测本机
Edge 一直盖在上面，SetWindowPos(HWND_TOPMOST) 也救不回来（可能是虚拟桌面/合成
层级问题）。改用 PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT=2)：直接从窗口
自己的合成表面取像素，被谁盖住都无所谓。

PrintWindow 对某些窗口会「返回 TRUE 但全黑」，所以这里检测到全黑会自动回退到
「激活 + 屏幕截图」，并把失败原因打出来。

用法：python scripts/win_capture.py <输出png> [窗口标题关键字]
"""

import ctypes
import ctypes.wintypes as wt
import sys
import time

from PIL import Image

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
dwmapi = ctypes.windll.dwmapi
user32.SetProcessDPIAware()

SW_RESTORE = 9
HWND_TOPMOST = -1
HWND_NOTOPMOST = -2
SWP_NOMOVE = 0x0002
SWP_NOSIZE = 0x0001
PW_RENDERFULLCONTENT = 0x00000002
SRCCOPY = 0x00CC0020
DIB_RGB_COLORS = 0


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wt.DWORD),
        ("biWidth", wt.LONG),
        ("biHeight", wt.LONG),
        ("biPlanes", wt.WORD),
        ("biBitCount", wt.WORD),
        ("biCompression", wt.DWORD),
        ("biSizeImage", wt.DWORD),
        ("biXPelsPerMeter", wt.LONG),
        ("biYPelsPerMeter", wt.LONG),
        ("biClrUsed", wt.DWORD),
        ("biClrImportant", wt.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wt.DWORD * 3)]


def find(keyword: str):
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if keyword.lower() in buf.value.lower():
            hits.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return hits


def window_rect(hwnd):
    rect = wt.RECT()
    if dwmapi.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(rect), ctypes.sizeof(rect)) != 0:
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return rect


def print_window(hwnd) -> Image.Image:
    r = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top

    hdc_win = user32.GetWindowDC(hwnd)
    hdc_mem = gdi32.CreateCompatibleDC(hdc_win)
    hbm = gdi32.CreateCompatibleBitmap(hdc_win, w, h)
    gdi32.SelectObject(hdc_mem, hbm)

    ok = user32.PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT)

    bmi = BITMAPINFO()
    bmi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bmi.bmiHeader.biWidth = w
    bmi.bmiHeader.biHeight = -h  # 负号 = 自顶向下
    bmi.bmiHeader.biPlanes = 1
    bmi.bmiHeader.biBitCount = 32
    bmi.bmiHeader.biCompression = 0  # BI_RGB

    buf = ctypes.create_string_buffer(w * h * 4)
    got = gdi32.GetDIBits(hdc_mem, hbm, 0, h, buf, ctypes.byref(bmi), DIB_RGB_COLORS)

    gdi32.DeleteObject(hbm)
    gdi32.DeleteDC(hdc_mem)
    user32.ReleaseDC(hwnd, hdc_win)

    if not ok or got == 0:
        return None
    return Image.frombuffer("RGB", (w, h), buf, "raw", "BGRX", 0, 1)


def is_blank(img: Image.Image) -> bool:
    if img is None:
        return True
    small = img.convert("L").resize((160, 100))
    px = small.tobytes()
    dark = sum(1 for v in px if v < 110)
    non_white = sum(1 for v in px if v < 240)
    # 全黑 / 全白都算失败
    return dark / len(px) < 0.002 or non_white / len(px) < 0.01


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "scripts/_win.png"
    keyword = sys.argv[2] if len(sys.argv) > 2 else "OCR Studio"

    hits = find(keyword)
    if not hits:
        print(f"[FAIL] 没找到标题含 {keyword!r} 的窗口")
        return 1
    hwnd = hits[0]

    img = print_window(hwnd)
    if is_blank(img):
        print("[WARN] PrintWindow 拿到空白图，回退到「激活 + 屏幕截图」")
        if user32.IsIconic(hwnd):
            user32.ShowWindow(hwnd, SW_RESTORE)
        user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
        user32.SetForegroundWindow(hwnd)
        time.sleep(0.6)
        import mss

        r = window_rect(hwnd)
        w, h = r.right - r.left, r.bottom - r.top
        with mss.mss() as sct:
            shot = sct.grab({"left": r.left, "top": r.top, "width": w, "height": h})
            img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
        user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
        if is_blank(img):
            print("[FAIL] 回退后仍是空白图")
            return 1

    img.save(out)
    r = window_rect(hwnd)
    print(f"[INFO] 窗口矩形: ({r.left},{r.top}) {r.right - r.left}x{r.bottom - r.top}")
    print(f"[INFO] 已保存 {out} ({img.width}x{img.height})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
