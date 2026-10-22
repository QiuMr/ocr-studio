"""点击一个输入框，把剪贴板里的文本粘进去并回车。

用途：给系统「打开文件」对话框填路径。直接 SendKeys 打中文路径不可靠
（输入法状态、键盘布局都会影响），先由 PowerShell 的 Set-Clipboard 放好路径，
这里只负责「点一下 + Ctrl+V + 回车」。

用法：
    python scripts/ui_type_path.py <x> <y>
坐标是屏幕绝对像素。
"""

import ctypes
import sys
import time

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

VK_CONTROL = 0x11
VK_V = 0x56
VK_RETURN = 0x0D
KEYEVENTF_KEYUP = 0x0002


def tap(vk: int) -> None:
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.04)
    user32.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.04)


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    x, y = int(sys.argv[1]), int(sys.argv[2])

    user32.SetCursorPos(x, y)
    time.sleep(0.2)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(0.45)

    # 全选再粘，覆盖掉框里可能已有的内容
    user32.keybd_event(VK_CONTROL, 0, 0, 0)
    tap(0x41)  # A
    user32.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.15)

    user32.keybd_event(VK_CONTROL, 0, 0, 0)
    tap(VK_V)
    user32.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.35)

    tap(VK_RETURN)
    time.sleep(0.4)
    print(f"[INFO] 已在 ({x}, {y}) 粘贴路径并回车")
    return 0


if __name__ == "__main__":
    sys.exit(main())
