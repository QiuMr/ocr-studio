"""下载 MiSans 字体（小米官方字体服务）到前端 assets，并生成本地 @font-face CSS。

用法：
    python scripts/fetch_misans_font.py

产物：
    src/assets/fonts/misans/*.woff2   56 个 unicode-range 切片（400 字重）
    src/assets/fonts/misans.css       改写为相对路径的 @font-face 规则

为什么要落地到本地而不是在 index.html 里 link 官方 CDN：
    桌面应用必须离线可用；另外官方 CDN 是面向网页的分片服务，
    打包后依然依赖网络会导致首屏字体闪烁。

授权口径：MiSans 为小米商用字体，随包分发前请自行确认授权条款
（见 docs/PROGRESS.md 的风险记录）。
"""

import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "src" / "assets" / "fonts" / "misans"
OUT_CSS = ROOT / "src" / "assets" / "fonts" / "misans.css"

CSS_URL = (
    "https://cdn-font.hyperos.mi.com/font/css"
    "?family=MiSans:400:Chinese_Simplify,Latin&display=swap"
)

URL_RE = re.compile(r'url\("(https://cdn-file\.hyperos\.mi\.com/[^"]+\.woff2)"\)')


def fetch(url: str, timeout: int = 60) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def main() -> int:
    print(f"获取字体 CSS: {CSS_URL}")
    css = fetch(CSS_URL).decode("utf-8")
    urls = URL_RE.findall(css)
    print(f"发现 {len(urls)} 个 woff2 切片")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    total = 0
    for i, url in enumerate(urls, 1):
        name = url.rsplit("/", 1)[-1]
        dest = OUT_DIR / name
        if dest.exists() and dest.stat().st_size > 0:
            data_size = dest.stat().st_size
        else:
            data = fetch(url)
            dest.write_bytes(data)
            data_size = len(data)
        total += data_size
        # CSS 里的 URL 改写为相对路径（CSS 与 woff2 同目录）
        css = css.replace(url, f"./misans/{name}")
        if i % 10 == 0 or i == len(urls):
            print(f"  {i}/{len(urls)}  {total / 1024 / 1024:.2f} MB")

    OUT_CSS.write_text(css, encoding="utf-8")
    print(f"完成：{total / 1024 / 1024:.2f} MB -> {OUT_DIR}")
    print(f"CSS: {OUT_CSS}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
