"""验收「托盘常驻」：关窗口后服务不能停。

为什么单写一个脚本：这个行为没法靠截图判断——窗口消失是**成功**的表现，
但也很可能是进程崩了。必须同时验证三件事：
  1. 窗口从可见变为不可见（不是被最小化）
  2. 进程还活着
  3. HTTP 服务还能应答 /ocr/health

用法：
    python scripts/verify_tray.py [--exe <exe路径>] [--port 8848]

不带 --exe 时只做后两步（假定已经在跑）。
"""

import ctypes
import ctypes.wintypes as wt
import json
import subprocess
import sys
import time
import urllib.request

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()

WM_CLOSE = 0x0010


def find_windows(keyword: str) -> list[int]:
    hits: list[int] = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
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


def health(port: int) -> dict | None:
    # 必须绕系统代理，否则 127.0.0.1 也会走代理拿 502
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(f"http://127.0.0.1:{port}/ocr/health", timeout=3) as r:
            return json.loads(r.read().decode("utf-8"))
    except Exception as e:  # noqa: BLE001
        print(f"[WARN] /ocr/health 失败: {e}")
        return None


def count_processes() -> int:
    """数当前有几个 OCR Studio 进程。

    注意进程名是「OCR Studio」（空格），不是 exe 文件名的小写形式，
    按 `ocr-studio` 去找会一个都匹配不到——本机踩过这个坑。
    """
    out = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq OCR Studio.exe", "/NH"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="ignore",
    ).stdout
    return out.lower().count("ocr studio.exe")


def main() -> int:
    exe = None
    port = 8848
    argv = sys.argv[1:]
    for i, a in enumerate(argv):
        if a == "--exe":
            exe = argv[i + 1]
        elif a == "--port":
            port = int(argv[i + 1])

    if exe:
        print(f"[STEP] 启动 {exe}")
        # agent shell 直接起 exe 会让 WebView2 GPU 子进程崩溃 -> 白屏，
        # 必须经 explorer 启动（见 docs 记录）。
        subprocess.run(["explorer.exe", exe], check=False)
        deadline = time.time() + 40
        while time.time() < deadline:
            time.sleep(1.0)
            if health(port) is not None and find_windows("OCR Studio"):
                break
        time.sleep(2.0)

    hwnds = find_windows("OCR Studio")
    if not hwnds:
        print("[FAIL] 找不到 OCR Studio 窗口")
        return 1
    hwnd = hwnds[0]
    if not user32.IsWindowVisible(hwnd):
        print("[FAIL] 窗口本来就不可见，无法继续")
        return 1
    print(f"[OK] 窗口可见 hwnd=0x{hwnd:X}")

    h = health(port)
    if h is None:
        print("[FAIL] 关窗口前服务就没应答")
        return 1
    print(f"[OK] 关前服务正常 code={h.get('code')} status={h.get('data', {}).get('status')}")

    print("[STEP] 发送 WM_CLOSE（模拟点右上角关闭）")
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    time.sleep(2.0)

    visible = bool(user32.IsWindowVisible(hwnd))
    print(("[OK] 窗口已隐藏" if not visible else "[FAIL] 窗口仍然可见"))

    h2 = health(port)
    alive_ok = h2 is not None and h2.get("code") == 0
    print(("[OK] 关窗口后服务仍在应答" if alive_ok else "[FAIL] 关窗口后服务没了"))

    # 托盘图标存在性：枚举通知区域里的 ToolbarWindow32 私有按钮不靠 API 也能看，
    # 但依赖 explorer 的 UI 结构，容易误判，这里只作为提示不算失败项。
    print("[INFO] 托盘图标请在任务栏右下角人工确认（可能被折叠进「隐藏的图标」）")

    # 单实例：再启动一次，不能出现第二个常驻进程，否则第二个会因端口被占
    # 而显示「服务已停止」，用户会以为程序坏了。
    single_ok = True
    if exe:
        before = count_processes()
        subprocess.run(["explorer.exe", exe], check=False)
        time.sleep(4.0)
        after = count_processes()
        print(f"[{'OK' if after <= before else 'FAIL'}] 重复启动：进程数 {before} -> {after}（应不增加）")
        single_ok = after <= before
        h3 = health(port)
        if h3 is None or h3.get("code") != 0:
            print("[FAIL] 重复启动后服务被挤掉了")
            single_ok = False
        else:
            print("[OK] 重复启动后服务仍正常")

    return 0 if (not visible and alive_ok and single_ok) else 1


if __name__ == "__main__":
    sys.exit(main())
