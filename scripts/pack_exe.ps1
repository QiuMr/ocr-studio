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

这是什么
  OCR Studio 是一个本地 OCR / 找图 API 服务，不是「点按钮识图」的软件。
  双击启动后，它在本机 http://127.0.0.1:8848 开一个接口服务，
  你自己的程序（Python / C# / 易语言 / 按键精灵……）用 HTTP 调它就行。
  桌面窗口只是这个服务的控制面板：看服务开没开、地址是多少、能调什么。


三步上手
  1) 双击「OCR Studio.exe」，等首页显示「运行中」（首次启动约 5~10 秒加载模型）。
  2) 打开「接口」页，挑一个接口，点「复制地址」拿到 http://127.0.0.1:8848/...，
     再点「复制代码」拿现成的调用片段。
  3) 把它粘到你自己的程序里跑。不确定能不能通，就去「试跑」页传张图先测一遍。

  首页那块深色面板就是服务状态：运行中 / 已停止、地址、今日调用次数、平均耗时。
  停止或启动服务，点面板上的按钮即可。


界面导航（左侧 5 项）
  服务   服务状态、地址复制、启停、能力概览、接入代码。第一次用先看这里。
  接口   14 个接口的完整文档：参数、返回、错误码、请求示例，逐条可展开。
  试跑   在界面里直接传图验证，不用写代码。识别结果带耗时和框坐标。
  日志   实时看后端日志，可按级别过滤、搜索。排查「为什么没返回」先看这里。
  设置   模型档位、推理设备、端口、检测模型管理。改完自动保存。


接口一览（共 14 个，事实来源与「接口」页一致）

  认字（4 个）
    POST /ocr/shibie          整图识别，返回拼好的整段文字（最常用）
    POST /ocr/zhaozi          找字，只返回包含关键词的行，支持 /正则/
    POST /ocr/duohangjson     多行 JSON，每行的文本 + 四点坐标
    POST /ocr/danzijson       逐字定位，每个字一个框

  找图（6 个）
    POST /ocr/tuzhaotu1       上传大图 + 小图，精准档找位置
    POST /ocr/tuzhaotu2       上传大图 + 小图，容错档找位置
    POST /ocr/pingmuzhaotu1   服务自己截屏找图，精准档（返回屏幕绝对坐标）
    POST /ocr/pingmuzhaotu2   服务自己截屏找图，容错档
    POST /ocr/jubingzhaotu1   截指定窗口找图，精准档（hwnd 或窗口标题）
    POST /ocr/jubingzhaotu2   截指定窗口找图，容错档

  看图（2 个）
    POST /vision/detect       目标检测，框出物体类别（请求里要显式带 engine: yolo）
    POST /vision/pipeline     先框区域再认字，适合「先找聊天框再读里面的字」

  状态（2 个）
    GET  /ocr/health          服务和引擎是否就绪。脚本启动时先调它，code 为 0 才继续
    GET  /engine/list         列出本机可用的引擎（只扫目录，很快）


调用示例

  curl（命令行里直接试）
    curl -X POST http://127.0.0.1:8848/ocr/shibie ^
         -H "Content-Type: application/json" ^
         -d "{\"image\":\"<图片的 base64>\"}"

  Python
    import base64, json, urllib.request

    b64 = base64.b64encode(open("shot.png", "rb").read()).decode()
    body = json.dumps({"image": b64}).encode()
    req = urllib.request.Request(
        "http://127.0.0.1:8848/ocr/shibie", body,
        {"Content-Type": "application/json"})
    # 本机若开着系统代理，必须绕过，否则 127.0.0.1 也会被代理拦成 502
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    print(opener.open(req).read().decode())

  返回是统一的 {"code": 0, "message": "...", "data": {...}}：
    code 0    成功
    code 400  参数不对（少字段、base64 坏了）
    code 501  这个接口还没实现
    code 503  引擎还没加载好，等一下再试
    code 1002 当前引擎不支持这个能力（例如让 OCR 引擎做目标检测）
    code 1003 引擎名不认识


关窗口不会退出（托盘常驻）
  这是个常驻服务，所以点窗口右上角关闭只是把界面收到托盘，服务继续跑，
  后台仍然可以用 http://127.0.0.1:8848 调。
  托盘图标（任务栏右下角）右键：
    打开界面 / 启动服务 / 停止服务 / 退出 OCR Studio
  真正退出必须点托盘的「退出 OCR Studio」，否则服务一直在。
  重复双击 exe 不会开出第二个实例（也不会抢端口），只会把已有窗口拉出来。


目录里都是什么（缺一个都跑不起来，别单独拷 exe）
  OCR Studio.exe                 主程序
  onnxruntime.dll                推理框架
  DirectML.dll                   显卡加速后端（没有它就只能用 CPU）
  onnxruntime_providers_shared.dll
  models\                        模型文件。删了会提示「引擎未加载」
  使用说明.txt                   本文件


显卡加速没生效怎么办
  打开「服务」页看状态面板，或「设置」页看「推理设备」，确认显示的是 GPU。
  显示 CPU 说明 DirectML 没加载起来（常见于缺 dll、或显卡驱动过旧）。
  此时识别仍可用，只是慢一些；小图通常感觉不出来。


防火墙 / 端口
  程序只监听本机 127.0.0.1:8848，不对外网开放，不需要放行防火墙。
  端口被占用无法启动时，去「设置」里改成 8850 之类再保存，会自动重启服务。
  改完记得把调用地址一起改掉。


检测模型（可选）
  自带的是通用 COCO 模型，检出的是人/车这类物体，不是文字块——这是正常的。
  要用自己的模型：放进 models\detect\yolo\ 目录，或到「设置」页点「添加模型」
  选一个 .onnx 文件（只记录路径，不复制文件）。类别名自动从模型里读，不用手写。
  注意只支持单输出的检测模型；分割/姿态/分类模型会被拒绝并给出提示。


日志与排错
  「日志」页能实时看后端日志，可按级别过滤。
  磁盘日志在 %APPDATA%\com.ocrstudio.app\ocr-studio.log
  常见现象：
    首页一直「引擎未就绪」 -> 模型没放全，看日志里的「模型加载失败」
    接口返回 503           -> 同上，或模型正在加载，等几秒
    接口返回 1003          -> engine 参数写错，去「接口」页看可用值
    窗口白屏               -> 显卡兼容问题，程序会自动切软件渲染重启窗口


卸载
  先点托盘「退出 OCR Studio」，再删掉整个文件夹即可。
  想清干净，另外删掉 %APPDATA%\com.ocrstudio.app\ 这个目录（配置和日志）。


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
