# 发版：一条命令完成「出包 → 打 tag → 上传 → 生成 latest.json」。
#
# 为什么要有这个脚本：手工发版要依次改版本号、编译、打包、算 sha256、
# 写 latest.json、建 Release、传两个文件。最容易漏的两处是：
#   1. 改了 tauri.conf.json 却没同步 zip 文件名
#   2. latest.json 里的 sha256 还是上一个包的
# 这两样错了都**不会立刻报错**，只会让用户的自动更新静默失效，
# 或者装上一个校验不过的坏包 —— 事后再查极难定位。所以一律脚本化。
#
# 用法：
#   powershell -File scripts/release.ps1 -Notes "修了找图偏移"
#   powershell -File scripts/release.ps1 -SkipBuild -Draft        # 只重传，不重编
#
# 前置：gh 已登录（gh auth status 能看到账号），工作区已提交。

param(
  # 更新说明。会原样进 latest.json 的 notes，客户端「有新版本」下面显示的就是它。
  [string]$Notes = "",
  [string]$NotesFile = "",
  # 跳过编译与打包，直接用现有的 release/*.zip 上传（重发/补传时用）
  [switch]$SkipBuild,
  # 发成草稿：不占 Release 的 latest 指针，等确认无误再手动 Publish
  [switch]$Draft
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

function Fail([string]$msg) {
  Write-Host "[FAIL] $msg" -ForegroundColor Red
  exit 1
}
function Info([string]$msg) { Write-Host "[INFO] $msg" -ForegroundColor Cyan }
function Ok([string]$msg) { Write-Host "[OK]   $msg" -ForegroundColor Green }

# git 探测（与 build.ps1 探测 node/python 同一套理由）：
# 不保证每个 shell 的 PATH 里都有 git —— 精简环境、某些终端、以及
# 用 .NET/任务计划拉起的会话都可能没有。写死 `git` 会在第一步就
# CommandNotFoundException，而此时脚本还什么都没做，看着像脚本坏了。
# 用绝对路径调用，后续一律走 $gitExe。
$gitExe = $null
$gitCmd = Get-Command git -ErrorAction SilentlyContinue
if ($gitCmd) { $gitExe = $gitCmd.Source }
if (-not $gitExe) {
  $gitCands = @(
    "D:\Git\cmd\git.exe",
    "C:\Program Files\Git\cmd\git.exe",
    "C:\Program Files (x86)\Git\cmd\git.exe"
  )
  foreach ($c in $gitCands) {
    if (Test-Path $c) { $gitExe = $c; break }
  }
}
if (-not $gitExe) { Fail "找不到 git。装一个，或把它加进 PATH" }
Info "git: $gitExe"

# 跑一个原生 exe 并返回 @{ Code; Out }（Out 里同时含 stdout 与 stderr）。
#
# **必须包这一层**，两个原因：
# 1) git / gh 都把进度和提示写到 **stderr**（"Everything up-to-date"、"To https://..."、
#    "branch 'master' set up to track ..."、上传百分比全是 stderr），而
#    `$ErrorActionPreference = "Stop"` 会把原生命令的 stderr 当成终止性错误 ——
#    结果是「其实成功了，脚本却报错退出」。这类假失败最费时间。
# 2) 直接用 `& exe ...` 取输出，在某些会话里会拿到**空字符串**，退出码也会是空
#    （原生 exe 的 stdout/退出码重定向不稳定）。空输出会被下游误判成
#    「没有 origin 远程」这种完全错误的事实；空退出码会让 `if ($code -ne 0)`
#    在成功时也走失败分支。所以改走 .NET 的 Process 显式重定向两条流。
function Invoke-Native([string]$exePath, [string[]]$cmdArgs) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exePath
  # 参数里可能有空格（路径、提交信息），手工加引号，不用数组拼接
  $psi.Arguments = (($cmdArgs | ForEach-Object {
    if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
  }) -join ' ')
  $psi.WorkingDirectory = $root
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
  $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8
  $psi.CreateNoWindow = $true
  $p = [System.Diagnostics.Process]::Start($psi)
  $out = $p.StandardOutput.ReadToEnd()
  $err = $p.StandardError.ReadToEnd()
  $p.WaitForExit()
  return @{ Code = $p.ExitCode; Out = (($out + "`n" + $err).Trim()) }
}

function Invoke-Git([string[]]$gitArgs) { Invoke-Native $gitExe $gitArgs }

