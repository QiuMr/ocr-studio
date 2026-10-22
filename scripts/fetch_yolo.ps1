# 取一个目标检测模型：ultralytics 权重 -> ONNX(opset12, 固定 imgsz) + labels.txt
#
# 2026-10-22 起**不再按版本分目录**：引擎只有「目标检测」一类，模型结构
# （输出布局 / objectness / 输入边长）在加载时按 ONNX 形状自动识别。
# 所以这里也只需要一个扁平目录：models\detect\yolo\{model.onnx, labels.txt}
#
# 前提：本机 Python 已装 ultralytics（含 torch）。
#   C:\...\Python312\python.exe -m pip install ultralytics
#
# 用法（在仓库根目录）：
#   powershell -File scripts\fetch_yolo.ps1                       # 默认 yolov8n
#   powershell -File scripts\fetch_yolo.ps1 -Model yolo11n.pt     # 换模型
#   powershell -File scripts\fetch_yolo.ps1 -Model yolo12n.pt -Imgsz 960
#   powershell -File scripts\fetch_yolo.ps1 -Model D:\my\trained.pt
#
# 支持的输入：任何 ultralytics 能导出成**单输出** ONNX 的检测权重
# （yolov5u / yolov8 / yolo11 / yolo12 / yolo26 等）。
# v3/v4 的多输出 head 需要按 anchor 逐层解码，本引擎不支持，脚本会直接报错。
#
# labels.txt 与权重同源（读模型元数据），不手工维护。

param(
    [string]$Model = 'yolov8n.pt',
    [int]$Imgsz = 640,
    [string]$Python = 'C:\Users\13114\AppData\Local\Programs\Python\Python312\python.exe'
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$dir = Join-Path $root 'models\detect\yolo'
New-Item -ItemType Directory -Force -Path $dir | Out-Null

Write-Host "[INFO] 模型: $Model / imgsz=$Imgsz -> $dir"

$code = @'
import os, sys
from ultralytics import YOLO

d, weight, imgsz = sys.argv[1], sys.argv[2], int(sys.argv[3])
m = YOLO(weight)
p = m.export(format='onnx', opset=12, imgsz=imgsz, dynamic=False, simplify=True)
onnx_tmp = p if os.path.isabs(p) else os.path.abspath(p)
dst = os.path.join(d, 'model.onnx')
if onnx_tmp != dst:
    os.replace(onnx_tmp, dst)

names = m.model.names
# 类别名必须连续 0..N-1，否则 labels.txt 的行号与 class_id 会错位
assert sorted(names) == list(range(len(names))), names
with open(os.path.join(d, 'labels.txt'), 'w', encoding='utf-8') as f:
    for i in range(len(names)):
        f.write(str(names[i]) + '\n')
print('OK ->', dst, '+ labels.txt (%d 类)' % len(names))
'@

$tmp = Join-Path $env:TEMP "fetch_yolo_$PID.py"
[System.IO.File]::WriteAllText($tmp, $code, [System.Text.UTF8Encoding]::new($false))
try {
    & $Python $tmp $dir $Model $Imgsz
    if ($LASTEXITCODE -ne 0) { throw "导出失败（exit $LASTEXITCODE）。v3/v4 的多输出 head 不受支持，请用 ultralytics 单输出导出。" }
} finally {
    Remove-Item $tmp -ErrorAction SilentlyContinue
}

Get-ChildItem $dir | Select-Object Name, @{n='MB'; e={ [math]::Round($_.Length / 1MB, 2) }}
