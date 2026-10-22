<#
编译启动（出可运行的 exe）—— 唯一正确入口

为什么需要脚本：
  这个项目的 exe 用 `generate_context!` 在**编译期**把 dist 嵌进二进制。
  只跑 `npm run build` 的话，exe 里还是旧界面，双击看到的是上一版 UI，
  很容易误判成「前端没生效 / 缓存坏了」。
  必须先构建前端，再让 build.rs 重跑，最后 cargo build --release。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\build.ps1
  powershell ... -File scripts\build.ps1 -Pack     # 顺便出免安装 zip
  powershell ... -File scripts\build.ps1 -Run      # 构建完直接启动（用资源管理器拉起）
#>
param(
    [switch]$Pack,
    [switch]$Run
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$rustBin = Join-Path $root ".rust\cargo\bin"
$cargoExe = Join-Path $rustBin "cargo.exe"
if (-not (Test-Path $cargoExe)) {
    Write-Host "[FAIL] 找不到项目内 Rust 工具链: $cargoExe" -ForegroundColor Red
    exit 1
}

$env:RUSTUP_HOME = Join-Path $root ".rust\rustup"
$env:CARGO_HOME = Join-Path $root ".rust\cargo"
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = "D:\rust-target" }
$env:TMP = "D:\tmp"
$env:TEMP = "D:\tmp"
if (-not (Test-Path $env:TMP)) { New-Item -ItemType Directory -Path $env:TMP -Force | Out-Null }
$env:PATH = "$rustBin;$env:PATH"

function Step($n, $msg) { Write-Host "`n[$n] $msg" -ForegroundColor Cyan }

# ---- 1. 前端 ----
Step 1 "构建前端（tsc + vite）"
if (-not (Test-Path (Join-Path $root "node_modules"))) {
    Write-Host "[INFO] 安装前端依赖…"
    & npm install
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] npm install 失败" -ForegroundColor Red; exit 1 }
}
& npm run build
if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] 前端构建失败（先看上面的 TS 报错）" -ForegroundColor Red; exit 1 }

# ---- 2. 让 build.rs 重跑，把新 dist 嵌进 exe ----
Step 2 "触发重新嵌入前端资源"
(Get-Item (Join-Path $root "src-tauri\build.rs")).LastWriteTime = Get-Date

# ---- 3. release 构建 ----
Step 3 "cargo build --release（LTO，通常 3~5 分钟）"
& $cargoExe build --release --manifest-path (Join-Path $root "src-tauri\Cargo.toml")
if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] Rust 构建失败" -ForegroundColor Red; exit 1 }

$exe = Join-Path $env:CARGO_TARGET_DIR "release\ocr-studio.exe"
if (-not (Test-Path $exe)) {
    Write-Host "[FAIL] 没找到产物: $exe" -ForegroundColor Red
    exit 1
}
Write-Host "[PASS] 产物: $exe" -ForegroundColor Green

# ---- 4. 校验新前端确实嵌进去了 ----
Step 4 "校验嵌入资源"
$checker = Join-Path $root "scripts\check_embedded_assets.py"
if (Test-Path $checker) {
    $assets = Get-ChildItem (Join-Path $root "dist\assets") -Filter "*.js" | Select-Object -ExpandProperty Name
    & python $checker $exe @assets
    if ($LASTEXITCODE -ne 0) { Write-Host "[WARN] 嵌入校验未通过（见上）" -ForegroundColor Yellow }
}

# ---- 5. 可选打包 ----
if ($Pack) {
    Step 5 "打包免安装 zip"
    Get-Process | Where-Object { $_.ProcessName -like "*OCR*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "scripts\pack_exe.ps1")
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] 打包失败" -ForegroundColor Red; exit 1 }
}

# ---- 6. 可选启动 ----
if ($Run) {
    $target = if ($Pack) { Join-Path $root "release\OCR-Studio\OCR Studio.exe" } else { $exe }
    # 必须用资源管理器拉起：agent/终端直接跑 exe 时 WebView2 的 GPU 子进程会崩，
    # 表现就是「白窗口 10 秒」或「界面起不来」。
    Write-Host "`n[INFO] 启动: $target" -ForegroundColor Cyan
    & explorer.exe $target
}

Write-Host "`n[PASS] 完成" -ForegroundColor Green
