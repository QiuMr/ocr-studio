# OCR Studio 项目进度记录

> 本文档记录「现在做到哪里了」。每次有实质进展后更新，旧结论不删除、只标注状态。
> 最后更新：2026-10-22（检测模型可自定义 + 类名从模型读 + 外壳改浅灰）

---

## 0. 一句话现状

**M3 全部完成**（抽象层 / DDDDOCR / 目标检测 / pipeline 串联），此后是用户反馈驱动的
几轮改造。**本轮（10-22 晚）：检测模型变成「用户自己的」**——可以放多个、可以指向任意
路径（不复制文件）；**类名自动从 ONNX metadata 读**，`labels.txt` 退化为兜底；
**标题栏与左侧栏改成通用浅灰**（外壳浅灰 / 内容纯白）。
82 单测 + 22 重量级全绿，PP-OCR 精度**零变化**，`yolo_parity` 仍 8/8 IoU=1.0000。

上一轮完成：目标检测去版本化（结构按 ONNX 形状探测）、标题栏 ACL 权限修复、
侧栏去蓝、全局垂直对齐、DirectML 假象纠正。

M2 收口内容（找图 7 端点、单字 JSON、实时日志页、界面精修）、M3.0 抽象层、
M3.1 DDDDOCR、M3.2 目标检测、M3.3 pipeline 串联均已完成。

**分发方式改为免安装 zip**（用户明确要求：不再出 MSI/NSIS）：
`tauri.conf.json` 设 `bundle.active = false`，改用 `scripts/pack_exe.ps1` 打
`exe + runtime/*.dll + models/ + 使用说明.txt` 的压缩包。**单独拷 exe 是跑不起来的**
（缺 onnxruntime.dll 会静默退回 CPU，缺 DirectML.dll 直接起不来），这一点已写进分发说明。
不需要的模型可删目录减体积：`models/ocr/ddddocr/`（54MB）、`models/detect/yolo/`（约 19MB）。

已完成里程碑：M0（环境基线）、M1（推理核心与 Python 逐位对齐 + HTTP/命令/前端全套）、
M2（找图 + 单字 JSON + 日志页 + 界面精修）、M3.0（多引擎抽象）、M3.1（DDDDOCR）、
M3.2（目标检测）、M3.3（pipeline 串联）、M4 界面反馈修复。

### 0.0 本轮（检测模型自定义 + 外壳浅灰）做了什么

| 项 | 结果 |
|---|---|
| 多模型 | `catalog::list_yolo_models(root, external)` 扫 `detect/yolo/` 下**所有** `*.onnx`（含一层子目录，兼容旧的 `v8/`），每个文件一条引擎；`id` = 文件名（撞名退到「父目录/文件名」再撞加序号）。**列表第一个 = 默认模型**，它的 spec 是不带 id 的 `yolo`；其余是 `yolo:<id>` |
| 任意路径 | `config.json` 增 `detect_models: Vec<String>`；设置页「添加模型」用 Win32 **`GetOpenFileNameW`** 弹系统选择框（不为一个场景引 tauri-plugin-dialog），选中只记路径**不复制文件**；`remove_detect_model` 只删记录不动磁盘。外部文件被挪走只 warn 跳过 |
| 类名从模型读 | `yolo::ModelMeta::read()` 读 metadata 的 `names`（解析 Python dict repr，兼容 JSON 风格 / 转义 / 乱序 / 带数字的类名）、`task`、`imgsz`、`description`。优先级 **metadata > labels.txt > `class_{id}`** |
| 明确拒绝非检测模型 | `task != detect`（segment/pose/obb/classify）直接报错并说明原因：这些模型的输出多 mask 系数 / 关键点通道，按检测解码不会崩、只会给出错类别的框 |
| 类名回填 | `/engine/list` 扫描阶段不加载模型 → 读不到类名。新增 `AppState::engine_status_list()`（HTTP 与 IPC **共用**），在引擎加载过后把 classes 回填进列表，并把 note 里「类名在加载时读取」换成「已加载 N 类」 |
| 引擎名回显 | `/vision/detect` 出参的 `engine` 改成**规范化后的 key**（传 `yolo:v8` 也回 `yolo`，不再让人以为版本号还有意义） |
| 外壳配色 | `styles.css` 增 `--color-chrome: #f7f7f8`；标题栏与左侧栏用它，主区保持纯白——靠**底色差**分层不加线（VS Code / Notion 的做法）。侧栏选中态从「浅灰块」改成**纯白浮起**（+ 极淡描边 + 微阴影），hover 用黑 4~7% 叠加；标题栏按钮 hover 同步改黑叠加 |
| 前端 | 设置页新增「目标检测模型」卡片（模型列表 + 默认标记 + 来源 + 类别数 + 路径 + 添加/移除/打开目录）；测试台引擎下拉显示「目标检测（默认模型）」/「目标检测 · <模型名>」，并把上次选中的引擎记进 localStorage |
| 打包 | `pack_exe.ps1` 拷贝 models 后清掉 `*.pt` / `ref_*.json` / `*.bak`（`.pt` 是训练权重，运行时只要 onnx；`ref_*.json` 只是开发期比对参考） |
| 验收 | 把 `labels.txt` 整个移走后重跑：类名仍是 `person / bus`（证明来自 metadata）；`model.onnx`+`plate.onnx` 并存 → 下拉两条、各自可加载；UI 实测「5 目标 · 876ms」，设置页显示「80 类」；`cargo test --lib` 82 通过、`--ignored` 22 项全绿（`yolo_parity` 8/8 IoU=1.0000 conf 差=0） |

待办：`api_smoke` 这次被沙箱规则拦住写入 exe（既存环境问题，不是代码问题），下次换目标目录再跑；
v5/v7 onnx 到手后跑 parity；C 盘长期只剩 1~2GB。

### 0.0.1 上轮（用户反馈四项）做了什么

| 项 | 结果 |
|---|---|
| 目标检测去版本化 | 删 `YoloVersion`；`EngineKind::Yolo` 无参数，`key()` = `yolo`（别名 `detect`）；加载时读 ONNX `inputs[0].dims` / `outputs[0].dims` + labels 类别数，探测出 `YoloLayout { 布局, 有无 objectness, 输入边长, 候选数, 通道数 }`，`decode()` 直接吃它。覆盖 v5/v6/v7/v8/v10/v11/v12/v26 这类单输出 head |
| 目录扁平化 | 约定 `models/detect/yolo/{model.onnx,labels.txt}`；旧的 `detect/yolo/v8/` 自动下探一层仍能扫到。实盘已把模型搬到扁平目录 |
| 明确拒绝 | v3/v4 多输出 head、自带 NMS 的端到端导出（`[1,300,6]`）→ 返回可读的 `NotReady`，不给垃圾框 |
| 窗口按钮/拖动（真 bug） | 根因：`core:default` 的 window 权限只有只读命令，**没有** `allow-start-dragging` / `allow-minimize` / `allow-toggle-maximize` / `allow-close`，前端 `catch` 把 rejection 吞了 → 「点了没反应」。已在 `capabilities/default.json` 补齐；标题栏中段占位 div 补 `data-tauri-drag-region`（Tauri 只认事件目标自身）；最大化时图标切「还原」 |
| 侧栏 | 删除左侧选中竖条；选中态 `bg-neutral-100 + text-neutral-900`，去掉全部 blue；底部版本圆点改中性色 |
| 垂直对齐 | 全站卡片头第一行统一 `h-8 items-center`（原来「有按钮的卡片标题比没按钮的低 7px」）；PageHeader 改 `items-center`；工具条 Label 统一 `leading-none`；测试台左右卡片标题基线对齐 |
| DirectML 假象 | `ort_util::directml_available()` 先查 `ep::directml::api()`，不可用直接走 CPU 并如实上报；`runtime/onnxruntime.dll` 从 1.31.0 **CPU** 构建换成 1.24.3 **DirectML** 构建（20.1MB）。日志里 4 条 `DirectML is not supported` 消失 |
| 验收 | `cargo test --lib` 73 通过；`--ignored` 22 项全绿（含 pipeline_match 逐字全对、yolo_parity 8/8 IoU=1.0000 conf 差=0）；拖动/最大化/还原/最小化/关闭五项实测通过；HTTP `/vision/detect` 实测 5 框，旧写法 `yolo:v8` 兼容 |

