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

# 调 git 并返回 @{ Code; Out }。
#
# **必须包这一层**：git 把进度和提示都写到 **stderr**（"Everything up-to-date"、
# "To https://..."、"branch 'master' set up to track ..." 全是 stderr），
# 而 `$ErrorActionPreference = "Stop"` 会把原生命令的 stderr 当成终止性错误。
# 结果是「推送其实成功了，脚本却报错退出」——这类假失败最费时间：
# 输出看着像出错，实际一步都没做错。
function Invoke-Git([string[]]$gitArgs) {
  $prev = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $out = & git -C $root @gitArgs 2>&1
    return @{ Code = $LASTEXITCODE; Out = (($out | Out-String).Trim()) }
  } finally {
    $ErrorActionPreference = $prev
  }
}

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
# gh 也是把进度写到 stderr 的（上传 225MB 时会刷百分比），
# 同样不能让它触发 Stop。
$prevEA = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $gh @ghArgs
$ghCode = $LASTEXITCODE
$ErrorActionPreference = $prevEA
if ($ghCode -ne 0) { Fail "创建 Release 失败（gh 退出码 $ghCode）" }

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
