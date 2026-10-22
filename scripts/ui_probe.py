"""在一个进程里完成「激活窗口 -> 可选按键 -> 点击 -> 立刻截图」。

为什么需要它：分开调用 ui_click.py 与 shot_window.py 时，两次进程之间
Radix 的 tooltip / 焦点状态会残留（实测 tooltip 卡住不消失），
导致「点开了截图却看不到」的假失败。把动作与抓屏放到同一进程里，
并在点击后只等极短时间就抓屏，能稳定拿到弹出层展开的瞬间。

用法：
    python scripts/ui_probe.py <x> <y> <输出png> [--key VK] [--wait 毫秒]
坐标是**屏幕绝对像素**。
"""

import ctypes
import ctypes.wintypes as wt
import sys
import time

import mss
from PIL import Image

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

VK_ESCAPE = 0x1B
KEYEVENTF_KEYUP = 0x0002

VK_MENU = 0x12
HWND_TOPMOST = -1
HWND_NOTOPMOST = -2
SWP_NOMOVE = 0x0002
SWP_NOSIZE = 0x0001

# 不声明 argtypes 时 HWND(-1)（HWND_TOPMOST）按 32 位传，
# SetWindowPos 返回 0 静默失败；声明后才靠谱。
user32.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
user32.SetWindowPos.restype = wt.BOOL
user32.SetForegroundWindow.argtypes = [wt.HWND]
user32.SetForegroundWindow.restype = wt.BOOL


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


def tap(vk: int) -> None:
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.03)
    user32.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    x, y, out = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
    wait = 350
    key = None
    args = sys.argv[4:]
    for i, a in enumerate(args):
        if a == "--wait":
            wait = int(args[i + 1])
        elif a == "--key":
            key = args[i + 1]

    hwnd = None
    hits = find_window("OCR Studio")
    if hits:
        hwnd = hits[0]
        if user32.IsIconic(hwnd):
            user32.ShowWindow(hwnd, 9)
        # **顺序要紧**：先敲 ALT 解锁 Windows 的前台切换限制，再 SetWindowPos(TOPMOST)，
        # 最后才 SetForegroundWindow。反过来（先 SetForegroundWindow）会被前台锁拒绝，
        # 点击落到压在 App 上面的窗口上 —— 表现是「点了没反应，截图还是原页面」
        # （本轮就因此误判过一次，以为坐标算错了）。
        tap(VK_MENU)
        user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
        user32.SetForegroundWindow(hwnd)
        user32.BringWindowToTop(hwnd)
        time.sleep(0.6)

    if key:
        tap(int(key, 16) if key.lower().startswith("0x") else int(key))
        time.sleep(0.15)

    user32.SetCursorPos(4, 4)          # 先移开，让 hover 态复位
    time.sleep(0.1)
    user32.SetCursorPos(x, y)
    time.sleep(0.12)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(wait / 1000.0)

    if hwnd:
        # 用 DWM 真实边界，否则会截进 DWM 阴影外的桌面内容（见 shot_window.py 说明）
        rect = wt.RECT()
        if ctypes.windll.dwmapi.DwmGetWindowAttribute(
            hwnd, 9, ctypes.byref(rect), ctypes.sizeof(rect)
        ) != 0:
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
    else:
        rect = wt.RECT()
        rect.left, rect.top, rect.right, rect.bottom = 0, 0, 1920, 1080
    w, h = rect.right - rect.left, rect.bottom - rect.top
    with mss.mss() as sct:
        shot = sct.grab({"left": rect.left, "top": rect.top, "width": w, "height": h})
        img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    img.save(out)
    if hwnd:
        user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    print(f"[INFO] 点击 ({x}, {y}) 后 {wait}ms 截图 -> {out} ({w}x{h})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