### 0.0.2 再上轮（M3.3 pipeline 串联）做了什么

| 项 | 结果 |
|---|---|
| 模型 | `scripts/fetch_yolo.ps1`：官方 yolov8n.pt → ultralytics 导出 ONNX（opset12/640/simplify）→ 重命名 model.onnx，**labels.txt 从权重元数据同源生成**（80 类），已实测跑通 |
| 引擎实现 | 新增 `src-tauri/src/engine/yolo.rs`：letterbox（114 填充、`round_ties_even` 复刻 Python `round()`、缩放复用 cv2 逐位复刻的 `resize_bilinear`）、RGB `/255`、v8 `[1,4+C,N]` 转置解码（v5/v7 的 `[1,N,5+C]` obj×cls 路径也写了，**未实测——缺模型**）、class-aware 贪心 NMS（conf 0.25/iou 0.7/max_det 300，与 ultralytics 默认一致） |
| 接口 | 新增 `POST /vision/detect`（`conf`/`iou`/`class_filter` 可选参数）与 Tauri `vision_detect` 命令；`OcrOptions` 增检测三字段；错误码落地 ENGINES.md 4.3：**1002 引擎不支持该能力 / 1003 未知引擎**（实测：PP-OCR 打 detect 得 1002，`engine=yolo` 缺版本得 1003） |
| 前端 | 测试台按能力切换形态：选检测引擎后「开始识别」走检测，右侧是「检测结果」列表（类别 id 徽章按类别着色 + 置信度 + 坐标），图上框文字显示「类别 + 置信度%」，方向分类开关隐藏；api 层新增 `detectImage()`（Tauri/HTTP 双通道） |
| 对齐验证 | 新增 `tests/yolo_parity.rs` + `scripts/ref_yolo.py`（ultralytics 跑同一份 onnx 出参考 JSON）。**关键结论：测试图必须用 PNG**——JPEG 各家解码器有 ±1 的 IDCT 抖动，会把置信度推动 0.001~0.01、大框 IoU 压到 0.97，全是解码器噪声；换 PNG 后 IoU/conf 全部归零，证明我们的链路与 ultralytics 逐位一致 |
| UI 验收 | 无人值守实测：贴 bus.png → 选 YOLO → 开始识别，「5 目标 · 713ms」，5 框与参考 JSON 完全一致 |

### 0.0.1 上轮（M3.1 DDDDOCR）做了什么

| 项 | 结果 |
|---|---|
| 模型与字典 | 新增 `scripts/fetch_ddddocr.ps1`：从 PyPI sdist 取 `common.onnx`/`common_old.onnx`，**同源生成 charset.txt**（8210 项，第 0 项是 blank 空串），已实测跑通 |
| 引擎实现 | 新增 `src-tauri/src/engine/ddddocr.rs`：灰度 64 高 + LANCZOS + `/255`、单通道 `[1,1,64,W]`、logits 解码（blank 在字典 0 位）、逐字框复用 CTC 帧区间 |
| 行检测复用 | 新增 `src-tauri/src/ocr/detector.rs::LineDetector`：只加载 det 不加载 rec，ddddocr 借它找行（它自带的 det 是 YOLOX 目标检测，切文档很差） |
| ORT 公共层 | 抽出 `src-tauri/src/ocr/ort_util.rs`（环境单例 / 建会话 / 预热降级），PP-OCR 只是**原样搬家**，行为不变 |
| 引擎工厂 | `engine::load_engine(spec, …)`：`ppocr:*` 走老路、`ddddocr:*` 走新引擎、`yolo:*` 明确报「未实现」而不是返回空壳 |
| 接口 | `OcrOptions` 增 `engine` 字段（JSON 与 multipart 都认）；`/ocr/{shibie,zhaozi,duohangjson,danzijson}` 统一走 `recognize_lines`/`recognize_details`；`ocr_recognize`/`ocr_danzijson` 命令增 `engine` 参数 |
| 状态 | `AppState` 增额外引擎单槽（`alt`），切换引擎丢弃上一个；`/ocr/health` 在 PP-OCR 未加载时会报额外引擎状态，不再误导成「引擎未加载」；`/engine/list` 回填额外引擎的真实设备 |
| 能力声明修正 | ddddocr 由「无 CTC、字框只能等分估计」改成真实情况：**有 CTC 帧区间，逐字框是真映射**；可用性不再要求自带 det |
| 前端 | 测试台新增引擎下拉（只列已可用的非 PP-OCR 引擎，带「验证码快」这类人话标签），换引擎即清空旧结果 |
| 测试 | 新增 `tests/ddddocr_parity.rs`：7 张裁剪行图与官方 Python 实现**逐字完全一致**；整图 det+rec 链路跑通（找得到 `8848`/`13800`） |

### 0.1 上上轮（M3.0 多引擎抽象）做了什么

| 项 | 结果 |
|---|---|
| 抽象层 | 新增 `src-tauri/src/engine/{mod.rs,catalog.rs}`：`EngineKind`/`EngineSpec`/`Capabilities`/`TextLine`/`Detection`/`VisionEngine`/`VisionError`（`YoloVersion` 已于 10-22 移除，见 6.4） |
| 首个实现 | `impl VisionEngine for OcrEngine`——只做「包一层」，`recognize_detailed` 内部一字节没动，这是精度零变化的结构性保证 |
| 命名避让 | trait 用 `infer`/`status` 而非 `recognize`/`health`，避免「具体类型走 inherent、`dyn` 走 trait」的双份行为（详见 ENGINES.md 2.1） |
| 目录发现 | `catalog::discover()` 只读文件系统、不加载模型；9 个候选（ppocr×3 + ddddocr×2 + yolo×4），缺文件时明说缺哪个 |
| 新旧路径 | `ppocr_dir` 新路径 `ocr/ppocr/<lvl>/` 优先，回退旧路径 `models/<lvl>/`，老用户模型不用搬家 |
| 接口 | 新增 `GET /engine/list` 与 `list_engines` 命令（同源逻辑），已加载实例会回填 `actual_device` |
| YOLO 坑位固化 | 早期用 `YoloVersion` 枚举把「有无 obj / 填充值」写进类型；10-22 改为加载时按 ONNX 形状探测（`YoloLayout`），版本枚举已删除 |
| 前端 | 设置页新增「识别引擎」卡片：名字 + 可用/未安装 + GPU/CPU + 能力标签 + 缺失文件路径 |
| 测试 | 新增 8 个用例（spec 解析/歧义拒绝/key 往返/YOLO 版本语义×4 + 目录发现×4）；`/engine/list` 冒烟测试不需模型即可跑 |

### 0.1 本轮之前（界面精修 + M2 收口）做了什么

