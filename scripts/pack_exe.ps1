# 免安装分发包打包脚本（不打 MSI/NSIS，直接给 exe + dll + models）
#
# 为什么需要这个脚本：`tauri.conf.json` 已设 `bundle.active = false`（用户不要安装包），
# 但 Tauri 的 `bundle.resources` 只在**打安装包时**才把 runtime/*.dll 拷到 exe 旁边。
# 单拿 exe 出去会因为缺 onnxruntime.dll 直接起不来（或静默退回 CPU）。
#
# 用法：
#   1. pnpm build
#   2. cargo build --release   （或 pnpm tauri build，但不要带 bundle）
#   3. powershell -File scripts/pack_exe.ps1
# 产物：release/OCR-Studio-<版本>-win64.zip

param(
  [string]$OutRoot = "release",
  [switch]$SkipModels
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$version = "0.1.0"

# ---- 1. 找 release exe -------------------------------------------------------
# 注意本机把 CARGO_TARGET_DIR 指到了 D:\rust-target，不能只认 src-tauri\target
$candidates = @(
  (Join-Path $root "src-tauri\target\release\ocr-studio.exe"),
  "D:\rust-target\release\ocr-studio.exe"
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $exe) {
  throw "找不到 release exe。请先 cargo build --release"
}
Write-Host "[INFO] 使用 exe: $exe"

# ---- 2. 准备目录 -------------------------------------------------------------
$name = "OCR-Studio"
$stage = Join-Path $root (Join-Path $OutRoot $name)
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

Copy-Item $exe (Join-Path $stage "OCR Studio.exe") -Force

# 运行时 DLL：DirectML 靠这两个。少一个就会静默退回 CPU
$runtime = Join-Path $root "src-tauri\runtime"
Get-ChildItem $runtime -Filter *.dll | ForEach-Object {
  Copy-Item $_.FullName (Join-Path $stage $_.Name) -Force
  Write-Host ("[INFO] + {0} ({1:N1} MB)" -f $_.Name, ($_.Length / 1MB))
}

# 模型：不打包的话用户首次启动会看到「引擎未加载」
if (-not $SkipModels) {
  $models = Join-Path $root "models"
  if (Test-Path $models) {
    $dstModels = Join-Path $stage "models"
    Copy-Item $models $dstModels -Recurse -Force
    # 训练/对齐用的中间产物不进分发包：
    #   *.pt            原始权重（6~130MB，运行时只需要 onnx）
    #   ref_*.json      跟 ultralytics 逐框比对用的参考框（只有开发时需要）
    #   *.bak           手工改文件留下的备份
    $junk = Get-ChildItem $dstModels -Recurse -File | Where-Object {
      $_.Extension -in @('.pt', '.bak') -or $_.Name -like 'ref_*.json'
    }
    foreach ($f in $junk) {
      Remove-Item $f.FullName -Force
      Write-Host ("[INFO] - {0} ({1:N1} MB)" -f $f.Name, ($f.Length / 1MB))
    }
    Write-Host "[INFO] + models/"
  } else {
    Write-Warning "models 目录不存在，分发包将缺少模型"
  }
}

# ---- 3. 说明文件 -------------------------------------------------------------
$readme = @'
OCR Studio —— 免安装版使用说明
================================

怎么用
  双击「OCR Studio.exe」。不需要安装，也不会写注册表。
  首次启动要在本机建缓存，会比之后慢几秒。

目录里都是什么（缺一个都跑不起来，别单独拷 exe）
  OCR Studio.exe                 主程序
  onnxruntime.dll                推理框架
  DirectML.dll                   显卡加速后端（没有它就只能用 CPU）
  onnxruntime_providers_shared.dll
  models\                        模型文件。删了会提示「引擎未加载」

显卡加速没生效怎么办
  进入「仪表盘」，看「推理设备」是不是显示 GPU。
  显示 CPU 说明 DirectML 没加载起来（常见于缺 dll、或显卡驱动过旧），
  此时识别仍可用，只是慢一些。

防火墙 / 端口
  程序只监听本机 127.0.0.1:8848，不对外网开放。
  端口被占用时，去「设置」里改成 8850 之类再保存。

日志与排错
  界面左侧「运行日志」页能实时看日志。
  磁盘日志在 %APPDATA%\com.ocrstudio.app\ocr-studio.log

卸载
  直接删掉整个文件夹即可。

系统要求
  Windows 10 1809 及以上（需要 WebView2 运行时，Win11 自带；
  Win10 若提示缺 WebView2，装一次 Microsoft Edge WebView2 Runtime 即可）
'@
$readmePath = Join-Path $stage "使用说明.txt"
# 记事本按 ANSI 读无 BOM 文件会乱码，这里强制带 BOM
[System.IO.File]::WriteAllText($readmePath, $readme, (New-Object System.Text.UTF8Encoding($true)))

# ---- 4. 压缩 ----------------------------------------------------------------
$zip = Join-Path $root (Join-Path $OutRoot ("OCR-Studio-{0}-win64.zip" -f $version))
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
$size = (Get-Item $zip).Length / 1MB
Write-Host ("[PASS] 已生成 {0} ({1:N1} MB)" -f $zip, $size)
Write-Host "[INFO] 分发时把这个 zip 给别人，解压即用"
