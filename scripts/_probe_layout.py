"""layout_table.onnx（PicoDet 架构）原型：在整页里定位表格。

难点全在后处理：DFL 解码 + 网格中心点 + letterbox 还原。参数组合不多，
用「白纸上贴一个表格」来判对错——框必须盖住表格区域。

跑法（PowerShell 的 python）：
    python scripts/_probe_layout.py
"""
import numpy as np
import onnxruntime as ort
from PIL import Image, ImageDraw, ImageFont

MODEL = "models/_probe/layout_table.onnx"
IN_H, IN_W = 800, 608
MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
STD = np.array([0.229, 0.224, 0.225], dtype=np.float32)
STRIDES = (8, 16, 32, 64)
REG_MAX = 7


def make_page():
    """A4 比例的白纸：上面几行正文，中间一个表格，下面再来几行。"""
    W, H = 1000, 1400
    img = Image.new("RGB", (W, H), "white")
    d = ImageDraw.Draw(img)
    try:
        f = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 26)
        fs = ImageFont.truetype(r"C:\Windows\Fonts\msyh.ttc", 22)
    except OSError:
        f = fs = ImageFont.load_default()

    d.text((80, 70), "第二节 采购清单", font=f, fill="black")
    for i in range(4):
        d.text((80, 140 + i * 40), f"这是一段正文说明文字，第 {i + 1} 行，用来模拟真实文档版面。",
               font=fs, fill="black")

    # 表格：占页面中间一块
    tb = {"x0": 100, "y0": 360, "x1": 900, "y1": 760}
    cols = [100, 300, 500, 700, 900]
    rows = [360, 460, 560, 660, 760]
    for r0, r1 in zip(rows, rows[1:]):
        for c0, c1 in zip(cols, cols[1:]):
            d.rectangle([c0, r0, c1, r1], outline="black", width=2)
    head = ["品名", "规格", "数量", "金额"]
    for i, t in enumerate(head):
        d.text((cols[i] + 16, rows[0] + 34), t, font=fs, fill="black")
    body = [["苹果", "kg", "12", "36.00"], ["香蕉", "kg", "8", "24.00"], ["合计", "", "20", "60.00"]]
    for r, row in enumerate(body):
        for c, t in enumerate(row):
            if t:
                d.text((cols[c] + 16, rows[r + 1] + 34), t, font=fs, fill="black")

    for i in range(5):
        d.text((80, 840 + i * 40), f"表格之后的补充说明，第 {i + 1} 行。", font=fs, fill="black")
    return img, tb


def preprocess(img):
    w, h = img.size
    scale = min(IN_H / h, IN_W / w)
    rw, rh = int(round(w * scale)), int(round(h * scale))
    small = img.resize((rw, rh), Image.BILINEAR)

    # PaddleDetection 的 Resize：先缩到目标框内，再用 114 补齐
    canvas = np.full((IN_H, IN_W, 3), 114.0, dtype=np.float32)
    canvas[:rh, :rw] = np.asarray(small, dtype=np.float32)

    a = canvas[:, :, ::-1]               # BGR
    a = (a / 255.0 - MEAN) / STD
    return np.transpose(a, (2, 0, 1))[None], scale, (rw, rh)


def dfl(box32):
    """[N,32] -> [N,4] 像素距离（单位：stride）。"""
    b = box32.reshape(-1, 4, REG_MAX + 1)
    b = b - b.max(axis=2, keepdims=True)
    e = np.exp(b)
    p = e / e.sum(axis=2, keepdims=True)
    return (p * np.arange(REG_MAX + 1)).sum(axis=2)


def nms(boxes, scores, iou_thr=0.6):
    order = scores.argsort()[::-1]
    keep = []
    while order.size:
        i = order[0]
        keep.append(i)
        if order.size == 1:
            break
        rest = order[1:]
        xx1 = np.maximum(boxes[i, 0], boxes[rest, 0])
        yy1 = np.maximum(boxes[i, 1], boxes[rest, 1])
        xx2 = np.minimum(boxes[i, 2], boxes[rest, 2])
        yy2 = np.minimum(boxes[i, 3], boxes[rest, 3])
        inter = np.clip(xx2 - xx1, 0, None) * np.clip(yy2 - yy1, 0, None)
        a_i = (boxes[i, 2] - boxes[i, 0]) * (boxes[i, 3] - boxes[i, 1])
        a_r = (boxes[rest, 2] - boxes[rest, 0]) * (boxes[rest, 3] - boxes[rest, 1])
        iou = inter / np.maximum(a_i + a_r - inter, 1e-9)
        order = rest[iou <= iou_thr]
    return np.array(keep, dtype=int)


def postprocess(outs, scale, conf=0.3, dist_x_stride=True):
    """outs 顺序：4 个 score（[1,N,1]）后跟 4 个 box（[1,N,32]）。"""
    boxes, scores = [], []
    for lv, s in enumerate(STRIDES):
        score = outs[lv][0]              # [N,1]
        box = outs[4 + lv][0]            # [N,32]
        gh, gw = int(np.ceil(IN_H / s)), int(np.ceil(IN_W / s))
        if gh * gw != score.shape[0]:
            print(f"  跳过 stride {s}: grid {gh}x{gw} != {score.shape[0]}")
            continue

        prob = 1.0 / (1.0 + np.exp(-score[:, 0]))
        dist = dfl(box) * (s if dist_x_stride else 1.0)

        ys, xs = np.meshgrid(np.arange(gh), np.arange(gw), indexing="ij")
        cx = (xs.ravel() + 0.5) * s
        cy = (ys.ravel() + 0.5) * s
        x1 = cx - dist[:, 0]
        y1 = cy - dist[:, 1]
        x2 = cx + dist[:, 2]
        y2 = cy + dist[:, 3]
        keep = prob > conf
        boxes.append(np.stack([x1[keep], y1[keep], x2[keep], y2[keep]], axis=1))
        scores.append(prob[keep])

    boxes = np.concatenate(boxes)
    scores = np.concatenate(scores)
    if len(boxes) == 0:
        return []
    keep = nms(boxes, scores)
    out = []
    for i in keep:
        x1, y1, x2, y2 = boxes[i] / scale
        out.append((float(x1), float(y1), float(x2), float(y2), float(scores[i])))
    return out


def main():
    img, truth = make_page()
    img.save("models/_probe/_page.png")
    x, scale, (rw, rh) = preprocess(img)
    print(f"页面 {img.size} -> 缩放 {scale:.4f} 贴到 {rw}x{rh}")

    sess = ort.InferenceSession(MODEL, providers=["CPUExecutionProvider"])
    outs = sess.run(None, {"image": x})
    print("输出", [o.shape for o in outs])

    print("\n真值表格框:", [truth[k] for k in ("x0", "y0", "x1", "y1")])
    for mode, dx in (("距离×stride", True), ("距离不乘", False)):
        res = postprocess(outs, scale, dist_x_stride=dx)
        print(f"\n--- {mode} --- {len(res)} 个框")
        for r in res[:6]:
            print("   ", [round(v, 1) for v in r])

    res = postprocess(outs, scale)
    vis = img.copy()
    dv = ImageDraw.Draw(vis)
    for x1, y1, x2, y2, sc in res:
        dv.rectangle([x1, y1, x2, y2], outline="red", width=3)
        dv.text((x1 + 4, max(0, y1 - 18)), f"{sc:.2f}", fill="red")
    vis.save("models/_probe/_page_boxes.png")


if __name__ == "__main__":
    main()
