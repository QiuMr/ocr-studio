# OCR Studio

开箱即用的本地 OCR / 找字 / 找图桌面工具。**完全离线**，不联网、不上传图片；
自带 HTTP 接口，方便易语言、按键精灵、Python 等外部脚本调用。

- 技术栈：Tauri 2 + React 19 + TypeScript + Tailwind v4；推理用 ONNX Runtime（ort）
- 检测/识别模型：PP-OCR 三档（tiny / small / medium），GPU 走 DirectML
- 找图：零均值 NCC 模板匹配，灰度金字塔加速，1080p 全屏找 64px 模板约 60ms

## 快速开始

```powershell
pnpm install
powershell -File scripts\fetch_models.ps1 -Level all   # PP-OCR 三档模型（tiny/small/medium，-Level small 只下一档）
powershell -File scripts\fetch_ddddocr.ps1           # 可选：DDDDOCR 识别模型（54MB）
powershell -File scripts\fetch_yolo.ps1              # 可选：检测模型（默认 yolov8n，约 19MB，需本机装 ultralytics）
powershell -File scripts\fetch_yolo.ps1 -Model yolo11n.pt   # 换其它 ultralytics 检测权重
python scripts\fetch_yoloe.py --image assets\bus.png --box 30,230,800,776 --name bus  # 可选：YOLOE-26 语义找图模型
pnpm build                       # 前端（tsc --noEmit + vite build）
cd src-tauri
cargo build --release            # exe 在 $CARGO_TARGET_DIR/release（本机为 D:\rust-target）
powershell -File ..\scripts\pack_exe.ps1   # 打免安装分发包
```

本项目的 Rust 工具链是**项目内私有安装**，跑 cargo 前必须先设：

```powershell
$env:RUSTUP_HOME = "D:\Python测试\OCR-Studio\.rust\rustup"
$env:CARGO_HOME  = "D:\Python测试\OCR-Studio\.rust\cargo"
```

### 分发（免安装 zip）

`tauri.conf.json` 里 `bundle.active = false`，**不再产出 MSI/NSIS**。
`scripts/pack_exe.ps1` 会打出 `release/OCR-Studio-<版本>-win64.zip`：

```
OCR Studio.exe           主程序
onnxruntime.dll          推理框架
DirectML.dll             显卡加速后端
onnxruntime_providers_shared.dll
models/                  模型（缺了会提示「引擎未加载」）
使用说明.txt
```

> 单独把 exe 拷给别人是跑不起来的：缺 `DirectML.dll` 会起不来，
> 缺 `onnxruntime.dll` 会静默退回 CPU。务必整包分发。
>
> `runtime\onnxruntime.dll` 必须是**带 DirectML 的构建**（用
> `scripts\copy_runtime.ps1` 从 `onnxruntime-directml` 包取）。CPU 版也能跑，
> 但启动时会 warn「不带 DML 执行提供器，改用 CPU」，界面上的推理设备会如实显示 CPU。
>
> `models/detect/yolo/` **出厂为空**（程序不预置检测模型）；`models/ocr/ddddocr/` 占 54MB，
> 只用 PP-OCR 的话可以整目录删掉，引擎列表里会显示「未安装」，其余功能不受影响。

### 分发包只带 small，其余模型按需下载

主包**只带 PP-OCRv6 small**（加上推理运行时一共约 50MB）。tiny / medium / ddddocr /
提示找图 / 检测模型都不在里面，用户在设置页「模型库」点一下才下载。

可选模型的打包与上传是一条命令：

```bash
python scripts/build_models.py            # 只打包 + 生成清单
python scripts/build_models.py --upload    # 打包 + 上传到 Release
```

**模型挂在固定 tag `models` 的 Release 上，而且那个 Release 必须标成
`--prerelease`。** GitHub 的 `releases/latest` 取的是「最新创建的正式 release」，
模型库要是正式 release 且比版本发得晚，`releases/latest/download/latest.json`
就会 404，自动更新全线报废。

清单有内置兜底（`src-tauri/assets/catalog.json`，编译期嵌入）：没网时也能
显示「有哪些能力可以加」，只是点下载会失败。清单三层：网络 → 本地缓存（1h）→ 内置。

### 前端改动不生效？

`generate_context!` 在**编译期**把 `dist` 嵌进 exe。改完前端只跑 `pnpm build`
是不够的，必须重新 `cargo build`，否则 exe 里还是旧界面。

## 版本与自动更新

**版本号只有一个来源：`src-tauri/tauri.conf.json` 的 `version`。**

