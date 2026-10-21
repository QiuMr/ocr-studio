"""找图接口的端到端验证：真实屏幕 -> HTTP 服务 -> 命中坐标比对。

为什么需要它：`capture_live.rs` 验证的是库内代码，`api_smoke.rs` 验证的是
「上传两张图」的纯 HTTP 路径。两者之间的空白是「**服务进程里真的能截到屏，
且返回的屏幕绝对坐标是对的**」—— 这条链只有把 App 跑起来才能验。

做法：本脚本自己截一张屏、从里面裁一块当模板，再让服务去截它自己看到的屏幕
并找这块模板。两边截屏间隔很短，桌面静止时坐标应严格相等。

用法：
    python scripts/e2e_find.py                # 需先启动 ocr-studio.exe
    python scripts/e2e_find.py --port 8848
"""

import argparse
import base64
import json
import sys
import urllib.error
import urllib.request

import numpy as np

try:
    import mss
except ImportError:
    print("需要 mss：pip install mss", file=sys.stderr)
    raise

# 本机服务必须绕开系统代理：很多环境把 127.0.0.1 也塞进 http_proxy，
# 走代理会拿到 502（实测如此，不是服务没起来）。
_OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def grab_screen():
    """抓全屏，返回 (BGR ndarray, 左上角 x, 左上角 y)。"""
    with mss.mss() as sct:
        mon = sct.monitors[0]  # 0 = 所有显示器的合并区域
        raw = np.array(sct.grab(mon))  # BGRA
        bgr = raw[:, :, :3].copy()
        return bgr, mon["left"], mon["top"]


def variance_of(patch):
    g = patch.astype(np.float32)
    return float(g.var())


def pick_textured(screen, tw, th, margin=80):
    """找方差最大的 tw x th 区域。`margin` 用来避开任务栏/边缘。"""
    h, w = screen.shape[:2]
    best = (-1.0, 0, 0)
    step = 32
    for y in range(margin, h - th - margin, step):
        for x in range(margin, w - tw - margin, step):
            v = variance_of(screen[y : y + th, x : x + tw])
            if v > best[0]:
                best = (v, x, y)
    return best[1], best[2], best[0]


def encode_png_b64(bgr):
    import cv2

    ok, buf = cv2.imencode(".png", bgr)
    if not ok:
        raise RuntimeError("PNG 编码失败")
    return base64.b64encode(buf.tobytes()).decode("ascii")


def post(base, path, payload, timeout=30):
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        base + path,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with _OPENER.open(req, timeout=timeout) as r:
            return r.status, json.loads(r.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read().decode("utf-8"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8848)
    ap.add_argument("--size", type=int, default=64, help="模板边长")
    args = ap.parse_args()
    base = f"http://127.0.0.1:{args.port}"

    # 0) 服务在跑吗
    try:
        with _OPENER.open(base + "/ocr/health", timeout=5) as r:
            health = json.loads(r.read().decode("utf-8"))
    except Exception as e:
        print(f"[FAIL] 连不上 {base}/ocr/health: {e}")
        print("       请先启动 ocr-studio.exe")
        return 1
    print(f"[ok] health = {json.dumps(health, ensure_ascii=False)}")

    # 1) 本脚本截一张，裁模板
    screen, sx, sy = grab_screen()
    print(f"[ok] 本脚本截屏 {screen.shape[1]}x{screen.shape[0]} @({sx},{sy})")
    tw = th = args.size
    tx, ty, var = pick_textured(screen, tw, th)
    if var < 20:
        print(f"[warn] 屏幕过于单调（方差 {var:.1f}），结果可能不稳定")
    tpl = screen[ty : ty + th, tx : tx + tw]
    tpl_b64 = encode_png_b64(tpl)
    # 本脚本坐标是「合并虚拟屏幕内」的，换算成屏幕绝对坐标
    abs_x, abs_y = sx + tx, sy + ty
    print(f"[ok] 模板裁自脚本坐标 ({tx},{ty}) = 屏幕绝对 ({abs_x},{abs_y})，方差 {var:.1f}")

    # 2) 屏幕找图（精准）
    st, resp = post(base, "/ocr/pingmuzhaotu1", {"template": tpl_b64})
    print(f"\n[pingmuzhaotu1] HTTP {st}\n{json.dumps(resp, ensure_ascii=False, indent=2)}")
    if st != 200 or resp.get("code") != 0:
        print("[FAIL] 屏幕找图接口报错")
        return 1
    d = resp["data"]
    if not d["found"]:
        print("[FAIL] 没找到 —— 同一块桌面，模板必然存在")
        return 1
    got = (d["x"], d["y"])
    want = (abs_x, abs_y)
    print(f"[check] 命中 {got} vs 期望 {want}，score {d['score']:.4f}")
    if got != want:
        print(f"[FAIL] 坐标不一致，差 ({got[0]-want[0]}, {got[1]-want[1]})")
        return 1
    if d["score"] < 0.95:
        print(f"[FAIL] 分数过低 {d['score']:.4f}（同一桌面应接近 1.0）")
        return 1
    print(f"[PASS] 屏幕找图：坐标零误差，score {d['score']:.4f}，耗时 {d['elapsed_ms']}ms")

    # 3) 容错端点同样要能命中
    st, resp = post(base, "/ocr/pingmuzhaotu2", {"template": tpl_b64})
    d2 = resp.get("data") or {}
    if st != 200 or not d2.get("found") or (d2["x"], d2["y"]) != want:
        print(f"[FAIL] 容错端点异常: HTTP {st} {resp}")
        return 1
    print(f"[PASS] 容错端点：坐标一致，score {d2['score']:.4f}")

    # 4) tuzhaotu：把脚本截的整屏上传当搜索图，坐标应为「图内坐标」
    st, resp = post(base, "/ocr/tuzhaotu1", {
        "image": encode_png_b64(screen),
        "template": tpl_b64,
    })
    d3 = resp.get("data") or {}
    if st != 200 or not d3.get("found"):
        print(f"[FAIL] tuzhaotu1 异常: HTTP {st} {resp}")
        return 1
    # 上传图是合并虚拟屏幕坐标系，所以期望是 (tx, ty) 而不是屏幕绝对坐标
    if (d3["x"], d3["y"]) != (tx, ty):
        print(f"[FAIL] tuzhaotu 坐标应为图内 ({tx},{ty})，实际 ({d3['x']},{d3['y']})")
        return 1
    if (d3["origin_x"], d3["origin_y"]) != (0, 0):
        print(f"[FAIL] source=image 时 origin 必须为 (0,0)，实际 ({d3['origin_x']},{d3['origin_y']})")
        return 1
    print(f"[PASS] 传图找图：图内坐标 ({d3['x']},{d3['y']}) 正确，origin=(0,0)")
    print(
        f"[time] 截屏找图 {d['elapsed_ms']}ms vs 传图找图 {d3['elapsed_ms']}ms "
        f"-> 截图链路约占 {d['elapsed_ms'] - d3['elapsed_ms']}ms"
    )

    # 5) 参数错误路径
    st, resp = post(base, "/ocr/pingmuzhaotu1", {})
    if st != 400:
        print(f"[FAIL] 缺模板应返回 400，实际 {st}")
        return 1
    print(f"[PASS] 缺模板 -> 400：{resp.get('msg')}")

    print("\n全部通过。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
