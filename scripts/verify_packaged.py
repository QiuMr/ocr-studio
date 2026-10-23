"""验收「免安装分发包」解压即用：从解压目录起服务，跑一遍本轮新增/改动的接口。

为什么单独一个脚本，而不是复用 `verify_http_contract.py`：
那个脚本管的是**协议契约**（404/405/CORS 信封），与模型在不在无关；
这个脚本管的是**这份拷贝的模型是否齐全**——它专门盯 `models/` 里
「哪些该带、哪些不该带」，以及缺东西时接口有没有**明确报错**而不是静默返回空。

覆盖：
  1. /ocr/health          服务起来了、模型根目录指向解压目录（不是源码目录）
  2. /ocr/shibie          认字主链路
  3. /ocr/quyuocr         区域识别（框坐标已平移回原图）
  4. /vision/qrcode       扫码（纯 CPU，与引擎无关）
  5. /vision/prompt visual 图找图：分发包**带全了**这套模型，必须能出框
  6. /vision/prompt ref    按中文名找：参考库教一次 → 按名字找 → 删掉（6c）
                           `mode=text` 已整体移除，必须明确 400 并指路（6b）

「不该带什么」也在这里钉住：language 找图的 242MB 文本塔（`mobileclip2_b.ts`）、
`*_texthead.onnx`、`tokenizer*.json`、`text_pe.bin`、`zh_words.json` 一旦重新
混进分发包，体积会悄悄涨回三百多 MB，而接口其实根本不用它。

用法：
    python scripts/verify_packaged.py --exe "release/OCR-Studio/OCR Studio.exe" [--port 8848]

脚本会 taskkill 掉已有的 "OCR Studio.exe"（进程名带空格），结束时也收掉自己起的那个。
"""

import argparse
import base64
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 本机若开着系统代理，127.0.0.1 也会被代理拦成 502，必须显式绕开
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

fails: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"[{'OK' if ok else 'FAIL'}] {name}{(' — ' + detail) if detail else ''}")
    if not ok:
        fails.append(name)


def req(method: str, url: str, body: bytes | None = None, headers: dict | None = None):
    """返回 (status, body_bytes)。连不上时 status=0。"""
    r = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with opener.open(r, timeout=120) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except urllib.error.URLError:
        return 0, b""


def post_json(url: str, obj: dict):
    body = json.dumps(obj).encode("utf-8")
    st, raw = req("POST", url, body, {"Content-Type": "application/json"})
    try:
        return st, json.loads(raw.decode("utf-8"))
    except Exception:
        return st, {"_raw": raw[:300].decode("utf-8", "replace")}


def get_json(url: str):
    st, raw = req("GET", url)
    try:
        return st, json.loads(raw.decode("utf-8"))
    except Exception:
        return st, {"_raw": raw[:300].decode("utf-8", "replace")}


def b64(path: Path) -> str:
    return base64.b64encode(path.read_bytes()).decode()


def procs() -> int:
    out = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq OCR Studio.exe", "/NH"],
        capture_output=True, text=True, encoding="utf-8", errors="ignore",
    ).stdout
    return out.lower().count("ocr studio.exe")


def stop() -> None:
    subprocess.run(["taskkill", "/IM", "OCR Studio.exe", "/F"],
                   capture_output=True, check=False)
    for _ in range(40):
        if procs() == 0:
            return
        time.sleep(0.25)


def start(exe: str, port: int) -> None:
    # 必须走 explorer.exe：agent shell 里直接起 exe 会让 WebView2 的 GPU 子进程
    # 起不来（白屏）。服务本身不受影响，但用 explorer 起最接近用户双击的场景。
    subprocess.run(["explorer.exe", exe], check=False)
    for _ in range(120):
        time.sleep(0.5)
        st, _ = req("GET", f"http://127.0.0.1:{port}/ocr/health")
        if st == 200:
            return
    raise SystemExit("[FAIL] 服务 60 秒内没起来")


