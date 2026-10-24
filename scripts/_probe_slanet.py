"""SLANet-plus 原型：先证明前处理 + 解码是对的，再翻译成 Rust。

全部细节对齐 paddlex 的 TableRunnerPredictor / TableLabelDecode：
  ReadImage(BGR) -> ResizeByLong(488) -> Normalize -> Pad(127.5) -> ToCHW
  argmax 逐位置 + 遇 eos 停 + **不去重**（不是 CTC）
  字符表 = ["sos"] + metadata["character"] + ["eos"]
  bbox 0-1000 归一化，x 乘原图宽、y 乘原图高

跑法（PowerShell 的 python，Bash 那个解释器没装 onnxruntime）：
    python scripts/_probe_slanet.py
    python scripts/_probe_slanet.py --diag     # 打印原始张量
"""
import sys

import numpy as np
import onnxruntime as ort
from PIL import Image, ImageDraw, ImageFont

MODEL = "models/_probe/slanet-plus.onnx"
MAX_LEN = 488
MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)
TD_TOKENS = {"<td>", "<td", "<td></td>"}


def make_bench(small=True):
    """画表格。small=True 时 3 列 3 行（第一行前两列合并）——32 个位置装得下。

    为什么要给两个尺寸：模型输出序列只有 32 个位置，一个单元格要吃掉
    `<td` `>` `</td>` 三个 token，4x4 的表 53 个 token 会**被截断**。
    """
    img_w, img_h = (560, 320) if small else (780, 460)
    img = Image.new("RGB", (img_w, img_h), "white")
    d = ImageDraw.Draw(img)
    try:
        f = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 20 if small else 22)
    except OSError:
        f = ImageFont.load_default()

    if small:
        cols = [20, 200, 380, 540]
        rows = [20, 120, 220, 300]
        cells = [
            (20, 20, "合并了两列"), (380, 20, "金额"),
            (20, 120, "苹果"), (200, 120, "12"), (380, 120, "36.00"),
            (20, 220, "香蕉"), (200, 220, "8"), (380, 220, "24.00"),
        ]
        merged = (200, 20, 200, 120)
    else:
        cols = [20, 200, 380, 560, 760]
        rows = [20, 100, 180, 260, 340]
        cells = [
            (20, 20, "合并单元格跨两列"), (380, 20, "数量"), (560, 20, "金额"),
            (20, 100, "苹果"), (200, 100, "kg"), (380, 100, "12"), (560, 100, "36.00"),
            (20, 180, "香蕉"), (200, 180, "kg"), (380, 180, "8"), (560, 180, "24.00"),
            (20, 260, "合计"), (380, 260, "20"), (560, 260, "60.00"),
        ]
        merged = (200, 20, 200, 100)

    for r0, r1 in zip(rows, rows[1:]):
        for c0, c1 in zip(cols, cols[1:]):
            d.rectangle([c0, r0, c1, r1], outline="black", width=2)
    # 合并：把中间那条竖线抹掉
    mx, my0, _, my1 = merged
    d.line([mx, my0 + 2, mx, my1 - 2], fill="white", width=6)

    for x, y, t in cells:
        if t:
            d.text((x + 14, y + 26), t, font=f, fill="black")
    return img


def preprocess(img):
    w, h = img.size
    ratio = MAX_LEN / max(w, h)
    rw, rh = max(1, int(w * ratio)), max(1, int(h * ratio))
    small = img.resize((rw, rh), Image.BILINEAR)

    # BGR + RGB 顺序的 mean/std —— PaddleOCR 的固有错配，必须照抄
    a = np.asarray(small, dtype=np.float32)[:, :, ::-1]
    a = (a / 255.0 - MEAN) / STD

    # 归一化之后再 pad，pad 填 **0**（PaddleOCR 的 PaddingTableImage 是 zeros，
    # paddlex 那套 127.5 是另写的，套在这个模型上会出乱码）
    canvas = np.zeros((MAX_LEN, MAX_LEN, 3), dtype=np.float32)
    canvas[:rh, :rw] = a
    return np.transpose(canvas, (2, 0, 1))[None], (w, h)


def decode(loc, struct, ori_w, ori_h, tokens):
    prob = struct[0]                      # [L, C]
    idx = prob.argmax(axis=1)
    score = prob.max(axis=1)
    end_idx = len(tokens) - 1             # 最后一个是 eos

    struct_list, bboxes = [], []
    for i, ci in enumerate(idx):
        ci = int(ci)
        if i > 0 and ci == end_idx:
            break
        if ci == 0:                       # sos，忽略
            continue
        t = tokens[ci]
        if t in TD_TOKENS:
            s = np.zeros(8)
            s[0::2] = ori_w
            s[1::2] = ori_h
            bboxes.append(loc[0, i].astype(np.float64) * s)
        struct_list.append(t)

    boxes = []
    for b in bboxes:
        xs, ys = b[0::2], b[1::2]
        boxes.append((float(xs.min()), float(ys.min()),
                      float(xs.max() - xs.min()), float(ys.max() - ys.min())))
    return struct_list, boxes, float(score.mean())


def load():
    sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    md = sess.get_modelmeta().custom_metadata_map
    tokens = ["sos"] + md["character"].split("\n") + ["eos"]
    return sess, tokens


def diag(sess, tokens):
    img = make_bench()
    x, (w, h) = preprocess(img)
    loc, struct = sess.run(None, {"x": x})
    idx = struct[0].argmax(axis=1)
    print("loc 形状", loc.shape, "值域", float(loc.min()), float(loc.max()))
    print("struct argmax:", list(idx))
    print("图尺寸", w, h)
    for i in range(min(8, loc.shape[1])):
        print(f"  [{i}] idx={int(idx[i]):2d} {tokens[int(idx[i])][:12]!r:16}",
              np.round(loc[0, i], 1))


def main():
    img = make_bench()
    img.save("models/_probe/_bench_table.png")

    sess, tokens = load()
    print(f"字符表 {len(tokens)} 项（含 sos/eos），模型类别数 "
          f"{sess.get_outputs()[1].shape[-1]}")

    if "--diag" in sys.argv:
        diag(sess, tokens)
        return

    x, (w, h) = preprocess(img)
    loc, struct = sess.run(None, {"x": x})
    print("输入", x.shape, "loc", loc.shape, "struct", struct.shape)

    struct_list, boxes, score = decode(loc, struct, w, h, tokens)
    print(f"\n结构（{len(struct_list)} token，平均置信 {score:.3f}）：")
    print(" ".join(t if t.strip() else "_" for t in struct_list))

    print(f"\n{len(boxes)} 个单元格 bbox（原图坐标）：")
    for b in boxes:
        print("   ", [round(v, 1) for v in b])

    vis = img.copy()
    dv = ImageDraw.Draw(vis)
    for x0, y0, bw, bh in boxes:
        dv.rectangle([x0, y0, x0 + bw, y0 + bh], outline="red", width=2)
    vis.save("models/_probe/_bench_boxes.png")

    print("\nHTML:", "".join(struct_list))


if __name__ == "__main__":
    main()
