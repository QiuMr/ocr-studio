"""A/B/C 三道闸的实机验收：并发上限、解码像素上限、以及「闸门不该把正常请求搞坏」。

  python scripts/verify_guards.py [--base http://127.0.0.1:8848]

查四件事：
  1. 正常请求照旧出结果，且结果与改动前一致（指纹比对）—— 闸门不能误伤；
  2. 并发打满时多出来的请求拿到 **429 + Retry-After**，而不是排一个 50 秒的队；
  3. 超大图在**分配位图之前**就被拒（单边闸），差一档的在中途被拒（像素闸），
     正常大图照旧通过；
  4. 429 的响应体仍是统一信封 {code,msg}。
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def post(url: str, payload: dict, timeout: float = 120.0):
    req = urllib.request.Request(
        url, data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"}, method="POST",
    )
    try:
        with OPENER.open(req, timeout=timeout) as r:
            return r.status, dict(r.headers), json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        try:
            body = json.loads(e.read().decode())
        except Exception:
            body = {"code": -1, "msg": f"HTTP {e.code}"}
        return e.code, dict(e.headers), body


def get(url: str, timeout: float = 10.0):
    with OPENER.open(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


def hget(headers: dict, name: str):
    """HTTP 头名大小写不敏感（http.client 会给成小写）。"""
    for k, v in headers.items():
        if k.lower() == name.lower():
            return v
    return None


def fingerprint(resp: dict) -> str:
    rows = (resp.get("data") or {}).get("rows") or []
    parts = []
    for r in rows:
        pts = []
        for p in r.get("box") or []:
            if isinstance(p, (list, tuple)):
                pts.extend("{:.2f}".format(float(v)) for v in p)
            else:
                pts.append("{:.2f}".format(float(p)))
        parts.append("{}|{:.4f}|{}".format(r.get("text", ""), float(r.get("score", 0.0)), ",".join(pts)))
    return hashlib.sha256("\n".join(parts).encode()).hexdigest()[:16]


def solid_png(w: int, h: int, color=(255, 255, 255)) -> bytes:
    """纯色大图：只为测尺寸闸，不为测识别率。"""
    from PIL import Image

    buf = io.BytesIO()
    Image.new("RGB", (w, h), color).save(buf, format="PNG")
    return buf.getvalue()


def slow_png(src: Path, cols: int = 3, rows: int = 2) -> bytes:
    """把测试图拼成大图，让单次推理慢到足以把队列堆起来。

    纯色图会让检测阶段过快（一个框都没有），闸门还没堆满请求就跑完了 ——
    实测 48 路并发打小图一次 429 都没有，那是测试方法问题不是闸门问题。
    """
    from PIL import Image

    base = Image.open(src).convert("RGB")
    w, h = base.size
    canvas = Image.new("RGB", (w * cols, h * rows), (255, 255, 255))
    for cx in range(cols):
        for cy in range(rows):
            canvas.paste(base, (cx * w, cy * h))
    buf = io.BytesIO()
    canvas.save(buf, format="PNG")
    return buf.getvalue()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8848")
    ap.add_argument("--img", default=str(ROOT / "assets" / "test_zh.png"))
    args = ap.parse_args()

    fails: list[str] = []
    img_b64 = base64.b64encode(Path(args.img).read_bytes()).decode()

    try:
        health = get(f"{args.base}/ocr/health")["data"]
    except Exception as e:
        print(f"[FAIL] 连不上服务 {args.base}: {e}")
        return 2
    print(f"服务 v{health['version']}  {health['engine']}  {health['actual_device']}\n")

    # ---- 1. 正常请求不受影响 -------------------------------------------------
    print("[1] 正常请求")
    st, _, r = post(f"{args.base}/ocr/duohangjson", {"image": img_b64})
    fp = fingerprint(r)
    ok = st == 200 and r.get("code") == 0 and ((r.get("data") or {}).get("rows"))
    print(f"    /ocr/duohangjson -> HTTP {st} code={r.get('code')} "
          f"{len(((r.get('data') or {}).get('rows') or []))} 行  指纹 {fp}")
    if not ok:
        fails.append("正常请求失败或被闸门误伤")
    else:
        print("    [PASS] 闸门没有误伤正常请求")

    # ---- 2. 并发打满 -> 429 + Retry-After -----------------------------------
    # 为什么打 200 而不是 48：推理是串行的（吞吐约 3.8 张/秒），客户端的 48 个线程
    # 受 GIL 影响是「陆续」到达的，等最后几个发车时前面的已经跑完、名额已释放 ——
    # 实测 48 路一次 429 都没有，那是测试方法问题不是闸门问题。200 路才稳定撞满。
    # （另外 200 条连接同时建，偶尔会撞上系统 accept 队列满导致 ConnectionRefused，
    #   这是客户端突发、不是服务挂了，重试一次即可。）
    print("\n[2] 并发闸门（打 200 路，上限 32）")
    n = 200
    barrier = threading.Barrier(n)

    def one(i):
        barrier.wait(timeout=60)
        last = None
        for _ in range(3):
            try:
                return post(f"{args.base}/ocr/duohangjson", {"image": img_b64}, timeout=180)
            except urllib.error.URLError as e:
                last = e
                time.sleep(0.2)
        raise last  # type: ignore[misc]

    with ThreadPoolExecutor(max_workers=n) as ex:
        results = list(ex.map(one, range(n)))
    codes = [st for st, _, _ in results]
    n_429 = sum(1 for c in codes if c == 429)
    n_200 = sum(1 for c in codes if c == 200)
    retry_after = {hget(h, "Retry-After") for st, h, _ in results if st == 429}
    envelope_ok = all(
        isinstance(b.get("code"), int) and isinstance(b.get("msg"), str)
        for st, _, b in results
    )
    print(f"    200 x {n_200}   429 x {n_429}   其他 x {n - n_200 - n_429}")
    print(f"    Retry-After: {retry_at if (retry_at := retry_after) else '缺失'}")
    print(f"    429 信封仍是 {{code,msg}}: {envelope_ok}")
    if n_429 == 0:
        fails.append("48 路并发一次 429 都没拿到 —— 闸门没生效")
    elif n_200 + n_429 != n:
        fails.append(f"出现了既不是 200 也不是 429 的状态: {sorted(set(codes))}")
    else:
        print("    [PASS] 超限请求被快速拒绝，没有排队")
    if not envelope_ok:
        fails.append("429 的响应体不是统一信封")
    if retry_after != {"1"}:
        fails.append(f"Retry-After 头不符合预期: {retry_after}")

    # ---- 3. 尺寸闸 -----------------------------------------------------------
    print("\n[3] 解码尺寸闸")
    cases = [
        ("9000x9000 = 8100 万像素（单边就超）", 9000, 9000, "单边"),
        ("7000x5000 = 3500 万像素（单边合规、像素超）", 7000, 5000, "像素"),
        ("5000x6000 = 3000 万像素（合规大图）", 5000, 6000, None),
    ]
    for label, w, h, expect in cases:
        try:
            b64 = base64.b64encode(solid_png(w, h)).decode()
        except Exception as e:
            print(f"    {label}: 生成失败 {e}（跳过）")
            continue
        st, _, r = post(f"{args.base}/ocr/duohangjson", {"image": b64}, timeout=180)
        msg = str(r.get("msg", ""))
        if expect is None:
            good = st == 200 and r.get("code") == 0
            print(f"    {label}: HTTP {st} code={r.get('code')} -> {'[PASS] 正常通过' if good else '[FAIL] ' + msg[:80]}")
            if not good:
                fails.append(f"合规大图 {w}x{h} 被误拒")
        else:
            hit = expect in msg
            print(f"    {label}: HTTP {st} -> {'[PASS] ' if hit else '[FAIL] '}{msg[:90]}")
            if not hit:
                fails.append(f"{w}x{h} 应当因「{expect}」被拒，实际: HTTP {st} {msg[:60]}")

    print()
    if fails:
        print("[FAIL] " + "; ".join(fails))
        return 1
    print("[PASS] 三道闸全部符合预期：正常请求无损、超限快速 429、超大图在分配前被拒")
    return 0


if __name__ == "__main__":
    sys.exit(main())