# gh 同理。**只用于参数简单的子命令**（view / upload）：release create 的
# --notes 里带换行，拼成命令行字符串会被拆开，那条仍用数组展开直接调。
function Invoke-Gh([string[]]$ghArgs) { Invoke-Native $gh $ghArgs }

# ── 1. 版本号：唯一来源 tauri.conf.json ────────────────────────────────
# 必须与 Rust 侧 build.rs 注入的 OCR_STUDIO_VERSION 同源，否则会出现
# 「程序里显示 0.1.0，Release 却叫 v0.2.0」——用户报版本号时两边对不上。
$confPath = Join-Path $root "src-tauri\tauri.conf.json"
$confRaw = [System.IO.File]::ReadAllText($confPath, [System.Text.UTF8Encoding]::new($false))
if ($confRaw -notmatch '"version"\s*:\s*"([^"]+)"') { Fail "tauri.conf.json 里找不到顶层 version" }
$ver = $Matches[1]
$tag = "v$ver"
Info "版本 $ver（tag $tag）"

# ── 2. 仓库与 gh ───────────────────────────────────────────────────
$gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
if (-not $gh -and $env:OCR_GH) { $gh = $env:OCR_GH }
if (-not $gh) { Fail "找不到 gh 命令。装一个，或设环境变量 OCR_GH 指向 gh.exe" }

$remoteRes = Invoke-Git @("remote", "get-url", "origin")
if ($remoteRes.Code -ne 0 -or -not $remoteRes.Out) { Fail "没有 origin 远程，先 git remote add origin <url>" }
$remote = $remoteRes.Out
# https://github.com/OWNER/REPO.git 或 git@github.com:OWNER/REPO.git
if ($remote -match 'github\.com[:/]+([^/]+)/(.+?)(?:\.git)?$') {
  $owner = $Matches[1]
  $repo = $Matches[2]
} else {
  Fail "无法从 origin 解析 GitHub 仓库：$remote"
}
$slug = "$owner/$repo"
Info "仓库 $slug"

# ── 3. 工作区必须干净 ────────────────────────────────────────────────
# 带着未提交改动发版，等于发布一个「源码里查不到对应版本」的二进制。
# 这是发版事故里最常见、也最难解释的一类。
$dirty = (Invoke-Git @("status", "--porcelain")).Out
if ($dirty) {
  Write-Host $dirty
  Fail "工作区有未提交改动。先提交（或 stash）再发版"
}

# ── 4. tag 不能已经存在 ──────────────────────────────────────────────
$exists = (Invoke-Git @("tag", "--list", $tag)).Out
if ($exists) { Fail "$tag 已存在。发新版请先改 tauri.conf.json 的 version" }

# ── 5. 编译 + 打包 ──────────────────────────────────────────────────
if (-not $SkipBuild) {
  Info "编译（scripts/build.ps1）..."
  & (Join-Path $PSScriptRoot "build.ps1")
  if ($LASTEXITCODE -ne 0) { Fail "编译失败" }

  Info "打包（scripts/pack_exe.ps1）..."
  & (Join-Path $PSScriptRoot "pack_exe.ps1")
  if ($LASTEXITCODE -ne 0) { Fail "打包失败" }
}

$zip = Join-Path $root "release\OCR-Studio-$ver-win64.zip"
if (-not (Test-Path $zip)) { Fail "产物不存在：$zip" }
$size = (Get-Item $zip).Length
$hash = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLower()
Ok ("产物 {0:N1} MB  sha256={1}..." -f ($size / 1MB), $hash.Substring(0, 16))

# ── 6. 生成 latest.json ─────────────────────────────────────────────
# URL 指向**本版本**的 Release 资产，而不是 latest/download ——
# latest.json 自己由 latest/download 指过来，里面的 url 再指回去会绕圈，
# 而且用户点「更新」时应该拿到的就是他看到的那个版本。
$assetUrl = "https://github.com/$slug/releases/download/$tag/OCR-Studio-$ver-win64.zip"

if ($NotesFile) {
  if (-not (Test-Path $NotesFile)) { Fail "说明文件不存在：$NotesFile" }
  $Notes = [System.IO.File]::ReadAllText($NotesFile, [System.Text.UTF8Encoding]::new($false))
}
if (-not $Notes) {
  # 没有手写说明时退化成「看提交记录」，总比空白强
  $Notes = (Invoke-Git @("log", "-1", "--pretty=format:%s")).Out
}

