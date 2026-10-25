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
# 版本号的唯一来源是 `src-tauri/tauri.conf.json` 的 version。
# 这里**不写死**：写死过一次就会跟程序内显示的版本、GitHub Release 的 tag 漂开，
# 而发出去的包没法远程改版本号。Rust 侧由 build.rs 从同一个文件注入
# `OCR_STUDIO_VERSION`（见 src-tauri/src/version.rs）。
# 用 -Encoding UTF8 显式读：本机 Get-Content 默认按 ANSI(GBK) 解，中文会乱。
$confPath = Join-Path $root "src-tauri\tauri.conf.json"
$confRaw = [System.IO.File]::ReadAllText($confPath, [System.Text.UTF8Encoding]::new($false))
if ($confRaw -notmatch '"version"\s*:\s*"([^"]+)"') {
  throw "在 tauri.conf.json 里找不到顶层 version 字段：$confPath"
}
$version = $Matches[1]
Write-Host "[INFO] 版本号（来自 tauri.conf.json）: $version"

# 打包只搬文件，不编译（编译走 build.ps1）。这里不设 cargo env，
# 免得以后有人以为「打包会顺手 build」。

# 删本脚本自己产生的构建产物（release/ 下的暂存目录、剔出的 junk、旧 zip）。
#
# **刻意不走 Remove-Item**：本机 shell 的 safe-delete 包装会对这些路径抛
# SAFE_DELETE_FAIL_CLOSED（trash 对「项目内的构建产物」这类路径不稳定，
# detail 显示 OK 也照抛）。这些都是打包中间产物，进回收站没有意义
# （永远不会「恢复」一个暂存目录），所以用 .NET 直删，稳定且语义更对。
function Remove-BuildArtifact([string]$path) {
  if (-not (Test-Path $path)) { return }
  $item = Get-Item $path -Force
  if ($item.PSIsContainer) {
    [System.IO.Directory]::Delete($path, $true)
  } else {
    [System.IO.File]::Delete($path)
  }
}

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
if (Test-Path $stage) { Remove-BuildArtifact $stage }
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
    # 只带 PP-OCRv6 small（开箱能认字）。其余模型走设置页「模型库」按需下载，
    # 由 scripts/build_models.py 打成 zip 挂在固定 tag `models` 的 Release 上。
    #
    # 为什么不全带：三档 OCR（7/32/133MB）+ ddddocr（52MB）+ 提示找图（23MB）
    # 合起来 250MB 以上，而单个用户实际只用其中一两个。主包从 225MB 降到约 60MB。
    #
    # 下面的剔除逻辑保留着 —— 它们是「万一目录里又多了东西」的保险，
    # 在只拷 small 之后基本不会命中，但删掉就等于把这道保险撤了。
    if (-not (Test-Path (Join-Path $models "small"))) {
      throw "缺少 models/small —— 这是开箱 OCR 的基础模型，必须带进分发包"
    }
    New-Item -ItemType Directory -Path $dstModels -Force | Out-Null
    $smallDst = Join-Path $dstModels "small"
    New-Item -ItemType Directory -Path $smallDst -Force | Out-Null
    $smallFiles = Get-ChildItem (Join-Path $models "small") -File | Where-Object {
      $_.Name -ne 'inspect.json' -and $_.Name -notlike 'reference_*'
    }
    foreach ($f in $smallFiles) {
      Copy-Item $f.FullName (Join-Path $smallDst $f.Name) -Force
    }
    Write-Host ("[INFO] + models/small（{0} 个文件，开箱 OCR）" -f $smallFiles.Count)
    # 检测模型目录只留说明文件：告诉用户自己的 YOLO 模型放哪儿，
    # 同时保证目录存在（引擎扫描时不用处理「目录不存在」这种分支）。
    $detectDst = Join-Path $dstModels "detect\yolo"
    New-Item -ItemType Directory -Path $detectDst -Force | Out-Null
    $detectReadme = Join-Path $models "detect\yolo\README.txt"
    if (Test-Path $detectReadme) {
      Copy-Item $detectReadme (Join-Path $detectDst "README.txt") -Force
    }
    Write-Host "[INFO] - 其余模型不进包（设置页「模型库」按需下载）"
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
      Remove-BuildArtifact $f.FullName
      Write-Host ("[INFO] - {0} ({1:N1} MB)" -f $f.Name, ($f.Length / 1MB))
    }
    # 开发夹具（yolo 精度基线用的模型与参考框）不进分发包
    $fixtures = Join-Path $dstModels "_fixtures"
    if (Test-Path $fixtures) {
      Remove-BuildArtifact $fixtures
      Write-Host "[INFO] - models/_fixtures（开发夹具）"
    }
    # PP-OCR 逐张量比对用的行图与参考张量（tensor_diff / ddddocr_parity 才读）
    $dumpDir = Join-Path $dstModels "small\dump"
    if (Test-Path $dumpDir) {
      Remove-BuildArtifact $dumpDir
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
          Remove-BuildArtifact $_.FullName
          Write-Host ("[INFO] - detect/yolo/{0}" -f $_.Name)
        }
    }
    # ---- 提示找图只留两件套：检测器 + 视觉提示编码器 ----
    #
    # 文本提示已于 2026-10-23 移除（实测在界面控件上分数只有 0.1~0.2，等于噪声）。
    # 既然功能没了，配套文件一个都不带：文本塔（242MB）、CLIP 词表、文本提示头、
    # 预烘焙向量、中英词典。留着只会让用户以为「还没配好」。
    $yoloeDst = Join-Path $dstModels "yoloe"
    if (Test-Path $yoloeDst) {
      $junk = @(
        "mobileclip2_b.ts", "mobileclip2_b.safetensors",
        "tokenizer.min.json", "tokenizer.json",
        "text_pe.bin", "zh_words.json"
      )
      # 文本提示头的 .onnx 和它的 .json 元数据一起剔。只剔 .onnx 会留下
      # 一个孤零零的 yoloe26n_texthead.json，用户照着它去找 .onnx 永远找不到。
      $junk += (Get-ChildItem $yoloeDst -Filter "*_texthead.*" -File |
        ForEach-Object { $_.Name })
      foreach ($n in $junk) {
        $p = Join-Path $yoloeDst $n
        if (Test-Path $p) {
          Remove-BuildArtifact $p
          Write-Host "[INFO] - yoloe/$n（文本提示已移除，不带）"
        }
      }
    }
    Write-Host "[INFO] + models/（small 开箱可用，其余按需下载）"
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
  接口   20 个接口的完整文档：参数、返回、错误码、请求示例，逐条可展开。
  试跑   在界面里直接传图验证，不用写代码。识别结果带耗时和框坐标。
  日志   实时看后端日志，可按级别过滤、搜索。排查「为什么没返回」先看这里。
  设置   模型档位、推理设备、端口、检测模型管理、中文参考库、AI 控制。改完自动保存。


