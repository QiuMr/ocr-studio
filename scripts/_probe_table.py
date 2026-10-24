"""一次性的模型探查脚本：打印两个表格模型的输入输出，不做别的。

跑法（必须 PowerShell 的 python，Bash 那边的解释器没装 onnxruntime）：
    python scripts/_probe_table.py
"""
import onnxruntime as ort

for f in ["models/_probe/slanet-plus.onnx", "models/_probe/layout_table.onnx"]:
    print("=" * 60)
    print(f)
    s = ort.InferenceSession(f, providers=["CPUExecutionProvider"])
    for i in s.get_inputs():
        print(f"  IN  {i.name:20} {i.shape} {i.type}")
    for o in s.get_outputs():
        print(f"  OUT {o.name:20} {o.shape} {o.type}")
    print("  metadata:", dict(s.get_modelmeta().custom_metadata_map))
