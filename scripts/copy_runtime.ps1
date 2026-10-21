# 拷贝 ONNX Runtime 运行期 DLL 到 src-tauri/runtime/，并设置 ort 的加载路径。
#
# 背景：ort crate 默认会在构建期从 GitHub 下载 onnxruntime，慢且版本不可控。
# 本机 Python 环境里已有一份 onnxruntime-directml 1.24.3（含 DirectML.dll），
# 直接复用，配合 ORT_STRATEGY=system + ORT_LIB_LOCATION 使用。
#
# 用法：
#   .\copy_runtime.ps1                 # 拷贝 DLL 并打印构建所需环境变量
#   .\copy_runtime.ps1 -ForBuild       # 拷贝并输出 $env: 设置语句

param(
    [string]$Source = "C:\Users\13114\AppData\Local\Programs\Python\Python312\Lib\site-packages\onnxruntime\capi",
    [string]$Root = (Split-Path -Parent $PSScriptRoot),
    [switch]$ForBuild
)

$ErrorActionPreference = 'Stop'
$destDir = Join-Path $Root 'src-tauri\runtime'
New-Item -ItemType Directory -Force $destDir | Out-Null

$files = @('onnxruntime.dll', 'DirectML.dll', 'onnxruntime_providers_shared.dll')
foreach ($f in $files) {
    $src = Join-Path $Source $f
    if (-not (Test-Path $src)) {
        Write-Warning "缺失: $src"
        continue
    }
    Copy-Item $src (Join-Path $destDir $f) -Force
    Write-Host "已拷贝 $f"
}

Get-ChildItem $destDir | Select-Object Name, Length | Format-Table -AutoSize

if ($ForBuild) {
    # ort 需要知道 DLL 位置；ORT_STRATEGY=system 关闭构建期下载
    Write-Host ""
    Write-Host "构建前请设置："
    Write-Host "  `$env:ORT_STRATEGY = 'system'"
    Write-Host "  `$env:ORT_LIB_LOCATION = '$((Join-Path $destDir 'onnxruntime.dll'))'"
}