def make_qr() -> Path:
    import qrcode  # 本机已装；没装的话这一项会跳过

    img = qrcode.make("https://example.com/ocr-studio")
    p = Path(tempfile.gettempdir()) / "ocr_smoke_qr.png"
    img.save(p)
    return p


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True)
    ap.add_argument("--port", type=int, default=8848)
    args = ap.parse_args()

    exe = str(Path(args.exe).resolve())
    port = args.port
    base = f"http://127.0.0.1:{port}"

    if not Path(exe).exists():
        print(f"[FAIL] exe 不存在: {exe}")
        return 2

    # ---------------- 0. 分发包体积防线（不需要起服务，先查） ----------------
    print("\n=== 分发包不该带的东西（语言找图已移除） ===")
    root = Path(exe).parent
    banned = [
        "models/yoloe/mobileclip2_b.ts",          # 242MB 英文文本塔
        "models/yoloe/mobileclip2_b.safetensors",  # 同上，torch 原始权重
        "models/yoloe/tokenizer.min.json",         # CLIP BPE 词表
        "models/yoloe/tokenizer.json",
        "models/yoloe/text_pe.bin",                # 预烘焙提示词
        "models/yoloe/zh_words.json",              # 中英词典（已删模块）
    ]
    for rel in banned:
        check(f"没有 {rel}", not (root / rel).exists(), str(root / rel))
    stray = sorted(p.name for p in (root / "models" / "yoloe").glob("*_texthead.onnx"))
    check("没有 *_texthead.onnx", not stray, str(stray))

    try:
        stop()
        start(exe, port)

        # ---------------- 1. health ----------------
        print("\n=== /ocr/health ===")
        st, raw = req("GET", f"{base}/ocr/health")
        j = json.loads(raw.decode("utf-8"))
        d = j.get("data", {})
        print(json.dumps(d, ensure_ascii=False, indent=2)[:600])
        check("health code=0", j.get("code") == 0)
        check("引擎已就绪", d.get("status") in ("ready", "ok", "Ready"), str(d.get("status")))
        check("端口正确", d.get("port") == port, str(d.get("port")))
        actual = d.get("actual_device") or d.get("device")
        print(f"[INFO] 实际推理设备: {actual}")

        # ---------------- 2. shibie ----------------
        print("\n=== /ocr/shibie（认字主链路） ===")
        zh = ROOT / "assets" / "test_zh.png"
        st, j = post_json(f"{base}/ocr/shibie", {"image": b64(zh)})
        text = (j.get("data") or {}).get("text", "")
        check("shibie code=0", j.get("code") == 0, str(j.get("msg", ""))[:120])
        check("shibie 有文字返回", bool(text.strip()), repr(text[:60]))
        print(f"[INFO] 识别文字: {text[:80]!r}")

        # ---------------- 2b. multipart 大图（>2MB）----------------
        # axum 的 `DefaultBodyLimit` 默认只有 **2MB**，而 multipart 走的是
        # `Multipart` extractor（JSON 路径自己用 `to_bytes(body, 32MB)`，不受影响）。
        # 没显式放开的话，用户传一张 3MB 的截图就 400
        # 「Error parsing multipart/form-data request」，而同一张图走 JSON 却正常，
        # 极难排查。这里用一张 >2MB 的图钉住它。
        print("\n=== /ocr/shibie multipart（>2MB 大图，验 DefaultBodyLimit） ===")
        try:
            import numpy as np
            from PIL import Image

            big = Path(tempfile.gettempdir()) / "ocr_smoke_big.png"
            rng = np.random.default_rng(0)
            Image.fromarray(
                rng.integers(0, 256, (1200, 1200, 3), dtype=np.uint8)
            ).save(big)
            raw_bytes = big.read_bytes()
            print(f"[INFO] 大图 {len(raw_bytes) / 1e6:.2f} MB")
            boundary = "----ocrsmoke"
            body = (
                f"--{boundary}\r\n"
                f'Content-Disposition: form-data; name="image"; filename="big.png"\r\n'
                f"Content-Type: image/png\r\n\r\n"
            ).encode() + raw_bytes + f"\r\n--{boundary}--\r\n".encode()
            st, raw = req(
                "POST", f"{base}/ocr/shibie", body,
                {"Content-Type": f"multipart/form-data; boundary={boundary}"},
            )
            j = json.loads(raw.decode("utf-8"))
            check("multipart 大图 code=0", j.get("code") == 0, str(j.get("msg", ""))[:160])
        except ImportError:
            print("[SKIP] 没装 numpy/PIL，跳过大图 multipart 项")

        # ---------------- 3. quyuocr ----------------
        print("\n=== /ocr/quyuocr（区域识别） ===")
        st, j = post_json(f"{base}/ocr/quyuocr",
                          {"image": b64(zh), "region": [0, 0, 400, 200]})
        d = j.get("data") or {}
        check("quyuocr code=0", j.get("code") == 0, str(j.get("msg", ""))[:120])
        check("quyuocr 回显 region", isinstance(d.get("region"), list), str(d.get("region")))
        check("quyuocr 有原图尺寸", bool(d.get("width") and d.get("height")),
              f"{d.get('width')}x{d.get('height')}")
        print(f"[INFO] region={d.get('region')} text={d.get('text','')[:60]!r}")

        # ---------------- 4. qrcode ----------------
        print("\n=== /vision/qrcode（扫码，纯 CPU） ===")
        try:
            qr = make_qr()
            st, j = post_json(f"{base}/vision/qrcode", {"image": b64(qr)})
            d = j.get("data") or {}
            check("qrcode code=0", j.get("code") == 0, str(j.get("msg", ""))[:120])
            check("qrcode 认出 1 个码", d.get("count") == 1, str(d.get("count")))
            codes = d.get("codes") or []
            got = codes[0].get("text") if codes else ""
            check("qrcode 内容正确",
                  "example.com/ocr-studio" in (got or ""), repr(got))
            print(f"[INFO] 扫码结果: {got!r} format={codes[0].get('format') if codes else None}")
        except ImportError:
            print("[SKIP] 没装 qrcode 库，跳过扫码项")

        # ---------------- 5. prompt visual（图找图） ----------------
        print("\n=== /vision/prompt mode=visual（图找图，分发包应带全） ===")
        bus = ROOT / "assets" / "bus.png"
        st, j = post_json(f"{base}/vision/prompt", {
            "mode": "visual",
            "image": b64(bus),
            "ref_image": b64(bus),
            "ref_boxes": "31,231,801,776",   # bus.png 里那辆巴士（810x1080）
            "ref_names": "巴士",
            "conf": 0.25,
        })
        d = j.get("data") or {}
        print(json.dumps(d, ensure_ascii=False)[:500])
        check("prompt visual code=0", j.get("code") == 0, str(j.get("msg", ""))[:160])
        check("prompt visual 命中", bool(d.get("found")), str(d.get("found")))
        hits = d.get("matches") or []
        if hits:
            h = hits[0]
            # 目标就是参考图本身，命中框应与参考框大致重合（IoU 不严格，只看中心）
            cx, cy = h.get("center_x"), h.get("center_y")
            in_box = cx is not None and 31 <= cx <= 801 and 231 <= cy <= 776
            check("命中框落在巴士上", bool(in_box), f"center=({cx},{cy}) score={h.get('score')}")
            print(f"[INFO] 命中 {len(hits)} 个，首个 center=({cx},{cy}) "
                  f"score={h.get('score')} class={h.get('class_name')}")
        check("返回 engine=yoloe-prompt", d.get("engine") == "yoloe-prompt", str(d.get("engine")))

        # ---------------- 6. prompt ref（按中文名找 / 语言找图已移除） ----------------
        # 老块「mode=text 应该命中」已随功能移除一起删掉：留在这里会永远 FAIL，
        # 因为 text 现在是明确拒绝（见 6b）。

        # ---------- 6b. 语言提示已移除：必须明确 400 + 指路，不是静默失效 ----------
        print("\n=== /vision/prompt mode=text（已移除，应当明确指路） ===")
        st, j = post_json(f"{base}/vision/prompt", {
            "mode": "text",
            "image": b64(bus),
            "words": "人,公交车",
            "conf": 0.25,
        })
        msg = str(j.get("msg", ""))
        check("text 模式被拒（400 信封 code!=0）", j.get("code") != 0, f"code={j.get('code')}")
        check("报错文案说明已移除并指路 promptref",
              "已移除" in msg and "promptref" in msg, msg[:200])

        # 没教过的名字：明确拒绝 + 列出可用的名字 / 提示先教
        print("\n=== mode=ref 用没教过的名字应明确报错并指路 ===")
        st, j = post_json(f"{base}/vision/prompt", {
            "mode": "ref",
            "image": b64(bus),
            "words": "宇宙飞船",
            "conf": 0.25,
        })
        msg = str(j.get("msg", ""))
        check("未教过的名字被拒", j.get("code") != 0, f"code={j.get('code')}")
        check("报错文案指路（promptref / 教过的名字）",
              "还没教过" in msg and "promptref" in msg, msg[:200])

        # ---------- 6c. 中文参考库（教一次 → 按中文名找 → 忘掉） ----------
        print("\n=== /vision/promptref（中文参考库：教/找/删） ===")
        st, j = post_json(f"{base}/vision/promptref", {
            "name": "我的巴士",
            "image": b64(bus),
            "box": [31, 231, 801, 776],
        })
        check("教一次 code=0", j.get("code") == 0, str(j.get("msg", ""))[:160])

        st, j = get_json(f"{base}/vision/promptref")
        d = j.get("data") or {}
        got = [r.get("name") for r in (d.get("refs") or [])]
        check("参考库列表里有它", "我的巴士" in got, str(got))

        st, j = post_json(f"{base}/vision/prompt", {
            "mode": "ref",
            "image": b64(bus),
            "words": "我的巴士",
            "conf": 0.25,
        })
        d = j.get("data") or {}
        print(json.dumps(d, ensure_ascii=False)[:400])
        check("按中文名找 code=0", j.get("code") == 0, str(j.get("msg", ""))[:160])
        check("按中文名找 命中", bool(d.get("found")), str(d.get("found")))
        hits = d.get("matches") or []
        if hits:
            cx, cy = hits[0].get("center_x"), hits[0].get("center_y")
            in_box = cx is not None and 31 <= cx <= 801 and 231 <= cy <= 776
            check("命中框落在巴士上", bool(in_box), f"center=({cx},{cy})")

        # query string 里中文必须 percent-encode，否则 http.client 按 ascii 发不出去
        st, j = req("DELETE", f"{base}/vision/promptref?name="
                   + urllib.parse.quote("我的巴士"))
        try:
            j = json.loads(j)
        except Exception:
            j = {}
        check("删除 code=0", j.get("code") == 0, str(j.get("msg", ""))[:160])

        st, j = get_json(f"{base}/vision/promptref")
        got = [r.get("name") for r in ((j.get("data") or {}).get("refs") or [])]
        check("删完后列表也干净", "我的巴士" not in got, str(got))

        # ---------- 6d. AI 接口默认关闭：明确 403 并指路，不是 404 ----------
        print("\n=== /ai/v1/*（默认关闭，应明确拒绝） ===")
        st, body = req("GET", f"{base}/ai/v1/tools")
        text = body.decode("utf-8", "replace")
        check("AI tools 默认 403", st == 403, f"status={st}")
        check("403 文案指路设置页", "设置" in text, text[:120])
        st, body = req("POST", f"{base}/ai/v1/tool", b'{"tool":"get_status"}',
                       {"Content-Type": "application/json"})
        check("AI tool 默认 403", st == 403, f"status={st}")

    finally:
        stop()

    print(f"\n{'[PASS] 全部通过' if not fails else '[FAIL] ' + '；'.join(fails)}")
    return 0 if not fails else 1


if __name__ == "__main__":
    sys.exit(main())
