"""验收 HTTP 层的「统一错误信封」与 CORS 开关。

覆盖三件容易漏的事：
  1. 路径打错 -> 404 且 msg 里给出最接近的可用路径（不是空 body）
  2. 方法用错 -> 405 且 msg 里说明该用什么方法
  3. CORS 关闭时不给跨域头；打开后 OPTIONS 预检返回 204、POST 带 ACAO 头

用法：
    python scripts/verify_http_contract.py --exe "<OCR Studio.exe>" [--port 8848]

脚本会临时改写 %APPDATA%\\com.ocrstudio.app\\config.json 的 allow_browser 字段，
结束时**原样还原**（即使中途失败也走 finally）。
"""

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

APP_DIR = Path(os.environ["APPDATA"]) / "com.ocrstudio.app"
CFG = APP_DIR / "config.json"

opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

fails: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"[{'OK' if ok else 'FAIL'}] {name}{(' — ' + detail) if detail else ''}")
    if not ok:
        fails.append(name)


def req(method: str, url: str, body: bytes | None = None, headers: dict | None = None):
    """返回 (status, headers, body)。

    headers 的 key **统一转小写**：axum 发出的头名是小写
    （`access-control-allow-origin`），而 `dict(HTTPMessage)` 会丢掉
    urllib 本来提供的「大小写不敏感」特性，直接按驼峰查会全部取不到，
    看起来像「服务没返回 CORS 头」——本轮就误判过一次。
    """
    r = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with opener.open(r, timeout=8) as resp:
            return resp.status, {k.lower(): v for k, v in resp.headers.items()}, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, {k.lower(): v for k, v in e.headers.items()}, e.read()
    except urllib.error.URLError:
        # 服务还没起来（连接被拒）：状态码用 0 表示「没连上」，
        # 不能当成 HTTP 4xx 处理，否则 wait_port_free 会立刻误判为已空闲。
        return 0, {}, b""


def procs() -> int:
    out = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq OCR Studio.exe", "/NH"],
        capture_output=True, text=True, encoding="utf-8", errors="ignore",
    ).stdout
    return out.lower().count("ocr studio.exe")


def stop() -> None:
    subprocess.run(
        ["taskkill", "/IM", "OCR Studio.exe", "/F"], capture_output=True, check=False
    )
    for _ in range(40):
        if procs() == 0:
            return
        time.sleep(0.25)


def start(exe: str) -> None:
    subprocess.run(["explorer.exe", exe], check=False)
    for _ in range(60):
        time.sleep(0.5)
        st, _, _ = req("GET", f"http://127.0.0.1:{port}/ocr/health")
        if st == 200:
            return
    raise SystemExit("[FAIL] 服务 30 秒内没起来")


def wait_port_free() -> None:
    for _ in range(40):
        st, _, _ = req("GET", f"http://127.0.0.1:{port}/ocr/health")
        if st != 200:
            return
        time.sleep(0.25)


def set_allow(v: bool) -> None:
    cfg = json.loads(CFG.read_text(encoding="utf-8")) if CFG.exists() else {}
    cfg["allow_browser"] = v
    CFG.write_text(json.dumps(cfg, ensure_ascii=False, indent=2), encoding="utf-8")


def main() -> int:
    global port
    exe = None
    port = 8848
    argv = sys.argv[1:]
    for i, a in enumerate(argv):
        if a == "--exe":
            exe = argv[i + 1]
        elif a == "--port":
            port = int(argv[i + 1])
    if not exe:
        print(__doc__)
        return 2

    base = f"http://127.0.0.1:{port}"
    original = CFG.read_text(encoding="utf-8") if CFG.exists() else None
    try:
        # ---------------- CORS 关闭（默认）----------------
        stop()
        wait_port_free()
        set_allow(False)
        start(exe)
        print("\n=== allow_browser = false（默认） ===")

        st, _, body = req("GET", f"{base}/ocr/shibei")
        try:
            j = json.loads(body.decode("utf-8"))
        except Exception:
            j = {}
        check("打错路径返回 404", st == 404, f"HTTP {st}")
        check("404 用统一信封", j.get("code") == 404, json.dumps(j, ensure_ascii=False)[:120])
        check(
            "404 给出最接近路径",
            "/ocr/shibie" in (j.get("msg") or ""),
            j.get("msg", ""),
        )

        st, _, body = req("GET", f"{base}/ocr/shibie")
        j = json.loads(body.decode("utf-8")) if body else {}
        check("方法用错返回 405", st == 405, f"HTTP {st}")
        check("405 说明该用 POST", "POST" in (j.get("msg") or ""), j.get("msg", ""))

        st, hdr, body = req("OPTIONS", f"{base}/ocr/shibie", headers={"Origin": "http://x.test"})
        check("CORS 关闭时无 ACAO 头", "access-control-allow-origin" not in hdr, f"HTTP {st}")
        j = json.loads(body.decode("utf-8")) if body else {}
        check(
            "CORS 关闭时 OPTIONS 提示去设置里开",
            "浏览器页面调用" in (j.get("msg") or ""),
            j.get("msg", ""),
        )

        # ---------------- CORS 打开 ----------------
        stop()
        wait_port_free()
        set_allow(True)
        start(exe)
        print("\n=== allow_browser = true ===")

        st, hdr, _ = req("OPTIONS", f"{base}/ocr/shibie", headers={"Origin": "http://x.test"})
        check("预检返回 204", st == 204, f"HTTP {st}")
        check(
            "预检回显 Origin",
            hdr.get("access-control-allow-origin") == "http://x.test",
            str(hdr.get("access-control-allow-origin")),
        )
        check(
            "预检允许 POST",
            "POST" in (hdr.get("access-control-allow-methods") or ""),
            str(hdr.get("access-control-allow-methods")),
        )

        st, hdr, _ = req(
            "GET", f"{base}/ocr/health", headers={"Origin": "http://x.test"}
        )
        check("普通请求也带 ACAO 头", hdr.get("access-control-allow-origin") == "http://x.test",
              str(hdr.get("access-control-allow-origin")))
    finally:
        stop()
        if original is not None:
            CFG.write_text(original, encoding="utf-8")
            print("[INFO] 已还原 config.json")

    print(f"\n{'[PASS] 全部通过' if not fails else '[FAIL] ' + '；'.join(fails)}")
    return 0 if not fails else 1


if __name__ == "__main__":
    sys.exit(main())
