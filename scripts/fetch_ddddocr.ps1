# 下载 DDDDOCR 的识别模型与字典到 models/ocr/ddddocr/<variant>/
#
# 为什么单独一个脚本：ddddocr 的模型不在 RapidOCR 的发布仓库里，而是随 PyPI 的
# sdist 一起发的。所以这里走 `pip download --no-binary :all:` 取源码包再解包，
# 附带的好处是字典（charsets.py）与模型**同源同版本**，不会出现「模型换了字典没换」
# 那种整行文字错位一位的隐性 bug。
#
# 用法：
#   .\fetch_ddddocr.ps1                # 只装 common（54MB，效果更好）
#   .\fetch_ddddocr.ps1 -Variant old   # 只装 old（13.6MB，更小更快）
#   .\fetch_ddddocr.ps1 -Variant all   # 两版都装
#
# 产物：
#   common/rec.onnx     <- common.onnx      配 CHARSET_BETA
#   common/charset.txt  <- CHARSET_BETA（8210 项，第 0 项是 blank 空串）
#   old/rec.onnx        <- common_old.onnx  配 CHARSET_OLD
#   old/charset.txt     <- CHARSET_OLD

param(
    [string]$Variant = 'common',
    [string]$Version = '1.6.1',
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

$ErrorActionPreference = 'Stop'
$modelsDir = Join-Path $Root 'models\ocr\ddddocr'
$tmp = Join-Path $env:TEMP ("ddddocr_sdist_" + [System.Guid]::NewGuid().ToString('N').Substring(0, 8))

if ($Variant -eq 'all') { $variants = @('common', 'old') }
else { $variants = @($Variant) }

foreach ($v in $variants) {
    if ($v -notin @('common', 'old')) { throw "未知版本: $v（可选 common / old / all）" }
}

New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    Write-Host "下载 ddddocr $Version 源码包..."
    python -m pip download "ddddocr==$Version" --no-deps --no-binary :all: -d $tmp --quiet
    $tar = Get-ChildItem $tmp -Filter 'ddddocr-*.tar.gz' | Select-Object -First 1
    if (-not $tar) { throw "没有下载到 sdist，检查 pip 是否可用 / 网络是否通" }

    # 本机没有 7-Zip，但 Windows 自带的 bsdtar 能直接解 .tar.gz
    Push-Location $tmp
    & "$env:SystemRoot\System32\tar.exe" -xf $tar.Name
    Pop-Location

    $src = Join-Path $tmp ("ddddocr-" + $Version + "\ddddocr")
    if (-not (Test-Path $src)) { throw "解包后找不到 $src" }

    foreach ($v in $variants) {
        $dst = Join-Path $modelsDir $v
        New-Item -ItemType Directory -Force -Path $dst | Out-Null

        # common 配 CHARSET_BETA，old 配 CHARSET_OLD——这个对应关系写错就整行错位
        if ($v -eq 'common') {
            $onnx = 'common.onnx'
            $csVar = 'CHARSET_BETA'
        } else {
            $onnx = 'common_old.onnx'
            $csVar = 'CHARSET_OLD'
        }

        Copy-Item (Join-Path $src $onnx) (Join-Path $dst 'rec.onnx') -Force

        # 字典必须「一行一个字符」，且**保留第 0 行的空串**（它代表 blank）。
        # 用 python 生成而不是手工复制，避免编码/换行把空行吃掉。
        $py = @'
import importlib.util, sys
src, var, out = sys.argv[1], sys.argv[2], sys.argv[3]
spec = importlib.util.spec_from_file_location("cs", src)
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
chars = getattr(m, var)
assert chars[0] == "", "第 0 项应当是空串（blank），实际是 %r" % chars[0]
with open(out, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(chars))
print("%s -> %s (%d 项)" % (var, out, len(chars)))
'@
        $py = $py -replace "`r`n", "`n"
        $pyFile = Join-Path $tmp 'gen_charset.py'
        [System.IO.File]::WriteAllText($pyFile, $py, [System.Text.UTF8Encoding]::new($false))
        python $pyFile (Join-Path $src 'charsets.py') $csVar (Join-Path $dst 'charset.txt')

        Write-Host "=== $v 完成 ==="
        Get-ChildItem $dst | Select-Object Name, Length | Format-Table -AutoSize
    }
}
finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "DDDDOCR 模型就绪。注意：行检测复用 models\<level>\det.onnx，先跑 fetch_models.ps1。"