接口一览（共 20 个，事实来源与「接口」页一致）

  认字（5 个）
    POST /ocr/shibie          整图识别，返回拼好的整段文字（最常用）
    POST /ocr/zhaozi          找字，只返回包含关键词的行，支持 /正则/
    POST /ocr/duohangjson     多行 JSON，每行的文本 + 四点坐标
    POST /ocr/danzijson       逐字定位，每个字一个框
    POST /ocr/quyuocr         区域识别：只认你指定那块矩形里的字（框坐标已平移回原图）

  找图（9 个）
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
    GET/POST/DELETE /vision/promptref
                              中文参考库：框一次 + 起个中文名，以后按中文名找

  看图（4 个）
    POST /vision/detect       目标检测，框出物体类别（请求里要显式带 engine: yolo）
    POST /vision/pipeline     先框区域再认字，适合「先找聊天框再读里面的字」
    POST /vision/qrcode       扫码：读出二维码 / 条形码里的内容（纯 CPU，引擎没就绪也能用）
    POST /ocr/biaoge          表格识别：定位表格并还原行列结构，出 text / html / cells
                              （mode 选 auto / structure / detect，详见「表格识别」一节）

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
  端口被占用起不来时，服务页会直接告诉你「被哪个程序（进程号）占了」，
  而不是只说一句启动失败。去「设置」里改成 8850 之类再保存，会自动重启服务。
  改完记得把调用地址一起改掉。


界面打不开怎么办（多半是缺 WebView2 运行时）
  界面是 WebView2 画的，Rust 那个 8848 服务跟它无关 —— 界面打不开时，
  你的脚本照样能调接口。启动时如果检测到没装，会弹窗说明并提供一键安装
  （约 1.5MB，装完重启程序即可，不需要管理员权限）。
  当时选了「暂不处理」，之后想装可以再双击一次程序，它会重新问。
  Win7 / Win8 装不了 WebView2，界面在这两个系统上无法运行。


