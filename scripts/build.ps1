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
    [switch]$Run,
    [switch]$SkipFrontend
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

# ---- node 探测 ----
# npm run build 走的是 PATH 里的 npm；agent 会话的 PATH 常常没有 node
# （npm 会静默 exit 0 什么都不干，dist 不生成，最后报「嵌入校验未通过」这种
# 离题万里的错误）。这里显式找一遍，找不到就直接报错，别让它猜。
# 注意：不用 Join-Path 拼候选路径 —— agent 会话里 USERPROFILE/LOCALAPPDATA
# 可能是空的，Join-Path 遇空参数直接抛终止错误。
$nodeExe = $null
$cmd = Get-Command node -ErrorAction SilentlyContinue
if ($cmd) {
    $nodeExe = $cmd.Source
} else {
    $bases = @()
    if ($env:NODE_HOME) { $bases += $env:NODE_HOME.TrimEnd("\") }
    if ($env:LOCALAPPDATA) { $bases += ($env:LOCALAPPDATA.TrimEnd("\") + "\WorkBuddy\binaries\node") }
    if ($env:USERPROFILE) { $bases += ($env:USERPROFILE.TrimEnd("\") + "\.workbuddy\binaries\node") }
    $bases += "C:\Users\13114\.workbuddy\binaries\node"
    $bases += "C:\Program Files\nodejs"
    foreach ($b in $bases) {
        $hit = Get-ChildItem -Path $b -Filter "node.exe" -Recurse -Depth 2 -ErrorAction SilentlyContinue |
               Select-Object -First 1
        if ($hit) { $nodeExe = $hit.FullName; break }
    }
}
if (-not $nodeExe) {
    Write-Host "[FAIL] PATH 里没有 node，常见位置也没找到。装个 Node.js 或把它的目录加进 PATH。" -ForegroundColor Red
    exit 1
}
$nodeDir = Split-Path -Parent $nodeExe
if ($env:PATH -notlike "*$nodeDir*") { $env:PATH = "$nodeDir;$env:PATH" }
Write-Host "[INFO] node: $nodeExe"

# ---- python 探测（同样理由：agent 会话 PATH 里常常没有 python）----
$pyExe = $null
$pyCmd = Get-Command python -ErrorAction SilentlyContinue
if ($pyCmd) {
    $pyExe = $pyCmd.Source
} else {
    $pyBases = @()
    if ($env:LOCALAPPDATA) { $pyBases += ($env:LOCALAPPDATA.TrimEnd("\") + "\Programs\Python") }
    $pyBases += "C:\Users\13114\AppData\Local\Programs\Python"
    foreach ($b in $pyBases) {
        $hit = Get-ChildItem -Path $b -Filter "python.exe" -Recurse -Depth 2 -ErrorAction SilentlyContinue |
               Where-Object { $_.FullName -notlike "*-32\*" } | Select-Object -First 1
        if ($hit) { $pyExe = $hit.FullName; break }
    }
}
if (-not $pyExe) {
    Write-Host "[FAIL] PATH 里没有 python，常见位置也没找到。" -ForegroundColor Red
    exit 1
}
$pyDir = Split-Path -Parent $pyExe
if ($env:PATH -notlike "*$pyDir*") { $env:PATH = "$pyDir;$env:PATH" }
Write-Host "[INFO] python: $pyExe"

function Step($n, $msg) { Write-Host "`n[$n] $msg" -ForegroundColor Cyan }

# ---- 1. 前端 ----
# `-SkipFrontend` 是给 agent 环境留的口子：那边的 PowerShell 工具调 native exe
# 不可靠（stdout 丢失、退出码为空、node.exe 被当成「文档」），前端得在 Git Bash
# 里先跑完。用户自己的终端没这个问题，正常走完整流程。
if ($SkipFrontend) {
    Step 1 "跳过前端构建（-SkipFrontend）"
    if (-not (Test-Path (Join-Path $root "dist\index.html"))) {
        Write-Host "[FAIL] dist 不存在，先构建前端" -ForegroundColor Red
        exit 1
    }
} else {
Step 1 "构建前端（tsc + vite）"

# vite 会先清空 dist（里面上百个字体切片），文件数一多就会被环境的 safe-delete
# 拦下 —— 表现是「Build failed in Xs」但其实是**删不掉**，不是代码问题。
# 先用 .NET API 删干净，vite 就不用再删了（删完记得：它按进程 CWD 解析，用绝对路径）。
$dist = Join-Path $root "dist"
if (Test-Path $dist) { [System.IO.Directory]::Delete($dist, $true) }
# agent 会话里 PowerShell shim 不会创建 $LASTEXITCODE，读它直接抛错；
# 先钉一个 0，后面 native exe 跑完会覆盖它。
Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
if (-not (Test-Path (Join-Path $root "node_modules"))) {
    Write-Host "[INFO] 安装前端依赖…"
    & npm install
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] npm install 失败" -ForegroundColor Red; exit 1 }
}
# vite 会把进度/reporter 信息写到 **stderr**（PowerShell 看到就报 NativeCommandError，
# 配上本脚本的 `$ErrorActionPreference="Stop"` 会直接中止 —— 实际上构建是成功的。
# 所以这一段临时降级成 Continue，成败只看退出码。
#
# 为什么不走 `npm run build`：agent 会话里 npm 是个 PowerShell shim，
# 调用完 `$LASTEXITCODE` 变量根本不存在（读它直接抛「检索不到变量」），
# 而且 PATH 没 node 时它会静默 exit 0、什么都不干。直接用 node 跑两步最稳。
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
& $nodeExe ".\node_modules\typescript\bin\tsc" --noEmit
if ($LASTEXITCODE -ne 0) {
    Write-Host "[FAIL] TypeScript 检查未通过（看上面的报错）" -ForegroundColor Red
    exit 1
}
& $nodeExe ".\node_modules\vite\bin\vite.js" build > $env:TEMP\ocr-vite.log 2>&1
$code = $LASTEXITCODE
Get-Content $env:TEMP\ocr-vite.log -Tail 10 | ForEach-Object { Write-Host $_ }
$ErrorActionPreference = $prevEap
if ($code -ne 0) { Write-Host "[FAIL] 前端构建失败（先看上面的报错）" -ForegroundColor Red; exit 1 }
if (-not (Test-Path (Join-Path $root "dist\index.html"))) {
    Write-Host "[FAIL] dist 没生成，vite 实际没干活" -ForegroundColor Red
    exit 1
}
}

# ---- 2. 让 build.rs 重跑，把新 dist 嵌进 exe ----
Step 2 "触发重新嵌入前端资源"
(Get-Item (Join-Path $root "src-tauri\build.rs")).LastWriteTime = Get-Date

# ---- 3. 接口清单一致性（前后端一本账 + 组不漏）----
# 放在 Rust 构建之前：纯文本检查，几十毫秒，漏端点/漏组在这里就拦下，
# 不要等构建完跑起来才发现界面上少了一组接口。
#
# 每个 native exe 调用前都重置 $LASTEXITCODE：agent 的 PowerShell 环境
# 跑完 native exe 后这个变量是**空值**（不更新也不创建），直接读会误判失败。
# 重置后「没更新」= 0 = 通过；用户环境里 native exe 会正常写入真实退出码。
Step 3 "接口清单一致性"
$epChecker = Join-Path $root "scripts\check_endpoints.py"
if (Test-Path $epChecker) {
    Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
    & $pyExe $epChecker
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] 接口清单与后端路由不一致（见上），终止构建" -ForegroundColor Red
        exit 1
    }
}

# ---- 4. release 构建 ----
Step 4 "cargo build --release（LTO，通常 3~5 分钟）"
$exeBefore = $null
$exePath = Join-Path $env:CARGO_TARGET_DIR "release\ocr-studio.exe"
if (Test-Path $exePath) { $exeBefore = (Get-Item $exePath).LastWriteTime }
Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
& $cargoExe build --release --manifest-path (Join-Path $root "src-tauri\Cargo.toml")
if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] Rust 构建失败" -ForegroundColor Red; exit 1 }

# 产物双保险：退出码可能因为环境问题不可信，文件不存在/没更新就是没建成
if (-not (Test-Path $exePath)) {
    Write-Host "[FAIL] 没找到产物: $exePath" -ForegroundColor Red
    exit 1
}
if ($null -ne $exeBefore -and (Get-Item $exePath).LastWriteTime -le $exeBefore) {
    Write-Host "[FAIL] 产物没被重新编译（时间戳没变），Rust 实际没构建" -ForegroundColor Red
    exit 1
}
Write-Host "[PASS] 产物: $exePath" -ForegroundColor Green

# ---- 5. 校验新前端确实嵌进去了 ----
Step 5 "校验嵌入资源"
$checker = Join-Path $root "scripts\check_embedded_assets.py"
if (Test-Path $checker) {
    $assets = Get-ChildItem (Join-Path $root "dist\assets") -Filter "*.js" | Select-Object -ExpandProperty Name
    Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
    & $pyExe $checker $exePath @assets
    if ($LASTEXITCODE -ne 0) { Write-Host "[WARN] 嵌入校验未通过（见上）" -ForegroundColor Yellow }
}

# ---- 6. 可选打包 ----
if ($Pack) {
    Step 6 "打包免安装 zip"
    Get-Process | Where-Object { $_.ProcessName -like "*OCR*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    Set-Variable -Name LASTEXITCODE -Value 0 -Scope Global -Force
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "scripts\pack_exe.ps1")
    if ($LASTEXITCODE -ne 0) { Write-Host "[FAIL] 打包失败" -ForegroundColor Red; exit 1 }
}

# ---- 7. 可选启动 ----
if ($Run) {
    $target = if ($Pack) { Join-Path $root "release\OCR-Studio\OCR Studio.exe" } else { $exePath }
    # 必须用资源管理器拉起：agent/终端直接跑 exe 时 WebView2 的 GPU 子进程会崩，
    # 表现就是「白窗口 10 秒」或「界面起不来」。
    Write-Host "`n[INFO] 启动: $target" -ForegroundColor Cyan
    & explorer.exe $target
}

Write-Host "`n[PASS] 完成" -ForegroundColor Green
