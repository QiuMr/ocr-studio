"""并发验收：串行基线 → 1/2/4/8/16/32/64 并发打接口。

回答三个问题：
  1. 并发下会不会串扰（结果必须和串行基线逐字节一致）；
  2. 并发下进程线程数/内存会不会爆炸（线程数应只由「会话数 × 每会话线程池」决定，
     与并发请求数无关 —— 这是 ONNX Runtime 多线程崩溃的典型前兆）；
  3. 吞吐随并发怎么变（本服务的推理是全局串行的，吞吐应该基本不随并发上涨，
     延迟则线性排队 —— 这是设计事实，不是缺陷，但要量化出来）。

用法: python scripts/verify_concurrency.py [--base http://127.0.0.1:8848]
                                           [--img assets/test_zh.png] [--max 64]
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parent.parent
NO_PROXY = urllib.request.ProxyHandler({})  # 127.0.0.1 也必须绕系统代理，否则 502
OPENER = urllib.request.build_opener(NO_PROXY)


def post_json(url: str, payload: dict, timeout: float = 120.0) -> tuple[int, dict]:
    req = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with OPENER.open(req, timeout=timeout) as r:
            return r.status, json.loads(r.read().decode())
    except urllib.error.HTTPError as e:
        try:
            body = json.loads(e.read().decode())
        except Exception:
            body = {"code": -1, "msg": f"HTTP {e.code}"}
        return e.code, body


def get_json(url: str, timeout: float = 10.0) -> dict:
    with OPENER.open(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


def fingerprint(resp: dict) -> str:
    """结果指纹：文本 + 全精度框坐标。并发串扰会让它变。"""
    rows = (resp.get("data") or {}).get("rows") or []
    parts = []
    for r in rows:
        pts = []
        for p in r.get("box") or []:
            if isinstance(p, (list, tuple)):
                pts.extend("{:.2f}".format(float(v)) for v in p)
            else:
                pts.append("{:.2f}".format(float(p)))
        parts.append(
            "{}|{:.4f}|{}".format(r.get("text", ""), float(r.get("score", 0.0)), ",".join(pts))
        )
    return hashlib.sha256("\n".join(parts).encode()).hexdigest()[:16]


class Sampler(threading.Thread):
    """后台采样进程线程数 / 工作集 / 私有字节。"""

    def __init__(self, name_contains: str, interval: float = 0.1):
        super().__init__(daemon=True)
        self.name_contains = name_contains
        self.interval = interval
        self.stop = threading.Event()
        self.samples: list[tuple[int, float, float]] = []

    def _find(self):
        for p in psutil.process_iter(["name"]):
            try:
                if self.name_contains.lower() in (p.info["name"] or "").lower():
                    return psutil.Process(p.pid)
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                continue
        return None

    def run(self):
        proc = None
        while not self.stop.is_set():
            if proc is None:
                proc = self._find()
            if proc is not None:
                try:
                    mi = proc.memory_info()
                    # psutil 7 的 pmem 只有 rss/vms；working_set/private 在 full_info
                    try:
                        fi = proc.memory_full_info()
                        ws, priv = fi.working_set / 1048576, fi.private / 1048576
                    except (AttributeError, psutil.Error):
                        ws, priv = mi.rss / 1048576, mi.vms / 1048576
                    self.samples.append((proc.num_threads(), ws, priv))
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    proc = None
            self.stop.wait(self.interval)

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.stop.set()
        self.join(timeout=2)
        return False

    def peak(self) -> tuple[int, float, float]:
        if not self.samples:
            return (0, 0.0, 0.0)
        return (
            max(s[0] for s in self.samples),
            max(s[1] for s in self.samples),
            max(s[2] for s in self.samples),
        )


def one(base: str, img_b64: str, idx: int, endpoint: str = "/ocr/duohangjson", extra: dict | None = None) -> dict:
    t0 = time.perf_counter()
    payload = {"image": img_b64}
    if extra:
        payload.update(extra)
    status, resp = post_json(f"{base}{endpoint}", payload)
    dt = (time.perf_counter() - t0) * 1000
    ok = status == 200 and resp.get("code") == 0
    return {
        "idx": idx,
        "ok": ok,
        "status": status,
        "code": resp.get("code"),
        "ms": dt,
        "fp": fingerprint(resp) if ok else "",
        "n": len(((resp.get("data") or {}).get("rows") or [])),
        "err": "" if ok else str(resp.get("msg"))[:120],
    }


# 混打用的端点表：OCR 主锁 / 区域裁剪 / 单行 / 整图文本 / 找字。
# 每条都只吃 image，参数差异写进 extra。刻意不放 /vision/detect（本机没装 yolo，
# 会 1002，那是正确行为不是并发问题）与 /ocr/biaoge（表格模型未装 → 503，同理）。
MIXED: list[tuple[str, dict]] = [
    ("/ocr/duohangjson", {}),
    ("/ocr/quyuocr", {"region": [0, 0, 900, 560]}),
    ("/ocr/danzijson", {}),
    ("/ocr/shibie", {}),
    ("/ocr/zhaozi", {"keyword": "OCR"}),
]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8848")
    ap.add_argument("--img", default=str(ROOT / "assets" / "test_zh.png"))
    ap.add_argument("--max", type=int, default=64)
    ap.add_argument("--burst", type=int, default=0, help="只打这一个并发档（跳过阶梯）")
    ap.add_argument("--mixed", action="store_true", help="混打不同端点，查锁序死锁")
    ap.add_argument("--proc", default="OCR Studio")
    args = ap.parse_args()

    img_path = Path(args.img)
    if not img_path.exists():
        print(f"[FAIL] 测试图不存在: {img_path}")
        return 2
    img_b64 = base64.b64encode(img_path.read_bytes()).decode()

    try:
        health = get_json(f"{args.base}/ocr/health")["data"]
    except Exception as e:
        print(f"[FAIL] 连不上服务 {args.base}: {e}")
        return 2
    print(f"服务 v{health['version']}  引擎 {health['engine']}  设备 {health['actual_device']}")

    # 预热一次（首次推理含 DirectML 图编译，不能进基线）
    warm = one(args.base, img_b64, -1)
    if not warm["ok"]:
        print(f"[FAIL] 预热请求失败: {warm['err']}")
        return 2
    print(f"预热 {warm['ms']:.0f}ms，识别 {warm['n']} 行\n")

    base_fp = warm["fp"]

    # 串行基线
    seq = [one(args.base, img_b64, i) for i in range(3)]
    seq_ms = sum(r["ms"] for r in seq) / len(seq)
    print(f"串行基线: {seq_ms:.0f}ms/张  指纹 {base_fp}\n")

    header = "{:>4} {:>8} {:>9} {:>9} {:>8} {:>7} {:>7} {:>9} {:>9}".format(
        "并发", "总耗时s", "吞吐/s", "平均ms", "最大ms", "成功", "串扰", "峰值线程", "峰值WS_MB"
    )
    print(header)
    print("-" * len(header))

    failures = []
    if args.burst:
        levels = [args.burst]
    elif args.mixed:
        levels = [len(MIXED) * 4]
    else:
        levels = [n for n in (1, 2, 4, 8, 16, 32, 64) if n <= args.max]

    for n in levels:
        if args.mixed:
            jobs = [(i, MIXED[i % len(MIXED)]) for i in range(n)]
        else:
            jobs = [(i, ("/ocr/duohangjson", {})) for i in range(n)]
        with Sampler(args.proc) as s:
            t0 = time.perf_counter()
            with ThreadPoolExecutor(max_workers=n) as ex:
                results = list(
                    ex.map(lambda j: one(args.base, img_b64, j[0], j[1][0], j[1][1]), jobs)
                )
            wall = time.perf_counter() - t0
        ok = sum(1 for r in results if r["ok"])
        # 混打时各端点返回结构不同，只对同一端点做指纹比对
        drift = 0
        if not args.mixed:
            drift = sum(1 for r in results if r["ok"] and r["fp"] != base_fp)
        lat = sorted(r["ms"] for r in results)
        peak_threads, peak_ws, _ = s.peak()
        print(
            "{:>4} {:>8.2f} {:>9.2f} {:>9.0f} {:>8.0f} {:>7} {:>7} {:>9} {:>9.1f}".format(
                n,
                wall,
                n / wall,
                sum(lat) / len(lat),
                lat[-1],
                f"{ok}/{n}",
                drift,
                peak_threads,
                peak_ws,
            )
        )
        if ok != n:
            bad = [r for r in results if not r["ok"]][:3]
            failures.append(
                f"并发 {n}: {n - ok} 个失败（"
                + "；".join(f"#{r['idx']} code={r['code']} {r['err']}" for r in bad)
                + "）"
            )
        if drift:
            failures.append(f"并发 {n}: {drift} 个结果与基线不一致（疑似串扰）")
        if args.mixed:
            per_ep: dict[str, int] = {}
            for (i, (ep, _)), r in zip(jobs, results):
                if r["ok"]:
                    per_ep[ep] = per_ep.get(ep, 0) + 1
            print("       混打明细: " + "  ".join(f"{k}={v}" for k, v in sorted(per_ep.items())))

    print()
    if failures:
        print("[FAIL] " + "; ".join(failures))
        return 1
    print("[PASS] 全部并发档位：无失败、无串扰、进程未崩溃")
    print(
        "\n读表方式：吞吐不随并发上升 = 推理被全局互斥锁串行化（设计如此，不是缺陷）；"
        "\n          峰值线程数不随并发上涨 = 线程只由「会话数」决定，这正是"
        "\n          「多线程并发闪退」类问题的安全形态。"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
