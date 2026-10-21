"""点击屏幕坐标并截图，用于无人交互时验收界面。

用途：Tauri 窗口里切页、点按钮，都靠真实鼠标事件驱动，比改代码改默认页可靠。

用法：
    python scripts/ui_click.py <x> <y> [等待毫秒]
坐标是**屏幕绝对像素**（脚本已声明 DPI 感知，与 GetWindowRect 口径一致）。
"""

import ctypes
import sys
import time

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004


def click(x: int, y: int) -> None:
    user32.SetCursorPos(x, y)
    time.sleep(0.08)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.05)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def main() -> int:
    if len(sys.argv) < 3:
        print("用法: python scripts/ui_click.py <x> <y> [等待毫秒]", file=sys.stderr)
        return 2
    x, y = int(sys.argv[1]), int(sys.argv[2])
    wait_ms = int(sys.argv[3]) if len(sys.argv) > 3 else 600
    click(x, y)
    time.sleep(wait_ms / 1000.0)
    print(f"[INFO] 已点击 ({x}, {y})，等待 {wait_ms}ms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