发版时只改这一处，其余三处会自动跟随：

| 用到版本号的地方 | 怎么拿到 |
|---|---|
| 程序里显示 / `/ocr/health` 的 `version` | `build.rs` 读配置注入 `OCR_STUDIO_VERSION`（见 `src-tauri/src/version.rs`） |
| zip 文件名 `OCR-Studio-<版本>-win64.zip` | `pack_exe.ps1` 读同一个文件 |
| GitHub Release 的 tag `v<版本>` | `release.ps1` 读同一个文件 |

写死过的代价：曾经 `pack_exe.ps1` 里有一份 `$version = "0.1.0"`，改版本时忘了它，
结果 zip 名字和程序内版本对不上——用户报版本号时两边说的不是一回事。

### 发一条版本

```powershell
# 1. 改 src-tauri/tauri.conf.json 的 version（例如 0.2.0）
# 2. 提交
git add -A; git commit -m "release: v0.2.0"
# 3. 一条命令：编译 → 打包 → 打 tag → 上传 → 生成 latest.json
powershell -File scripts/release.ps1 -Notes "修了找图偏移；新增 xxx"
```

脚本会先拦下三种发版事故：工作区有未提交改动、tag 已存在、产物缺失。

### 自动更新怎么工作

程序**不走** `tauri-plugin-updater`——那个只认安装器（NSIS/MSI）产出的签名更新包，
本项目是免安装 zip，硬套会在「找不到安装器」和「已是最新」之间反复。

自己走一条更简单的路：

```
程序启动 / 点「检查更新」
        ↓  GET releases/latest/download/latest.json
   比对 version（**按数值比，不是字符串**，否则 0.10.0 会被判成比 0.9.0 旧）
        ↓  有新版 → 下载 zip → 校验 size + sha256 → 解压到 %TEMP%
   把当前 exe 复制一份到 %TEMP% 当「更新助手」，主进程退出
        ↓
   助手等旧进程真的消失 → 覆盖安装目录 → 启动新 exe
```

> **为什么必须「退出后再替换」**：Windows 不允许覆盖正在运行的 exe，
> 而且 `onnxruntime.dll` 被主进程加载着，进程没退干净就覆盖会
> `ERROR_SHARING_VIOLATION`，目录停在半新半旧——这是最难救的一种状态。
> 所以助手用 `OpenProcess` 轮询 PID 而不是 `sleep` 几秒。

> **为什么助手要在 `tauri::Builder` 之前分流**：它和主程序是同一个 exe，
> `tauri-plugin-single-instance` 会把它当成「用户又双击了一次」踢掉，
> 表现为「点了更新，程序关掉再也没起来」。

