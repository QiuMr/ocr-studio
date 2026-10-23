"""用**程序自己的界面截图**验收「按中文名找」（中文参考库），全程无人值守。

为什么另起一个脚本（不并进 verify_packaged.py）：素材是现截的程序窗口，
要起窗口、点导航、等渲染；verify_packaged.py 是纯 HTTP 断言。两者混在一起，
「包对不对」和「界面在不在」会互相拖累。

流程：
  1. 起服务（explorer 拉起 exe —— agent shell 里直接跑会白屏，见项目记忆）
  2. 点左侧「设置」导航，截一张窗口图 A
  3. 已移除的 `mode=text` 必须明确报错并指路（不是静默失效）
  4. 参考库：POST /vision/promptref 教一次「左侧侧边栏」（框住整条侧边栏）
  5. 同图按名字找（mode=ref）→ 命中中心应落在教的那个框里
  6. 点「接口」导航截一张 B，跨图再找一次（参考库的核心价值：换图还能找）
  7. DELETE 清理参考库，输出 PASS / FAIL 汇总

用法：python scripts/verify_ui_zh.py --exe "release/OCR-Studio/OCR Studio.exe"
"""

import argparse
import base64
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import ctypes
import ctypes.wintypes as wt
import mss
import numpy as np
from PIL import Image

user32 = ctypes.windll.user32
user32.SetProcessDPIAware()
user32.SetForegroundWindow.argtypes = [wt.HWND]
user32.SetForegroundWindow.restype = wt.BOOL
user32.SetWindowPos.argtypes = [
    wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_uint
]
user32.SetWindowPos.restype = wt.BOOL

VK_MENU = 0x12
HWND_TOPMOST, HWND_NOTOPMOST = -1, -2
SWP_NOMOVE, SWP_NOSIZE = 0x0002, 0x0001

# 左侧导航固定 5 项（服务/接口/试跑/日志/设置），但**不写死像素坐标**：
# 窗口可以从默认 1102x722 被拉伸，DPI 缩放也会改变物理像素，任何硬编码的
# 比例一旦过期就会「点到隔壁」——踩过一次（点了「设置」其实还在「服务」页，
# 后续教参考图教到了正文标题上，找图自然是 0 命中）。
# 所以改成从截图里**测**：左侧竖条按行投影找暗像素带，取最后 5 段。
NAV_ITEMS = ["服务", "接口", "试跑", "日志", "设置"]


class Fail(Exception):
    pass


def check(name: str, cond: bool, detail: str = "") -> None:
    tag = "[PASS]" if cond else "[FAIL]"
    print(f"{tag} {name}" + (f" —— {detail}" if detail else ""), flush=True)
    if not cond:
        raise Fail(name)


def find_hwnd():
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        n = user32.GetWindowTextLengthW(hwnd)
        if n == 0:
            return True
        buf = ctypes.create_unicode_buffer(n + 1)
        user32.GetWindowTextW(hwnd, buf, n + 1)
        if "ocr studio" in buf.value.lower():
            hits.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return hits[0] if hits else None


def window_rect(hwnd):
    """DWM 真实边界（GetWindowRect 会多出约 15px 阴影，见 shot_window.py）。"""
    rect = wt.RECT()
    if ctypes.windll.dwmapi.DwmGetWindowAttribute(
        hwnd, 9, ctypes.byref(rect), ctypes.sizeof(rect)
    ) != 0:
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return rect


def tap_key(vk):
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.03)
    user32.keybd_event(vk, 0, 0x0002, 0)


def activate(hwnd):
    """把窗口抬到最前并激活。顺序照 ui_probe.py：先 ALT 解锁前台锁。"""
    tap_key(VK_MENU)
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    user32.SetForegroundWindow(hwnd)
    user32.BringWindowToTop(hwnd)
    time.sleep(0.5)


def activate_click(hwnd, x, y, wait_ms=900):
    activate(hwnd)
    user32.SetCursorPos(4, 4)
    time.sleep(0.1)
    user32.SetCursorPos(x, y)
    time.sleep(0.12)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(wait_ms / 1000.0)


