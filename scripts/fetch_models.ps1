# 下载 OCR 模型到 models/<level>/
#
# 模型清单见同目录 models.manifest.json（URL + SHA256 均来自 RapidOCR 官方仓库）
# 三档全部采用 PP-OCRv6；PP-OCRv6 暂无 cls 模型，方向分类沿用 PP-OCRv5 LCNet。
#
# 用法：
#   .\fetch_models.ps1                 # 下载 small 档（默认）
#   .\fetch_models.ps1 -Level all      # 下载 tiny/small/medium 全部
#   .\fetch_models.ps1 -Level tiny

param(
    [string]$Level = 'small',
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$modelsDir = Join-Path $Root 'models'
$manifestPath = Join-Path $PSScriptRoot 'models.manifest.json'

$manifest = Get-Content -Path $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
$base = $manifest.base

function Get-FileSha256([string]$Path) {
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        try { return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLower() }
        finally { $sha.Dispose() }
    }
    finally { $stream.Dispose() }
}

function Download-One([string]$Url, [string]$Dest, [string]$Sha256) {
    if ((Test-Path $Dest) -and ((Get-Item $Dest).Length -gt 0)) {
        Write-Host "已存在: $Dest"
        return
    }
    $tmp = "$Dest.part"
    if (Test-Path $tmp) { Remove-Item $tmp -Force }
    Write-Host "下载: $Url"
    Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing

    if ($Sha256) {
        $actual = Get-FileSha256 $tmp
        if ($actual -ne $Sha256.ToLower()) {
            Remove-Item $tmp -Force -ErrorAction SilentlyContinue
            throw "SHA256 校验失败: $Url`n期望 $Sha256`n实际 $actual"
        }
        Write-Host "  SHA256 OK"
    }
    Move-Item -Path $tmp -Destination $Dest -Force
}

$levels = if ($Level -eq 'all') { @('tiny', 'small', 'medium') } else { @($Level) }

foreach ($lvl in $levels) {
    $lvlNode = $manifest.levels.PSObject.Properties[$lvl]
    if ($null -eq $lvlNode) { throw "未知档位: $lvl（可选 tiny/small/medium/all）" }

    $dir = Join-Path $modelsDir $lvl
    New-Item -ItemType Directory -Force $dir | Out-Null

    # det / rec / cls / keys 四项
    foreach ($prop in $lvlNode.Value.PSObject.Properties) {
        $item = $prop.Value
        $url = "$base/$($item.path)"
        Download-One $url (Join-Path $dir $item.file) $item.sha256
    }

    # 生成 meta.json（PP-OCRv6 默认参数）
    # 注意：input_size / rec_img_h / rec_img_w 以 ONNX 实际输入形状为准，见 inspect_models.py
    $meta = [ordered]@{
        model_version = 'PP-OCRv6'
        det = [ordered]@{ input_size = 960; mean = @(0.485, 0.456, 0.406); std = @(0.229, 0.224, 0.225) }
        rec = [ordered]@{ img_h = 48; img_w = 320 }
    } | ConvertTo-Json -Depth 5
    Set-Content -Path (Join-Path $dir 'meta.json') -Value $meta -Encoding UTF8

    Write-Host ""
    Write-Host "=== $lvl 档完成 ==="
    Get-ChildItem $dir | Select-Object Name, Length | Format-Table -AutoSize
}

Write-Host "模型下载完成。"