用户数据在 `%APPDATA%\com.ocrstudio.app\`（配置、教过的目标、日志），不在包里，
覆盖时不会丢。

## 界面

产品是「API 工具」，界面只是它的控制面板 —— 导航固定 5 项：

| 页面 | 作用 |
|---|---|
| 服务 | 服务状态（运行中 / 已停止）、地址复制、启停、能力概览、接入代码 |
| 接口 | 19 个接口的完整文档：参数、返回、错误码、可复制的请求示例 |
| 试跑 | 在界面里直接传图验证（识别 / 找图 / 语义找图 / 提示找图 / 扫码），带耗时与框坐标 |
| 日志 | 后端 tracing 实时日志，按级别过滤、搜索、暂停、复制 |
| 设置 | 模型档位、推理设备、端口、检测模型管理、中文参考库、AI 控制、开机自启、CORS |

侧边栏可收起成图标栏（按钮在标题栏左上角，状态会记住）。
**关窗口 = 收进托盘**（服务继续跑），真正退出要点托盘菜单的「退出 OCR Studio」。

### 托盘菜单

不是系统原生菜单 —— 那是 `tauri::menu::Menu` 画的，白底黑字、没有图标、
字体跟着系统走，样式**改不了**。现在是自己画的一个无边框浮层窗口：

- 头部显示程序版本 + 服务状态（`运行中 · 127.0.0.1:8848` / `已停止`），每 2 秒刷新
- 打开界面 / 启停服务 / 检查更新 / 退出
- **有更新时「检查更新」会变成一条蓝色高亮条目**（`有更新 v0.1.3` + 圆点），
  点它直接打开界面去升级 —— 这是原生菜单做不到的

点菜单外面、按 Esc 都会自动收起。

> 诊断：`--debug-tray` 会让**已经运行的实例**立刻弹出托盘菜单（Win11 把托盘图标
> 折叠进「^」里，脚本点击很难命中，抓图验收要用这个）。
> 另见 `OCR_NO_CORE`（跳过引擎）、`OCR_WEBVIEW_ARGS`（指定渲染参数）。

## HTTP 接口

服务只监听 `127.0.0.1`，默认端口 8848（可在设置里改）。**共 20 个端点**
（认字 5 / 找图 9 / 看图 3 / 文档 1 / 状态 2）。完整清单与参数以 `src/lib/endpoints.ts` 为
**唯一事实源**（首页概览与「接口」页同源），与 `src-tauri/src/api/routes.rs` 的 `PATHS` 一一对应。

| 端点 | 说明 |
|---|---|
| `GET /ocr/health` | 服务与引擎状态（脚本启动时先探它，code 为 0 才继续） |
| `GET /engine/list` | 列出本机可用引擎与能力（只扫目录、不加载模型） |
| `POST /ocr/shibie` | 整图识别，返回拼接文本 + 框 + 分数 |
| `POST /ocr/zhaozi` | 按关键词（或 `/正则/`）筛选命中行 |
| `POST /ocr/duohangjson` | 多行结构 |
| `POST /ocr/danzijson` | **逐字**：每行每个字的四点框 + 置信度 + CTC 帧区间 |
| `POST /ocr/quyuocr` | **区域识别**：只认指定矩形里的字，框坐标已平移回原图 |
| `POST /ocr/tuzhaotu1\|2` | 在**上传的图**里找图（1=精准 ≥0.92，2=容错 ≥0.72） |
| `POST /ocr/jubingzhaotu1\|2` | 截**指定窗口**找图（hwnd 或窗口标题） |
| `POST /ocr/pingmuzhaotu1\|2` | 截**整个屏幕**找图（返回屏幕绝对坐标） |
| `POST /ocr/yuyizhaotu` | **语义找图**：不用模板图，让检测模型直接找目标（`source` 选 image/screen/window） |
| `POST /vision/prompt` | **提示找图**：当场给参考图+框（`mode=visual`）或教过的名字（`mode=ref`），换目标不用重导模型 |
| `GET/POST/DELETE /vision/promptref` | **中文参考库**：教一次（框+中文名）→ 按名字找，走的就是图找图通道 |
| `POST /vision/detect` | **通用目标检测**（`engine: "yolo"`，返回类别框 + 置信度；OCR 引擎调它会得到 code 1002） |
| `POST /vision/pipeline` | **检测 + 识别串联**：检测引擎找区域 → PP-OCR 框内认字（`detect_engine` 缺省 `yolo`） |
| `POST /vision/qrcode` | **扫码**：读出二维码 / 条形码内容（纯 CPU，与引擎无关） |
| `POST /ocr/biaoge` | **表格识别**：表格图 → HTML/TSV + 每个格子的坐标和文字（`mode` 选 auto/structure/detect） |

请求体支持 JSON（`{"image": "<base64>"}`）与 multipart（文件字段 `image`），上限 32MB。

找图类（模板 / 语义 / 提示）**出参完全同构**（`found / x / y / center_x / center_y / score /
matched / matches[] / origin_x / origin_y`），脚本换个路径就能用；语义与提示找图额外多
`engine` 与命中里的 `class_id / class_name`。

### 选择引擎

所有识别端点都接受可选字段 `engine`：

```json
{ "image": "<base64>", "engine": "ddddocr:common" }
```

| 值 | 说明 |
|---|---|
| 不传 / `ppocr:*` | PP-OCR（默认）。成段文字、扫描件、截图用这个 |
| `ddddocr:common` | DDDDOCR 54MB 版。验证码、短英文数字更快，长文本会丢空格与标点 |
| `ddddocr:old` | DDDDOCR 13.6MB 版。同上，精度略低但体积小 |
| `yolo` | **默认**检测模型。目标检测（别名 `detect`），**只能配 `POST /vision/detect` 与 `/vision/pipeline` 用**，不能做文字识别 |
| `yolo:<模型名>` | 选另一个检测模型（`model.onnx` 之外的，或设置里加的外部路径）；模型名就是 onnx 文件名去掉扩展名 |

`/vision/detect` 额外接受 `conf`（置信度下限，默认 0.25）、`iou`（NMS 阈值，默认 0.7）、
`class_filter`（类别 id 数组，缺省全部）。后处理与 ultralytics 逐框对齐
（实测 IoU=1.0 / conf 差=0，见 docs/ENGINES.md 6.2）。

`/vision/pipeline` 额外接受 `detect_engine`（缺省 `yolo`）。注意：COCO 类别与
文本区域不对齐，yolov8n+coco 检出的是人/车不是文字块——**pipeline 面向自训检测模型**。

DDDDOCR 自己不做行检测（它自带的 det 是 YOLOX 目标检测，切文档很差），
行检测**复用 PP-OCR 的 det 模型**，所以用 ddddocr 时 `models/small/det.onnx` 必须在。

### 目标检测：模型是你自己的

只有一个引擎族 `yolo`（别名 `detect`），但**模型可以有多个、可以放在任意位置**：

- 往 `models\detect\yolo\` 丢 onnx —— 每个 `*.onnx` 都是一个可选项，
  **按文件名排序后的第一个就是默认模型**；
- 或者去「设置 → 目标检测模型 → 添加模型」直接指向任意目录的 onnx（**不复制文件**，
  配置里只记路径）。`.pt` 请先用 `yolo export format=onnx` 导出。

> 这个目录**出厂为空**：程序不预置任何检测模型。没有模型时检测类接口会明确提示，
> 不会静默返回空。想快速要一个现成的，跑 `scripts\fetch_yolo.ps1`（yolov8n+coco）。

模型结构在加载时从 ONNX 的输入输出形状**自动识别**：输出是 `[1,C,N]` 还是 `[1,N,C]`、
有没有 objectness、输入边长多少、是不是 NMS-free 端到端（`[1,N,6+nm]`）。
所以 `yolov5u / yolov8 / yolo11 / yolo12 / yolo26 / yolov10 / YOLOE` 这类单输出导出
直接放进去就能跑，换模型不用改配置。

**类名自动从模型里读**（ultralytics 导出会写进 ONNX metadata 的 `names`），
不用手写 `labels.txt`。`labels.txt` 只是给非 ultralytics 导出的兜底。

`models\detect\yolo\v8\model.onnx` 这种旧的分层结构仍然认（`v8` 会作为模型名）。

不支持的两种情况会**明确报错**而不是给出垃圾框：

| 情况 | 原因 |
|---|---|
| v3/v4 原始多输出 head（3 个输出张量） | 要按 anchor 逐层解码，另一套逻辑 |
| `task` 是 `pose` / `obb` / `classify` | 输出关键点 / 角度 / 类别向量，按检测解会整体错位 |

`task=segment` 的模型**只取检测头**（掩码系数列会被跳过），所以 YOLOE-26 这种
官方只放 `-seg` 权重的情况也能用——这正是语义找图默认走的那条路。

### 语义找图（YOLOE-26）

模板找图要求「长得一模一样」；语义找图让检测模型自己认目标，**不挑外观**。
代价是只认得模型见过的类别。

YOLOE-26 是开放词汇检测模型，用 `scripts/fetch_yoloe.py` 导出的版本里
提示词在**导出 ONNX 时就烘焙进权重**，导出后改不了类别：

```powershell
python scripts\fetch_yoloe.py --image assets\bus.png --box 30,230,800,776 --name bus
# 产出 models/detect/yolo/yoloe26_bus.onnx，之后 /ocr/yuyizhaotu 里 engine 传 yolo:yoloe26_bus
```

### 提示找图（`/vision/prompt`，换目标不用换模型）

上面那条的提示烘焙在权重里；这条把提示当成检测器的**额外输入**，当场给：

| 模式 | 怎么给提示 | 依赖 |
|---|---|---|
| `mode=visual`（图找图） | 一张参考图 + 在上面框住要找的东西 | `models/yoloe/` 两件套（随包带，约 20MB） |
| `mode=ref`（按名字找） | 几个**教过的**名字，如 `"提交按钮,搜索框"` | 同上 + 中文参考库里有这些名字 |

两者出参都与模板找图同构。模型由 `scripts/export_yoloe_prompt.py` 导出到 `models/yoloe/`：
检测器（`*_prompt.onnx`，双输入 `images` + `pe`）、视觉提示编码器（`*_vpe.onnx`）。
比的是「长得像不像」，所以换皮肤、按钮变按下态照样认得。

> **`mode=text`（语言找图）已于 2026-10-23 移除。** 它把词编码成语义向量去找
> 「语义上像的」，在 COCO 那类大物体上能看，到界面控件这种细粒度目标上分数只有
> 0.1~0.2 量级（等于噪声）；为它要背 242MB 文本塔 + 英文 BPE 词表 + 中英词典。
> 传 `mode=text` 会得到一条明确指路错误（先教一次、再 `mode=ref`）。

### 按中文名找：中文参考库

「教会它认自家界面里的元素」——这也是唯一需要的「中文」支持。做法是框一次、起个中文名，
之后只写名字。**走的就是图找图那条视觉通道**（存的是当初框的那块图），所以中文名随便起，
不需要任何词表，也不需要模型认识这个东西。

参考库存在本机配置目录（`%APPDATA%` 下的 `prompt_refs/`），只存参考图，不上传、不联网：

```bash
# 教一次：整张参考图 + 图上的一个框 + 中文名
curl -X POST http://127.0.0.1:8848/vision/promptref \
  -H "Content-Type: application/json" \
  -d '{"name":"提交按钮","image":"<base64>","box":[100,200,180,230]}'

