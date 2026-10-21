# 下载 OCR 模型到 models/<level>/
#
# 说明：
# - 每档模型需要 det.onnx / rec.onnx / (cls.onnx 可选) / keys.txt / meta.json
# - 默认从 RapidOCR 官方发布的 PP-OCR ONNX 模型获取（PP-OCRv4/v5），
#   PP-OCRv6 的 ONNX 导出待验证后可替换同名文件。
# - 本脚本只负责下载与整理，不做模型转换。

param(
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$modelsDir = Join-Path $Root 'models'

# RapidOCR 模型下载源（modelscope 镜像，稳定）
# 目录结构参考 RapidOCR 仓库: det / rec / cls 模型 + 字典
$sources = @{
    'det' = 'https://modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_infer.onnx'
    'rec' = 'https://modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/rec/ch_PP-OCRv4_rec_infer.onnx'
    'cls' = 'https://modelscope.cn/models/RapidAI/RapidOCR/resolve/master/onnx/PP-OCRv4/cls/ch_ppocr_mobile_v2.0_cls_infer.onnx'
    'keys' = 'https://modelscope.cn/models/RapidAI/RapidOCR/resolve/master/paddle/PP-OCRv4/rec/ppocr_keys_v1.txt'
}

function Download-File($url, $dest) {
    if (Test-Path $dest) { Write-Host "已存在: $dest"; return }
    New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
    Write-Host "下载: $url"
    Invoke-WebRequest -Uri $url -OutFile $dest -UseBasicParsing
}

# 先只装 small 档用于打通链路
$level = 'small'
$dir = Join-Path $modelsDir $level
New-Item -ItemType Directory -Force $dir | Out-Null

Download-File $sources['det']  (Join-Path $dir 'det.onnx')
Download-File $sources['rec']  (Join-Path $dir 'rec.onnx')
Download-File $sources['cls']  (Join-Path $dir 'cls.onnx')
Download-File $sources['keys'] (Join-Path $dir 'keys.txt')

# 生成 meta.json（PP-OCRv4 默认参数）
$meta = @{
    model_version = 'PP-OCRv4'
    det = @{ input_size = 960; mean = @(0.485, 0.456, 0.406); std = @(0.229, 0.224, 0.225) }
    rec = @{ img_h = 48; img_w = 320 }
} | ConvertTo-Json -Depth 5
Set-Content -Path (Join-Path $dir 'meta.json') -Value $meta -Encoding UTF8

Write-Host "完成。模型目录: $dir"
Get-ChildItem $dir | Select-Object Name, Length | Format-Table -AutoSize
