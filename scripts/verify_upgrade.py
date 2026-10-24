# -*- coding: utf-8 -*-
"""实机升级验收：用 AI 接口操作自己的界面，从 0.1.6 升到 0.1.7。

为什么这么做而不是手动点：升级链路（检查更新 → 下载 → 校验 → 助手替换 → 重启）
一环断了用户是无感的，只有端到端跑通才算数。而「用 AI 点自己的界面」
顺带把 click_at / find_text / focus_window 在真实窗口上也验了一遍。

安全边界：全程只点 OCR Studio 自己窗口里的按钮，不碰别的程序；
升级完成后进程是同一个 exe 路径，只是版本号变了。
"""
import io
import json
import pathlib
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, r"D:\Python测试\OCR-Studio\scripts")
from verify_ai import Ai, CFG  # noqa: E402

OUT = pathlib.Path(r"D:\Python测试\OCR-Studio\release\upgrade_shots")
OUT.mkdir(parents=True, exist_ok=True)

cfg = json.loads(CFG.read_text(encoding="utf-8"))
ai = Ai(f"http://127.0.0.1:{cfg['port']}", cfg["ai_token"])

# 目标版本从线上 latest.json 读，不写死 —— 写死的话每次发版都要记着改，
# 忘改就会出现「其实已经是最新版本却跑去测升级」的假阴性。
DEFAULT_TARGET = "0.1.10"


def remote_target() -> str:
    import urllib.request
    try:
        op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        raw = op.open(
            "https://github.com/QiuMr/ocr-studio/releases/latest/download/latest.json",
            timeout=10,
        ).read()
        return json.loads(raw).get("version") or DEFAULT_TARGET
    except Exception as e:
        print(f"[WARN] 读不到线上版本号，用默认 {DEFAULT_TARGET}（{e}）")
        return DEFAULT_TARGET


TARGET = remote_target()


def health_version():
    op = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        h = json.load(op.open("http://127.0.0.1:8848/ocr/health", timeout=3))
        return (h.get("data") or {}).get("version")
    except Exception:
        return None


def app_hwnd():
    wins = (ai.call("list_windows")[1].get("data") or {}).get("windows") or []
    cand = [w for w in wins if "OCR Studio" in (w.get("title") or "")]
    return cand[0] if cand else None


def shot(hwnd, name):
    st, r = ai.call("screenshot", source="window", hwnd=hwnd)
    p = (r.get("data") or {}).get("image_path")
    if p and pathlib.Path(p).exists():
        dst = OUT / name
        dst.write_bytes(pathlib.Path(p).read_bytes())
        return str(dst)
    return None


