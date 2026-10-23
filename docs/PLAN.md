# OCR Studio —— Rust + ort + Tauri 桌面 OCR 工具

> 目标：开箱即用的桌面 OCR / 找字 / 找图 一体化工具，带完整本地 HTTP 接口，支持 Tiny/Small/Medium 三档模型切换，GPU 加速（DirectML），界面用 Web 前端，产物为原生 exe、无 Python 运行时。

---

## 1. 机器现状（已确认）

| 项 | 值 |
|---|---|
| 系统 | Windows（待脚本确认具体版本，需 Win10 1903+ 才支持 DirectML） |
| CPU 架构 | AMD64 |
| 显卡 | AMD Radeon(TM) Graphics（集显，**无 NVIDIA**） |

**关键结论**：GPU 加速走 **ONNX Runtime DirectML EP**（`onnxruntime.dll` + `DirectML.dll`），不能用 CUDA。DirectML 在 Win10 1903+ / Win11 上对 AMD 核显可用，性能不如独显 CUDA，但足够加速推理。CPU 版作为兜底。

---

## 2. 技术栈

| 层 | 选型 | 说明 |
|---|---|---|
| 壳 | Tauri 2.x | 产物小、原生窗口、可打包 NSIS/MSI |
| 前端 | **React + TypeScript + Vite + Tailwind CSS + shadcn/ui** | 2026-10-21 由 Vue 3 + shadcn-vue 改为本方案；界面 = 控制面板 + 测试台 |
| 推理 | `ort` crate（onnxruntime 2.x，feature: `directml` + `ndarray`） | 加载 ONNX，GPU/CPU 可切换 |
| 图像 | `image` + `ndarray` + 自研预处理/后处理 | 缩放、归一化、DB 后处理、CTC 解码 |
| 截图/窗口 | `windows` crate（PrintWindow / BitBlt / EnumWindows / GetWindowRect） | 句柄找图、屏幕找图的基础 |
| HTTP | `axum` + `tokio` | 本地服务，接口 1:1 复刻 |
| 配置/日志 | `serde` + `toml` + `tracing` + `tauri-plugin-log` | 配置文件与日志落盘 |
| 打包 | `tauri build`（NSIS）+ 自定义脚本拷贝 ort DLL | 需处理 `onnxruntime.dll`/`DirectML.dll` 随包 |

> 备选：模板匹配如需更强能力可引入 `opencv` crate，但编译链重；优先纯 Rust 自研 NCC 匹配（约 100 行，可控）。

---

## 3. 目录结构（规划）

```
D:/Python测试/OCR-Studio/
├── docs/
│   └── PLAN.md                # 本文档
├── models/
│   ├── tiny/                  # det.onnx / rec.onnx / cls.onnx / keys.txt
│   ├── small/
│   └── medium/
├── src-tauri/                 # Rust 后端
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── config.rs          # 配置加载/保存
│   │   ├── model_manager.rs   # 三档模型加载、懒加载缓存、切换
│   │   ├── ocr/
│   │   │   ├── mod.rs
│   │   │   ├── engine.rs      # 会话构建（DirectML/CPU 探测）
│   │   │   ├── det.rs         # DB 检测后处理
│   │   │   ├── rec.rs         # CTC 解码
│   │   │   ├── cls.rs         # 方向分类
│   │   │   └── preprocess.rs  # resize/normalize/NCHW
│   │   ├── matcher/
│   │   │   └── template.rs    # NCC 模板匹配（精准/容错两种）
│   │   ├── capture/
│   │   │   ├── screen.rs      # BitBlt 全屏/区域截图
│   │   │   └── window.rs      # EnumWindows / PrintWindow / GetWindowRect
│   │   ├── api/
│   │   │   ├── mod.rs
│   │   │   ├── routes.rs      # 全部 /ocr/* 路由
│   │   │   └── dto.rs         # 请求/响应结构
│   │   └── commands.rs        # Tauri invoke 命令（UI 调 Rust）
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── build.rs
├── src/                       # 前端 Vue
│   ├── main.ts
│   ├── App.vue
│   ├── views/
│   │   ├── Dashboard.vue      # 模型选择 / GPU 状态 / 服务启停 / 端口
│   │   ├── Playground.vue     # 拖图识别、找字、找图
│   │   ├── Logs.vue
│   │   └── Settings.vue
│   ├── components/
│   └── stores/
├── scripts/
│   ├── fetch_models.ps1       # 下载/放置模型
│   └── copy_runtime.ps1       # 拷贝 ort + DirectML DLL
├── package.json
└── vite.config.ts
```

