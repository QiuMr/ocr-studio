"""验收 AI 操控接口（/ai/v1/*）：35 个工具是不是真能用。

为什么要这个脚本：
  工具清单是**声明**，能不能跑是另一回事。历史上就出现过「清单里有、exec 里没实现」
  和「返回说成功、其实坐标算错」两类问题。这里对每一项至少打一次真请求，
  并且专门盯三件最容易错的事：
    1. 坐标是不是**屏幕绝对坐标**（截图/截窗口后能否直接点）
    2. 边界是不是清楚（缺参数报错要点名缺哪个、未知工具要给候选）
    3. 危险操作有没有被**如实拒绝**（不认识的键不能瞎按）

默认**不**真的点击/打字（那会动到用户正在用的窗口），只验参数校验路径。
想看真注入加 `--inject`：会开一个记事本，往里打字 → 认出来 → 在字上点一下 →
关掉不保存。全程发生在自己的窗口里，不碰别的程序。

用法：
    python scripts/verify_ai.py                       # 服务已在跑，自动读配置
    python scripts/verify_ai.py --exe "D:/rust-target/release/ocr-studio.exe"

注意：带依赖（PIL / qrcode）的脚本要用 **PowerShell 的 python** 跑，
agent 的 bash python 环境里没有这些包。
"""

import argparse
import ctypes
import io
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
import zlib
from pathlib import Path

APP_DIR = Path(os.environ["APPDATA"]) / "com.ocrstudio.app"
CFG = APP_DIR / "config.json"
TMP = Path(os.environ.get("TEMP", ".")) / "ocr_ai_verify"
TMP.mkdir(parents=True, exist_ok=True)

opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

fails: list[str] = []
skips: list[str] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    print(f"[{'OK' if ok else 'FAIL'}] {name}{(' — ' + detail) if detail else ''}")
    if not ok:
        fails.append(name)


def skip(name: str, why: str) -> None:
    print(f"[SKIP] {name} — {why}")
    skips.append(name)


def http(method: str, url: str, body: bytes | None = None, headers: dict | None = None):
    r = urllib.request.Request(url, data=body, method=method, headers=headers or {})
    try:
        with opener.open(r, timeout=60) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except urllib.error.URLError:
        return 0, b""


# ---------------------------------------------------------------- AI 调用


class Ai:
    def __init__(self, base: str, token: str):
        self.base = base.rstrip("/")
        self.token = token

    def _headers(self) -> dict:
        return {"Authorization": f"Bearer {self.token}", "Content-Type": "application/json"}

    def tools(self):
        st, body = http("GET", f"{self.base}/ai/v1/tools", None, self._headers())
        return st, json.loads(body or b"{}")

    def call(self, tool: str, **args):
        payload = json.dumps({"tool": tool, "args": args}, ensure_ascii=False).encode()
        st, body = http("POST", f"{self.base}/ai/v1/tool", payload, self._headers())
        try:
            obj = json.loads(body or b"{}")
        except json.JSONDecodeError:
            obj = {"ok": False, "error": body[:200].decode("utf-8", "replace")}
        return st, obj


# ---------------------------------------------------------------- 测试素材


def make_table_png(path: Path) -> bool:
    """画一张 3x3 的表格图（有框线 + 文字）。PIL 不在就退化成纯框线版本。"""
    try:
        from PIL import Image, ImageDraw

        img = Image.new("RGB", (600, 260), "white")
        d = ImageDraw.Draw(img)
        xs = [20, 220, 400, 580]
        ys = [20, 100, 180, 240]
        for y in ys:
            d.line([(xs[0], y), (xs[-1], y)], fill="black", width=3)
        for x in xs:
            d.line([(x, ys[0]), (x, ys[-1])], fill="black", width=3)
        d.text((40, 45), "Item", fill="black")
        d.text((240, 45), "Qty", fill="black")
        d.text((420, 45), "Price", fill="black")
        img.save(path)
        return True
    except Exception:
        # 没 PIL：手写一张只有框线的 PNG（结构识别照样能跑，只是格内没字）
        w, h = 600, 260
        rows = [[255] * (w * 3) for _ in range(h)]
        for y in (20, 100, 180, 240):
            for x in range(0, w):
                for dy in range(3):
                    if 0 <= y + dy < h:
                        rows[y + dy][x * 3 : x * 3 + 3] = [0, 0, 0]
        for x in (20, 220, 400, 499):
            for y in range(20, 240):
                for dx in range(3):
                    rows[y][x * 3 + dx * 3 : x * 3 + dx * 3 + 3] = [0, 0, 0]
        raw = b"".join(
            b"\x00" + bytes(v for px in row for v in px) for row in rows
        )

        def chunk(tag: bytes, data: bytes) -> bytes:
            return (
                len(data).to_bytes(4, "big")
                + tag
                + data
                + zlib.crc32(tag + data).to_bytes(4, "big")
            )

        png = (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", w.to_bytes(4, "big") + h.to_bytes(4, "big") + b"\x08\x02\x00\x00\x00")
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b"")
        )
        path.write_bytes(png)
        return False