检测模型（全部由你自己提供）
  程序**不预置**任何检测模型——这是你自己的模型工作台。
  要用检测功能，把 .onnx 检测模型放进 models\detect\yolo\ 目录，
  或到「设置」页点「添加模型」选一个 .onnx（只记录路径，不复制文件）。
  目录里每个 onnx 都是一个可选项，列表第一个是默认模型。
  类别名自动从模型里读（ultralytics 导出会写进 metadata），不用手写 labels.txt。
  只支持单输出的轴对齐检测模型（v5/v6/v7/v8/v10/v11/v12/v26、YOLOE，
  含 NMS-free 端到端导出）；分割/姿态/旋转框/分类模型会被拒绝并说明原因。
  没有放模型时，接口会明确提示「没有找到检测模型」，不会静默返回空。


识别精度（三档，设置 → 模型）
  tiny    最快，字大清晰的截图够用
  small   默认，速度与精度均衡
  medium  最准，字小、模糊、背景花的图用它，会慢一些
  随包只带 small（装完就能认字）。tiny / medium 以及下面的提示找图、
  表格识别，都在「设置 → 模型库」里：点一下下载，有进度条，装完立刻可用，
  不用重启。网络下不动时可以用旁边的「手动导入」选本地 zip。


表格识别（/ocr/biaoge）
  一张图三种用法，靠 mode 选：
    auto（默认）  先在整页里找表格，再逐张识别。一张都没找到会退回
                  「整图当一张表格」并返回 fallback:true。
    structure    直接把整图当一张表格（图本来就是裁好的表格时最准）。
    detect       只给表格位置，不做结构识别。
  出参：text 是列间 Tab、行间换行的纯文本（直接粘进 Excel）；
  html 是带 colspan/rowspan 的 <table>；cells 给每个格子的坐标和文字。
  with_text=false 只出结构，快一倍。
  需要先在「模型库」下载「表格识别」（约 13MB，检测 + 结构两个模型一起）。


提示找图（/vision/prompt）
  这是「换目标不用换模型」的那条：提示当成检测器的额外输入，当场给就行。
  比的是「长得像不像」，所以换了皮肤、按钮变成按下态照样认得。
    图找图（mode=visual）  给一张参考图 + 在上面框住要找的东西。
    按名字找（mode=ref）   给几个**教过的**名字（见下「按中文名找」）。
  models\yoloe\ 下只需要两个文件：检测器（*_prompt.onnx）+ 视觉提示编码器
  （*_vpe.onnx），共约 20MB，随包带。
  举例：POST /vision/prompt  {"mode":"visual","image":"<要找的图>",
        "ref_image":"<参考图>","ref_boxes":"10,20,110,60","conf":0.25}

按中文名找（中文参考库，等于「教会它认自家界面里的元素」）
  图找图每次都要给参考图；嫌麻烦就先教一次、起个中文名，之后只写名字：
    1) POST /vision/promptref，给一张参考图 + 图上的框 + 起个中文名（如「提交按钮」）；
    2) 之后 /vision/prompt 用 mode=ref + words="提交按钮" 就能找；
    3) GET /vision/promptref 看教过哪些，DELETE 按名字忘掉。
  走的就是图找图那条视觉通道（存的是当初框的那块图），所以中文名随便起，
  不需要任何词表，也不需要模型认识这个东西。
  参考库存在本机 %APPDATA%\com.ocrstudio.app\prompt_refs\，只存参考图，不上传、不联网。

  注：早期还有一个「语言找图」（mode=text，写几个词让它找语义上像的）。
  实测在界面控件这类细粒度目标上分数只有 0.1~0.2 量级，等于噪声，已整体移除；
  传 mode=text 会得到一条明确的指路错误。


AI 控制（让本机 AI 助手直接操控这个服务，默认关闭）
  设置 → AI 控制：打开开关、复制连接码、一键复制提示词。
  把提示词粘给任何跑在**这台电脑上**的 AI（编程助手、本地脚本、本地模型）就行，
  **不用装任何东西**（不是 MCP；网页版 AI 在云端，访问不到 127.0.0.1，只有本机能用）。
    GET  /ai/v1/tools   工具清单：name / summary / risk 风险档 / 参数说明
    POST /ai/v1/tool    调工具：{"tool":"ocr_image","args":{...},"request_id":"..."}
  每次请求要带请求头 Authorization: Bearer <连接码>（设置页可复制、可重置）。
  写操作建议带 request_id：同一次意图重试带同一个值，不会重复执行。
  风险分三档：readonly（只看）/ write（会改状态或读屏幕）/ dangerous（启停服务、
  改端口阈值，会影响正在调接口的其他脚本）。AI 的每次调用都记在「日志」页里。


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
if (Test-Path $zip) { Remove-BuildArtifact $zip }
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
$size = (Get-Item $zip).Length / 1MB
Write-Host ("[PASS] 已生成 {0} ({1:N1} MB)" -f $zip, $size)
Write-Host "[INFO] 分发时把这个 zip 给别人，解压即用"
