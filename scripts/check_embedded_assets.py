"""检查 exe 内是否嵌入了前端资源（Tauri 的 dist 资源）。

Tauri 在「生产模式」下会把 frontendDist 目录内容嵌进二进制；开发模式则不嵌，
而是走 build.devUrl（例如 http://localhost:1420）。
所以：exe 里能搜到 dist/assets 的 js 文件名 => 资源已嵌入。

用法：python scripts/check_embedded_assets.py <exe> [资源名...]
"""

import sys
from pathlib import Path


def scan(path: Path, needles: list[str]) -> dict[str, bool]:
    found = {n: False for n in needles}
    remaining = set(needles)
    with path.open("rb") as f:
        tail = b""
        while True:
            chunk = f.read(8 * 1024 * 1024)
            if not chunk:
                break
            buf = tail + chunk
            for n in list(remaining):
                if n.encode() in buf:
                    found[n] = True
                    remaining.discard(n)
            if not remaining:
                break
            tail = buf[-64:]  # 处理跨块边界
    return found


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 1
    exe = Path(sys.argv[1])
    needles = sys.argv[2:]
    print(f"文件: {exe} ({exe.stat().st_size / 1024 / 1024:.1f} MB)")
    res = scan(exe, needles)
    for k, v in res.items():
        print(f"  {'[有]' if v else '[无]'} {k}")
    embedded = any(res.values())
    print("[结论] 资源已嵌入（生产模式）" if embedded else "[结论] 未嵌入资源（开发模式，走 devUrl）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
