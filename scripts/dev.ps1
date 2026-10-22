<#
开发启动（唯一正确入口）

为什么需要这个脚本：
  本项目的 Rust 工具链是**项目内私有安装**（.rust\cargo），没有写进系统 PATH 的可靠位置。
  直接 `npm run tauri dev` 时，Tauri CLI 会用 `cargo metadata` 找 workspace，
  一旦当前终端的环境变量没加载，就会报：

      failed to run 'cargo metadata' ... program not found

  窗口根本不会出现，看起来像「界面起不来」。这不是代码问题，是环境问题。
  本脚本把工具链环境一次性铺好，再交给 Tauri。

用法：
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\dev.ps1
  可选：-NoWatch  只编译运行一次（不给 Tauri 的文件监听）
#>
param(
    [switch]$NoWatch
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$rustBin = Join-Path $root ".rust\cargo\bin"
$cargoExe = Join-Path $rustBin "cargo.exe"

if (-not (Test-Path $cargoExe)) {
    Write-Host "[FAIL] 找不到项目内 Rust 工具链: $cargoExe" -ForegroundColor Red
    Write-Host "       项目依赖私有工具链，请不要用系统 cargo；确认 .rust 目录没被删。"
    exit 1
}

# 1) 工具链目录
$env:RUSTUP_HOME = Join-Path $root ".rust\rustup"
$env:CARGO_HOME = Join-Path $root ".rust\cargo"
# 2) 构建产物放在 D 盘，避免污染项目目录（这个项目在机械盘上，target 会很占空间）
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = "D:\rust-target" }
$env:TMP = "D:\tmp"
$env:TEMP = "D:\tmp"
if (-not (Test-Path $env:TMP)) { New-Item -ItemType Directory -Path $env:TMP -Force | Out-Null }

# 3) 关键：把 cargo 放到 PATH 最前面，Tauri CLI 才会找到它
$env:PATH = "$rustBin;$env:PATH"

$found = (Get-Command cargo -ErrorAction SilentlyContinue)
if (-not $found) {
    Write-Host "[FAIL] cargo 仍不可见，PATH 里可能被其它条目覆盖" -ForegroundColor Red
    exit 1
}
Write-Host "[INFO] cargo: $($found.Source)"
Write-Host "[INFO] CARGO_TARGET_DIR: $env:CARGO_TARGET_DIR"

# 4) 依赖没装就直接装（小白最容易漏的一步）
if (-not (Test-Path (Join-Path $root "node_modules"))) {
    Write-Host "[INFO] 首次运行，安装前端依赖…"
    & npm install
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[FAIL] npm install 失败" -ForegroundColor Red
        exit 1
    }
}

# 5) devUrl 固定 1420：被别的进程占着会直接起不来，先探一下
$busy = Get-NetTCPConnection -LocalPort 1420 -State Listen -ErrorAction SilentlyContinue
if ($busy) {
    Write-Host "[WARN] 1420 已被占用（可能是上一次 dev 没退干净）：" -ForegroundColor Yellow
    $busy | ForEach-Object {
        $p = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue
        if ($p) { Write-Host ("       PID {0} {1}" -f $p.Id, $p.ProcessName) }
    }
    Write-Host "       如界面空白，先关掉占用进程再重试。"
}

$tauriArgs = @("run", "tauri", "dev")
if ($NoWatch) { $tauriArgs += "--no-watch" }

& npm @tauriArgs
exit $LASTEXITCODE
