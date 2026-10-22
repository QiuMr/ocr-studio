#!/usr/bin/env python3
"""探针：文本提示编码器能否导出成 ONNX（token ids → pe）。

链路（对照 ultralytics 的 get_text_pe）：
    ids = clip.clip.tokenize(texts)              # OpenAI CLIP SimpleTokenizer，输出 (B,77)
    f   = mobileclip_blt.ts(ids)                 # TorchScript，已内嵌 L2 归一化，输出 (B,512)
    pe  = F.normalize(head.reprta(f.unsqueeze(0)), dim=-1)   # reprta 是 YOLOE 的 SwiGLUFFN

要验证：
  1. .ts 能加载、能导出 ONNX；
  2. ONNX 与 torch 逐值一致；
  3. 不同文本给出不同 pe（语义真的进来了）。
"""
from __future__ import annotations

import os
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
os.environ.setdefault("YOLO_CONFIG_DIR", str(ROOT / ".ultralytics"))
WEIGHTS = ROOT / "models" / "_fixtures" / "weights"
TMP = ROOT / "models" / "_fixtures" / "_probe"
TMP.mkdir(parents=True, exist_ok=True)
TS = WEIGHTS / "mobileclip_blt.ts"


def main() -> None:
    import torch
    import torch.nn as nn
    import torch.nn.functional as F
    import clip as clip_pkg
    from ultralytics import YOLOE
    from ultralytics.nn.modules.head import YOLOEDetect

    print(f"[0] .ts 大小 {TS.stat().st_size/1e6:.1f} MB")
    encoder = torch.jit.load(str(TS), map_location="cpu").eval()
    print(f"[0] TorchScript 加载成功")

    tok = clip_pkg.clip.tokenize
    texts = ["person", "bus", "traffic light"]
    ids = tok(texts, truncate=True)
    print(f"[1] tokenize -> {tuple(ids.shape)} dtype={ids.dtype}")
    print(f"    ids[0][:12] = {ids[0][:12].tolist()}")

    with torch.no_grad():
        f = encoder(ids)
    print(f"[2] .ts 输出 {tuple(f.shape)}  行范数 {f.norm(dim=-1).tolist()}（应为 1.0，归一化内嵌）")

    # 拿 YOLOE 的 reprta（文本提示头）
    os.chdir(WEIGHTS)
    m = YOLOE("yoloe-26n-seg.pt")
    os.chdir(ROOT)
    model = m.model.float().eval()
    head = model.model[-1]
    assert isinstance(head, YOLOEDetect)
    reprta = head.reprta

    class TextEnc(nn.Module):
        """ids -> pe，等价于 head.get_tpe(encode_text(ids)).unsqueeze(0)。"""

        def __init__(self, enc, rt):
            super().__init__()
            self.enc = enc
            self.rt = rt

        def forward(self, ids):
            f = self.enc(ids)              # (B,512)
            x = f.unsqueeze(0)             # (1,B,512) —— get_text_pe 里 reshape(-1,len(text),D)
            return F.normalize(self.rt(x), dim=-1, p=2)

    te = TextEnc(encoder, reprta).eval()
    with torch.no_grad():
        pe_t = te(ids)
    print(f"[3] pe shape {tuple(pe_t.shape)} 行范数 {pe_t[0].norm(dim=-1).tolist()}")

    # 与 ultralytics 官方 get_text_pe 对照（同一份 ids 走官方路径）
    with torch.no_grad():
        ref = model.get_text_pe(texts)  # 内部会自己 tokenize
    print(f"[3] 官方 get_text_pe shape {tuple(ref.shape)}")
    if tuple(ref.shape) == tuple(pe_t.shape):
        print(f"[3] 与官方最大绝对差 {np.abs(ref.numpy()-pe_t.numpy()).max():.3e}")

    import onnx
    import onnxruntime as ort

    so = ort.SessionOptions()
    so.log_severity_level = 3
    out = TMP / "text_pe.onnx"
    with torch.no_grad():
        torch.onnx.export(
            te, (ids,), str(out),
            input_names=["token_ids"], output_names=["pe"],
            dynamic_axes={"token_ids": {0: "n"}, "pe": {0: "1", 1: "n"}},
            opset_version=12, do_constant_folding=True,
        )
    mo = onnx.load(str(out))
    print("[4] ONNX 输入:", [(i.name, [d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]) for i in mo.graph.input])
    print("[4] ONNX 输出:", [(o.name, [d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]) for o in mo.graph.output])

    sess = ort.InferenceSession(str(out), so, providers=["CPUExecutionProvider"])
    got = sess.run(["pe"], {"token_ids": ids.numpy()})[0]
    print(f"[5] ort pe {got.shape} 与 torch 最大绝对差 {np.abs(got - pe_t.numpy()).max():.3e}")

    # 不同文本 → 不同 pe
    ids2 = tok(["person"], truncate=True)
    g2 = sess.run(["pe"], {"token_ids": ids2.numpy()})[0]
    print(f"[6] 只喂 'person' → pe {g2.shape}，与三词版本首行余弦 "
          f"{float(np.dot(g2[0,0], got[0,0])):.4f}（应为 1.0，同词同行）")

    print(f"\n[OK] 文本编码器可导出：{out}")


if __name__ == "__main__":
    main()
