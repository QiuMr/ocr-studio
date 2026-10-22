"""端到端验证「开机自启」这条链路（不靠点坐标，走 CDP 直接调命令）。

验证四件事：
  1. 命令 `get_autostart` / `set_autostart` 确实注册成功（IPC 能调到）
  2. 打开后注册表 HKCU\\...\\Run 里真的多了一项，且指向当前 exe、带 --tray
  3. 关闭后那一项真的被删掉（不是只在内存里改了个变量）
  4. 带 --tray 启动时主窗口保持隐藏（开机自启不弹窗）

为什么要开远程调试：Tauri 命令只能从 WebView 里调。与其靠模拟点击去点那个
开关（坐标一错就白跑一整轮），不如直接把命令打进去——验的是同一条 IPC 通路。

用法：
    python scripts/verify_autostart.py --exe "<OCR Studio.exe>"
"""

import ctypes
import json
import os
import subprocess
import sys
import time
import urllib.request
import winreg

PORT = "9333"
BASE = f"http://127.0.0.1:{PORT}"
RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
VALUE = "OCR Studio"
EXE = r"D:\Python测试\OCR-Studio\release\OCR-Studio\OCR Studio.exe"

opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
fails: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"[{'OK' if ok else 'FAIL'}] {name}{(' — ' + detail) if detail else ''}")
    if not ok:
        fails.append(name)


def reg_value() -> str | None:
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as k:
            v, _ = winreg.QueryValueEx(k, VALUE)
            return v
    except OSError:
        return None


def http_json(url: str):
    with opener.open(url, timeout=5) as r:
        return json.loads(r.read().decode("utf-8"))


def kill() -> None:
    subprocess.run(
        ["taskkill", "/IM", "OCR Studio.exe", "/F"], capture_output=True, check=False
    )


def launch(exe: str, extra: list[str]):
    env = dict(os.environ)
    # 不能只设 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS：程序启动时若发现
    # OCR_WEBVIEW_ARGS，会调 `additional_browser_args()` 把它整个覆盖掉，
    # 调试端口就失效了（本轮踩过）。两个参数都从 OCR_WEBVIEW_ARGS 传，
    # 顺带用软件合成绕开 agent shell 里 WebView2 GPU 进程起不来的问题。
    env.pop("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", None)
    env["OCR_WEBVIEW_ARGS"] = (
        f"--disable-gpu-compositing --remote-debugging-port={PORT} " + " ".join(extra)
    )
    env["OCR_NO_CORE"] = "1"          # 不启 OCR 与 HTTP 服务，只验托盘/IPC
    return subprocess.Popen([exe], env=env, close_fds=True)


def wait_target(timeout: float = 45.0):
    """等 CDP 出现真正的应用页面（about:blank 不算）。"""
    end = time.time() + timeout
    while time.time() < end:
        time.sleep(1.0)
        try:
            targets = http_json(f"{BASE}/json")
        except Exception:
            continue
        for t in targets:
            if t.get("type") != "page":
                continue
            u = (t.get("url") or "").lower()
            if "index.html" in u or "tauri.localhost" in u or "localhost" in u:
                return t
    return None


def count_visible() -> int:
    u = ctypes.windll.user32
    u.SetProcessDPIAware()
    hits: list[int] = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _):
        n = u.GetWindowTextLengthW(hwnd)
        if n:
            buf = ctypes.create_unicode_buffer(n + 1)
            u.GetWindowTextW(hwnd, buf, n + 1)
            if "ocr studio" in buf.value.lower() and u.IsWindowVisible(hwnd):
                hits.append(hwnd)
        return True

    u.EnumWindows(cb, 0)
    return len(hits)


class CDP:
    def __init__(self, ws_url: str):
        import websocket

        self.ws = websocket.create_connection(
            ws_url, timeout=20, http_proxy_host=None, http_proxy_port=None
        )
        self.seq = 0

    def eval(self, expr: str, wait: float = 8.0):
        self.seq += 1
        self.ws.send(
            json.dumps(
                {
                    "id": self.seq,
                    "method": "Runtime.evaluate",
                    "params": {
                        "expression": expr,
                        "returnByValue": True,
                        "awaitPromise": True,
                    },
                }
            )
        )
        end = time.time() + wait
        self.ws.settimeout(0.5)
        while time.time() < end:
            try:
                raw = self.ws.recv()
            except Exception:
                continue
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == self.seq:
                res = msg.get("result", {})
                if "exceptionDetails" in res:
                    return None, res["exceptionDetails"].get("text", "异常")
                return res.get("result", {}).get("value"), None
        return None, "超时"

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


def main() -> int:
    exe = EXE
    argv = sys.argv[1:]
    for i, a in enumerate(argv):
        if a == "--exe":
            exe = argv[i + 1]

    kill()
    time.sleep(1.5)
    print(f"[INFO] 起始注册表值: {reg_value()!r}")

    proc = None
    cdp = None
    try:
        # ---------------- 第一段：正常窗口，验 IPC 与注册表 ----------------
        print(f"[STEP] 启动（正常窗口）{exe}")
        proc = launch(exe, ["--remote-allow-origins=*"])
        target = wait_target()
        if not target:
            print("[FAIL] CDP 里没等到应用页面（WebView 可能没渲染）")
            return 1
        print(f"[INFO] 页面: {target.get('url')}")

        cdp = CDP(target["webSocketDebuggerUrl"])
        val, err = cdp.eval("JSON.stringify({hasTauri: '__TAURI_INTERNALS__' in window})")
        print(f"[INFO] 注入检测: {val or err}")

        val, err = cdp.eval("window.__TAURI_INTERNALS__.invoke('get_autostart')")
        check("get_autostart 可调用", err is None, str(val or err))
        check("初始状态为关闭", val is False, f"返回 {val!r}")

        val, err = cdp.eval(
            "window.__TAURI_INTERNALS__.invoke('set_autostart', {enabled: true})"
        )
        check("set_autostart(true) 返回 true", val is True, str(val or err))
        rv = reg_value()
        check("注册表写入 Run 项", rv is not None, repr(rv))
        if rv:
            check("Run 项指向本 exe", "OCR Studio.exe" in rv, rv)
            check("启动参数带 --tray", "--tray" in rv, rv)

        val, err = cdp.eval(
            "window.__TAURI_INTERNALS__.invoke('set_autostart', {enabled: false})"
        )
        check("set_autostart(false) 返回 false", val is False, str(val or err))
        check("注册表已清理", reg_value() is None, repr(reg_value()))

        cdp.close()
        cdp = None

        # ---------------- 第二段：--tray 启动不弹窗 ----------------
        kill()
        time.sleep(1.5)
        print("[STEP] 启动（带 --tray）")
        env = dict(os.environ)
        env.pop("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", None)
        env["OCR_WEBVIEW_ARGS"] = "--disable-gpu-compositing"
        env["OCR_NO_CORE"] = "1"
        proc = subprocess.Popen([exe, "--tray"], env=env, close_fds=True)
        time.sleep(10)
        check("--tray 启动时无可见主窗口", count_visible() == 0, f"可见 {count_visible()} 个")
    finally:
        if cdp:
            cdp.close()
        kill()
        try:
            if proc:
                proc.wait(timeout=5)
        except Exception:
            pass

    print(f"\n{'[PASS] 全部通过' if not fails else '[FAIL] ' + '；'.join(fails)}")
    return 0 if not fails else 1


if __name__ == "__main__":
    sys.exit(main())