> ⚠️ `src/` 部分是**最初规划**（Vue 3）；2026-10-21 已改为 **React + TypeScript**（见 7.1），
> 实际为 `src/pages/*.tsx` + `src/components/*`，导航 = 服务 / 接口 / 试跑 / 日志 / 设置。
> `src-tauri/` 部分与现状基本一致，另新增了 `engine/`（多引擎）、`api/state.rs`、
> `logbuf.rs`、`barcode.rs`。

---

## 4. 推理管线

```
输入图 → (cls 方向分类，可选) → det(DB) → 后处理得框 → 按框裁剪 → rec(CTC) → 文本+坐标
```

- **检测 det**：PP-OCR DB 模型，输出概率图，后处理需复刻 PaddleOCR 的 `DBPostProcess`：二值化阈值、box_thresh、unclip_ratio（膨胀）、最小面积过滤、按框还原到原图坐标、可选的 `use_dilation`。
- **识别 rec**：PP-OCR CRNN/SVTR，输出 CTC logits → argmax → 去 blank/重复 → 按 `keys.txt` 映射字符，支持置信度。
- **找字**：OCR 出全部文本+框，再按关键词/正则过滤，返回命中框坐标。
- **找图**：模板匹配。精准 = 归一化互相关（TM_CCOEFF_NORMED）阈值 0.9~0.99；容错 = 多尺度缩放 + 灰度 + 阈值降低，返回最佳位置。
- **句柄找图**：`EnumWindows` 找句柄 → `PrintWindow`/`BitBlt` 截窗口 → 模板匹配。
- **屏幕找图**：`BitBlt` 截全屏/指定区域 → 模板匹配。

**精度对齐策略**：所有前后处理以 **RapidOCR（Python 版）的实现**为基准写单测，用同一张图对比输出文本与框，误差超阈值即视为 bug。

---

## 5. 模型策略（重要，含风险）

### 5.1 目标
三档：Tiny / Small / Medium，随包分发，界面下拉切换；每档一套 `det/rec` ONNX。

### 5.2 现实约束
- PP-OCRv6 发布于 2026-06，**是否能用 `paddle2onnx` 导出 ONNX 需实测**（阻塞项 R1）。
- 即使暂不能导出 v6，可用 **RapidOCR 已提供的 PP-OCRv4/v5 ONNX 模型**（det/rec/cls + keys）先跑通全链路。
- **代码按“模型目录 + 配置”设计**，v6 ONNX 一旦可用，直接替换 `models/*/` 下的文件，代码零改动。

### 5.3 目录约定（每档）
```
models/<level>/
├── det.onnx
├── rec.onnx
├── cls.onnx        # 可选
├── keys.txt        # 字典
└── meta.json       # {input_size, mean, std, rec_img_h, rec_img_w, ...}
```

---

## 6. HTTP 接口（原产品 12 条 + 后续扩展，现共 19 条）

> **事实源**：`src/lib/endpoints.ts`（首页概览与「接口」页同源）与
> `src-tauri/src/api/routes.rs` 的 `PATHS`。改路由必须同步这两处。
> 界面上的端点数取 `ENDPOINT_PATH_COUNT`（按路径去重 —— `/vision/promptref`
> 三种方法在 `endpoints.ts` 里拆 3 条讲，但只能算 1 个端点）。