def measure_nav(img):
    """从截图里测出左侧导航 5 项的行带（相对窗口的像素坐标）。

    做法：取左侧竖条（x 16 ~ 18% 宽），按行统计「暗像素个数」，
    连续的暗行合并成一段；合并后取**最后 5 段**——从下往上数最保险，
    因为标题栏的折叠图标 / 底部的版本号偶尔也会被算进来。
    返回 [{"name":..., "x0","y0","x1","y1","cx","cy"}, ...] 共 5 项。
    """
    a = np.array(img.convert("L"))
    h, w = a.shape
    x_lo, x_hi = 16, max(40, int(w * 0.18))
    dark = (a[:, x_lo:x_hi] < 120).sum(axis=1)

    segs = []
    for y in range(h):
        if dark[y] > 3:
            if segs and y - segs[-1][1] <= 3:
                segs[-1][1] = y
            else:
                segs.append([y, y])
    segs = [s for s in segs if s[1] - s[0] >= 5]
    if len(segs) < 5:
        raise Fail(f"左侧导航只测到 {len(segs)} 段（期望 5 段），截图可能没抓对")
    segs = segs[-5:]

    items = []
    for name, (y0, y1) in zip(NAV_ITEMS, segs):
        cols = (a[y0:y1 + 1, :x_hi] < 120).sum(axis=0)
        xs = [x for x in range(x_lo, x_hi) if cols[x] > 0]
        x0, x1 = (min(xs), max(xs)) if xs else (x_lo, x_lo + 40)
        items.append({
            "name": name, "x0": x0, "y0": y0, "x1": x1, "y1": y1,
            "cx": (x0 + x1) // 2, "cy": (y0 + y1) // 2,
        })
    return items


def grab(hwnd):
    rect = window_rect(hwnd)
    w, h = rect.right - rect.left, rect.bottom - rect.top
    with mss.mss() as sct:
        shot = sct.grab({"left": rect.left, "top": rect.top, "width": w, "height": h})
        img = Image.frombytes("RGB", shot.size, shot.bgra, "raw", "BGRX")
    user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE)
    return img


def b64(png: Path) -> str:
    return base64.b64encode(png.read_bytes()).decode()


def _read(resp_or_err) -> dict:
    """把响应体读成 dict。

    400 是本脚本的**正常预期**之一（`mode=text` 已移除、名字没教过都会被拒），
    所以不能指望 urllib 抛异常，必须把 HTTPError 也当响应读。
    """
    raw = resp_or_err.read().decode("utf-8", "replace")
    try:
        return json.loads(raw)
    except Exception:
        return {"code": -1, "msg": raw}