def make_qr_png(path: Path) -> bool:
    try:
        import qrcode
    except Exception:
        return False
    qrcode.make("OCR-STUDIO-AI-VERIFY").save(path)
    return True


def cursor_pos():
    class P(ctypes.Structure):
        _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]

    p = P()
    ctypes.windll.user32.GetCursorPos(ctypes.byref(p))
    return p.x, p.y


# ---------------------------------------------------------------- 主流程


def wait_port(base: str, seconds: float) -> bool:
    end = time.time() + seconds
    while time.time() < end:
        st, _ = http("GET", f"{base}/ocr/health")
        if st:
            return True
        time.sleep(0.5)
    return False


def run(ai: Ai) -> None:
    # ---------------- 1. 清单
    print("\n== 工具清单 ==")
    st, obj = ai.tools()
    tools = (obj.get("data") or {}).get("tools") or []
    check("GET /ai/v1/tools 可用", st == 200 and obj.get("ok") is True, f"HTTP {st}")
    check("工具数 = 35", len(tools) == 35, f"实际 {len(tools)}")

    names = [t["name"] for t in tools]
    check("工具名不重复", len(names) == len(set(names)))
    risks = {}
    for t in tools:
        risks[t["risk"]] = risks.get(t["risk"], 0) + 1
        if not t.get("summary") or not t.get("args") is not None:
            check(f"{t['name']} 有 summary 和 args", False)
    check(
        "三档风险齐全（只读 10 / 写 11 / 危险 14）",
        risks.get("readonly") == 10 and risks.get("write") == 11 and risks.get("dangerous") == 14,
        json.dumps(risks, ensure_ascii=False),
    )
    for must in (
        "find_text", "read_table", "detect_objects", "list_windows", "focus_window",
        "click_at", "type_text", "press_key", "install_model", "list_models", "forget_target",
    ):
        check(f"清单里有 {must}", must in names)

    # 清单顺序 = 先只读、后写、最后危险
    order = {"readonly": 0, "write": 1, "dangerous": 2}
    seq = [order.get(t["risk"], 9) for t in tools]
    check("清单按风险从低到高排列", seq == sorted(seq))

    # ---------------- 2. 状态与自己
    print("\n== 服务状态 ==")
    st, r = ai.call("get_status")
    d = r.get("data") or {}
    check("get_status 返回 running/port", r.get("ok") and "port" in d, r.get("summary", ""))
    check("summary 是人话（不是 JSON）", bool(r.get("summary")) and not r["summary"].startswith("{"))

    st, r = ai.call("list_endpoints")
    check("业务端点 20 个", (r.get("data") or {}).get("count") == 20)

    st, r = ai.call("get_logs", limit="5")  # 故意传字符串：AI 常这么写
    check("get_logs 接受字符串型 limit", r.get("ok") is True, r.get("error", ""))

    st, r = ai.call("wait", ms="120")
    check("wait 120ms", r.get("ok") is True and (r.get("data") or {}).get("waited_ms") == 120)

    st, r = ai.call("list_models")
    models = {m["id"]: m for m in (r.get("data") or {}).get("models", [])}
    check("list_models 有清单", len(models) > 0, f"{len(models)} 项")

    # ---------------- 3. 看
    print("\n== 截图 / 认字 / 定位 ==")
    st, r = ai.call("screenshot")
    shot = (r.get("data") or {}).get("image_path", "")
    check("screenshot 落盘", bool(shot) and Path(shot).is_file(), shot)
    check(
        "screenshot 带尺寸与原点",
        all(k in (r.get("data") or {}) for k in ("width", "height", "origin_x", "origin_y")),
    )
    if not shot or not Path(shot).is_file():
        return

    st, r = ai.call("ocr_image", image_path=shot, max_lines="5")
    text = (r.get("data") or {}).get("text", "")
    lines = (r.get("data") or {}).get("lines") or []
    check("ocr_image 认出字", bool(text.strip()), f"{len(text)} 字符")
    check("ocr_image 每行带坐标", all("center_x" in l and "center_y" in l for l in lines))

    if lines:
        first = lines[0]["text"][:4]
        st, r = ai.call("find_text", keyword=first, image_path=shot)
        hits = (r.get("data") or {}).get("matches") or []
        check(f"find_text 找到「{first}」", bool(hits), f"{len(hits)} 处")
        if hits:
            cx, cy = hits[0]["center_x"], hits[0]["center_y"]
            # 屏幕绝对坐标：截全屏时应当落在虚拟屏范围内，而不是「图内 0 起点」
            vs_x, vs_y = ctypes.windll.user32.GetSystemMetrics(76), ctypes.windll.user32.GetSystemMetrics(77)
            vs_w, vs_h = ctypes.windll.user32.GetSystemMetrics(78), ctypes.windll.user32.GetSystemMetrics(79)
            inside = vs_x <= cx <= vs_x + vs_w and vs_y <= cy <= vs_y + vs_h
            check("find_text 返回屏幕绝对坐标", inside, f"({cx},{cy}) vs 虚拟屏 {vs_x},{vs_y} {vs_w}x{vs_h}")

    st, r = ai.call("list_windows")
    wins = (r.get("data") or {}).get("windows") or []
    check("list_windows 有窗口", len(wins) > 0, f"{len(wins)} 个")
    check("list_windows 带 hwnd/矩形/前台标记",
          all(k in w for w in wins for k in ("hwnd", "x", "y", "width", "height", "foreground")))

    # ---------------- 4. 码 / 表 / 检测
    print("\n== 二维码 / 表格 / 检测 ==")
    qr = TMP / "qr.png"
    if make_qr_png(qr):
        st, r = ai.call("read_qrcode", image_path=str(qr))
        codes = (r.get("data") or {}).get("codes") or []
        check("read_qrcode 读出内容", bool(codes) and "OCR-STUDIO" in codes[0]["text"],
              codes[0]["text"] if codes else r.get("summary", ""))
    else:
        skip("read_qrcode", "没有 qrcode 库（用 PowerShell 的 python 跑就装好了）")

    table_png = TMP / "table.png"
    has_pil = make_table_png(table_png)
    if models.get("table", {}).get("installed"):
        st, r = ai.call("read_table", image_path=str(table_png), mode="structure")
        tables = (r.get("data") or {}).get("tables") or []
        check("read_table 认出结构", bool(tables), r.get("error", "") or r.get("summary", ""))
        if tables:
            t = tables[0]
            check("read_table 有 HTML", "<td" in t.get("html", ""), t.get("html", "")[:60])
            check("read_table 有 TSV 文本", "\n" in t.get("text", "") or "\t" in t.get("text", ""))
            check("read_table 给出每格坐标", len(t.get("cells") or []) >= 4,
                  f"{len(t.get('cells') or [])} 格")
        if not has_pil:
            print("       （PIL 不在，表格图是纯框线版；只看结构不看字）")
    else:
        skip("read_table", "表格模型没装（list_models 显示 table 未安装）")

    st, r = ai.call("detect_objects", image_path=str(table_png))
    if r.get("ok"):
        check("detect_objects 返回结构", "detections" in (r.get("data") or {}), r.get("summary", ""))
    else:
        # 没装检测模型时必须是「指路型」错误，不能是看不懂的内部错
        err = r.get("error", "")
        check("缺检测模型时给出可执行的提示", "模型" in err or "yolo" in err.lower(), err[:80])

    # ---------------- 5. 教 / 找 / 忘
    print("\n== 中文参考库 ==")
    if lines:
        b = lines[0]
        box = [b["x"], b["y"], b["x"] + max(8, b["width"]), b["y"] + max(8, b["height"])]
        st, r = ai.call("teach_target", name="验收目标", image_path=shot, box=box)
        check("teach_target 成功", r.get("ok") is True, r.get("error", ""))

        st, r = ai.call("list_targets")
        names_now = [t["name"] for t in (r.get("data") or {}).get("targets", [])]
        check("list_targets 能看到新教的", "验收目标" in names_now)

        # 提示找图模型是按需下载的。解压版/干净机器上没装，这时候报「缺模型」
        # 是**正确行为**，不该判 FAIL —— 否则每台没装 yoloe 的机器验收都是红的。
        st, rm = ai.call("list_models")
        has_yoloe = any(
            m.get("id") == "yoloe-prompt" and m.get("installed")
            for m in ((rm.get("data") or {}).get("models") or [])
        )
        if not has_yoloe:
            skip("find_target 不报错", "没装「提示找图」模型（按需下载），报缺模型是正确行为")
        else:
            st, r = ai.call("find_target", names="验收目标", image_path=shot, conf=0.1)
            check("find_target 不报错", r.get("ok") is True, r.get("error", ""))

        st, r = ai.call("forget_target", name="验收目标")
        check("forget_target 删掉了", (r.get("data") or {}).get("removed") is True, r.get("summary", ""))

    # ---------------- 6. 动手（只验校验路径，不真的点/打字）
    print("\n== 动手能力（只验安全边界）==")
    x, y = cursor_pos()
    st, r = ai.call("move_mouse", x=x, y=y)
    check("move_mouse 原地移动", r.get("ok") is True, r.get("error", ""))

    st, r = ai.call("press_key", keys="hyper")
    check("不认识的键被拒绝", r.get("ok") is False and "type_text" in (r.get("error") or ""),
          (r.get("error") or "")[:70])

    st, r = ai.call("click_at", x=100)
    check("click_at 缺参数点名缺 y", r.get("ok") is False and "y" in (r.get("error") or ""),
          (r.get("error") or "")[:70])

    st, r = ai.call("click_at", x=100, y=100, button="side")
    check("非法 button 被拒绝", r.get("ok") is False and "left" in (r.get("error") or ""))

    st, r = ai.call("focus_window", title="这个窗口肯定不存在-xyz")
    check("找不到窗口时列出候选", r.get("ok") is False and "list_windows" in (r.get("error") or ""),
          (r.get("error") or "")[:70])

    skip("click_at / type_text 真注入", "会动到用户正在用的窗口，改用手动验收")

    # ---------------- 7. 安全与错误
    print("\n== 安全 / 错误处理 ==")
    st, _ = http("POST", f"{ai.base}/ai/v1/tool", b'{"tool":"get_status"}')
    check("不带连接码 → 401", st == 401, f"HTTP {st}")

    st, r = ai.call("no_such_tool")
    check("未知工具给候选", r.get("ok") is False and "tools" in (r.get("error") or ""),
          (r.get("error") or "")[:70])

    st, r = ai.call("ocr_image", image_path="C:/definitely/not/here.png")
    check("读不存在的图报清楚", r.get("ok") is False and "读不了" in (r.get("error") or ""),
          (r.get("error") or "")[:60])