| 项 | 结果 |
|---|---|
| 去分割线 | 标题栏底部、侧边栏右侧两条 1px 实线全部去掉，层级改用留白 + 底色表达 |
| 侧边栏折叠 | 196px ↔ 60px 图标栏，折叠按钮在**标题栏左上角**，状态存 localStorage |
| 全局手势 | 根因是 Tailwind v4 preflight 把 `button` 的 cursor 重置成 `default`，在 base 层统一纠正（含 disabled=not-allowed、hover/active/focus 三态） |
| 小白引导 | 新增 `PageHeader`/`Hint`/`EmptyState`/`Callout`，每页一句话说明 + 三步空状态 + 术语问号 |
| 拖拽上传 | `useImageDrop` 同时支持 Tauri 原生拖放（`onDragDropEvent`，DOM 收不到）与浏览器 HTML5 |
| 图片交互 | Ctrl+滚轮缩放、双击复位、悬浮缩放控件 |
| 日志页 | tracing Layer → 环形缓冲(2000) + broadcast → 攒批 emit `log-lines`；前端级别过滤/搜索/暂停/清屏/复制 |
| 单字 JSON | `ctc_decode_spans` 给帧区间 → 按比例切行宽 → `WarpMap::to_source` 投回原图；cls 翻转时 x 取反 |
| 免安装分发 | `bundle.active=false` + `scripts/pack_exe.ps1`（自动带 DLL/models/说明） |

### 0.1 白屏问题的完整结论（2026-10-21 深夜，排了三小时）

现象：双击 exe，窗口一片白，什么也没有；偶尔弹出
`msedgewebview2 has stopped working / Error launching CrashSender.exe`。

查证顺序与结论：

1. 前端本身没问题——同一份 `dist` 用本机 Edge 无头加载，仪表盘渲染完整。
2. App 进程下 **0 个 WebView2 子进程**，窗口是「有窗无视图」。
3. Chromium 日志（需从代码里传 `--enable-logging`，wry 会覆盖
   `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`）暴露两条真相：
   - `FATAL:gpu_data_manager_impl_private.cc:408] GPU process isn't usable. Goodbye.`
     —— GPU 进程启动即崩（`exit_code=7`），Chromium 判定 GPU 不可用后**终止浏览器进程**；
   - 窗口实际在访问 `http://localhost:1420`（`ERR_CONNECTION_REFUSED`）。
4. **根因**：`tauri = { features = [] }` 少了 `custom-protocol`。
   这个 feature 决定 `generate_context!` 是否把 `frontendDist` 嵌进二进制，
   不开的话**连 `--release` 也按开发模式加载 `build.devUrl`**。
   官方 `pnpm tauri build` 会自动带上，裸 `cargo build` 不会 —— 属于典型陷阱。
5. GPU 崩溃与我们的代码无关：同一 exe 由资源管理器启动（干净环境）时 GPU 正常、

   界面正常；在本项目的 agent shell 里启动才必崩（该 shell 环境会让 WebView2 的
   GPU 子进程起不来）。仍加了自动兜底，见下。

修复内容：

| 改动 | 说明 |
|---|---|
| `Cargo.toml` | `tauri` 打开 `custom-protocol`（否则 exe 永远白屏） |
| `lib.rs` 主窗口改为运行时创建 | 只有运行时建窗才能从代码里传 `additional_browser_args`；`tauri.conf.json` 的 `windows` 留空 |
| 白屏看门狗 | 前端挂载后 `invoke("ui_ready")`；后端 10 秒没收到就判定 WebView 已死，销毁并用 `--disable-gpu-compositing` 重建，并写 `webview_compat=true` 到配置，下次直接走兼容模式 |
| 建窗失败重试 | 建窗直接报错时先用兼容参数重试一次，避免「没有窗口 → 事件循环立刻退出」 |
| 文件日志 | release 是 GUI 子系统没有控制台，现在日志落 `<数据目录>/ocr-studio.log`（超 8MB 自动重置），白屏类问题有据可查 |
| 诊断开关 | `OCR_NO_CORE=1` 跳过引擎+HTTP；`OCR_WEBVIEW_ARGS=...` 直接指定 WebView2 参数 |

配套脚本（都可复跑）：`scripts/shot_window.py`（按窗口标题截图并统计非白/暗像素比例，
用于无 GUI 会话时肉眼验收）、`scripts/cdp_probe.py`（WebView2 远程调试端口探页面状态）、
`scripts/minidump_info.py`（零依赖解析 Crashpad 转储，定位崩溃模块）、
`scripts/check_embedded_assets.py`（查 exe 是否嵌入前端资源）。

实测：由资源管理器启动 `D:\rust-target\release\ocr-studio.exe`，
窗口 1116×729，仪表盘正常显示「引擎状态 运行中 / DirectML(GPU) / 字典 18708 / 端口 8848」。

打包产物 ~~MSI / NSIS~~ —— **2026-10-21 起不再产出安装包**（用户要求直接 exe 分发）：

- `tauri.conf.json`：`bundle.active = false`
- 分发用 `scripts/pack_exe.ps1`，产出 `release/OCR-Studio-0.1.0-win64.zip`，
  内含 `OCR Studio.exe + runtime/*.dll + models/ + 使用说明.txt`
- **注意**：`bundle.resources` 只在打安装包时生效，`bundle.active=false` 后
  必须靠这个脚本手动拷 DLL，否则拿出去的 exe 会缺 onnxruntime.dll

打包注意：vite 的 `emptyOutDir` 会一次性删 `dist/assets` 下 60+ 个文件，
本机 agent 环境的批量删除保护会拦下来（`SAFE_DELETE_BULK_CONFIRM_REQUIRED`），
表现为 `pnpm build` 直接失败。**构建前先把 dist 改名**（`Rename-Item dist dist_old_x`），
vite 就会新建空目录，比删安全。

前端改完必须 `cargo build` 才生效：`generate_context!` 在**编译期**把 `dist`
嵌进 exe，只跑 `pnpm build` 的话 exe 里还是旧界面（本轮踩过一次，白浪费一轮截图）。

---

## 1. 验证状态（可复现）

```
cargo test                       # 40 个单元测试
cargo test -- --ignored          # + 16 个重量级集成测试（需模型/DLL/真实桌面）
pnpm build                       # 前端 tsc --noEmit + vite build
cargo build --release            # 无安装包（bundle.active=false）
powershell -File scripts/pack_exe.ps1   # 打免安装 zip
```

| 测试 | 结果 |
|---|---|
| 单元测试（geometry / det / rec / preprocess / resize fixture / **matcher**） | **40 / 40 通过** |
| `matcher::*`（7 个） | **7 / 7 通过**：原位命中(score>0.999)、亮度缩放免症、无假阳、纯色模板不误检、NMS 多目标、超大模板拒绝、1080p 多层金字塔 |
| `pipeline_match::rust_matches_python_reference` | **通过**：文本不一致 0、框偏差 >2px 0、置信度偏差 >0.05 0 |
| `tensor_diff::rec_tensor_matches_python_dump` | 通过（rec 输入张量与 Python dump 一致） |
| `tensor_diff::warp_perspective_matches_python_crop` | 通过（透视裁剪 **0/3069 逐位一致**） |
| `tensor_diff::rec_probe_matches_python_on_same_tensor` | 通过（同一张量下 Rust/Python 逐帧 argmax 一致） |
| `api_smoke`（13 个） | **13 / 13 通过**：health、shibie(JSON/multipart)、zhaozi(子串/正则)、**6 个找图路径**、**danzijson 逐字**、400 错误路径 |
| `capture_live`（5 个，`--ignored`） | **5 / 5 通过**：全屏尺寸→自匹配**位置零误差、score=1.0**、区域截图坐标本地化、窗口枚举、窗口截图 136 万非黑像素 |
| `scripts/e2e_find.py`（服务进程级） | **全部通过**：真实桌面自截模板 → `/ocr/pingmuzhaotu1` 命中**坐标零误差、score=1.0000**；容错端点一致；`tuzhaotu` 图内坐标与 `origin=(0,0)` 正确；缺模板返回 400 |
| `scripts/e2e_danzi.py`（服务进程级） | **通过**：UI 截图 39 行 / 192 字，**每行字数与文本长度一致**、**每个字框都落在行框内** |
| `matcher::bench_find_1080p`（`--ignored`） | 性能观测：灰阶 76ms / 建金字塔 129ms / **粗扫 38.5ms** / 精化 23ms（debug 构建） |

