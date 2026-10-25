"""全端点 smoke：拿 assets/test_zh.png 把 19 个端点各打一遍，确认 A/B/C 改动没误伤谁。

  python scripts/smoke_endpoints.py [--base http://127.0.0.1:8848] [--img assets/test_zh.png]

判定：
  - code == 0                                  -> PASS
  - 引擎没装（1003 / 503）或该功能未配模型       -> SKIP（环境缺东西，不是回归）
  - 其它                                       -> FAIL

找图类接口用「同一张图既当模板又当大图」—— 模板必然命中自己，
  分低可能是尺寸/策略问题，但只要 code==0 就说明链路是通的。
"""

from __future__ import annotations

import argparse
import base64
import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))

# 引擎缺失类错误码：环境问题，不算回归
ENV_CODES = {1002, 1003, 503}


def req(url: str, payload: dict | None = None, timeout: float = 180.0):
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode()
        headers["Content-Type"] = "application/json"
    r = urllib.request.Request(url, data=data, headers=headers)
    try:
        with OPENER.open(r, timeout=timeout) as resp:
            return resp.status, json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read().decode())
        except Exception:
            return e.code, {}
    except Exception as e:
        return -1, {"msg": f"{type(e).__name__}: {e}"}


def png_size(data: bytes) -> tuple[int, int]:
    """从 PNG IHDR 读宽高（不引第三方依赖）。"""
    if data[:8] == b"\x89PNG\r\n\x1a\n" and data[12:16] == b"IHDR":
        w = int.from_bytes(data[16:20], "big")
        h = int.from_bytes(data[20:24], "big")
        return w, h
    return 0, 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8848")
    ap.add_argument("--img", default=str(ROOT / "assets" / "test_zh.png"))
    args = ap.parse_args()

    raw = Path(args.img).read_bytes()
    b64 = base64.b64encode(raw).decode()
    w, h = png_size(raw)
    base = args.base.rstrip("/")

    # (名称, 方法, 路径, payload 工厂) —— payload None = GET
    cases: list[tuple[str, str, str, dict | None]] = [
        ("health",            "GET",  "/ocr/health",            None),
        ("engine-list",       "GET",  "/engine/list",           None),
        ("shibie",            "POST", "/ocr/shibie",            {"image": b64}),
        ("zhaozi",            "POST", "/ocr/zhaozi",            {"image": b64, "keyword": "的"}),
        ("duohangjson",       "POST", "/ocr/duohangjson",       {"image": b64}),
        ("danzijson",         "POST", "/ocr/danzijson",         {"image": b64}),
        ("quyuocr",           "POST", "/ocr/quyuocr",           {"image": b64, "region": [0, 0, 400, 200]}),
        ("tuzhaotu1",         "POST", "/ocr/tuzhaotu1",         {"image": b64, "template": b64}),
        ("tuzhaotu2",         "POST", "/ocr/tuzhaotu2",         {"image": b64, "template": b64}),
        # 局部/屏幕找图：截窗口的要 title 或 hwnd（没有窗口时失败属环境问题）
        ("jubingzhaotu1",     "POST", "/ocr/jubingzhaotu1",     {"title": "OCR Studio", "template": b64}),
        ("jubingzhaotu2",     "POST", "/ocr/jubingzhaotu2",     {"title": "OCR Studio", "template": b64}),
        ("pingmuzhaotu1",     "POST", "/ocr/pingmuzhaotu1",     {"image": b64, "template": b64}),
        ("pingmuzhaotu2",     "POST", "/ocr/pingmuzhaotu2",     {"image": b64, "template": b64}),
        ("yuyizhaotu",        "POST", "/ocr/yuyizhaotu",        {"image": b64, "prompt": "文字"}),
        ("vision-prompt",     "POST", "/vision/prompt",
         {"image": b64, "ref_image": b64, "ref_boxes": f"0,0,{w},{h}"}),
        ("vision-promptref-add", "POST", "/vision/promptref",
         {"name": "smoke目标", "image": b64, "box": [0, 0, w, h]}),
        ("vision-promptref-list", "GET", "/vision/promptref",   None),
        ("vision-detect",     "POST", "/vision/detect",         {"image": b64}),
        ("vision-pipeline",   "POST", "/vision/pipeline",       {"image": b64}),
        ("vision-qrcode",     "POST", "/vision/qrcode",         {"image": b64}),
        ("biaoge",            "POST", "/ocr/biaoge",            {"image": b64}),
    ]

    n_pass = n_skip = n_fail = 0
    for name, method, path, payload in cases:
        st, body = req(f"{base}{path}", payload)
        code = body.get("code")
        if code == 0:
            verdict, n_pass = "PASS", n_pass + 1
        elif code in ENV_CODES:
            verdict, n_skip = "SKIP", n_skip + 1
        else:
            verdict, n_fail = "FAIL", n_fail + 1
        detail = ""
        if verdict != "PASS":
            detail = f"  code={code} msg={str(body.get('msg'))[:80]}"
        print(f"  [{verdict}] {name:18s} {method:4s} {path:22s} HTTP {st}{detail}")

    print(f"\n[结论] PASS {n_pass} / SKIP {n_skip} / FAIL {n_fail}（共 {len(cases)}）")
    return 1 if n_fail else 0


if __name__ == "__main__":
    sys.exit(main())