def run_inject(ai: Ai) -> None:
    """真注入回环：记事本里打字 → 认出来 → 点一下。

    为什么要在记事本里做：输入只会进**前台窗口**，随便找个窗口打字会污染用户数据。
    记事本是自己开的、不落盘（最后直接杀进程，连「要不要保存」都不会弹）。
    """
    print("\n== 真注入（记事本回环）==")
    before = {w["hwnd"] for w in ((ai.call("list_windows")[1].get("data") or {}).get("windows") or [])}
    proc = subprocess.Popen(["notepad.exe"])
    target = None
    for _ in range(20):
        time.sleep(0.5)
        wins = ((ai.call("list_windows")[1].get("data") or {}).get("windows") or [])
        cand = [
            w for w in wins
            if w["hwnd"] not in before and ("记事本" in w["title"] or "Notepad" in w["title"])
        ]
        if cand:
            target = cand[0]
            break
    if not target:
        skip("记事本回环", "没等到记事本窗口（可能是精简系统没装记事本）")
        subprocess.run(["taskkill", "/F", "/PID", str(proc.pid)], capture_output=True)
        return

    try:
        st, r = ai.call("focus_window", hwnd=target["hwnd"])
        got_focus = (r.get("data") or {}).get("foreground") is True
        check("focus_window 抢到前台", r.get("ok") is True, r.get("summary", ""))
        if not got_focus:
            skip("打字 / 点击回环", "没抢到前台焦点（Windows 前台锁），输入会打到别的窗口")
            return

        mark = "OCRStudioTest42"
        st, r = ai.call("type_text", text=mark)
        check("type_text 输入成功", r.get("ok") is True, r.get("error", ""))

        # **打完必须等一下再截**：PrintWindow 抓的是「当前这一帧」，
        # 输入事件刚发出去时界面还没重绘，抓到的是上一帧 —— 会误判成「打字没生效」。
        hits = []
        for _ in range(6):
            ai.call("wait", ms=300)
            st, r = ai.call("find_text", keyword=mark, source="window", hwnd=target["hwnd"])
            hits = (r.get("data") or {}).get("matches") or []
            if hits:
                break
        check("打进去的字被认出来", bool(hits), r.get("summary", ""))
        if not hits:
            return
        # 窗口截图的原点应该是窗口左上角（不是 0），否则坐标不能直接点
        ox, oy = (r.get("data") or {}).get("origin_x"), (r.get("data") or {}).get("origin_y")
        on_screen = hits[0]["center_x"] >= 0 and hits[0]["center_y"] >= 0
        check("窗口截图坐标已换算到屏幕", on_screen, f"origin=({ox},{oy})")

        st, r = ai.call("click_at", x=hits[0]["center_x"], y=hits[0]["center_y"])
        check("click_at 点下去成功", r.get("ok") is True, r.get("error", ""))

        st, r = ai.call("press_key", keys="ctrl+a")
        check("press_key 组合键成功", r.get("ok") is True, r.get("error", ""))
    finally:
        # 直接杀进程：有未保存内容时点关闭会弹「要不要保存」，自动化里是死锁
        subprocess.run(["taskkill", "/F", "/PID", str(proc.pid)], capture_output=True)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default="", help="没起服务时用这个 exe 拉起来")
    ap.add_argument("--base", default="", help="默认按配置里的端口")
    ap.add_argument("--token", default="", help="默认读配置里保存的连接码")
    ap.add_argument("--inject", action="store_true", help="真注入测试（开一个记事本，打字+点击，测完关掉）")
    a = ap.parse_args()

    cfg = {}
    if CFG.is_file():
        cfg = json.loads(CFG.read_text(encoding="utf-8"))
    base = a.base or f"http://127.0.0.1:{cfg.get('port', 8848)}"
    token = a.token or cfg.get("ai_token", "")
    if not token:
        print("[FAIL] 没有连接码：先在设置 → AI 控制里打开并复制")
        return 2
    if not cfg.get("ai_enabled", False):
        print("[FAIL] AI 接口没开：设置 → AI 控制 → 打开")
        return 2

    if not wait_port(base, 0.1):
        if not a.exe:
            print(f"[FAIL] 服务没起（{base}）。用 --exe 指定程序，或先启动它")
            return 2
        print(f"[INFO] 拉起 {a.exe}")
        subprocess.Popen(["explorer.exe", str(Path(a.exe).resolve())])
        if not wait_port(base, 45):
            print("[FAIL] 45 秒内服务没起来")
            return 2

    ai = Ai(base, token)
    run(ai)
    if a.inject:
        run_inject(ai)

    print()
    if fails:
        print(f"[结论] {len(fails)} 项失败：{', '.join(fails)}")
        return 1
    print(f"[结论] 全部通过（跳过 {len(skips)} 项）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