| 方法 | 路径 | 入参 | 出参 |
|---|---|---|---|
| POST | `/ocr/shibie` | image(base64/文件) | text, boxes, scores |
| POST | `/ocr/zhaozi` | image + 关键词/正则 | 命中框列表 |
| POST | `/ocr/duohangjson` | image | 多行 [{text, box, score}] |
| POST | `/ocr/danzijson` | image | 单字 [{char, box, score}] |
| POST | `/ocr/quyuocr` | image + region | 只认矩形内的字，框坐标已平移回原图 |
| POST | `/ocr/tuzhaotu1` | screen + template | {found, x, y, score} 精准 |
| POST | `/ocr/tuzhaotu2` | screen + template | 同上 容错 |
| POST | `/ocr/jubingzhaotu1` | hwnd + template | 同上 精准 |
| POST | `/ocr/jubingzhaotu2` | hwnd + template | 同上 容错 |
| POST | `/ocr/pingmuzhaotu1` | template(+region) | 同上 精准 |
| POST | `/ocr/pingmuzhaotu2` | template(+region) | 同上 容错 |
| POST | `/ocr/yuyizhaotu` | source + 检测模型 | 语义找图，出参与模板找图同构 |
| POST | `/vision/prompt` | mode + 参考图/提示词 | 提示找图，出参与模板找图同构 |
| GET/POST/DELETE | `/vision/promptref` | 中文参考库：name+image+box / 列表 / 删除 | 教一次按中文名找，走 VPE 通道 |
| POST | `/vision/detect` | image + engine:yolo | 目标检测 |
| POST | `/vision/pipeline` | image + detect_engine | 检测 + 识别串联 |
| POST | `/vision/qrcode` | image | 二维码 / 条形码 |
| GET | `/ocr/health` | — | {status, model, device} |
| GET | `/engine/list` | — | 可用引擎与能力 |

约定：图像支持 `multipart/form-data` 或 `application/json` 的 base64（默认 base64 兼容脚本党习惯）；
请求体上限 32MB（`DefaultBodyLimit`，见 `routes.rs`）。

---

## 7. UI 设计

产品是 **API 工具**，界面只是它的控制面板。导航固定 5 项（原方案 4 项，已加「接口」页）：

- **服务**：服务状态（运行中/已停止）、地址复制、启停、能力概览、接入代码。
- **接口**：19 个端点的完整文档（参数 / 返回 / 错误码 / 可复制示例），数据来自 `endpoints.ts`。
- **试跑**：拖入/粘贴图片 → 识别 / 找图 / 语义找图 / 提示找图 / 扫码；左图右结果，框可叠加。
- **日志**：实时日志（tracing → 前端事件）。
- **设置**：端口、默认档位、设备、检测模型管理、中文参考库、**AI 控制**（开关 / 连接码 / 一键复制提示词）、开机自启、CORS。

### 7.0 AI 操控接口（`/ai/v1/*`，2026-10-23 新增）

给**本机 AI 助手**用的两个端点，复用 8848，默认关闭：

| 端点 | 作用 |
|---|---|
| `GET /ai/v1/tools` | 工具清单：name / summary / risk（readonly/write/dangerous）/ 参数 schema |
| `POST /ai/v1/tool` | 调工具：`{tool, args, request_id}` → `{ok, summary, data, error}` |

要点（与 WPE Lab 参考项目同款设计）：Bearer 连接码常数时间比较；`request_id`
幂等回放（保留 200 条 + inflight 占位）；120 次/分钟滑动窗口限流；写操作记审计
日志；工具名/参数错误给**可读的下一步**（AI 靠它自我修正，含最接近的候选工具名）。
HTTP 层在 AppState 持 `ai_enabled`/`ai_token`（config.json），开关关闭时 403 并指路。
**不进 PATHS**：那张表是用户脚本的接口清单，AI 入口不该混进去。
明确不提供给 AI：退出程序、改开机自启、改 AI 开关与连接码。

### 中文目标：只留「按名字找」（2026-10-23 晚，用户拍板彻底移除文本提示）

历程（三次迭代，最终结论是「砍掉」）：

1. 词表映射：MobileCLIP2-B 是英文语义空间，中文原样喂会被 BPE 拆成字节 token
   （实测 19 个常用词只有「人」偶然命中）→ 加 `zh_words` 中英词典。
