"""把 OCR Studio 窗口强制拉到最前，然后截图。

为什么需要它：`SetForegroundWindow` 受 Windows 前台锁限制，别的窗口刚抢过焦点时
会静默失败 —— 表现是 mss 抓到的是压在 App 上面的浏览器/资源管理器画面。
这里先 `SetWindowPos(HWND_TOPMOST)` 再设前台，最后撤回 topmost，实测稳定。

用法：python scripts/focus_shot.py <输出png>
"""

import ctypes
import ctypes.wintypes as wt
import sys
import time

import mss
from PIL import Image

user32 = ctypes.windll.user32
dwmapi = ctypes.windll.dwmapi
user32.SetProcessDPIAware()

# **必须显式声明 argtypes**：不声明时 ctypes 把 HWND(-1)（HWND_TOPMOST）按 32 位
# 传，SetWindowPos 返回 0 静默失败；声明后返回 1。配合下面的 ALT 技巧才稳。
user32.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
user32.SetWindowPos.restype = wt.BOOL
user32.SetForegroundWindow.argtypes = [wt.HWND]
user32.SetForegroundWindow.restype = wt.BOOL

SW_RESTORE = 9
HWND_TOPMOST = -1
HWND_NOTOPMOST = -2
SWP_NOMOVE = 0x0002
SWP_NOSIZE = 0x0001
VK_MENU = 0x12


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


def main() -> int:
    out = sys.argv[1] if len(sys.argv) > 1 else "scripts/_focus.png"
    hits = find("OCR Studio")
    if not hits:
        print("[FAIL] 没找到 OCR Studio 窗口")
        return 1
    hwnd = hits[0]
    if user32.IsIconic(hwnd):
        user32.ShowWindow(hwnd, SW_RESTORE)
    # 先敲一下 ALT 解锁 Windows 的前台切换限制，否则 SetForegroundWindow 会被拒。
    user32.keybd_event(VK_MENU, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(VK_MENU, 0, 2, 0)
    time.sleep(0.05)
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    user32.SetForegroundWindow(hwnd)
    user32.BringWindowToTop(hwnd)
    time.sleep(0.6)

    rect = wt.RECT()
    if dwmapi.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(rect), ctypes.sizeof(rect)) != 0:
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w, h = rect.right - rect.left, rect.bottom - rect.top
    with mss.mss() as sct:
        shot = sct.grab({"left": rect.left, "top": rect.top, "width": w, "height": h})
        img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    img.save(out)
    user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    print(f"[INFO] 窗口矩形: ({rect.left},{rect.top}) {w}x{h}")
    print(f"[INFO] 已保存 {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
