"""通过 WebView2 远程调试端口探测 App 内真实渲染状态。

用法：
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
    & ocr-studio.exe
    python scripts/cdp_probe.py

输出：页面 URL、DOM 尺寸、#root 内容长度、控制台报错、以及截图到
scripts/_cdp_screenshot.png。

注意：本机系统代理会让 urllib 访问 127.0.0.1 也走代理并拿到 502，
必须用 ProxyHandler({}) 绕开。
"""

import base64
import json
import sys
import time
import urllib.request

DEBUG_PORT = (sys.argv[1] if len(sys.argv) > 1 else "9333").strip()
DEBUG_BASE = f"http://127.0.0.1:{DEBUG_PORT}"
OUT_PNG = "scripts/_cdp_screenshot.png"

_opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http_get(url: str, timeout: float = 5.0):
    with _opener.open(url, timeout=timeout) as r:
        return json.loads(r.read().decode("utf-8"))


class CDP:
    def __init__(self, ws_url: str):
        import websocket  # websocket-client

        self.ws = websocket.create_connection(
            ws_url,
            timeout=15,
            # 本机服务同样要绕代理
            http_proxy_host=None,
            http_proxy_port=None,
        )
        self.seq = 0
        self.events = []

    def send(self, method: str, params: dict | None = None) -> int:
        self.seq += 1
        self.ws.send(json.dumps({"id": self.seq, "method": method, "params": params or {}}))
        return self.seq

    def pump(self, seconds: float) -> None:
        """在给定时间内持续收集事件（控制台消息 / 异常）。"""
        end = time.time() + seconds
        self.ws.settimeout(0.3)
        while time.time() < end:
            try:
                raw = self.ws.recv()
            except Exception:
                continue
            if not raw:
                continue
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if "method" in msg:
                self.events.append(msg)

    def call(self, method: str, params: dict | None = None, wait: float = 2.0):
        mid = self.send(method, params)
        end = time.time() + wait
        self.ws.settimeout(0.3)
        while time.time() < end:
            try:
                raw = self.ws.recv()
            except Exception:
                continue
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == mid:
                return msg
            if "method" in msg:
                self.events.append(msg)
        return None

    def close(self) -> None:
        try:
            self.ws.close()
        except Exception:
            pass


def evaluate(cdp: CDP, expr: str):
    r = cdp.call(
        "Runtime.evaluate",
        {"expression": expr, "returnByValue": True, "awaitPromise": True},
    )
    if not r:
        return None, "无响应"
    res = r.get("result", {})
    if "exceptionDetails" in res:
        return None, res["exceptionDetails"].get("text", "异常")
    return res.get("result", {}).get("value"), None


def main() -> int:
    try:
        targets = http_get(f"{DEBUG_BASE}/json")
    except Exception as e:
        print(f"[FAIL] 连不上调试端口 {DEBUG_BASE}: {e}")
        print("       确认启动 App 时设置了 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222")
        return 1

    pages = [t for t in targets if t.get("type") == "page"]
    if not pages:
        print(f"[FAIL] 没有 page 目标，targets={[t.get('type') for t in targets]}")
        return 1
    # 优先挑我们自己的页面（tauri.localhost / localhost / index.html），
    # 避免抓到同一个调试端口上其它 WebView2 宿主的 newtab 页面。
    tgt = pages[0]
    for p in pages:
        u = (p.get("url") or "").lower()
        if "tauri.localhost" in u or u.endswith("index.html") or "localhost" in u:
            tgt = p
            break
    if (tgt.get("url") or "").startswith("edge://"):
        print(f"[FAIL] 只找到非应用页面: {[p.get('url') for p in pages]}")
        return 1
    print(f"[INFO] 页面: {tgt.get('url')}")
    print(f"[INFO] 标题: {tgt.get('title')}")

    cdp = CDP(tgt["webSocketDebuggerUrl"])
    for m in ("Runtime.enable", "Log.enable", "Page.enable"):
        cdp.send(m)
    cdp.pump(1.5)

    val, err = evaluate(cdp, "location.href")
    print(f"[INFO] 当前 URL: {val or err}")

    val, err = evaluate(
        cdp,
        "JSON.stringify({"
        "readyState: document.readyState,"
        "rootChildren: document.getElementById('root') ? document.getElementById('root').childElementCount : -1,"
        "rootHtmlLen: document.getElementById('root') ? document.getElementById('root').innerHTML.length : -1,"
        "bodyText: (document.body.innerText || '').slice(0, 200),"
        "innerW: window.innerWidth, innerH: window.innerHeight,"
        "hasTauri: '__TAURI_INTERNALS__' in window,"
        "sheets: document.styleSheets.length,"
        "scripts: [...document.querySelectorAll('script')].map(s => s.src)"
        "})",
    )
    print("[INFO] 页面状态:")
    if val:
        info = json.loads(val)
        for k, v in info.items():
            print(f"        {k} = {v}")
    else:
        print(f"        取值失败: {err}")

    # 动态插入 root 里看是否渲染出东西（用于区分「JS 崩了」与「CSS 隐藏了」）
    val, err = evaluate(
        cdp,
        "(() => { const r = document.getElementById('root');"
        " if (!r) return 'no #root';"
        " const s = getComputedStyle(r);"
        " return JSON.stringify({display: s.display, height: s.height,"
        " w: r.getBoundingClientRect().width, h: r.getBoundingClientRect().height,"
        " html: r.innerHTML.slice(0, 300)}); })()",
    )
    print(f"[INFO] #root 详情: {val or err}")

    # 控制台 / 异常
    errors = []
    for ev in cdp.events:
        m = ev.get("method")
        p = ev.get("params", {})
        if m == "Runtime.exceptionThrown":
            d = p.get("exceptionDetails", {})
            desc = (d.get("exception") or {}).get("description") or d.get("text")
            errors.append(f"EXCEPTION: {desc}")
        elif m == "Runtime.consoleAPICalled" and p.get("type") in ("error", "warning"):
            args = p.get("args", [])
            text = " ".join(str(a.get("value", a.get("description", ""))) for a in args)
            errors.append(f"CONSOLE[{p.get('type')}]: {text[:400]}")
        elif m == "Log.entryAdded":
            e = p.get("entry", {})
            if e.get("level") in ("error", "warning"):
                errors.append(f"LOG[{e.get('level')}]: {e.get('text')} {e.get('url') or ''}")
    if errors:
        print("[ERRORS]")
        for e in errors[:20]:
            print(f"        {e}")
    else:
        print("[ERRORS] 无控制台错误")

    # 截图
    shot = cdp.call("Page.captureScreenshot", {"format": "png"}, wait=4.0)
    if shot and "result" in shot:
        data = base64.b64decode(shot["result"]["data"])
        with open(OUT_PNG, "wb") as f:
            f.write(data)
        print(f"[INFO] 截图已保存: {OUT_PNG} ({len(data)} 字节)")
    else:
        print("[WARN] 截图失败")

    cdp.close()
    ok = bool(val) and err is None
    print("[PASS] 探测完成" if ok else "[FAIL] 页面异常")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
