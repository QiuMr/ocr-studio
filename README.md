# OCR Studio

开箱即用的本地 OCR / 找字 / 找图桌面工具。**完全离线**，不联网、不上传图片；
自带 HTTP 接口，方便易语言、按键精灵、Python 等外部脚本调用。

- 技术栈：Tauri 2 + React 19 + TypeScript + Tailwind v4；推理用 ONNX Runtime（ort）
- 检测/识别模型：PP-OCR 三档（tiny / small / medium），GPU 走 DirectML
- 找图：零均值 NCC 模板匹配，灰度金字塔加速，1080p 全屏找 64px 模板约 60ms

## 快速开始

```powershell
pnpm install
powershell -File scripts\fetch_models.ps1            # PP-OCR 三档模型（默认只下 small）
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
> `models/ocr/ddddocr/` 占 54MB、`models/detect/yolo/` 占约 19MB，只用 PP-OCR 的话
> 可以整目录删掉，引擎列表里会显示「未安装」，其余功能不受影响。

### 前端改动不生效？

`generate_context!` 在**编译期**把 `dist` 嵌进 exe。改完前端只跑 `pnpm build`
是不够的，必须重新 `cargo build`，否则 exe 里还是旧界面。

## 界面

| 页面 | 作用 |
|---|---|
| 仪表盘 | 引擎状态、模型档位、推理设备、本地 HTTP 服务开关 |
| 测试台 | 拖拽/粘贴图片 → 文字识别（文本 / 逐行 / 逐字 / JSON）或找图定位 |
| 运行日志 | 后端 tracing 实时日志，按级别过滤、搜索、暂停、复制 |
| 设置 | 端口、默认档位/设备、找图阈值、渲染兼容模式 |

侧边栏可收起成图标栏（按钮在标题栏左上角，状态会记住）。

## HTTP 接口

服务只监听 `127.0.0.1`，默认端口 8848（可在设置里改）。

| 端点 | 说明 |
|---|---|
| `GET /ocr/health` | 引擎状态 |
| `POST /ocr/shibie` | 整图识别，返回拼接文本 + 框 + 分数 |
| `POST /ocr/zhaozi` | 按关键词（或 `/正则/`）筛选命中行 |
| `POST /ocr/duohangjson` | 多行结构 |
| `POST /ocr/danzijson` | **逐字**：每行每个字的四点框 + 置信度 + CTC 帧区间 |
| `POST /ocr/tuzhaotu1\|2` | 在**上传的图**里找图（1=精准 ≥0.92，2=容错 ≥0.72） |
| `POST /ocr/jubingzhaotu1\|2` | 截**指定窗口**找图 |
| `POST /ocr/pingmuzhaotu1\|2` | 截**整个屏幕**找图 |
| `POST /ocr/yuyizhaotu` | **语义找图**：不用模板图，让检测模型直接找目标（`source` 选 image/screen/window）。出参与模板找图**完全同构**，脚本换个路径就能用 |
| `POST /vision/detect` | **通用目标检测**（`engine: "yolo"`，返回类别框 + 置信度；OCR 引擎调它会得到 code 1002） |
| `POST /vision/pipeline` | **检测 + 识别串联**：检测引擎找区域 → PP-OCR 框内认字（`detect_engine` 缺省 `yolo`） |
| `GET /engine/list` | 列出本机可用引擎与能力（只扫目录、不加载模型） |

请求体支持 JSON（`{"image": "<base64>"}`）与 multipart（文件字段 `image`）。

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

YOLOE-26 是开放词汇检测模型，提示词在**导出 ONNX 时就烘焙进权重**，导出后改不了类别：

```powershell
python scripts\fetch_yoloe.py --image assets\bus.png --box 30,230,800,776 --name bus
# 产出 models/detect/yolo/yoloe26_bus.onnx，之后 /ocr/yuyizhaotu 里 engine 传 yolo:yoloe26_bus
```

本机走的是**视觉提示**（给参考图 + 框），因为文本提示要 ultralytics 的 CLIP fork，
而那要拉 git 子模块、本机 MSYS 的 `sh` fork 是坏的。详见 docs/PROGRESS.md §7.8。

## 验证

```powershell
cargo test                    # 92 个单元测试
cargo test -- --ignored       # 22 个重量级：精度基线比对、真实截图、逐字对齐、ddddocr/yolo 跨语言对齐
cargo test --test yoloe_smoke -- --ignored   # 语义找图冒烟（目录里没有检测模型会自动跳过）
python scripts/e2e_find.py    # 真实桌面找图端到端
python scripts/e2e_danzi.py <图片>   # 逐字框端到端断言
python scripts/ref_yolo.py models/detect/yolo/model.onnx assets/bus.png out.json   # yolo 官方实现参照
python scripts/ref_ddddocr.py --image models/small/dump/box_0_src.png   # ddddocr 官方实现参照
```

`scripts/shot_window.py` 可按窗口标题截图，并在无 GUI 会话里用它验收界面渲染。

## 文档

- `docs/PLAN.md` —— 总体方案与里程碑
- `docs/PROGRESS.md` —— **现在做到哪里了**（含踩坑记录）
- `docs/ENGINES.md` —— 多引擎（DDDDOCR / YOLO）扩展设计
- `docs/DECISIONS.md` —— 关键技术决策与实测依据