# 之后按名字找
curl -X POST http://127.0.0.1:8848/vision/prompt \
  -H "Content-Type: application/json" \
  -d '{"mode":"ref","image":"<base64>","words":"提交按钮"}'
```

没教过的名字会**明确报错并列出已有的名字**，不会静默返回空。

`GET /vision/promptref` 看教过哪些，`DELETE /vision/promptref` 按名字忘掉。
同名重复教会覆盖；名字不存在删也返回成功（`removed=false`），重复删不报错。

## AI 操控接口（给本机 AI 助手用）

设置页 → **AI 控制**：打开开关、复制连接码、一键复制提示词。把提示词粘给任何
跑在**这台电脑上**的 AI（编程助手、本地脚本、本地模型），它就能直接操控服务：

```
GET  /ai/v1/tools   工具清单（name / summary / risk 档位 / 参数说明）
POST /ai/v1/tool    调一个工具：{"tool":"ocr_image","args":{...},"request_id":"..."}
```

- **不是 MCP**：AI 会发 HTTP 就够了，不用装任何东西。网页版 AI 在云端，
  访问不到 `127.0.0.1`，只有本机 AI 能用。
- **鉴权**：每次请求带 `Authorization: Bearer <连接码>`（常数时间比较）；
  开关关着时直接 403 并提示去哪开。
- **给 AI 看的返回**：`{ok, summary, data, error}` —— `summary` 是给用户转述的人话，
  `error` 会告诉 AI 下一步怎么做（缺哪个参数、去看清单、还是让用户去开开关）。
- **风险档位**：`readonly`（只看）/ `write`（会改状态或读屏幕）/ `dangerous`
  （启停服务、改端口阈值，会影响正在调接口的其他脚本）。写操作记审计日志
  （运行日志里能看到 AI 干了什么）。
- **幂等**：写操作带 `request_id`，同一次意图重试带同一个值，原样回放不执行第二遍。
- **限流** 120 次/分钟（AI 写错提示词容易进死循环）。
- **刻意不提供**：退出程序、改开机自启、改 AI 开关与连接码 —— 这些让用户在界面上自己来。

## 验证

```powershell
cargo test                    # 138 个单元测试 + 17 个 API 冒烟 + 7 个 AI 接口端到端
cargo test -- --ignored       # 29 个重量级：精度基线、真实截图、逐字对齐、跨语言对齐、提示找图
python scripts/verify_http_contract.py --exe "<OCR Studio.exe>"   # 404/405 与 CORS 信封
python scripts/verify_packaged.py --exe "release/OCR-Studio/OCR Studio.exe"  # 分发包端到端
python scripts/verify_ui_zh.py --exe "release/OCR-Studio/OCR Studio.exe"    # 截图自己的程序，验收按名字找（教/找/删 + 跨图泛化）
python scripts/e2e_find.py    # 真实桌面找图端到端
python scripts/e2e_danzi.py <图片>   # 逐字框端到端断言
python scripts/ref_yolo.py models/detect/yolo/model.onnx assets/bus.png out.json   # yolo 官方实现参照
python scripts/ref_ddddocr.py --image models/small/dump/box_0_src.png   # ddddocr 官方实现参照
```

`scripts/shot_window.py` 可按窗口标题截图，并在无 GUI 会话里用它验收界面渲染。

## 文档

- `docs/PLAN.md` —— 总体方案与里程碑（含视觉规范）
- `docs/PROGRESS.md` —— **现在做到哪里了**（含踩坑记录）
- `docs/ENGINES.md` —— 多引擎（DDDDOCR / YOLO / YOLOE 提示）扩展设计
- `src/lib/endpoints.ts` —— 接口清单的**事实源**（首页概览与接口页同源）
