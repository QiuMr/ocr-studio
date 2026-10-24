"""诊断 loc 输出：为什么 bbox 偏。"""
import importlib.util

spec = importlib.util.spec_from_file_location("p", "scripts/_probe_slanet.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
m.diag()
