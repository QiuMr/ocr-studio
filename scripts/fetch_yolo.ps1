# 取 YOLO 检测模型：官方权重 -> ultralytics 导出 ONNX(opset12, 640) + labels.txt
#
# 前提：本机 Python 已装 ultralytics（含 torch）。
#   C:\...\Python312\python.exe -m pip install ultralytics
#
# 用法（在仓库根目录）：
#   powershell -File scripts\fetch_yolo.ps1              # 默认 v8n
#   powershell -File scripts\fetch_yolo.ps1 -Version v8  # 等价
# 产物：models\detect\yolo\v8\{model.onnx, labels.txt}
#   labels.txt 与权重同源（从模型元数据读 COCO 80 类），不手工维护。

param(
    [string]$Version = 'v8',
    [string]$Python = 'C:\Users\13114\AppData\Local\Programs\Python\Python312\python.exe'
)

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

# 仓库根 = 本脚本的上两级
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$dir = Join-Path $root "models\detect\yolo\$Version"
New-Item -ItemType Directory -Force -Path $dir | Out-Null

# 目前只有 v8 有现成导出流程；v5/v7/v11 的导出源与目录留给后续版本
if ($Version -ne 'v8') {
    Write-Host "[SKIP] 目前只支持 v8（yolov8n.pt -> ONNX）；$Version 请自行导出后放入 $dir" -ForegroundColor Yellow
    exit 0
}

$pt = Join-Path $dir 'yolov8n.pt'
if (-not (Test-Path $pt)) {
    Write-Host "[INFO] 下载 yolov8n.pt ..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest `
        -Uri 'https://github.com/ultralytics/assets/releases/download/v8.3.0/yolov8n.pt' `
        -OutFile $pt -UseBasicParsing
}

$code = @'
import os, sys
from ultralytics import YOLO

d = sys.argv[1]
m = YOLO(os.path.join(d, 'yolov8n.pt'))
p = m.export(format='onnx', opset=12, imgsz=640, dynamic=False, simplify=True)
onnx_tmp = p if os.path.isabs(p) else os.path.abspath(p)
dst = os.path.join(d, 'model.onnx')
if onnx_tmp != dst:
    os.replace(onnx_tmp, dst)

names = m.model.names
assert len(names) == 80 and set(names) == set(range(80)), names
with open(os.path.join(d, 'labels.txt'), 'w', encoding='utf-8') as f:
    for i in range(80):
        f.write(names[i] + '\n')
print('OK ->', dst, '+ labels.txt (80)')
'@

$tmp = Join-Path $env:TEMP "fetch_yolo_$PID.py"
[System.IO.File]::WriteAllText($tmp, $code, [System.Text.UTF8Encoding]::new($false))
try {
    & $Python $tmp $dir
    if ($LASTEXITCODE -ne 0) { throw "导出失败（exit $LASTEXITCODE）" }
} finally {
    Remove-Item $tmp -ErrorAction SilentlyContinue
}

Get-ChildItem $dir | Select-Object Name, @{n='MB'; e={ [math]::Round($_.Length / 1MB, 2) }}
