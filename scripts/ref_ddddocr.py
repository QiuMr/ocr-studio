"""ddddocr（common 版）rec 的 Python 参考实现，仅用于 Rust 实现的对齐验证。

不进分发包，也不参与运行时。它逐行复刻 ddddocr 1.6.1 `core/ocr_engine.py` 的
预处理与 CTC 解码，用来回答一个问题：**Rust 版本识别出来的字一样吗？**

关键点（都是实测确认过的，别凭印象改）：
1. 预处理顺序是「先 resize 再转灰度」，不是先灰度再 resize。
   PIL 的 convert('L') 用 ITU-R 601-2 系数（0.299/0.587/0.114），
   与 image crate 默认的 Rec.709 系数不同，差在个别灰阶。
2. resize 用 `Image.LANCZOS`（support=3 的窗函数 sinc），不是双线性。
3. 高度固定 64，宽度 `int(w * 64 / h)` —— 是**截断**不是四舍五入。
4. 输入 `[1, 1, 64, W]`，像素除以 255，不做 mean/std 标准化。
5. 输出 `[1, T, C]` 或 `[T, 1, C]`，argmax 后先「去连续重复」再「丢 index 0（blank）」。
   charset 长度 8210，`charset[0] == ''` 是 blank，**不是**空格。

用法：
    python scripts/ref_ddddocr.py --image assets/test_zh.png
    python scripts/ref_ddddocr.py --image models/small/dump/box_0_src.png --dump-npy out.npy
"""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
MODEL = ROOT / "models" / "ocr" / "ddddocr" / "common" / "rec.onnx"
CHARSET = ROOT / "models" / "ocr" / "ddddocr" / "common" / "charset.txt"

TARGET_H = 64


def load_charset(path: Path) -> list[str]:
    """按行读，**不过滤空行**：第 0 行就是 blank。"""
    text = path.read_text(encoding="utf-8")
    return text.split("\n")


def preprocess(img: Image.Image) -> np.ndarray:
    rgb = img.convert("RGB")
    w, h = rgb.size
    target_w = max(1, int(w * (TARGET_H / h)))
    resized = rgb.resize((target_w, TARGET_H), Image.LANCZOS)
    gray = resized.convert("L")
    arr = np.asarray(gray, dtype=np.float32) / 255.0
    return arr[None, None, :, :]


def ctc_decode(probs: np.ndarray, charset: list[str]) -> tuple[str, list[tuple[int, int, float]]]:
    """返回 (文本, [(字符索引, 帧起点, 帧终点+1, 该帧概率)])。"""
    # 兼容 [T,1,C] 与 [1,T,C]
    if probs.ndim == 3:
        probs = probs[:, 0, :] if probs.shape[1] == 1 else probs[0, :, :]
    if probs.shape[-1] != len(charset):
        raise SystemExit(
            f"类别数 {probs.shape[-1]} 与字典长度 {len(charset)} 不一致，字典用错了版本"
        )
    idx = probs.argmax(axis=1)
    conf = probs.max(axis=1)

    out = []
    prev = -1
    for t, i in enumerate(idx):
        i = int(i)
        if i != prev and i != 0:
            out.append((i, t, t + 1, float(conf[t])))
        prev = i

    text = "".join(charset[i] if i < len(charset) else "" for i, _, _, _ in out)
    return text, out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--box", help="裁剪框 x0,y0,x1,y1（左闭右开），缺省整图当一行")
    ap.add_argument("--dump-npy", help="把预处理后的张量存成 .npy，便于逐值比对")
    ap.add_argument("--json", action="store_true", help="输出纯 JSON")
    args = ap.parse_args()

    import onnxruntime as ort

    img = Image.open(args.image)
    if args.box:
        x0, y0, x1, y1 = (int(v) for v in args.box.split(","))
        img = img.crop((x0, y0, x1, y1))

    tensor = preprocess(img)
    if args.dump_npy:
        np.save(args.dump_npy, tensor)

    sess = ort.InferenceSession(str(MODEL), providers=["CPUExecutionProvider"])
    name = sess.get_inputs()[0].name
    out = sess.run(None, {name: tensor})[0]

    charset = load_charset(CHARSET)
    text, spans = ctc_decode(out, charset)

    result = {
        "charset_size": len(charset),
        "crop_size": list(img.size),
        "input_shape": list(tensor.shape),
        "output_shape": list(out.shape),
        "text": text,
        # 便于跨语言比对的张量指纹：求和与均值对预处理差异极其敏感
        "tensor_sum_ppm": int(round(float(tensor.sum()) * 1_000_000)),
        "tensor_first8": [round(float(v), 6) for v in tensor.reshape(-1)[:8]],
        "frames": int(spans[-1][2] if spans else 0),
        "char_spans": [{"i": i, "t0": t0, "t1": t1} for i, t0, t1, _ in spans],
    }
    if args.json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
