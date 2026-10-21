# OCR Studio

开箱即用的本地 OCR / 找字 / 找图桌面工具。**完全离线**，不联网、不上传图片；
自带 HTTP 接口，方便易语言、按键精灵、Python 等外部脚本调用。

- 技术栈：Tauri 2 + React 19 + TypeScript + Tailwind v4；推理用 ONNX Runtime（ort）
- 检测/识别模型：PP-OCR 三档（tiny / small / medium），GPU 走 DirectML
- 找图：零均值 NCC 模板匹配，灰度金字塔加速，1080p 全屏找 64px 模板约 60ms

## 快速开始

```powershell
pnpm install
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

请求体支持 JSON（`{"image": "<base64>"}`）与 multipart（文件字段 `image`）。

## 验证

```powershell
cargo test                    # 40 个单元测试
cargo test -- --ignored       # 16 个重量级：精度基线比对、真实截图、逐字对齐
python scripts/e2e_find.py    # 真实桌面找图端到端
python scripts/e2e_danzi.py <图片>   # 逐字框端到端断言
```

`scripts/shot_window.py` 可按窗口标题截图，并在无 GUI 会话里用它验收界面渲染。

## 文档

- `docs/PLAN.md` —— 总体方案与里程碑
- `docs/PROGRESS.md` —— **现在做到哪里了**（含踩坑记录）
- `docs/ENGINES.md` —— 多引擎（DDDDOCR / YOLO）扩展设计
- `docs/DECISIONS.md` —— 关键技术决策与实测依据