2. PE 预烘焙：砍掉 242MB 文本塔后，词表内的词改为离线查表（`text_pe.bin`，482KB）。
3. **整条移除**（当前）：用户实测后判断「语言提示找图太特定，没用，还占位置」。
   数据支持这个判断——真实界面截图上 12 个词最高只有 0.207，YOLOE 对 UI 细粒度
   概念本就不可用。为它维护的链路：242MB 塔 + CLIP 词表 + 提示头 onnx + 中英词典 +
   烘焙工具 + 2 项 parity 用例，全部只为一个噪声级功能服务。

移除范围：
- `engine/zh_words.rs`、`engine/text_pe_cache.rs`、`src/bin/bake_text_pe.rs` 删除
- `prompt.rs` 只保留 `visual_pe` + `pe_for_refs`；`PromptAssets` 只剩检测器 + VPE
- Cargo 依赖 `mobileclip2-b-rs` 移除（构建时间也随之下降）
- API：`/vision/prompt` 的 `mode` 只剩 `visual` / `ref`；传 `text` 得到**明确指路**
  400（说清「已移除 + 先教一次 + 用 ref」），不是静默失效
- AI 工具 `find_word` → `find_target`（只认教过的名字，未命中给出「先 teach_target」
  与现有名字列表）
- 打包：`*_texthead.onnx` / tokenizer / `text_pe.bin` / `zh_words.json` 一并剔掉

**保留且更重要的那条**：`engine/prompt_refs.rs` 中文参考库——框一次起中文名 → 走 VPE
视觉通道（比的是长得像不像）。任意中文名都能用，不需要词表，也不需要模型认识它。
`/vision/prompt` 用 `mode=ref` 引用这些名字。

### 中文场景精简包（已并入上一条）

**PP-OCR 三档齐了**（用户指出「现在只有一种」）：`scripts/models.manifest.json`
本就定义了 tiny/small/medium（RapidOCR v3.10.0，PP-OCRv6 官方三档），此前只落了
small。已补下 tiny（6.3MB）与 medium（138.6MB），SHA256 全过，三档随包带。

### 7.1 视觉规范（2026-10-21 确定）

| 项 | 规范 |
|---|---|
| 框架 | React + TypeScript（Vite，Tauri 2 壳不变） |
| 样式 | Tailwind CSS |
| 组件库 | shadcn/ui |
| 字体 | **MiSans**（小米字体，`@font-face` 本地引入并设为全局默认；商用授权需确认后再随包分发） |
| 配色 | **纯白为主，黑色为辅，蓝色点缀** |
| 按钮 | **每个按钮都必须带图标**（图标库用 `lucide-react`，shadcn/ui 默认图标源） |
| 标题栏 | **自定义标题栏：不要图标、不要标题**，仅窗口控制按钮 |

自定义标题栏落地：`tauri.conf.json` 的 `app.windows[0]` 设 `"decorations": false`，
前端用 `getCurrentWindow().minimize()/toggleMaximize()/close()` 自绘。

> 原方案为 Vue 3 + shadcn-vue，`src/` 下现有 Vue 骨架需整体替换。

### 7.2 圆角与字号梯度（2026-10-23 统一，**改界面前必读**）

规范落在 `src/styles.css` 的 `@theme` 里，做法是**直接覆盖 Tailwind 的默认档位**
（`--radius-sm/md/lg/xl`、`--text-xs/sm`），所以 `rounded-md/lg/xl`、`text-sm` 这些
既有类名会自动等于下表中的值，不需要逐处改。**禁止**再写 `rounded-[Npx]` 与
`text-[N.x px]`（半像素值在 100% 缩放下发虚）。

圆角四档（规则：容器越大圆角越大；子元素比父元素小一档）：