def post(base: str, path: str, body: dict, timeout=120) -> dict:
    req = urllib.request.Request(
        base + path, json.dumps(body).encode(), {"Content-Type": "application/json"}
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(req, timeout=timeout) as r:
            return _read(r)
    except urllib.error.HTTPError as e:
        return _read(e)


def delete(base: str, path: str, timeout=30) -> dict:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    req = urllib.request.Request(base + path, method="DELETE")
    try:
        with opener.open(req, timeout=timeout) as r:
            return _read(r)
    except urllib.error.HTTPError as e:
        return _read(e)


def wait_health(base: str, seconds: int = 90) -> None:
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for _ in range(seconds * 2):
        try:
            with opener.open(base + "/ocr/health", timeout=5) as r:
                j = json.loads(r.read().decode())
                if j.get("code") == 0:
                    return
        except Exception:
            pass
        time.sleep(0.5)
    raise Fail("服务 90 秒内未就绪")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", required=True, help="分发包里的 OCR Studio.exe")
    ap.add_argument("--keep", action="store_true", help="验收完不杀进程（调试用）")
    args = ap.parse_args()
    exe = str(Path(args.exe).resolve())
    base = "http://127.0.0.1:8848"
    tmp = Path(__file__).parent / "_verify_ui"
    tmp.mkdir(exist_ok=True)

    subprocess.run(["taskkill", "/F", "/IM", "OCR Studio.exe"],
                   capture_output=True)
    time.sleep(0.8)
    subprocess.Popen(["explorer.exe", exe])
    print(f"[INFO] 已拉起 {exe}", flush=True)
    wait_health(base)
    print("[PASS] 服务就绪", flush=True)

    hwnd = find_hwnd()
    if not hwnd:
        raise Fail("没找到 OCR Studio 窗口")
    rect = window_rect(hwnd)
    w, h = rect.right - rect.left, rect.bottom - rect.top
    print(f"[INFO] 窗口 {w}x{h}", flush=True)
    if w < 600 or h < 400:
        raise Fail(f"窗口太小（{w}x{h}），导航会挤在一起，请拉大窗口")

    # 先激活再抓一张，从里面量出 5 个导航项的真实位置
    activate(hwnd)
    nav_items = measure_nav(grab(hwnd))
    print("[INFO] 测到的导航项: " + ", ".join(
        f"{it['name']}({it['cx']},{it['cy']})" for it in nav_items), flush=True)
    by_name = {it["name"]: it for it in nav_items}

    def nav(item: str, out: str) -> Path:
        it = by_name[item]
        activate_click(hwnd, rect.left + it["cx"], rect.top + it["cy"])
        p = tmp / out
        grab(hwnd).save(p)
        print(f"[INFO] 已切到「{item}」并截图 -> {p}", flush=True)
        return p

    # ---- 3. 语言提示已移除：应当明确指路，不是静默失效 ----
    a = nav("设置", "settings.png")
    j = post(base, "/vision/prompt",
             {"mode": "text", "words": "按钮", "source": "image", "image": b64(a)})
    msg = str(j.get("msg", ""))
    check("已移除的语言提示被明确拒绝", j.get("code") != 0 and "已移除" in msg, msg[:160])
    check("报错里指路 promptref", "promptref" in msg, msg[:160])

    # ---- 4. 参考库：教一次「左侧侧边栏」 ----
    # 教什么粒度决定成败，实测（同一张设置页截图，默认 conf=0.25，看**首位命中**）：
    #   「日志」那一行文字 (38x21)     → 0.16，等于噪声，0 命中
    #   整行「图标+文字」(166x36)      → 0.16；切页 0.11，0 命中
    #   导航块（5 项，170x190）        → 同图 0.74 但**被正文卡片顶到第 2 位**；
    #                                    切页后正文候选全进来，导航块直接掉出前 3
    #   整条侧边栏 (180x670)           → 同图 **0.94**、切页 **0.84**，首位都对
    # 结论：VPE 认的是「一块有结构的整体」。细条 / 半块既不稳又会被局部纹理骗走，
    # 整条侧边栏反而最稳（它自带一个大面积的稳定版式，页码切换不影响）。
    # 坐标全部来自量出来的行带，不写死像素。
    TEACH_NAME = "左侧侧边栏"
    box = [0, 30, int(w * 0.163), h - 22]
    j = post(base, "/vision/promptref",
             {"name": TEACH_NAME, "image": b64(a), "box": box})
    check("参考库：教一次 code=0", j.get("code") == 0, j.get("msg", "")[:160])

    # ---- 5. 同图按名字找，中心应落在教的框里 ----
    j = post(base, "/vision/prompt",
             {"mode": "ref", "words": TEACH_NAME, "source": "image", "image": b64(a)})
    dets = j.get("data", {}).get("matches", []) if j.get("code") == 0 else []
    check("参考库：同图按中文名找有命中", j.get("code") == 0 and len(dets) >= 1,
          f"code={j.get('code')} 命中 {len(dets)} 个" if j.get("code") == 0 else j.get("msg", "")[:160])
    if dets:
        d = dets[0]
        cx, cy = d.get("center_x"), d.get("center_y")
        inside = box[0] - 12 <= cx <= box[2] + 12 and box[1] - 12 <= cy <= box[3] + 12
        check("参考库：命中中心落在教的框内", inside,
              f"center=({cx:.0f},{cy:.0f}) box={box} score={d.get('score', 0):.3f}")

    # ---- 6. 跨图泛化：切到「接口」页再找一次 ----
    b = nav("接口", "api.png")
    # 先钉住「导航真的切页了」：上一轮就是点在空处没切走，两张图其实同一页，
    # 结果看着像「找图不准」，实际是素材没换。字节比对最省事。
    check("两张截图不是同一页（导航真的切过去了）",
          a.read_bytes() != b.read_bytes(),
          f"settings={a.stat().st_size}B api={b.stat().st_size}B")
    j = post(base, "/vision/prompt",
             {"mode": "ref", "words": TEACH_NAME, "source": "image", "image": b64(b)})
    dets = j.get("data", {}).get("matches", []) if j.get("code") == 0 else []
    check("参考库：换一张图还能找到", j.get("code") == 0 and len(dets) >= 1,
          f"code={j.get('code')} 命中 {len(dets)} 个" if j.get("code") == 0 else j.get("msg", "")[:160])

    # ---- 7. 清理 ----
    j = delete(base, "/vision/promptref?name=" + urllib.request.quote(TEACH_NAME))
    check("参考库：删除 code=0", j.get("code") == 0, j.get("msg", "")[:160])

    print("\n[PASS] 截图验收全部通过（素材全部来自程序界面现截）", flush=True)
    if not args.keep:
        subprocess.run(["taskkill", "/F", "/IM", "OCR Studio.exe"], capture_output=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Fail as e:
        print(f"\n[FAIL] 验收未通过：{e}", flush=True)
        subprocess.run(["taskkill", "/F", "/IM", "OCR Studio.exe"], capture_output=True)
        sys.exit(1)
    except Exception:
        # 网络异常 / KeyError 之类也要把窗口收掉，否则下次跑会端口占用
        import traceback

        traceback.print_exc()
        subprocess.run(["taskkill", "/F", "/IM", "OCR Studio.exe"], capture_output=True)
        sys.exit(1)
