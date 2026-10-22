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
    #   *.pt               原始权重（6~130MB，运行时只需要 onnx）
    #   ref_*.json         跟 ultralytics 逐框比对用的参考框（只有开发时需要）
    #   reference_*.json   精度基线（pipeline_match / yoloe_prompt_parity 用的），
    #                      只有跑 `cargo test -- --ignored` 时才读，运行时用不到
    #   inspect.json       模型结构快照（scripts/inspect_models.py 产出，纯开发用）
    #   *.bak              手工改文件留下的备份
    $junk = Get-ChildItem $dstModels -Recurse -File | Where-Object {
      $_.Extension -in @('.pt', '.bak') -or
        $_.Name -like 'ref_*.json' -or
        $_.Name -like 'reference_*.json' -or
        $_.Name -eq 'inspect.json'
    }
    foreach ($f in $junk) {
      Remove-Item $f.FullName -Force
      Write-Host ("[INFO] - {0} ({1:N1} MB)" -f $f.Name, ($f.Length / 1MB))
    }
    # 开发夹具（yolo 精度基线用的模型与参考框）不进分发包
    $fixtures = Join-Path $dstModels "_fixtures"
    if (Test-Path $fixtures) {
      Remove-Item $fixtures -Recurse -Force
      Write-Host "[INFO] - models/_fixtures（开发夹具）"
    }
    # PP-OCR 逐张量比对用的行图与参考张量（tensor_diff / ddddocr_parity 才读）
    $dumpDir = Join-Path $dstModels "small\dump"
    if (Test-Path $dumpDir) {
      Remove-Item $dumpDir -Recurse -Force
      Write-Host "[INFO] - models/small/dump（逐张量比对夹具）"
    }
    # 检测模型也不进分发包：models/detect/yolo/ 只留 README.txt。
    # 程序**不预置**检测模型（用户明确要求「yolo 模型应该全部用户自定义」）。
    # 开发机上放着的模型是拿来测的，跟着发出去就又变回「内置模型」了。
    $detectDir = Join-Path $dstModels "detect\yolo"
    if (Test-Path $detectDir) {
      Get-ChildItem $detectDir -Recurse -File |
        Where-Object { $_.Name -ne 'README.txt' } |
        ForEach-Object {
          Remove-Item $_.FullName -Force
          Write-Host ("[INFO] - detect/yolo/{0}" -f $_.Name)
        }
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
  接口   18 个接口的完整文档：参数、返回、错误码、请求示例，逐条可展开。
  试跑   在界面里直接传图验证，不用写代码。识别结果带耗时和框坐标。
  日志   实时看后端日志，可按级别过滤、搜索。排查「为什么没返回」先看这里。
  设置   模型档位、推理设备、端口、检测模型管理。改完自动保存。


接口一览（共 18 个，事实来源与「接口」页一致）

  认字（5 个）
    POST /ocr/shibie          整图识别，返回拼好的整段文字（最常用）
    POST /ocr/zhaozi          找字，只返回包含关键词的行，支持 /正则/
    POST /ocr/duohangjson     多行 JSON，每行的文本 + 四点坐标
    POST /ocr/danzijson       逐字定位，每个字一个框
    POST /ocr/quyuocr         区域识别：只认你指定那块矩形里的字（框坐标已平移回原图）

  找图（8 个）
    POST /ocr/tuzhaotu1       上传大图 + 小图，精准档找位置
    POST /ocr/tuzhaotu2       上传大图 + 小图，容错档找位置
    POST /ocr/pingmuzhaotu1   服务自己截屏找图，精准档（返回屏幕绝对坐标）
    POST /ocr/pingmuzhaotu2   服务自己截屏找图，容错档
    POST /ocr/jubingzhaotu1   截指定窗口找图，精准档（hwnd 或窗口标题）
    POST /ocr/jubingzhaotu2   截指定窗口找图，容错档
    POST /ocr/yuyizhaotu      语义找图：不用模板，让检测模型直接找目标
                              （source 选 image/screen/window；出参与上面几条完全一样）
    POST /vision/prompt       提示找图：当场给「参考图 + 框住的地方」，或者直接给几个词，
                              换目标不用重新导模型（出参与上面几条完全一样）

  看图（3 个）
    POST /vision/detect       目标检测，框出物体类别（请求里要显式带 engine: yolo）
    POST /vision/pipeline     先框区域再认字，适合「先找聊天框再读里面的字」
    POST /vision/qrcode       扫码：读出二维码 / 条形码里的内容（纯 CPU，引擎没就绪也能用）

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

  返回是统一的 {"code": 0, "msg": "...", "data": {...}}：
    code 0    成功
    code 400  参数不对（少字段、base64 坏了）
    code 404  路径不存在（msg 里会给出最接近的可用路径，照着改就行）
    code 405  路径对了但方法不对（这些接口基本都是 POST）
    code 501  这个接口还没实现
    code 503  引擎还没加载好，等一下再试
    code 1002 当前引擎不支持这个能力（例如让 OCR 引擎做目标检测）
    code 1003 引擎名不认识


开机自动启动（可选）
  设置 → 本地服务 → 打开「开机自动启动」。
  开机后程序自动在后台跑起来，界面直接缩在托盘里，不会弹窗。
  关掉时会把注册表里那一项删干净，不留垃圾。


在网页里调（CORS，默认关闭）
  默认不允许网页跨域调用：这些接口能读你的屏幕（截屏找图），
  不能让随便哪个打开着的网页都能驱动它。
  确实需要在自己写的本地 HTML / Electron 页面里 fetch 时：
  设置 → 本地服务 → 打开「允许浏览器页面调用」，然后停一下服务再启动（或改个端口）生效。
  打开后浏览器发的 OPTIONS 预检会返回 204 放行，正常请求也会带跨域头。
  用完建议关回去。


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


检测模型（全部由你自己提供）
  程序**不预置**任何检测模型——这是你自己的模型工作台。
  要用检测功能，把 .onnx 检测模型放进 models\detect\yolo\ 目录，
  或到「设置」页点「添加模型」选一个 .onnx（只记录路径，不复制文件）。
  目录里每个 onnx 都是一个可选项，列表第一个是默认模型。
  类别名自动从模型里读（ultralytics 导出会写进 metadata），不用手写 labels.txt。
  只支持单输出的轴对齐检测模型（v5/v6/v7/v8/v10/v11/v12/v26、YOLOE，
  含 NMS-free 端到端导出）；分割/姿态/旋转框/分类模型会被拒绝并说明原因。
  没有放模型时，接口会明确提示「没有找到检测模型」，不会静默返回空。


提示找图（/vision/prompt，两种模式，能力不同）
  这是「换目标不用换模型」的那条：提示当成检测器的额外输入，当场给就行。
    图找图（mode=visual）  给一张参考图 + 在上面框住要找的东西。**开箱可用**。
    语言找图（mode=text）  直接给几个词，如 "person,bus"。**需要额外一个 242MB 的权重**。

  分发包里带了图找图需要的全部模型，但不带语言找图的文本塔权重
  （mobileclip2_b 约 242MB，为了控制体积默认没打进去）。缺它时接口会明确返回
  「缺 MobileCLIP2-B 权重 / 词表 / 提示头」，不会静默失败。
  要用语言找图：把 MobileCLIP2-B 的文本塔权重放进 models\yoloe\ 目录即可
  （文件名 mobileclip2_b.safetensors 或 mobileclip2_b.ts 都认），重启服务生效。
  这个文件是 ultralytics YOLOE 用的文本塔，装一次 ultralytics 后跑任意一次
  YOLOE 文本提示推理，它会自动下到本地缓存目录，从缓存里拷过来就行。


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
