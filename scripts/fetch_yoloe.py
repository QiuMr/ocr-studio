#!/usr/bin/env python3
"""导出 YOLOE-26 开放词汇模型为 ONNX，放进 models/detect/yolo/。

YOLOE 是 Ultralytics 的开放词汇检测/分割框架，三种提示方式：
  - 文本提示  给几个词（"红色的血条"）让它找；
  - 视觉提示  给一张参考图 + 一个框，让它找「跟这个像的」（**推荐，做「语义找图」用**）；
  - 免提示    用它内置的 4585 类词表，不写提示。

**关键约束**：提示是在**导出时烘焙进权重**的。导出后的 onnx 就是普通检测模型，
不能再改类别；换目标要重新导出（重跑本脚本即可）。

为什么用视觉提示而不是文本提示：
  文本提示要跑 CLIP 文本编码器（`git+https://github.com/ultralytics/CLIP.git`），
  本机装不上（git 子模块在本机 MSYS 环境拉不动）。视觉提示不需要文本编码器。

用法
----
  # 视觉提示（参考图 + 框）——做「语义找图」
  python scripts/fetch_yoloe.py visual --refer assets/bus.png \
         --box 31,230,801,775 --name bus_finder

  # 文本提示（需要 clip，本机目前装不上，会明确提示）
  python scripts/fetch_yoloe.py text --prompt "person,bus" --name coco_two

  # 免提示（内置 4585 类词表）
  python scripts/fetch_yoloe.py promptfree --name yoloe_pf

产物：models/detect/yolo/<name>.onnx（+ 同名 .txt 类名兜底文件）
"""
from __future__ import annotations

import argparse
import os
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "models" / "detect" / "yolo"

# 把 ultralytics 的配置/权重缓存放到项目里，别往 C 盘塞（本机 C 盘很紧）
os.environ.setdefault("YOLO_CONFIG_DIR", str(ROOT / ".ultralytics"))

# 原始 .pt 权重的缓存目录（11MB+，属于开发夹具，不进 git 也不进分发包）
WEIGHTS_CACHE = ROOT / "models" / "_fixtures" / "weights"


def load_yoloe(weights: str):
    """加载 YOLOE 权重；本地没有时让 ultralytics 下到缓存目录。

    为什么不能直接 `YOLOE("yoloe-26n-seg.pt")`：ultralytics 对**裸文件名**会下到
    当前工作目录，也就是仓库根——11MB 的 .pt 落在仓库根既污染工作区，
    又不在 .gitignore 的 `models/**` 规则里。这里先 chdir 到缓存目录再加载，
    加载完立刻切回来（模型已在内存里，后续导出路径不受影响）。
    """
    from ultralytics import YOLOE

    if Path(weights).exists():
        return YOLOE(weights)
    WEIGHTS_CACHE.mkdir(parents=True, exist_ok=True)
    cwd = Path.cwd()
    try:
        os.chdir(WEIGHTS_CACHE)
        print(f"      本地没有 {weights}，下载到 {WEIGHTS_CACHE}")
        return YOLOE(weights)
    finally:
        os.chdir(cwd)


def _install_fuse_patch() -> None:
    """绕开 ultralytics 8.4.13 在 YOLOE-26（end2end）导出时的 bug。

    导出流程是：`model.fuse()`（对 end2end head 会调 `head.fuse(None)`，
    把 one2many 分支的 cv2/cv3/cv4 置空）→ 紧接着 `head.fuse(pe)` 想把提示
    融进分类头，却仍去 fuse 已置空的 cv3/cv4 → `zip(None, None)` 抛
    `TypeError: 'NoneType' object is not iterable`。

    补丁：end2end 时 one2many 已被移除，只 fuse `one2one_cv3/one2one_cv4`。
    """
    import torch.nn as nn
    from ultralytics.nn.modules.head import YOLOEDetect

    if getattr(YOLOEDetect.fuse, "_yoloe_patched", False):
        return
    def _patched_fuse(self, txt_feats=None):
        import torch  # 闭包内解析，别依赖外层 import 的作用域

        if txt_feats is None:  # 去掉 one2many 分支（end2end 专用）
            self.cv2 = self.cv3 = self.cv4 = None
            return
        if getattr(self, "is_fused", False):
            return
        txt_feats = txt_feats.to(torch.float32).squeeze(0)
        if self.cv3 is not None and self.cv4 is not None:
            self._fuse_tp(txt_feats, self.cv3, self.cv4)
        if getattr(self, "end2end", False):
            self._fuse_tp(txt_feats, self.one2one_cv3, self.one2one_cv4)
        if hasattr(self, "reprta"):
            self.reprta = nn.Identity()
        self.is_fused = True

    _patched_fuse._yoloe_patched = True
    YOLOEDetect.fuse = _patched_fuse


