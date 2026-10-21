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
        user32.ShowWindow(hwnd, 9)
        user32.SetForegroundWindow(hwnd)
        time.sleep(0.25)

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
        rect = wt.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
    else:
        rect = wt.RECT()
        rect.left, rect.top, rect.right, rect.bottom = 0, 0, 1920, 1080
    w, h = rect.right - rect.left, rect.bottom - rect.top
    with mss.mss() as sct:
        shot = sct.grab({"left": rect.left, "top": rect.top, "width": w, "height": h})
        img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    img.save(out)
    print(f"[INFO] 点击 ({x}, {y}) 后 {wait}ms 截图 -> {out} ({w}x{h})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