$latest = [ordered]@{
  version  = $ver
  notes    = $Notes
  pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  url      = $assetUrl
  size     = $size
  sha256   = $hash
}
$latestPath = Join-Path $root "release\latest.json"
$jsonText = ($latest | ConvertTo-Json -Depth 5)
# **无 BOM**：serde_json 遇到 BOM 会直接解析失败（"expected value at line 1 column 1"），
# 而客户端每次启动都会读这个文件 —— 带 BOM 等于让所有人的更新检查永久报错。
[System.IO.File]::WriteAllText($latestPath, $jsonText, [System.Text.UTF8Encoding]::new($false))
Ok "已生成 release/latest.json"

# ── 7. 推送代码 + 建 Release ─────────────────────────────────────────
Info "推送 master..."
$pushRes = Invoke-Git @("push", "origin", "HEAD")
if ($pushRes.Code -ne 0) { Fail "推送失败：$($pushRes.Out)" }
if ($pushRes.Out -notmatch "Everything up-to-date") { Write-Host $pushRes.Out }

$ghArgs = @(
  "release", "create", $tag,
  $zip, $latestPath,
  "--repo", $slug,
  "--title", $tag,
  "--notes", $Notes
)
if ($Draft) { $ghArgs += "--draft" }

Info "创建 Release..."
# gh 也是把进度写到 stderr 的（上传时会刷百分比），同样不能让它触发 Stop。
$prevEA = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $gh @ghArgs
$ghCode = $LASTEXITCODE
$ErrorActionPreference = $prevEA
# **不用退出码判成败**：这个变量在本机会读成空值，而且 gh 上传大文件中断时
# 会留下一个「只传了 latest.json、没有 zip」的空壳 Release —— 退出码不管读成
# 什么，都不如直接问 GitHub「资产到底全不全」来得准。
# （2026-10-25 真实发生：50MB 的 zip 上传中断，Release 建成了但缺 zip，
#   而脚本因为退出码读空，报的是一条看不出根因的错。）
Info "gh 退出码: $ghCode（不作为判据，下面核对资产）"

# 核对资产：Release 上必须同时有 zip 和 latest.json
$zipLeaf = Split-Path -Leaf $zip
function Get-ReleaseAssetNames {
  $v = Invoke-Gh @("release", "view", $tag, "--repo", $slug, "--json", "assets,isDraft")
  if ($v.Code -ne 0) { return $null }
  try { return @(($v.Out | ConvertFrom-Json).assets | ForEach-Object { $_.name }) } catch { return $null }
}

$names = Get-ReleaseAssetNames
if ($null -eq $names) {
  Fail "读不到 Release 资产列表，无法确认是否上传完整。请手动检查 https://github.com/$slug/releases/tag/$tag"
}
$missing = @($zipLeaf, "latest.json") | Where-Object { $names -notcontains $_ }
if ($missing.Count -gt 0) {
  Write-Host "[WARN] Release 缺资产: $($missing -join ', ') —— 多半是上传中断" -ForegroundColor Yellow
  Info "补传缺少的资产..."
  $retryArgs = @("release", "upload", $tag)
  if ($missing -contains $zipLeaf) { $retryArgs += $zip }
  if ($missing -contains "latest.json") { $retryArgs += $latestPath }
  $retryArgs += @("--clobber", "--repo", $slug)
  $r = Invoke-Gh $retryArgs
  if ($r.Out) { Write-Host $r.Out }
  $names = Get-ReleaseAssetNames
  $missing = @($zipLeaf, "latest.json") | Where-Object { $names -notcontains $_ }
  if ($missing.Count -gt 0) { Fail "补传后仍然缺: $($missing -join ', ')。到 Releases 页面手动上传" }
  Ok "补传完成"
}
Ok "资产齐全：$($names -join ', ')"

Write-Host ""
Ok "发布完成：https://github.com/$slug/releases/tag/$tag"
if (-not $Draft) {
  Info "客户端更新源：https://github.com/$slug/releases/latest/download/latest.json"
  Write-Host ""
  Write-Host "  下一步自检（强烈建议）：" -ForegroundColor Yellow
  Write-Host "    1. 浏览器打开上面那个 latest.json，确认 version / size / sha256 是这次的值"
  Write-Host "    2. 运行「上一个版本」，点设置页的「检查更新」，看能不能一路自动换到 $ver"
} else {
  Info "这是草稿：确认没问题后到 Releases 页面点 Publish，latest 指针才会切过去"
}
