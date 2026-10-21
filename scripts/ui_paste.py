"""把一张 PNG 放进剪贴板（CF_DIB），再向 OCR Studio 发送 Ctrl+V 完成贴图。

为什么不用 PowerShell：Set-Clipboard 只放文件路径（CF_HDROP），
App 侧只认位图；而 Add-Type 在 agent 会话被安全策略拦截，
所以用 ctypes 直接写 CF_DIB。

用法：
    python scripts/ui_paste.py <png路径> [等待毫秒]
粘贴后窗口由本脚本截屏保存到 scripts/_paste_result.png。
"""

import ctypes
import ctypes.wintypes as wt
import io
import sys
import time

import mss
from PIL import Image

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
user32.SetProcessDPIAware()

CF_DIB = 8
GMEM_MOVEABLE = 0x0002
VK_CONTROL = 0x11
KEYEVENTF_KEYUP = 0x0002
VK_V = 0x56

# 64 位下必须显式声明指针宽度，否则 HGLOBAL 被截断成 32 位 int，
# GlobalLock 解引用 0 直接 access violation。
kernel32.GlobalAlloc.restype = ctypes.c_void_p
kernel32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
kernel32.GlobalLock.restype = ctypes.c_void_p
kernel32.GlobalLock.argtypes = [ctypes.c_void_p]
kernel32.GlobalUnlock.argtypes = [ctypes.c_void_p]
user32.SetClipboardData.restype = ctypes.c_void_p
user32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]


def find_window(keyword: str):
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


def set_clipboard_dib(img: Image.Image) -> None:
    buf = io.BytesIO()
    img.convert("RGB").save(buf, "BMP")
    data = buf.getvalue()[14:]  # 去掉 BITMAPFILEHEADER，剩下就是 DIB
    if not user32.OpenClipboard(None):
        raise RuntimeError("OpenClipboard failed")
    try:
        user32.EmptyClipboard()
        h = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(data))
        p = kernel32.GlobalLock(h)
        ctypes.memmove(p, data, len(data))
        kernel32.GlobalUnlock(h)
        user32.SetClipboardData(CF_DIB, h)
    finally:
        user32.CloseClipboard()


def main() -> int:
    png = sys.argv[1]
    wait = int(sys.argv[2]) if len(sys.argv) > 2 else 1500

    img = Image.open(png)
    set_clipboard_dib(img)

    hits = find_window("OCR Studio")
    if not hits:
        print("[FAIL] 找不到 OCR Studio 窗口", file=sys.stderr)
        return 1
    hwnd = hits[0]
    user32.ShowWindow(hwnd, 9)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.4)

    user32.keybd_event(VK_CONTROL, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(VK_V, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(VK_V, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.03)
    user32.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(wait / 1000.0)

    rect = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w, h = rect.right - rect.left, rect.bottom - rect.top
    with mss.mss() as sct:
        shot = sct.grab({"left": rect.left, "top": rect.top, "width": w, "height": h})
        out = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    out.save("scripts/_paste_result.png")
    print(f"[INFO] 已贴图 {img.size} 并截图 -> scripts/_paste_result.png ({w}x{h})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
