"""把可选模型打成 zip 并生成 catalog.json。

用法：
    python scripts/build_models.py            # 只打包 + 生成清单
    python scripts/build_models.py --upload   # 打包 + 生成 + 上传到 GitHub Release

为什么模型要打成 zip：
    PP-OCRv6 一档是 det/rec/cls + keys + meta 五个文件，逐个下发会让
    「装了一半算不算装上」变得很难判断。打成一个包、解到目录里，
    判定条件就退化成「目录里那几个文件在不在」。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODELS = ROOT / "models"
OUT = ROOT / "release" / "models"
CATALOG = ROOT / "src-tauri" / "assets" / "catalog.json"

RELEASE_TAG = "models"
REPO = "QiuMr/ocr-studio"

# 每个条目 = 一个 zip。src 里列的文件/目录会被放进 zip 根目录。
ENTRIES = [
    {
        "id": "ppocr-medium",
        "name": "PP-OCRv6 高精度",
        "group": "ocr",
        "note": "识别精度最高的一档，慢一些。老照片、低清扫描件、复杂排版建议用它。",
        "dir": "medium",
        "src": ["dir:medium"],
        "requires": ["det.onnx", "rec.onnx", "cls.onnx"],
        "endpoints": ["/ocr/ocr"],
        "recommended": True,
    },
    {
        "id": "ppocr-tiny",
        "name": "PP-OCRv6 极速",
        "group": "ocr",
        "note": "体积最小、速度最快，精度略低。配置低的机器或只求快时用。",
        "dir": "tiny",
        "src": ["dir:tiny"],
        "requires": ["det.onnx", "rec.onnx", "cls.onnx"],
        "endpoints": ["/ocr/ocr"],
        "recommended": False,
    },
    {
        "id": "ddddocr",
        "name": "ddddocr 验证码",
        "group": "ocr",
        "note": "专门认验证码和单行短文本，比通用 OCR 更快更准。",
        "dir": "ocr/ddddocr/common",
        "src": ["dir:ocr/ddddocr/common"],
        "requires": ["rec.onnx"],
        "endpoints": ["/ocr/ocr"],
        "recommended": False,
    },
    {
        "id": "yoloe-prompt",
        "name": "提示找图",
        "group": "find",
        "note": "给一张参考图圈一下，就能找「长得像的东西」。换目标不用换模型。",
        "dir": "yoloe",
        "src": ["files:" + ",".join([
            "yoloe/yoloe26n_prompt.onnx",
            "yoloe/yoloe26n_prompt.json",
            "yoloe/yoloe26n_vpe.onnx",
        ])],
        "requires": ["yoloe26n_prompt.onnx", "yoloe26n_vpe.onnx"],
        "endpoints": ["/vision/prompt", "/vision/promptref"],
        "recommended": True,
    },
    {
        # 一个能力两个模型，所以打成一个包：只下结构模型的话「在整页里找表格」
        # 就用不了，只下检测模型的话根本出不了 HTML —— 拆开卖只会让人下成半残。
        "id": "table",
        "name": "表格识别",
        "group": "doc",
        "note": "表格图片直接出 HTML 表格，每个单元格带坐标和文字；也能在整页文档里自动找到表格位置。",
        "dir": "table",
        "src": ["dir:table"],
        "requires": ["structure.onnx", "detect.onnx"],
        "endpoints": ["/ocr/biaoge"],
        "recommended": True,
    },
]


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def collect(entry: dict) -> list[tuple[Path, str]]:
    """把 entry 的 src 展开成 [(源文件, zip 内相对路径)]。"""
    pairs: list[tuple[Path, str]] = []
    for spec in entry["src"]:
        if spec.startswith("dir:"):
            rel = spec.split(":", 1)[1]
            d = MODELS / rel
            if not d.is_dir():
                raise SystemExit(f"[FAIL] 目录不存在：{d}")
            for p in sorted(d.rglob("*")):
                if p.is_file() and p.name not in ("inspect.json",) and not p.name.startswith("reference_"):
                    pairs.append((p, str(p.relative_to(d))))
        elif spec.startswith("files:"):
            for rel in spec.split(":", 1)[1].split(","):
                p = MODELS / rel
                if not p.is_file():
                    raise SystemExit(f"[FAIL] 文件不存在：{p}")
                pairs.append((p, p.name))
        else:
            raise SystemExit(f"[FAIL] 不认识的 src 写法：{spec}")
    return pairs


def build() -> list[dict]:
    OUT.mkdir(parents=True, exist_ok=True)
    models: list[dict] = []
    for e in ENTRIES:
        pairs = collect(e)
        total = sum(p.stat().st_size for p, _ in pairs)
        zip_name = f"{e['id']}-v1.zip"
        zip_path = OUT / zip_name
        if zip_path.exists():
            zip_path.unlink()
        with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as z:
            for src, rel in pairs:
                z.write(src, rel)
        size = zip_path.stat().st_size
        digest = sha256(zip_path)
        print(
            f"  {e['id']:<14} {len(pairs):>2} 个文件  "
            f"{total / 1048576:>7.1f}MB -> {size / 1048576:>7.1f}MB  {digest[:12]}"
        )
        models.append(
            {
                "id": e["id"],
                "name": e["name"],
                "group": e["group"],
                "note": e["note"],
                "file": zip_name,
                "dir": e["dir"],
                "size": size,
                "sha256": digest,
                "endpoints": e["endpoints"],
                "requires": e["requires"],
                "recommended": e["recommended"],
            }
        )
    return models


def write_catalog(models: list[dict]) -> None:
    catalog = {"version": 1, "models": models}
    CATALOG.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(catalog, ensure_ascii=False, indent=2)
    CATALOG.write_text(text, encoding="utf-8")
    # 同一份也放到 release/models 里，方便直接上传
    (OUT / "catalog.json").write_text(text, encoding="utf-8")
    print(f"\n[OK] 清单：{CATALOG.relative_to(ROOT)}（{len(models)} 个可选模型）")


def upload() -> None:
    gh = os.environ.get("OCR_GH") or "gh"
    try:
        subprocess.run([gh, "--version"], capture_output=True, check=True)
    except Exception:
        raise SystemExit(
            "[FAIL] 找不到 gh。把 gh 加进 PATH，或设 OCR_GH 指向 gh.exe"
        )

    # prerelease 是必须的：正式 release 会抢走 releases/latest，
    # 导致 latest.json 404、自动更新全线报废。
    r = subprocess.run(
        [gh, "release", "view", RELEASE_TAG, "--repo", REPO],
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        print(f"[INFO] 创建 Release {RELEASE_TAG}（prerelease）")
        subprocess.run(
            [
                gh, "release", "create", RELEASE_TAG,
                "--repo", REPO,
                "--prerelease",
                "--title", "模型库",
                "--notes",
                "可选模型包。程序设置页「模型库」会按需下载，不进主安装包。\n"
                "这个 Release 标成 prerelease 是刻意的：否则它会抢走 "
                "releases/latest，自动更新用的 latest.json 会 404。",
            ],
            check=True,
        )

    files = [str(OUT / "catalog.json")] + [
        str(OUT / f.name) for f in sorted(OUT.glob("*.zip"))
    ]
    print(f"[INFO] 上传 {len(files)} 个文件…")
    subprocess.run(
        [gh, "release", "upload", RELEASE_TAG, *files, "--repo", REPO, "--clobber"],
        check=True,
    )
    print("[OK] 上传完成")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--upload", action="store_true", help="上传到 GitHub Release")
    args = ap.parse_args()

    print("打包可选模型：")
    models = build()
    write_catalog(models)
    total = sum(m["size"] for m in models)
    print(f"可选模型合计 {total / 1048576:.1f}MB（不含在主包里）")

    if args.upload:
        upload()


if __name__ == "__main__":
    sys.exit(main())