def _report(onnx_path: Path) -> None:
    import onnx

    mo = onnx.load(str(onnx_path))
    print("  输入:", [(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in mo.graph.input])
    print("  输出:", [(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in mo.graph.output])
    for kv in mo.metadata_props:
        if kv.key in ("names", "task", "imgsz"):
            print(f"  meta {kv.key} = {kv.value[:160]}")


def _finish(exported: str, name: str, names: list[str]) -> None:
    """把导出结果搬到 models/detect/yolo/<name>.onnx 并写类名兜底文件。"""
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    dst = OUT_DIR / f"{name}.onnx"
    src = Path(exported)
    if src.resolve() != dst.resolve():
        shutil.move(str(src), str(dst))
    # 类名兜底：模型 metadata 里也有，但有的工具链会丢 metadata
    (OUT_DIR / f"{name}.txt").write_text("\n".join(names) + "\n", encoding="utf-8")
    size_mb = dst.stat().st_size / 1e6
    print(f"\n[OK] 已生成 {dst}  ({size_mb:.1f} MB)")
    _report(dst)
    print(
        "\n下一步：重启服务（或点设置页的「刷新」），"
        f"下拉里就会多出「找物体 · {name}」。"
    )


def do_visual(args) -> None:
    import numpy as np
    from ultralytics import YOLOE
    from ultralytics.models.yolo.yoloe import YOLOEVPSegPredictor

    weights = f"yoloe-26{args.size}-seg.pt"
    refer = str(Path(args.refer).resolve())
    box = np.array([args.box], dtype=np.float32)
    print(f"[1/3] 用一次性模型从参考图提取视觉提示向量（{weights}，参考 {refer}）")
    tmp = load_yoloe(weights)
    tmp.predict(
        refer,
        refer_image=refer,
        visual_prompts={"bboxes": box, "cls": np.array([0])},
        predictor=YOLOEVPSegPredictor,
        save=False,
        verbose=False,
    )
    vpe = tmp.model.pe.clone()
    del tmp
    print(f"      vpe 形状 {tuple(vpe.shape)}")

    print("[2/3] 干净模型写入提示向量并导出 ONNX")
    m = load_yoloe(weights)
    m.set_classes([args.name], embeddings=vpe)
    exported = m.export(format="onnx", opset=12, imgsz=args.imgsz, simplify=True)
    print("[3/3] 归档")
    _finish(exported, args.name, [args.name])


def do_text(args) -> None:
    from ultralytics import YOLOE

    try:
        import clip  # noqa: F401
    except ImportError:
        sys.exit(
            "文本提示需要 CLIP 文本编码器，本机装不上（ultralytics 的 CLIP fork 要拉\n"
            "git 子模块，而本机 MSYS 的 sh fork 是坏的：0xC0000142）。\n"
            "请改用视觉提示：python scripts/fetch_yoloe.py visual --refer <图> --box x1,y1,x2,y2\n"
            "或免提示：python scripts/fetch_yoloe.py promptfree"
        )

    names = [s.strip() for s in args.prompt.split(",") if s.strip()]
    if not names:
        sys.exit("--prompt 不能为空，例如 --prompt \"person,bus\"")

    weights = f"yoloe-26{args.size}-seg.pt"
    print(f"[1/2] 文本提示 {names} → 导出（{weights}）")
    m = load_yoloe(weights)
    m.set_classes(names)
    exported = m.export(format="onnx", opset=12, imgsz=args.imgsz, simplify=True)
    print("[2/2] 归档")
    _finish(exported, args.name, names)


def do_promptfree(args) -> None:
    from ultralytics import YOLOE

    weights = f"yoloe-26{args.size}-seg-pf.pt"
    print(f"[1/2] 免提示模型（内置 4585 类词表）→ 导出（{weights}）")
    m = load_yoloe(weights)
    names = list(m.names.values()) if isinstance(m.names, dict) else list(m.names)
    exported = m.export(format="onnx", opset=12, imgsz=args.imgsz, simplify=True)
    print("[2/2] 归档")
    _finish(exported, args.name, names)


def main() -> None:
    ap = argparse.ArgumentParser(description="导出 YOLOE-26 为 ONNX 检测模型")
    ap.add_argument("mode", choices=["visual", "text", "promptfree"])
    ap.add_argument("--size", default="n", choices=["n", "s", "m", "l", "x"], help="模型规模，默认 n")
    ap.add_argument("--name", default=None, help="产物名（决定下拉里的显示名与文件名）")
    ap.add_argument("--imgsz", type=int, default=640)
    ap.add_argument("--refer", help="[visual] 参考图路径")
    ap.add_argument("--box", help="[visual] 参考框 x1,y1,x2,y2")
    ap.add_argument("--prompt", help="[text] 逗号分隔的类别词")
    args = ap.parse_args()

    if args.mode == "visual":
        if not args.refer or not args.box:
            sys.exit("visual 模式需要 --refer 和 --box")
        parts = [int(float(x)) for x in args.box.split(",")]
        if len(parts) != 4:
            sys.exit("--box 格式应为 x1,y1,x2,y2")
        args.box = parts
        args.name = args.name or f"yoloe26{args.size}_visual"
    elif args.mode == "text":
        args.name = args.name or f"yoloe26{args.size}_text"
    else:
        args.name = args.name or f"yoloe26{args.size}_pf"

    # 只允许英文/数字/下划线做文件名，避免路径编码坑
    if not all(c.isalnum() or c in "_-" for c in args.name):
        sys.exit("--name 只允许字母、数字、下划线、连字符（中文名请在设置页里改显示名）")

    _install_fuse_patch()
    {"visual": do_visual, "text": do_text, "promptfree": do_promptfree}[args.mode](args)


if __name__ == "__main__":
    main()