| 类名 | 值 | 用途 |
|---|---|---|
| `rounded-sm` | 6px | 浮层里的小项（下拉选项）、行内代码、微标签 |
| `rounded-md` | 8px | 按钮 / 输入框 / 下拉触发器 / tab / 导航项 / 窗口按钮 |
| `rounded-lg` | 10px | 卡片内部的块：分段控件容器、滚动区、提示条、搜索框 |
| `rounded-xl` | 14px | 卡片 / 面板 / 独立浮层 / 大块容器 |

另有刻意不走梯度的两档：`rounded-full`（胶囊 / 圆点 / 头像）、`rounded`=4px
（≤24px 的微图标按钮、浮层上的微角标、截图上的框标记）。

字号七档（只留整数）：10 微角标 · 11 次要说明·表头 · 12 正文 · 13 强调正文·卡片标题·按钮 ·
14 强强调 · 16 小节标题 · 26 页面主标题·大数字。

按钮高度四档（`src/components/ui/button.tsx`），另有 `icon-sm h-8 w-8`、`icon-xs h-7 w-7`；
**图标统一 14px**（调用方写 `className="h-3.5 w-3.5"`）：

| size | 高 | 字号 | 用途 |
|---|---|---|---|
| `xs` | h-7 (28px) | 12px | 密集行内动作（工具条、卡片角上的小按钮） |
| `sm` | h-8 (32px) | 12px | 次级按钮（表单旁、卡片头） |
| `default` | h-9 (36px) | 13px | 主按钮 |
| `lg` | h-10 (40px) | 13px | 强调按钮（落地页 / 空状态） |

手写（不走 `<Button>`）的控件也必须落在同样的档位上：h-7 / h-8 的控件 → `rounded-md` +
`text-[12px]`；分段控件用「容器 `rounded-lg` + 项 `rounded-md`」（8+2 内距 = 10，正好同心）。
另外 **CardHeader 的第一行统一包一层 `flex h-8 items-center`** —— 左右并排的两张卡，
一边标题行里塞了按钮/徽章而另一边没塞，`items-center` 会让两类标题差 ~7px。

---

## 8. 里程碑

| 阶段 | 内容 | 预计 |
|---|---|---|
| M0 | 环境确认、Tauri 骨架跑起来、模型文件就位 | 0.5–1 天 |
| M1 | 推理引擎（ort 加载 det/rec）+ DB/CTC 后处理 + `/ocr/shibie`（CPU）跑通 | 2–3 天 |
| M2 | 找字/多行/单字 + 模板匹配 + 截图 + 句柄接口 | 2–3 天 |
| M3 | DirectML GPU 探测与降级、三档模型切换、全部接口联调 | 1–2 天 |
| M4 | UI 打磨、日志、配置、NSIS 打包 + DLL 随包 | 2–3 天 |

---

## 9. 风险与对策

| 编号 | 风险 | 对策 |
|---|---|---|
| R1 | PP-OCRv6 无法用 paddle2onnx 导出 ONNX | 先用 RapidOCR v4/v5 ONNX 跑通；持续跟踪 v6 导出，接口/目录不变 |
| R2 | ort DirectML 在 AMD 核显上的兼容/性能 | M1 先 CPU 跑通，M3 实测 DirectML；失败自动降级 CPU 并提示 |
| R3 | 前后处理与 Paddle 精度不一致 | 以 RapidOCR Python 实现为基准，固定测试图对比 |
| R4 | PrintWindow 对部分独占全屏/DirectX 程序截黑 | 文档说明限制；提供 BitBlt 全屏兜底 |
| R5 | 打包遗漏 onnxruntime.dll / DirectML.dll | 构建脚本统一拷贝 + 安装目录校验 + 启动自检报错 |
| R6 | 模板匹配性能（大图×多尺度） | 先降采样粗匹配再原图精修；限制最大尺寸 |

---

## 10. 下一步（本轮之后）

1. 确认 Windows 版本与 Rust/Node 工具链（脚本已查）。
2. 初始化 Tauri 2 + Vue 3 + TS 骨架（`npm create tauri-app` 或手动）。
3. 放置第一套模型（先 RapidOCR v5 ONNX）到 `models/small/`。
4. 写 `ocr/engine.rs` 最小加载 + 用一张测试图验证 det/rec 输出。