def find_blue_button(hwnd):
    """按颜色找界面里的蓝色主按钮（「立即更新」这种），返回屏幕坐标。

    为什么需要它：OCR 对**白字蓝底**的按钮基本识别不出来（PP-OCR small 在
    低对比小字号上就这样），find_text 死活找不到「立即更新」，但按钮明明在
    屏幕上。对自动化来说这就是「眼睛不好使」—— 那就换个感官：主按钮是
    blue-600 实底，在一屏浅色界面里是最扎眼的一坨蓝，按颜色聚类足够可靠。
    """
    st, r = ai.call("screenshot", source="window", hwnd=hwnd)
    d = r.get("data") or {}
    p = d.get("image_path")
    ox, oy = d.get("origin_x"), d.get("origin_y")
    if not p or ox is None:
        return None
    from PIL import Image

    im = Image.open(p).convert("RGB")
    w, h = im.size
    px = im.load()
    xs, ys = [], []
    for y in range(h):
        for x in range(w):
            cr, cg, cb = px[x, y]
            if cb > 120 and cb - cr > 60 and cb - cg > 40:
                xs.append(x)
                ys.append(y)
    if len(xs) < 200:
        return None
    # 按 40px 横带聚类后，取**最靠下的那一坨**，且要求它在页面左半侧。
    #
    # 为什么不是「最密集的」：设置页上同时有 4 个蓝按钮 —— 模型卡里三个
    # 「下载」（在最右侧）和更新卡里一个「立即更新」（左侧，页面最底部）。
    # 实测取最密集的那坨会命中上面的下载按钮，点下去开始下模型，
    # 升级什么都没发生，脚本还以为点过了。「立即更新」是这页最后一个
    # 交互元素，位置特征比颜色面积可靠。
    import collections

    bands = collections.Counter(y // 40 * 40 for y in ys)
    # 横带按 y 从大到小试，第一个「够蓝且在左半侧」的就是它
    for band in sorted(bands, reverse=True):
        sel = [(x, y) for x, y in zip(xs, ys) if band <= y < band + 40]
        if len(sel) < 120:
            continue
        cx = sum(x for x, _ in sel) // len(sel)
        if cx > w * 0.62:
            continue  # 右半侧是模型卡的「下载」按钮，不是更新
        cy = sum(y for _, y in sel) // len(sel)
        return {"center_x": ox + cx, "center_y": oy + cy, "votes": len(sel)}
    return None


def scroll_until(hwnd, keyword, rounds=12):
    """在窗口里往下滚，直到某个词出现。返回它的坐标。

    设置页很长，「检查更新」在「版本与更新」卡里，首屏根本看不到 ——
    不滚就是找不到，而找不到会被误判成「没有更新功能」。
    """
    cx = None
    cy = None
    for i in range(rounds):
        ai.call("wait", ms=250)
        st, r = ai.call("find_text", keyword=keyword, source="window", hwnd=hwnd)
        hits = (r.get("data") or {}).get("matches") or []
        if hits:
            hits.sort(key=lambda m: m["center_y"])
            return hits[0]
        if cx is None:
            w = app_hwnd() or {}
            cx = (w.get("x") or 0) + (w.get("width") or 1116) // 2
            cy = (w.get("y") or 0) + 300
        ai.call("scroll", x=cx, y=cy, delta=-600)
    return None


def click_text(hwnd, keyword, expect=None, tries=5, rightmost=False):
    """找到关键词并点它，点完还要确认页面真的换了（expect 出现了）。

    为什么必须验证：点击发出去只代表「事件送到了」，不代表界面响应了。
    第一次实测就踩到 —— 点完立刻截图，看到的还是旧页面，脚本误判成点击失败。
    对自动化来说「点了没反应」和「点歪了」必须分开处理，所以点完必须复核。

    rightmost：同词多处时取最右的。侧栏导航和正文里会出现同一个词
    （比如「设置」），而**正文的大标题才是最靠右的那个** —— 拿它当
    「页面换成功了」的判据，比挑一个可能首屏看不到的词（「AI 控制」在
    设置页很下面）稳得多。
    """
    for _ in range(tries):
        ai.call("wait", ms=250)
        st, r = ai.call("find_text", keyword=keyword, source="window", hwnd=hwnd)
        hits = (r.get("data") or {}).get("matches") or []
        if not hits:
            continue
        hits.sort(key=lambda m: m["center_x"], reverse=rightmost)
        m = hits[0]
        st, r = ai.call("click_at", x=m["center_x"], y=m["center_y"])
        if not r.get("ok"):
            continue
        if expect is None:
            return m
        for _ in range(8):
            ai.call("wait", ms=300)
            st, r = ai.call("find_text", keyword=expect, source="window", hwnd=hwnd)
            hits = (r.get("data") or {}).get("matches") or []
            if hits:
                if rightmost:
                    hits.sort(key=lambda x: x["center_x"], reverse=True)
                    # 正文大标题在窗口内约 x=253（侧栏导航只到约 211）。
                    # 阈值取 240：窗口贴左边缘时正文 279、侧栏 211，分得开；
                    # 写成 300 会把这个标题判成侧栏，换页验证永远不命中。
                    if hits[0]["center_x"] > 240:
                        return m
                    continue
                return m
    return None


def main() -> int:
    v0 = health_version()
    print(f"[1] 升级前版本: {v0}")
    if v0 == TARGET:
        print("    已经是最新版，无需测")
        return 0

    w = app_hwnd()
    if not w:
        print("[FAIL] 没找到 OCR Studio 窗口")
        return 1
    hwnd = w["hwnd"]
    print(f"[2] 窗口: {w['title']} hwnd={hwnd}")

    # 抢焦点：AI 是后台进程，直接 SetForegroundWindow 会被前台锁静默拒绝。
    # 但**鼠标点击是用户输入，不受前台锁限制** —— 点一下窗口标题栏，
    # Windows 自己就把窗口带到前台了。这一步比任何 API 都可靠。
    st, r = ai.call("focus_window", hwnd=hwnd)
    if not (r.get("data") or {}).get("foreground"):
        ai.call("click_at", x=w["x"] + 200, y=w["y"] + 12)  # 标题栏（不是关闭按钮）
        ai.call("wait", ms=600)
        st, r = ai.call("focus_window", hwnd=hwnd)
    if not (r.get("data") or {}).get("foreground"):
        print("[FAIL] 没抢到前台焦点，点击会打到别的窗口，中止")
        return 1
    print("[3] 已提到前台")

    shot(hwnd, "1_before.png")

    # 先看设置页在不在（残留页面可能是测试台），不在就点侧栏「设置」
    st, r = ai.call("find_text", keyword="检查更新", source="window", hwnd=hwnd)
    if not ((r.get("data") or {}).get("matches")):
        m = click_text(hwnd, "设置", expect="设置", rightmost=True)
        print(f"[4] 点侧栏「设置」: {'OK' if m else 'FAIL'}")
        if not m:
            shot(hwnd, "4_settings_failed.png")
            return 1
    else:
        print("[4] 已在设置页")

    shot(hwnd, "2_settings.png")

    # 更新卡在设置页很下面，要滚过去
    m = scroll_until(hwnd, "检查更新")
    print(f"[5] 滚到「检查更新」: {'OK ' + str((m['center_x'], m['center_y'])) if m else 'FAIL'}")
    if not m:
        shot(hwnd, "5_no_check_button.png")
        return 1

    # 进设置页时会自动查一次，可能「立即更新」已经在了；没有就手动点检查更新
    m2 = scroll_until(hwnd, "立即更新", rounds=3)
    if not m2:
        st, r = ai.call("click_at", x=m["center_x"], y=m["center_y"])
        print(f"[6] 点「检查更新」: ok={r.get('ok')}")
        if not r.get("ok"):
            return 1
        # **等结果期间不能滚屏**：检查更新要走网络，而滚屏会把更新卡滚出视野，
        # 按钮明明出来了却搜不到 —— 第一次实测就是这么误判的。
        m2 = None
        for _ in range(20):
            ai.call("wait", ms=400)
            st, r = ai.call("find_text", keyword="立即更新", source="window", hwnd=hwnd)
            hits = (r.get("data") or {}).get("matches") or []
            if hits:
                hits.sort(key=lambda x: x["center_y"])
                m2 = hits[0]
                break
    if not m2:
        m2 = scroll_until(hwnd, "立即更新", rounds=8)
    if not m2:
        # OCR 认不出蓝底白字，按颜色兜底
        m2 = find_blue_button(hwnd)
        if m2:
            print(f"[7b] OCR 没认出按钮，按颜色定位到主按钮: {(m2['center_x'], m2['center_y'])}")
    shot(hwnd, "3_checked.png")
    if not m2:
        print("[FAIL] 没有出现「立即更新」（可能没检出新版本，或更新卡没刷出来）")
        return 1
    print(f"[7] 出现「立即更新」: {(m2['center_x'], m2['center_y'])}")

    st, r = ai.call("click_at", x=m2["center_x"], y=m2["center_y"])
    print(f"[8] 点「立即更新」: ok={r.get('ok')} {r.get('error', '')}")
    if not r.get("ok"):
        return 1

    # 下载 50MB + 校验 + 助手替换 + 重启，最多等 3 分钟
    print("[9] 等待升级完成（下载→校验→替换→重启）...")
    deadline = time.time() + 180
    last = None
    while time.time() < deadline:
        time.sleep(3)
        v = health_version()
        if v != last:
            print(f"    version -> {v}")
            last = v
        if v == TARGET:
            print(f"[OK] 升级成功：{v0} → {v}")
            return 0
    print("[FAIL] 3 分钟内没变成 " + TARGET + f"（最后看到 {last}）")
    dump_helper_log()
    return 1


def dump_helper_log():
    """把更新助手的日志打出来。

    助手的 tracing 没有 subscriber（它在 tauri::Builder 之前就分流了），
    以前它的日志全丢，表现就是「点了更新，程序关了，再也没起来」而零线索。
    updater.rs 现在把每步写进 TEMP 下的 ocr-studio-update/helper.log，
    这里负责在验收失败时把它亮出来。
    """
    import os
    p = os.path.join(os.environ.get("TEMP", ""), "ocr-studio-update", "helper.log")
    if not os.path.exists(p):
        print("    （没有 helper.log，助手可能根本没起来）")
        return
    print("    ---- helper.log ----")
    for line in io.open(p, encoding="utf-8", errors="replace").read().splitlines()[-25:]:
        print("    " + line[:200])


if __name__ == "__main__":
    sys.exit(main())
