"""检视 ONNX 模型的输入/输出结构，为 Rust 侧预处理与后处理提供硬参数。

用法: python inspect_models.py [models/small]
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import onnx
from onnx import TensorProto


def dim_str(dims) -> str:
    out = []
    for d in dims:
        if d.HasField("dim_value"):
            out.append(str(d.dim_value))
        elif d.HasField("dim_param"):
            out.append(d.dim_param or "?")
        else:
            out.append("?")
    return "[" + ", ".join(out) + "]"


def type_str(vi: onnx.ValueInfoProto) -> str:
    t = vi.type.tensor_type
    shape = dim_str(t.shape.dim)
    name = TensorProto.DataType.Name(t.elem_type) if t.elem_type else "?"
    return f"{name} {shape}"


def describe(model_path: Path) -> dict:
    m = onnx.load(str(model_path))
    g = m.graph
    info = {
        "file": model_path.name,
        "producer": f"{m.producer_name} {m.producer_version}",
        "opset": [f"{o.domain or 'ai.onnx'}:{o.version}" for o in m.opset_import],
        "inputs": [
            {"name": i.name, "type": type_str(i)} for i in g.input
        ],
        "outputs": [
            {"name": o.name, "type": type_str(o)} for o in g.output
        ],
        "nodes": [
            {"op": n.op_type, "name": n.name} for n in g.node
        ],
        "initializers": [
            {"name": t.name, "dims": list(t.dims)} for t in g.initializer
        ],
    }
    return info


def main() -> None:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else "models/small")
    if not root.is_absolute():
        root = Path(__file__).resolve().parent.parent / root

    report = {}
    for name in ("det.onnx", "rec.onnx", "cls.onnx"):
        p = root / name
        if not p.exists():
            continue
        info = describe(p)
        report[name] = info
        print(f"=== {name} ===")
        print(f"  producer : {info['producer']}")
        print(f"  opset    : {', '.join(info['opset'])}")
        for i in info["inputs"]:
            print(f"  input    : {i['name']:<24} {i['type']}")
        for o in info["outputs"]:
            print(f"  output   : {o['name']:<24} {o['type']}")
        print(f"  nodes    : {len(info['nodes'])}")
        print()

    keys = root / "keys.txt"
    if keys.exists():
        lines = keys.read_text(encoding="utf-8").splitlines()
        non_blank = [l for l in lines if l.strip() != ""]
        print("=== keys.txt ===")
        print(f"  lines={len(lines)} non_blank={len(non_blank)}")
        print(f"  first={non_blank[:5]}")
        print(f"  last ={non_blank[-5:]}")
        report["keys.txt"] = {
            "lines": len(lines),
            "non_blank": len(non_blank),
            "tail_has_blank": lines[-1] == "",
        }

    out = root / "inspect.json"
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n已写出 {out}")


if __name__ == "__main__":
    main()
