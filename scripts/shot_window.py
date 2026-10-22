"""按窗口标题截取真实窗口画面（含客户区），用于无 GUI 交互时肉眼验收渲染。

不依赖 pywin32：全部走 ctypes + mss + PIL。
用法：python scripts/shot_window.py <窗口标题关键字> [输出png]
"""

import ctypes
import ctypes.wintypes as wt
import sys

try:
    import mss
    from PIL import Image
except ImportError:
    print("需要 mss 与 pillow: pip install mss pillow", file=sys.stderr)
    raise

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

SW_RESTORE = 9


def find_window(keyword: str):
    result = []

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
            result.append((hwnd, buf.value))
        return True

    user32.EnumWindows(cb, 0)
    return result


def main() -> int:
    keyword = sys.argv[1] if len(sys.argv) > 1 else "OCR Studio"
    out = sys.argv[2] if len(sys.argv) > 2 else "scripts/_window.png"

    wins = find_window(keyword)
    if not wins:
        print(f"[FAIL] 没找到标题含 {keyword!r} 的可见窗口")
        return 1
    hwnd, title = wins[0]
    print(f"[INFO] 窗口: {title!r} hwnd={hwnd}")

    if user32.IsIconic(hwnd):
        print("[INFO] 窗口最小化，先还原")
        user32.ShowWindow(hwnd, SW_RESTORE)
    user32.SetForegroundWindow(hwnd)

    # 取「真实可见边界」。
    #
    # 直接用 GetWindowRect 会**多出 DWM 的阴影边距**（本机实测左右各多约 15px），
    # 截图上表现为窗框外凭空多出一条桌面内容 —— 容易误判成界面画错了。
    # DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS) 拿到的才是肉眼看到的窗口。
    dwmapi = ctypes.windll.dwmapi
    rect = wt.RECT()
    hr = dwmapi.DwmGetWindowAttribute(
        hwnd, 9, ctypes.byref(rect), ctypes.sizeof(rect)  # 9 = DWMWA_EXTENDED_FRAME_BOUNDS
    )
    if hr != 0:
        print("[WARN] DWM 取边界失败，回退 GetWindowRect（可能含阴影）")
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
    w = rect.right - rect.left
    h = rect.bottom - rect.top
    print(f"[INFO] 窗口矩形: ({rect.left},{rect.top}) {w}x{h}")

    with mss.mss() as sct:
        shot = sct.grab(
            {"left": rect.left, "top": rect.top, "width": w, "height": h}
        )
        img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    img.save(out)
    print(f"[INFO] 已保存 {out} ({w}x{h})")

    # 统计「窗口内部区域」的非白像素比例：白屏 -> 接近 0
    # 裁掉边框 12px，避免窗口阴影/边缘被算成内容
    inner = img.crop((12, 12, max(13, w - 12), max(13, h - 12))).convert("L")
    px = inner.tobytes()
    non_white = sum(1 for v in px if v < 240)
    ratio = non_white / len(px) * 100
    dark = sum(1 for v in px if v < 110)
    dark_ratio = dark / len(px) * 100
    print(f"[INFO] 内部非白比例: {ratio:.2f}%  暗像素(文字)比例: {dark_ratio:.2f}%")
    ok = ratio > 1.0 and dark_ratio > 0.05
    print("[PASS] 有渲染内容" if ok else "[FAIL] 疑似白屏")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
