"""把鼠标移到指定屏幕坐标，然后滚轮滚动 N 格。

为什么单独写一个：ui_probe.py 只做「点击 + 截图」，滚轮不属于点击，
而验收长页面（服务台、接口文档）时必须能往下翻。

用法：
    python scripts/ui_scroll.py <x> <y> [格数]     # 负数向上，正数向下
"""

import ctypes
import sys
import time

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

MOUSEEVENTF_WHEEL = 0x0800
WHEEL_DELTA = 120


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    x, y = int(sys.argv[1]), int(sys.argv[2])
    steps = int(sys.argv[3]) if len(sys.argv) > 3 else 3

    user32.SetCursorPos(x, y)
    time.sleep(0.2)
    for _ in range(abs(steps)):
        user32.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, -WHEEL_DELTA if steps > 0 else WHEEL_DELTA, 0)
        time.sleep(0.06)
    time.sleep(0.35)
    print(f"[INFO] 在 ({x}, {y}) 滚动 {steps} 格")
    return 0


if __name__ == "__main__":
    sys.exit(main())
