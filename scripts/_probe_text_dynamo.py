#!/usr/bin/env python3
"""探针：TorchScript 文本塔改用 dynamo 导出器能否成功。

legacy 导出器在 multi_head_attention 的 `unflatten(-1,[3,size])` 上卡在
"transpose for tensor of unknown rank"。torch 2.4 的 dynamo 导出器（需 onnxscript）
走 torch.export，形状处理完全不同，试试。
"""
from __future__ import annotations

import shutil
import tempfile
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
WEIGHTS = ROOT / "models" / "_fixtures" / "weights"
TMP = Path("D:/tmp-ocrstudio")


def main() -> None:
    import torch
    import clip as clip_pkg
    import onnxruntime as ort

    ts = WEIGHTS / "mobileclip2_b.ts"
    TMP.mkdir(parents=True, exist_ok=True)
    tmpdir = Path(tempfile.mkdtemp(prefix="dyn_", dir=str(TMP)))
    a = tmpdir / ts.name
    shutil.copy2(ts, a)
    enc = torch.jit.load(str(a), map_location="cpu").eval()
    print(f"[0] 文本塔已加载（副本 {a}）")

    ids = clip_pkg.clip.tokenize(["person", "bus"], truncate=True)
    print(f"[0] ids {tuple(ids.shape)}")

    so = ort.SessionOptions()
    so.log_severity_level = 3

    for opset in (17, 18, 20):
        out = tmpdir / f"tower_dyn_{opset}.onnx"
        try:
            torch.onnx.export(
                enc, (ids,), str(out),
                input_names=["token_ids"], output_names=["feat"],
                dynamic_axes={"token_ids": {0: "n"}, "feat": {0: "n"}},
                opset_version=opset, dynamo=True,
            )
        except Exception as e:  # noqa: BLE001
            print(f"[1] opset {opset} dynamo 失败：{type(e).__name__}: {str(e).splitlines()[0][:140]}")
            continue
        with torch.no_grad():
            ref = enc(ids).numpy()
        sess = ort.InferenceSession(str(out), so, providers=["CPUExecutionProvider"])
        got = sess.run(["feat"], {"token_ids": ids.numpy()})[0]
        d = np.abs(got - ref).max()
        print(f"[1] opset {opset} dynamo ✅ 导出成功 | ort {got.shape} vs torch {ref.shape} "
              f"最大差 {d:.3e} | 体积 {out.stat().st_size/1e6:.1f} MB")
        # 动态 batch 复核
        ids1 = ids[:1].numpy()
        g1 = sess.run(["feat"], {"token_ids": ids1})[0]
        print(f"[2] 喂 1 条 → {g1.shape}（应为 (1,512)），与批量首行最大差 "
              f"{np.abs(g1 - got[:1]).max():.3e}")
        shutil.rmtree(tmpdir, ignore_errors=True)
        print(f"\n[OK] dynamo 可用，产物 {out}")
        return

    print("\n❌ dynamo 也不行")
    shutil.rmtree(tmpdir, ignore_errors=True)


if __name__ == "__main__":
    main()
