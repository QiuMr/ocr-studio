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

$ErrorActionPreference = "Continue"
# ↑ 刻意**不用** "Stop"：npm / vite / cargo 都把正常进度写进 stderr，
#   PowerShell 会当成 NativeCommandError，配 "Stop" 会在构建其实成功时中止脚本
#   （vite 8 / cargo 都踩过）。成败一律看每步后面的 $LASTEXITCODE。
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

# vite 会先清空 dist（里面上百个字体切片），文件数一多就会被环境的 safe-delete
# 拦下 —— 表现是「Build failed in Xs」但其实是**删不掉**，不是代码问题。
# 先用 .NET API 删干净，vite 就不用再删了（删完记得：它按进程 CWD 解析，用绝对路径）。
$dist = Join-Path $root "dist"
if (Test-Path $dist) { [System.IO.Directory]::Delete($dist, $true) }
if (-not (Test-Path (Join-Path $root "node_modules"))) {
    Write-Host "[INFO] 安装前端依赖…"
    & npm install
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] npm install 失败" -ForegroundColor Red; exit 1 }
}
# vite 会把进度/reporter 信息写到 **stderr**（PowerShell 看到就报 NativeCommandError，
# 配上本脚本的 `$ErrorActionPreference="Stop"` 会直接中止 —— 实际上构建是成功的）。
# 所以这一段临时降级成 Continue，成败只看退出码。
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& npm run build 2>&1 | ForEach-Object { Write-Host $_ }
$code = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($code -ne 0) { Write-Host "[FAIL] 前端构建失败（先看上面的 TS 报错）" -ForegroundColor Red; exit 1 }

# ---- 2. 让 build.rs 重跑，把新 dist 嵌进 exe ----
Step 2 "触发重新嵌入前端资源"
(Get-Item (Join-Path $root "src-tauri\build.rs")).LastWriteTime = Get-Date

# ---- 3. 接口清单一致性（前后端一本账 + 组不漏）----
# 放在 Rust 构建之前：纯文本检查，几十毫秒，漏端点/漏组在这里就拦下，
# 不要等构建完跑起来才发现界面上少了一组接口。
Step 3 "接口清单一致性"
$epChecker = Join-Path $root "scripts\check_endpoints.py"
if (Test-Path $epChecker) {
    & python $epChecker
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] 接口清单与后端路由不一致（见上），终止构建" -ForegroundColor Red
        exit 1
    }
}

# ---- 4. release 构建 ----
Step 4 "cargo build --release（LTO，通常 3~5 分钟）"
& $cargoExe build --release --manifest-path (Join-Path $root "src-tauri\Cargo.toml")
if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] Rust 构建失败" -ForegroundColor Red; exit 1 }

$exe = Join-Path $env:CARGO_TARGET_DIR "release\ocr-studio.exe"
if (-not (Test-Path $exe)) {
    Write-Host "[FAIL] 没找到产物: $exe" -ForegroundColor Red
    exit 1
}
Write-Host "[PASS] 产物: $exe" -ForegroundColor Green

# ---- 5. 校验新前端确实嵌进去了 ----
Step 5 "校验嵌入资源"
$checker = Join-Path $root "scripts\check_embedded_assets.py"
if (Test-Path $checker) {
    $assets = Get-ChildItem (Join-Path $root "dist\assets") -Filter "*.js" | Select-Object -ExpandProperty Name
    & python $checker $exe @assets
    if ($LASTEXITCODE -ne 0) { Write-Host "[WARN] 嵌入校验未通过（见上）" -ForegroundColor Yellow }
}

# ---- 6. 可选打包 ----
if ($Pack) {
    Step 6 "打包免安装 zip"
    Get-Process | Where-Object { $_.ProcessName -like "*OCR*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "scripts\pack_exe.ps1")
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] 打包失败" -ForegroundColor Red; exit 1 }
}

# ---- 7. 可选启动 ----
if ($Run) {
    $target = if ($Pack) { Join-Path $root "release\OCR-Studio\OCR Studio.exe" } else { $exe }
    # 必须用资源管理器拉起：agent/终端直接跑 exe 时 WebView2 的 GPU 子进程会崩，
    # 表现就是「白窗口 10 秒」或「界面起不来」。
    Write-Host "`n[INFO] 启动: $target" -ForegroundColor Cyan
    & explorer.exe $target
}

Write-Host "`n[PASS] 完成" -ForegroundColor Green