**release 构建已实测**（2026-10-21）：
```
D:\rust-target\release\ocr-studio.exe    8.1 MB，启动 3s 内 /ocr/health 返回 ok
/ocr/health                              actual_device=directml, dict_size=18708
找图（1080p 全屏 + 64px 模板）            59~62 ms（截图 ~30ms + 匹配 ~30ms）
```

打包产物实测（2026-10-21，已改为免安装 zip，安装包不再产出）：
```
D:\rust-target\debug\ocr-studio.exe                 启动 8s 不退出
curl http://127.0.0.1:8848/ocr/health               {"status":"ok","model":"small",
                                                     "actual_device":"directml","dict_size":18708}
release/OCR-Studio-0.1.0-win64.zip                  exe + 3 个 dll + models + 使用说明
```
```

基线文件：`models/small/reference_output.json`（由 `scripts/ref_pipeline.py` 生成）。
测试图：`assets/test_zh.png`（900×560，7 行真实文本 + 1 个误检噪声框）。

---

## 2. 已完成

### M0 基线与环境
- git 仓库建立，基线提交 `826e75b`（此前项目无版本控制）。
- `.gitignore` 排除 `target`、`.rust/`（项目内 Rust 工具链）、`models/**/*.onnx|txt`、`*.dll`。
- 项目内 Rust 工具链：`.rust/`（rustup + cargo），避免污染用户环境。

### 模型与运行时
- `scripts/models.manifest.json` + `scripts/fetch_models.ps1`：三档（tiny/small/medium）det/rec/cls/keys 的
  URL + SHA256，带版本标签（`v3.10.0`）、临时文件 + 原子重命名。
- **small 档已下载并校验通过**：`det.onnx` 9.9MB / `rec.onnx` 21.2MB / `cls.onnx` 1.0MB / `keys.txt` 74KB。
- `scripts/copy_runtime.ps1`：把 `onnxruntime.dll`(1.31.0) / `DirectML.dll` / `onnxruntime_providers_shared.dll`
  拷到 `src-tauri/runtime/`。
- **风险 R1 解除**：PP-OCRv6 可用 `paddle2onnx` 导出 ONNX，RapidOCR v3.10.0 已提供 tiny/small/medium 三档。
- PP-OCRv6 **没有 cls 模型**，cls 沿用 PP-OCRv5 的 LCNet（与上游 RapidOCR 一致）。

### Rust 推理核心（`src-tauri/src/ocr/`）
| 文件 | 内容 |
|---|---|
| `engine.rs` | 会话构建、**真实预热推理**做 EP 探测（失败自动降级 CPU）、det→cls→rec 编排 |
| `preprocess.rs` | det/rec/cls 预处理 + `resize_bilinear`（逐位复刻 `cv2.resize`） |
| `det.rs` | DB 后处理：二值化→轮廓→mini box→打分→unclip→再取 mini box→排序 |
| `rec.rs` | CTC 解码（blank=0，char=`chars[idx-1]`，富一类保护） |
| `geometry.rs` | 最小外接矩形、unclip、多边形填充、透视变换、四点排序 |
| `resize_fixture_tests.rs` | **机器生成**的 cv2 对齐 fixture（勿手改，见 `scripts/gen_resize_fixture.py`） |

- 依赖：`ort 2.0.0-rc.15`（`load-dynamic` + `api-24` + `directml`，因本机无 MSVC 导入库 `.lib`）、
  `imageproc`、`ndarray 0.17`。
- EP：DirectML（AMD 核显，无 CUDA），CPU 兜底。

### HTTP 接口层（`src-tauri/src/api/`，2026-10-21 新增）
| 文件 | 内容 |
|---|---|
| `dto.rs` | 统一信封 `{code,msg,data?}`、`ApiError`（→ HTTP 状态码映射）、base64/图像解码 |
| `state.rs` | `AppState`：`Mutex<Option<OcrEngine>>` + `RwLock<AppConfig>` + 停机信号；模型目录解析；`spawn_blocking` 推理 |
| `routes.rs` | 路由表 1:1 对应 PLAN 第 6 节；JSON base64 与 multipart 双入参 |

- **已实现 4 个**：`/ocr/shibie`、`/ocr/zhaozi`（子串或 `/正则/`）、`/ocr/duohangjson`、`/ocr/health`。
- **找图类 6 个已实现**（M2，2026-10-21）：
  `/ocr/tuzhaotu1|2`（传图）、`/ocr/jubingzhaotu1|2`（截窗口）、`/ocr/pingmuzhaotu1|2`（截屏幕）。
  后缀 `1`=精准（NCC ≥ 0.92），`2`=容错（NCC ≥ 0.72）。
- **仅剩 1 个 501**：`/ocr/danzijson`（单字级定位，需 CTC 帧对齐）。
- 引擎按请求参数热切换：`level` / `device` / `use_cls` 与当前引擎不一致时自动重载。

### 截图（`src-tauri/src/capture.rs`，M2）
- 屏幕：`BitBlt(SRCCOPY | CAPTUREBLT)`；窗口：`PrintWindow(PW_RENDERFULLCONTENT)`，
  失败或**返回全黑图（假成功）**时回退 `BitBlt(GetWindowDC)`。
- 取像素统一 `GetDIBits` 32bpp top-down + 手工 BGRA→RGB。
- `EnumWindows` 枚举可见且有标题、无 owner 的顶层窗口。
- **坐标系必须配对**：`GetWindowRect`（含边框）配 `GetWindowDC`（原点=窗口左上角）。
  早先用 `ClientToScreen`+`GetClientRect`（客户区）配 `GetWindowDC`，右侧/底部会缺一块，
  已修正并实测窗口截图尺寸与 `GetWindowRect` 完全一致（1455×941）。

### 模板匹配（`src-tauri/src/match.rs`，M2）
算法：**灰度金字塔（≤1/16）+ 最粗层 NCC 全扫 + 逐层 ±2 精化 + NMS**。

- 粗筛也用 NCC 而不是 SQDIFF。原设计「SQDIFF 积分图粗筛」的前提是错的：
  SQDIFF 的 `ΣT²+ΣI²-2ΣTI` 里 `ΣTI` 同样要逐像素乘，**并不比 NCC 便宜**；
  而 SQDIFF 对亮度漂移敏感（实测模板 ×0.8 后真位置掉出 top-64 候选，直接漏检）。
  统一 NCC 后代码更短、更准，也更抗主题变化。
- 所有分数语义统一为**零均值 NCC**（`-1..=1`），对亮度/对比度线性变换免疫。
- 模板方差为 0（纯色块）时 NCC 无定义 → 返回 0 分**拒绝命中**，宁可漏不可假阳。
- NMS 用中心距离（阈值 `max(4, min(w,h)/2)`）合并重复命中。
- 性能实测（1920×1080 全屏找 64×64 模板，`scripts/e2e_find.py`）：
  | 构建 | 截屏+找图 | 传图找图（含 PNG 解码） | 截图链路 |
  |---|---|---|---|
  | debug | ~380 ms | ~450 ms | ~0 ms |
  | **release** | **59~62 ms** | **~31 ms** | **28~31 ms** |

  粗扫原先是热点（debug 424ms），三处优化后降到 38.5ms（约 11 倍）：
  1. `dot()` 内层从 `data[i]` 索引改成切片 `zip`（去掉逐元素边界检查与坐标乘法）；
  2. `Gray::from_rgb` 从 `pixels()` 迭代器改成 `raw.chunks_exact(3)`；
  3. 最粗层最小模板边长从 8 降到 6，让 64px 模板能多降一层
     （位置数 ÷4、模板面积 ÷4，合计约 17 倍）。
     代价是最粗层模板只有 8×6，判别力下降 —— 但粗筛只需保证真解仍在候选集，
     位置精度由后续精化负责，这个交换是划算的（7 个单测复跑全绿）。
- 建金字塔不再每层 `clone` 全尺寸像素，改为先降采样再 move。
- **⚠ 手动跑 `cargo build` 的 exe 必须把 `src-tauri/runtime/*.dll` 拷到 exe 旁边**，
  否则 `DmlExecutionProvider` 注册失败（报 "DirectML is not supported in this
  build of ONNX Runtime"）并静默退回 CPU。根因是 DirectML.dll 不在 exe 的 DLL
  搜索路径里（onnxruntime.dll 是绝对路径加载的，但它自己的依赖仍按标准顺序找）。
  打包产物（MSI/NSIS）由 `bundle.resources` 自动放到位，不受影响。

### Tauri 命令层（`src-tauri/src/commands.rs`）
`ocr_recognize` / `get_health` / `get_config` / `save_config`（热生效，端口变更自动重启服务）/
`set_level` / `set_device` / `server_start` / `server_stop` / `server_status` / `models_root` /
**`find_image` / `screen_preview` / `list_windows`**（M2）。
UI 走 invoke，外部脚本走 HTTP，两条路汇到同一个 `AppState`，不会各加载一份模型。

找图命令的关键设计：`find_image` 在 `source != image` 时**连同截图一起返回**（`preview` 字段）。
因为屏幕是活的，若前端先单独截一张图当背景、再调找图，两次截屏之间画面可能已变，
命中框就会画在过时背景上。由同一次匹配返回它自己看到的像素，框与背景必然对齐。
同时出参带 `origin_x`/`origin_y`（截图左上角的屏幕坐标），前端才能把绝对坐标换算回图内坐标
—— 这个偏移**不能靠前端反推**（多显示器时虚拟屏幕原点可能是负数）。

### 前端（`src/`，2026-10-21 从 Vue 迁移到 React）
```
src/App.tsx                   布局：TitleBar + 侧边导航 + 三页
src/components/TitleBar.tsx   自定义标题栏（无图标无标题，仅窗口控制）
src/components/ImageViewer.tsx  canvas 框叠加（按原图像素坐标等比缩放，识别框蓝色 / 找图框琥珀色）
src/components/FindPanel.tsx   找图面板：来源选择、模板上传、命中列表与可视化
src/components/ui/*.tsx       shadcn/ui 风格：button/card/input/label/badge/select/
                              tabs/switch/slider/separator/scroll-area/progress/tooltip
src/pages/                    Dashboard / Playground（识别 / 找图 双 Tab）/ SettingsPage
src/lib/api.ts                invoke 封装 + 浏览器降级到 http://127.0.0.1:8848
```
- 图标库 `lucide-react`，**每个按钮均带图标**。
- Playground 支持拖入 / Ctrl+V 粘贴 / 选文件，结果分「文本 / 逐行 / JSON」三个 Tab。
- 找图面板三个来源：上传图片 / 整个屏幕 / 指定窗口（下拉选标题 + 刷新）。
  在识别页载入的图会自动带过去当搜索图。
- 非 Tauri 环境（纯浏览器）下 Playground 自动降级到 HTTP，便于调 UI。

### 字体落地（`scripts/fetch_misans_font.py`）
从小米官方字体服务抓取 MiSans 400 字重的 56 个 unicode-range 切片（1.83MB）到
`src/assets/fonts/misans/`，并生成本地 `misans.css`（URL 改写为相对路径）。
**离线可用**，不依赖 CDN。授权口径见第 7 节。

---

## 3. 精度对齐：已定位并修复的三个坑

这三个是「与 Python/RapidOCR 差一点点」的真正根因，全部用探针脚本实证过，
不是猜的。每个都有锁定测试防回归。

### 3.1 `cv2.resize` 的两段式定点（影响面最大）

**现象**：rec 输入张量 0.3%~6% 的像素恒差 1 个量化单位，且 Rust 恒比 Python 大 1。

**根因**：OpenCV 4.10 通用 8u 路径是**横向 11bit 定点 + 纵向截断**的固定点实现：

```cpp
dst[x] = uchar((((b0*(S0[x]>>4))>>16) + ((b1*(S1[x]>>4))>>16) + 2)>>2);
```

横向结果不舍入，纵向先 `>>4` 截断到 7bit 再 `>>16` 截断，最后 `+2>>2`。
截断都朝零 → **结果系统性偏低**，正好解释「恒差 1、只在特定 frac 区间」。

**修复**：`resize_bilinear` 改为复刻该定点链路。效果：不一致元素从 4.0% 降到 **0.12%**。

**已知残差（工程代价，非原理障碍）**：首/末行仍有 ≤1 LSB 差异。
这两行是「源坐标被钳制到边界」的单 tap 退化场景，走的是 OpenCV 内部另一条
（SIMD 快路径）代码，无法从外部完全还原——枚举过 8bit/11bit 权重 × 5 种舍入全部打平。
fixture 测试因此对首末行放宽到 ≤1 LSB，其余行仍逐位断言。

### 3.2 `cv2.warpPerspective` 的坐标约定

**现象**：透视裁剪 20~27% 像素不一致，最大差 8~9 级。

**根因**（三个错误叠加）：
1. OpenCV 用「整数坐标 = 像素中心」约定（identity warp 原样返回）；
2. 源坐标小数位量化到 **1/32**（5 bit 定点）；
3. 越界时**逐 tap 补 0**，不是整像素填 0、也不是边缘复制。

**修复**：`sample_bilinear` 换成逐位复刻。效果：**0/3069 逐位一致，最大差 0。**

### 3.3 `pyclipper` 的整数语义（最后一道缺口，2026-10-21 修复）

**现象**：文本全对、框也全对，但误检框 #6 的置信度 Rust 0.894 vs Python 0.748，差 0.146。

**定位过程**（逐层排除，每一步都有实测）：
1. rec 输入张量只差 3/46080 个元素、≤1 LSB → 在 Python 里给张量加 1 LSB 做敏感性测试，
   真实文本框只抖 0.0001~0.0014，**排除输入侧**；
2. CPU EP vs DirectML EP 置信度完全一致（差 0.0000）→ **排除 EP**；
3. `GraphOptimizationLevel` 三档（BASIC/EXTENDED/ALL）结果完全一致 → **排除优化级别**；
4. 用 `rec_probe` 把 Python dump 的张量直接喂 Rust 的 rec 会话 → 逐帧 argmax 与 Python 一致 → **排除推理**；
5. 用 Rust 自己的 `rec_preprocess` 张量喂同一会话 → 仍一致 → **排除预处理**；
6. 打开管线调试输出 → 管线里 #6 的非 blank 帧是 `(33, 0.8937)`，而探针里是 `(33, 0.7476)` → **定位到 det 给的框不同**；
7. 打印全精度框：Rust `(445.271, 364.271)-(477.729, 394.729)`（32.46×30.46），
   Python `(445, 364)-(478, 395)`（33×31）。**取整后碰巧一样，所以之前的整数框比对看不出来。**

**根因**：Python 的 unclip 走 **pyclipper（整数坐标库）**。实测其语义：
- 输入坐标**截断**成整数（`IntPoint` 由 double 强转；452.6 + delta 7 → 445 而非 446）；
- 偏移距离保持 double（delta=6.5 时左扩 6、右扩 7，即 445.5→446、477.5→478，half-up）；
- 输出坐标**四舍五入**成整数。

纯浮点外扩 6.729167 得到 32.46 宽的框，`int()` 截断后变 32×30 的裁剪图；
pyclipper 得到 33×31。对噪声框，这 1px 的裁剪差就让置信度抖了 0.146。

**修复**：`unclip` 复刻上述整数语义。效果：**框 #6 置信度 Rust 0.748 vs Python 0.748，全部收敛。**

### 3.4 附带修复：DLL 搜索路径（生产隐患）

本机 `C:\WINDOWS\system32\onnxruntime.dll` 是 **1.17.1**，低于 ort 要求的 1.24.x。
打包后 DLL 在 exe 旁边所以没事，但 `tauri dev` / `cargo test` 时 exe 在 `target/debug`，
会误加载 system32 那份直接把进程带崩。已提取为公共 API
`ocr::ensure_ort_dylib_path(workspace_root)`，测试与将来的启动流程都调用它。

---

## 4. 已知残差与边界（诚实记录）

| 项 | 状态 | 性质 |
|---|---|---|
| `resize_bilinear` 首/末行 ≤1 LSB | 已量化，fixture 测试对该两行放宽容差 | 工程代价——OpenCV 内部 SIMD 路径，枚举过所有权重/舍入组合均打平 |
| unclip 用斜接 join，pyclipper 用 JT_ROUND 圆角 | 轴对齐框**逐位一致**；明显旋转的框 minAreaRect 偏大约 1.5px | 工程代价——需复刻 Clipper 弧的角步长离散化，已用测试把偏差锁在 ≤2px |
| `cls_should_rotate` 阈值语义 | **未验证**，需与 RapidOCR/PaddleOCR 对齐后才能启用 cls | 待办 |
| 置信度对误检框敏感 | 误检框在 1-LSB 级别就是混沌的（实测 147 像素随机扰动可抖 0.04） | 原理性——不是 bug，但意味着**不能用误检框的置信度做精度验收** |
| App 内 UI 点击链路 | **部分验证**：Playground 找图面板的底层（`find_image`/`screen_preview`/`list_windows`）
  已由集成测试与真实截图测试覆盖，但页面渲染与按钮回调仍未真机点击过 | 环境限制——需在有桌面的机器上过一遍 |
| **模板匹配不支持缩放** | 模板与目标尺寸差 >5% 时 NCC 分数明显下降，可能漏检 | 工程代价——多尺度搜索会让耗时线性翻倍（3 档就 ×3），规划为可选开关 |
| **模板匹配不支持旋转** | 模板旋转后 NCC 直接崩（典型 0.99 → 0.3） | 工程代价——需旋转模板阵列（18 档 ×18 耗时），且旋转产生的插值会进一步降分 |
| 亮度调整导致高光截断时 NCC 掉分 | 数学事实：`a*I+b` 被 255 截断后不再是线性变换 | 原理性——不是 bug。用例/数据无法完全规避，容错模式把阈值降到 0.72 应对 |
| 多显示器混合 DPI | 坐标统一按主屏 DPI 处理，混合 DPI 下 `GetSystemMetrics` 与物理像素有偏差 | 工程代价——需按 per-monitor DPI 逐个换算，未实现 |
| UAC 提权窗口截图 | 失败（不在同一桌面合成层） | 原理性——除非注入，无解 |
| ort `Environment` 进程级单例 | 已修：`OcrEngine::load` 复用全局 `OnceLock<Environment>` | 曾是真 bug——运行时切档位重载引擎会直接报 "only one environment is allowed per process" |
| 两套 `ModelLevel`/`Device` 类型 | 已修：统一定义在 `crate::config`，`ocr` 模块重新导出 | 曾是编译错误源 |

**区分说明**：前两项是工程代价问题（投入即可继续压缩）；第四项是原理性天花板，
堆精度救不回来，验收标准必须建立在真实文本框上。

---

## 5. 待办（按依赖顺序）

| # | 任务 | 状态 |
|---|---|---|
| 1 | `api/routes.rs` + `api/dto.rs`：`/ocr/*` 端点 | **已完成** |
| 2 | `commands.rs`：Tauri invoke 命令 | **已完成** |
| 3 | 前端测试台接真实后端 | **已完成**（代码 + 构建 + 打包验证；页面点击未验证，见第 4 节） |
| 4 | `model_manager.rs`：三档模型懒加载缓存 | 部分达成——`AppState` 已支持按请求热切换重载，但无 LRU 缓存，切档会整份重载 |
| 5 | `cls_should_rotate` 阈值语义对齐 RapidOCR | 待办（需读 RapidOCR 源码确认 label 顺序） |
| 6 | 截图/句柄找图 + 模板匹配 NCC → 7 个找图端点 | **已完成**（6 个端点 + 1 个仍 501；真实截图自匹配 score=1.0） |
| 7 | 单字 JSON：CTC 帧对齐 + 逆透视映射得到字符级 box | **已完成**（`ctc_decode_spans` + `WarpMap`；e2e 验证字数与文本一致、字框全部落在行框内） |
| 8 | 日志页：tracing → `app.emit` → 前端实时日志 | **已完成**（环形缓冲 2000 + 批量 emit；级别过滤/搜索/暂停/清屏/复制） |
| 9 | 模板匹配多尺度（±10%）开关 | 已评估，本轮**不做**：耗时 ×3，且容错模式已能覆盖大部分场景。做成可选参数留到有真实需求时加 |
| 10 | 模板匹配旋转（18 档） | 已评估，**不做**：耗时 ×18 且旋转插值会自我降分，收益低于代价 |
| 11 | 截图区域框选 UI（鼠标拖出 region） | 已评估，本轮**不做**：当前「整屏 / 整窗口」两种粒度够用，框选属交互打磨 |
| 12 | MiSans 商用授权口径确认 | 待用户确认（随包分发前） |
| 13 | 免安装 exe 分发（**用户明确不要 MSI/NSIS**） | `bundle.active=false` + `scripts/pack_exe.ps1` 打 zip（exe + dll + models + 说明） |
| 14 | 多引擎抽象（DDDDOCR / YOLO） | **M3.0 + M3.1 已完成**：抽象层 + `OcrEngine`/`DdddOcrEngine` 两个实现 + `/engine/list` + `/ocr/*` 的 `engine` 字段 + 设置页引擎卡片 + 测试台引擎下拉 |
| 15 | 引擎注册表（多引擎 LRU 单活） | **已用「PP-OCR + 额外引擎单槽」替代**：ddddocr common 就 54MB，同时开两个只浪费内存；单槽等价于 LRU 上限 1。要并行（YOLO+PP-OCR 做 pipeline）时再把 `alt` 换成 HashMap |

---

## 6. 踩坑记录（本机环境，会重复遇到）

1. **PowerShell 5.1 写 `.ps1` 必须 UTF-8 带 BOM + CRLF**，否则按 GBK 解码，中文注释全碎。
   Edit 工具改过 `.ps1` 后要重新转换。
2. **cv2 读不了中文路径**，一律用 `np.frombuffer + cv2.imdecode`（参考脚本里的 `imread_unicode`）。
3. **Git Bash 的 MSYS coreutils 与 runtime 版本不匹配**，`ls/grep/find` 等会 exit 127；
   全程用 PowerShell 工具。
4. **MSYS `timeout` 完全不可用**（`STATUS_DLL_INIT_FAILED`），要限时用程序自带参数或后台轮询。
5. **非 ASCII 路径的验证一律用 PowerShell 建目录/传参**，否则会误判成程序 bug。
6. **PowerShell 工具的工作目录与 `Set-Location` 对 .NET API 不一致**，.NET API 一律用绝对路径。
7. **PowerShell 控制台默认按 GBK 显示，会把 UTF-8 的中文输出打成乱码**，看起来像数据损坏。
   本轮就因为 `python ... | Out-String` 看到「婊︽幏鐚滄」而误判成 ddddocr 字符集坏了，
   实际是显示问题。要打印中文先设：
   ```powershell
   $env:PYTHONIOENCODING="utf-8"; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8
   ```
   或者把结果写进文件再用 Read 工具看。
8. **agent 的 bash 与 PowerShell 用的 python 不是同一个**：bash 是托管 3.13，PowerShell 是
   系统 3.12。给 3.12 装的包在 bash 里 import 会报 `numpy._core._multiarray_umath` 缺失。
   要跨工具跑 python，显式写解释器全路径。

---

## 7. 前端技术规范（2026-10-21 确定，取代原 Vue 方案）

> 原方案是 Vue 3 + Tailwind v4 + shadcn-vue，现改为下列规范。`src/` 下现有 Vue 骨架需替换。

| 项 | 规范 |
|---|---|
| 框架 | **React + TypeScript**（经 Vite，Tauri 2 壳不变） |
| 样式 | **Tailwind CSS** |
| 组件库 | **shadcn/ui** |
| 字体 | **MiSans**（小米商用字体，需从官方渠道获取并确认授权后再随包分发） |
| 配色 | **纯白为主**，**黑色为辅**，**蓝色点缀** |
| 按钮 | **每个按钮都必须带图标** |
| 标题栏 | **自定义标题栏**：**不要图标、不要标题**（仅窗口控制按钮） |

落地要点：
- 自定义标题栏需在 `src-tauri/tauri.conf.json` 的 `app.windows[0]` 设 `"decorations": false`，
  前端用 `getCurrentWindow().minimize()/toggleMaximize()/close()`（`@tauri-apps/api/window`）自绘控制按钮。
- 图标库用 `lucide-react`（shadcn/ui 默认图标源，与「每个按钮带图标」天然契合）。
- MiSans 走 `@font-face` 本地引入，`font-family` 设为全局默认。

**已全部落地（2026-10-21）**。补充两点实测结论：
- `decorations:false` 下窗口仍可正常启动与调整大小；标题栏整条加 `data-tauri-drag-region`，
  三个窗口控制按钮要 `onMouseDown` 阻止冒泡，否则点击会被当成拖拽起点。
- 打包时必须把 `runtime/*.dll` 加进 `bundle.resources`，否则用户机器上没有 onnxruntime
  （本机 system32 那份 1.17.1 版本过低，会直接把进程带崩）。MSI 体积因此 +15MB。

**MiSans 授权口径（待确认）**：MiSans 是小米发布的商用字体。本次通过小米官方字体服务
（`cdn-font.hyperos.mi.com`）获取并切片落地到 `src/assets/fonts/`，共 1.83MB / 56 个 unicode-range 切片。
**随包对外分发前，请自行确认授权条款**（官方页面通常允许免费商用，但以最新条款为准）。
若最终不随包分发，可改为运行时按需下载或换用开源字体（如思源黑体）。

---

## 7.5 M4：界面按「API 工具」重做（2026-10-22）

用户原话：**「我们这个是做的 api 工具，主要目的别搞忘了，你完全照抄借鉴的程序搞得像
我们就是个识别的程序而已，你得好好设计下」**。

### 定位纠正（这一节的根）

第一版是照着一个「键鼠同步器」的向导式界面做的（选功能 → 选图 → 看结果），
好看但**跑偏了**：那类软件是给人用的工具，操作本身就是产品；
而 OCR Studio 的交付物是**一台跑在 127.0.0.1 上的接口服务**，桌面界面只是它的控制面板。

所以首页要回答的三个问题变成：

1. 服务开没开？
2. 地址是什么（能一键复制走）？
3. 能调什么、怎么调（给现成代码）？

「上传一张图看效果」降级为**验证手段**，不再是首页主叙事。

### 信息架构

| 页 | 职责 |
|---|---|
| **服务**（首页） | 深色主卡：状态 / 地址 / 端口 / 已运行 / 设备 / 精度，一键启停与复制；三组能力概览；三步接入 + 四语言示例代码 |
| **接口** | 14 个端点的完整文档：统一约定、错误码表、按组列出、可展开看参数/返回/请求示例 |
| **试跑** | 验证接口通不通（原测试台）；载入图片**自动**跑一次 |
| **日志** | 每次调用的记录 |
| **设置** | 精度 / 显卡加速 / 找图阈值 / 本地服务 / 检测模型 / 高级 |

端点事实来源统一收敛到 `src/lib/endpoints.ts`（首页概览与接口页同源），
以后改后端路由必须同步这里。

### 厚重感怎么来的（具体手段，不是形容词）

| 手段 | 落点 |
|---|---|
| 字重整体上抬 | 页面标题 26px/700（原来是 17px/600）、卡片标题 700、说明文字 500（原来全是 400）、导航未选中项 500 |
| 全站唯一的深色块 | `.panel-dark`：`#12141b → #0a0c11` 斜向渐变 + 左上角蓝色径向光 + 内上沿 1px 高光，把「服务」做成视觉锚点 |
| 阴影分三档 | `.shadow-card`（静止）/ `.shadow-hover`（悬停）/ `.shadow-panel`（整块浮起），不再用 1px 边框分隔一切 |
| 主按钮有厚度 | `.btn-brand`：竖向渐变 + 上沿 inset 高光 + 蓝色外发光 + hover 抬 1px + active 压回 |
| 数字等宽 | `.tnum`，运行时长每秒跳动时不会左右抽 |

### 动效清单（都用 CSS keyframes，不引动画库）

| 名称 | 用在哪 |
|---|---|
| `drift` 14s | 深色卡右上角光斑极慢漂移，让块「活着」但不抢注意力 |
| `ping-ring` 2.4s | 运行中状态点的外扩水波 |
| `soft-pulse` 2s | 状态点呼吸 |
| `pop` 260ms | 复制成功后图标弹一下 |
| `num-in` 240ms | 运行时长数字翻入（`key={value}` 触发重放） |
| `fade-rise` 220ms | 切页、接口卡片展开 |
| `card-lift` | 可点卡片 hover 上浮 3px + 阴影加深 |

`prefers-reduced-motion` 未接（后续可在设置里加「减少动画」开关）。

### 这轮踩的坑（都已复现过，别再踩）

1. **前端改完必须 `cargo build`**。`generate_context!` 在**编译期**把 `dist` 嵌进 exe，
   只跑 `npm run build` 的话 exe 里还是旧界面 —— 本轮又踩了一次，看到旧 Dashboard 还以为缓存坏了。
   要触发重嵌：先 `npm run build`，再改 `src-tauri/build.rs` 时间戳后 `cargo build`（约 1 分钟）。
2. **在 agent shell 里直接跑 exe 必白屏**：WebView2 的 GPU 子进程在这个 shell 环境下起不来，
   10 秒后看门狗判定「前端未报到」并重建窗口。**必须 `explorer.exe <exe>` 启动**，
   然后 `scripts/shot_window.py` 抓窗口。
3. **`GetWindowRect` 会带 DWM 阴影边距**（本机左右各约 15px），截图会凭空多出窗口外的桌面内容，
   看起来像界面画错了。已改用 `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`，
   `shot_window.py` 与 `ui_probe.py` 都改了。
4. **切页要重置滚动**：滚动容器是共用的，从接口文档切到短页面会直接落在中间，
   看起来像「这页没有标题」。
5. **Radix `Tooltip.Trigger asChild` 包一个 flex 行会让文字宽度变 0**（truncate 成省略号，
   实测只剩一个小圆点）。这类纯展示行用原生 `title`，别套 Tooltip。

### 本轮验收

- `npm run build` 通过；`cargo test --lib` 82 通过（未改后端）。
- 界面实测：服务台（运行中 / 地址 / 已运行秒数在跳 / 三项统计）、能力三卡（4+6+2 与路由一致）、
  接入示例四语言切换、接口页 14 条 + 展开详情、试跑页自动识别、日志页、设置页。
- 新增脚本：`scripts/ui_scroll.py`（滚轮，验收长页面用）。

---

## 7.6 M5：托盘常驻 + 单实例 + 分发包说明（2026-10-22 下午）

### 为什么先做托盘

定位是「本地 API 服务」，那它就必须能**在后台一直活着**。没有托盘的话只有两个选择：
窗口开着占地方，或者关掉服务就死。所以关窗口的语义必须是「收到托盘」而不是「退出」。

### 行为定义

| 动作 | 结果 |
|---|---|
| 点窗口右上角关闭 | 窗口隐藏，**服务继续跑**（`CloseRequested` → `prevent_close` + `hide`） |
| 托盘菜单首行 | 只读状态：`● 运行中 · 127.0.0.1:8848` / `○ 已停止`，每 2 秒刷新且**变了才更新** |
| 托盘 打开界面 | 显示 + 取消最小化 + 聚焦 |
| 托盘 启动/停止服务 | 直接调 `commands::start_server_background` / `api::stop`，启动项与停止项按状态互斥置灰 |
| 托盘 退出 OCR Studio | `app.exit(0)`，这是**唯一**真正的退出方式 |
| 双击 exe 第二次 | 单实例插件拦截，不新开进程，把已有窗口拉出来 |

代码位置：`src-tauri/src/lib.rs` 的 `setup_tray` / `spawn_tray_ticker`。
启动逻辑从 `commands::server_start` 抽成 `pub fn start_server_background(&Arc<AppState>)`，
让托盘与 Tauri 命令共用一份（原来只能在 `State<'_, _>` 上调用，托盘拿不到）。

### 这轮踩的坑

1. **托盘不给 icon 在 Windows 上不可见**。`TrayIconBuilder::build_inner`（tauri-2.12.4）
   **不会**回退到 `default_window_icon()`，必须显式
   `.icon(app.default_window_icon().cloned())`。不设也能 build 成功，只是没有图标。
2. **`Menu::get` 的参数不是 `&str`**：签名 `get<'a, I>(&'a I)` 且约束
   `MenuId: PartialEq<&'a I>`，所以写 `menu.get("stop")`（`I = str`）；
   写成 `menu.get(&"stop")` 会变成 `&&str`，编译直接失败。
3. **菜单项文字/可用态更新要走主线程**：在 ticker 线程里直接 `set_text` 不可靠，
   用 `app.run_on_main_thread(...)` 包一层。`TrayIcon` / `Menu` 本身可 Clone + Send。
4. **`Menu::get` 返回 `MenuItemKind`**，要 `.as_menuitem().cloned()` 才能 `set_text` / `set_enabled`。
5. **进程名是 `OCR Studio`（带空格），不是 `ocr-studio`**。按 exe 文件名小写去找进程
   一个都匹配不到，导致「以为杀掉了旧实例、其实没杀」，第二次启动因端口占用而
   `HTTP 服务异常退出: 绑定 127.0.0.1:8848 失败`，界面显示「服务已停止」。
   **这正是「单实例」需求的来源**，不是脚本问题。
6. **管道会让后台任务的退出码失真**：`cargo test ... | Select-Object -Last 25`
   报 failed 但实际 82/82 通过。判基线要看 `test result:` 那一行或单独取 `$LASTEXITCODE`，
   别只信任务状态 —— 本轮因此误判「测试失败」，白查一轮。

### 没做成的验证（如实记录）

**托盘图标的可见性没能用截图确认。** Windows 11 会把新图标折叠进溢出面板，
而合成鼠标事件（`mouse_event`）点不开那个面板：按 `Shell_TrayWnd`→`TrayNotifyWnd`
真实矩形算出的坐标点下去，落到了输入法图标上，三次尝试都截到的是面板下面的窗口。
已放弃这条，改为**程序化验证行为**（见下），图标是否可见请人工看一眼任务栏右下角。
临时探测脚本已删除，只留 `scripts/verify_tray.py`。

### 验收（打包版实测，`release/OCR-Studio/`）

`scripts/verify_tray.py --exe "…\OCR Studio.exe" --port 8848` 输出：

```
[OK] 窗口可见 hwnd=…
[OK] 关前服务正常 code=0 status=ok
[OK] 窗口已隐藏                 ← WM_CLOSE 后 IsWindowVisible = false
[OK] 关窗口后服务仍在应答        ← /ocr/health 仍返回 code=0
[OK] 重复启动：进程数 1 -> 1（应不增加）
[OK] 重复启动后服务仍正常
exit=0
```

另外：`cargo test --lib` 82 通过；release 构建 5m20s；`check_embedded_assets.py`
确认新 `index-BWCERk58.js` 等 6 个资源都在 exe 里（不是旧界面）。

### 分发包说明重写（`scripts/pack_exe.ps1` 里的 `使用说明.txt`）

旧版还写着「进入『仪表盘』看推理设备」——仪表盘早合并进设置页了，属于**过时文档骗用户**；
而且完全没提这是 API 服务、一条接口都没列。新版结构：

产品定位 → 三步上手 → 5 项导航 → **14 个接口一览** → curl/Python 示例 →
错误码表（0/400/501/503/1002/1003）→ 托盘常驻与单实例 → 目录说明 →
显卡加速排查 → 端口 → 检测模型放法 → 常见现象对照 → 卸载 → 系统要求。

校验：135 行、22 处空行、CRLF、UTF-8 带 BOM（`EF BB BF`），记事本不会乱码。

产物：`release/OCR-Studio-0.1.0-win64.zip`（106.1 MB，含 exe + 3 个 DLL + models + 说明）。

---

## 8. 关键文件索引

```
docs/PLAN.md                      # 设计文档（11 节，含 M0-M4 与 R1-R6）
docs/PROGRESS.md                  # 本文档
scripts/ref_pipeline.py           # Python 黄金基线（RapidOCR 语义复刻）
scripts/dump_rec_tensor.py        # dump rec 输入张量，供 Rust 逐元素比对
scripts/dump_det_boxes.py         # dump det 全精度框与 unclip 中间量
scripts/probe_*.py                # 行为探针（OpenCV 定点 / pyclipper 语义 / EP 差异 / LSB 敏感性）
scripts/gen_resize_fixture.py     # 生成 resize 的 cv2 对齐 fixture 测试
scripts/fetch_misans_font.py      # 下载 MiSans 切片并生成本地 @font-face CSS
scripts/e2e_find.py               # 找图端到端验证（真实屏幕 -> HTTP 服务 -> 坐标比对）
scripts/models.manifest.json      # 三档模型 URL + SHA256
scripts/fetch_models.ps1          # 下载校验模型
scripts/copy_runtime.ps1          # 拷贝 ort DLL 到 src-tauri/runtime/
models/small/                     # small 档模型 + reference_output.json + dump/
src-tauri/src/ocr/                # Rust 推理核心
src-tauri/src/capture.rs          # 屏幕/窗口截图（BitBlt + PrintWindow）
src-tauri/src/match.rs            # 模板匹配（金字塔 NCC + NMS），模块名暴露为 matcher
src-tauri/src/api/                # HTTP 层（dto / state / routes）
src-tauri/src/commands.rs         # Tauri invoke 命令
src-tauri/tests/                  # pipeline_match / tensor_diff / api_smoke / capture_live
src/assets/fonts/misans/          # MiSans 切片（fetch_misans_font.py 生成，勿手改）
```
