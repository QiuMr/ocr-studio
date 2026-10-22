# OCR-Studio 项目长期记忆

## 前端技术规范（2026-10-21 用户确定，必须遵守）
- 技术栈：**React + TypeScript + Tailwind CSS + shadcn/ui**（Vite，Tauri 2 壳不变）。
  原 Vue 3 + shadcn-vue 方案已废弃，`src/` 下 Vue 骨架需整体替换。
- 字体：**MiSans**（小米），`@font-face` 本地引入并设为全局默认；商用授权需确认后再随包分发。
- 配色：**纯白为主，黑色为辅，蓝色点缀**。
- **每个按钮都必须带图标**（用 `lucide-react`，shadcn/ui 默认图标源）。
- **自定义标题栏：不要图标、不要标题**，仅窗口控制按钮。
  `tauri.conf.json` 已设 `decorations: false`，前端用 `getCurrentWindow()` 自绘。

## 项目环境约定
- Rust 工具链是**项目内私有安装**：跑 cargo 前必须设
  `RUSTUP_HOME=D:\Python测试\OCR-Studio\.rust\rustup`、`CARGO_HOME=D:\Python测试\OCR-Studio\.rust\cargo`。
- Bash 工具在本机 exit 127（MSYS coreutils 版本不匹配），**全程用 PowerShell 工具**。
- cv2 读不了中文路径，一律 `np.frombuffer + cv2.imdecode`。
- 写 `.ps1` 必须 UTF-8 带 BOM + CRLF（PowerShell 5.1 按 GBK 解码无 BOM 文件）。
- **手动跑 `cargo build` 出来的 exe，必须把 `src-tauri/runtime/*.dll` 拷到 exe 旁边**，
  否则 DirectML 不可用且**静默退回 CPU**（日志里才有 "DirectML is not supported in
  this build of ONNX Runtime"）。打包产物由 `bundle.resources` 自动处理。
- 起 App 做端到端验证要用 `run_in_background`；`Start-Process` 起的进程会被工具会话回收。
- Python 访问本机服务必须绕系统代理（`build_opener(ProxyHandler({}))`），
  否则 127.0.0.1 也走代理拿 502，会被误判成服务没起来。

## 精度基线（改动前后处理前必读）
- 黄金基线：`models/small/reference_output.json`，由 `scripts/ref_pipeline.py` 生成。
- 验收命令：`cargo test -- --ignored`（37 个测试，含 pipeline_match 全流程比对）。
- **必须复刻的三个底层语义**（详见 `docs/PROGRESS.md` 第 3 节）：
  1. `cv2.resize`：横向 11bit 定点 + 纵向截断的两段式实现；
  2. `cv2.warpPerspective`：整数坐标=像素中心、源坐标量化到 1/32、越界逐 tap 补 0；
  3. `pyclipper`：输入坐标截断成整数、距离保持 double、输出 half-up 四舍五入。
- **误检框的置信度是混沌的**，1-LSB 扰动能抖 0.04，**不能用误检框做精度验收**。
- 验收时必须打**全精度**框坐标：整数取整后会掩盖亚像素差异（本轮就因此漏判过）。

## 找图（模板匹配）设计要点
- 模块 `src-tauri/src/match.rs`，因 `match` 是关键字，lib.rs 用
  `#[path = "match.rs"] pub mod matcher;` 暴露为 `matcher`。
- **粗筛也用 NCC，不要用 SQDIFF**：SQDIFF 的 `ΣTI` 同样要逐像素乘，
  不比 NCC 便宜，且对亮度漂移敏感（模板 ×0.8 真位置就掉出候选，直接漏检）。
- 金字塔 ≤4 层，`MIN_TPL_SIDE = 6`（取 6 而非 8 是为了让 64px 模板能多降一层，
  粗扫快 11 倍）。位置精度靠逐层 ±2 精化保证。
- 纯色模板方差为 0 时 NCC 无定义 → 返回 0 拒命中，宁可漏不可假阳。
- 性能基线（release，1080p 全屏 + 64px 模板）：**60ms**（截图 30 + 匹配 30）。

## 截图与坐标（易错，务必遵守）
- **`GetWindowRect` 必须配 `GetWindowDC`/`PrintWindow`**（原点=窗口左上角，含边框）。
  用 `ClientToScreen` 的客户区尺寸去配会右侧/底部缺一块。
- `PrintWindow` 有"返回 TRUE 但全黑"的假成功，必须检测并回退 BitBlt。
- 命中坐标换算成图内坐标的偏移只能由**后端返回**（`origin_x/origin_y`），
  不能前端反推 —— 多显示器时虚拟屏幕原点可能是负数。
- 跑 find 类接口时，截图必须和匹配用**同一份像素**（`find_image` 返回 `preview`），
  否则两次截屏之间画面变了，框会画在过时背景上。

## 多引擎进度（2026-10-22）
- M3.0/M3.1/M3.2/M3.3 全部完成：DDDDOCR + 目标检测 + pipeline 串联，均已实测并提交。
  - M3.1：`engine/ddddocr.rs`；提交 e67699c。M3.2：`engine/yolo.rs`；c78b006。
  - M3.3：`/vision/pipeline`；4783de4。M4 界面修复：fb666f9 / ce80187。
- **目标检测不按版本分家（用户明确要求）**：`EngineKind::Yolo` 无数参数，`key()` = `yolo`。
  结构在**加载时探测**（`YoloLayout`）：`inputs[0].dims` 定输入边长、`outputs[0].dims`
  判别 `[1,C,N]`/`[1,N,C]`（小的那维是通道 C）、`labels.txt` 类别数定 objectness
  （C==4+cls 无、C==5+cls 有）。覆盖 v5/v6/v7/v8/v10/v11/v12/v26 单输出导出。
  不接受的两类要**明确报错**：v3/v4 多输出 head、`[1,300,6]` 端到端（候选数远小于网格）。
- **letterbox 填充统一 114**：yolov5 官方默认色也是 114，早期写「v5 用 0」是错的。
- 模型目录 `models/detect/yolo/{model.onnx,labels.txt}`；旧 `v8/` 子目录自动下探一层。
- **跨引擎对齐测试图必须用 PNG**：JPEG 各家解码器 IDCT 有 ±1 抖动，会把 YOLO 置信度
  推动 0.001~0.01、大框 IoU 压到 0.97，全是噪声不是 bug。换 PNG 后 yolo_parity
  8/8 框 IoU=1.0000、conf 差=0。
- **letterbox 复刻要点**：Python `round()` = 银行家舍入 → Rust `round_ties_even()`；
  缩放复用 `ocr::preprocess::resize_bilinear`（cv2 逐位复刻）；先 NMS 后还原坐标。
- **ultralytics 会按发行版名自动装 `onnxruntime` 最新版**（本机原是 onnxruntime-directml），
  半途失败会混装出新 wrapper+旧 native 的坏包。已改装 onnxruntime==1.24.3（CPU）根治，
  本机 Python 的 RapidOCR 退回 CPU 推理。
- 无人值守 UI 验收：`scripts/ui_probe.py`（激活→按键→点击→同进程立刻截图）、
  `scripts/ui_paste.py`（CF_DIB 剪贴板 + Ctrl+V；**GlobalAlloc/GlobalLock 必须显式声明
  64 位 restype，否则句柄截断 access violation**）。
- 测试基线：`cargo test --lib` 73 通过；`--ignored` 22 项全绿（PP-OCR 基线 pipeline_match 逐字全对）。
- 详细记录见 `docs/PROGRESS.md`；多引擎契约与实录见 `docs/ENGINES.md` 6.1~6.4。

## DirectML 必须显式验证（2026-10-22，差点长期跑在 CPU 上）
- `SessionBuilder::with_execution_providers` 对**单个** EP 注册失败只打
  `ERROR ort::ep` 日志然后**跳过**，整体仍返回 `Ok`。所以「建会话成功」不能证明 DML 生效。
- 判据用 `ort::ep::directml::api()`（内部 `GetExecutionProviderApi("DML")`），
  `None` 就是这份 DLL 没编 DML。已封装成 `ocr::ort_util::directml_available()`。
- 症状：界面显示 GPU/DirectML，日志有四条 `DirectML is not supported in this build
  of ONNX Runtime`，实际全程 CPU。
- `runtime/onnxruntime.dll` 必须是**带 DirectML 的构建**（约 20MB）；CPU 版约 18MB。
  取法：`pip download --no-deps --only-binary :all: -d <tmp> onnxruntime-directml==1.24.3`
  然后从 wheel 的 `onnxruntime/capi/` 取 3 个 dll（不要装它，会跟 ultralytics 打架）。
- **换 DLL 后必须重跑 `pipeline_match`**（数值可能极微变，本轮实测不变）。

## Tauri 2 窗口坑（2026-10-22，用户反馈「拖不动、关不掉」）
- `core:default` 的 window 权限**只有只读命令**，**不含** `allow-start-dragging` /
  `allow-minimize` / `allow-toggle-maximize` / `allow-close`。必须在
  `src-tauri/capabilities/default.json` 显式声明（含 `allow-is-maximized`）。
  少了权限的表现是「点了没反应」——前端 `catch` 把 rejection 吞了。
- 想确认某个权限在不在 `core:default` 里，查 `src-tauri/gen/schemas/acl-manifests.json`
  里 `core:window.default_permission.permissions`，别靠猜。
- **`data-tauri-drag-region` 只认事件目标自身，不继承**：容器带了、里面的占位 div 没带，
  那片区域就是拖不动的。
- 自绘标题栏按钮的真实位置（相对窗口宽 w，标题栏高 44）：关闭 `w-24`、最大化 `w-60`、
  最小化 `w-96`，y=22。**别按截图比例估坐标**，截图与窗口 rect 不是 1:1。

## 界面规范补充（2026-10-22 用户反馈）
- 侧栏**不要蓝色、不要选中项左侧竖条**，蓝只给主动作按钮/图标，不进导航。
- **外壳配色（10-22 晚用户要求「大家都在用的那种稍微灰一点」）**：
  标题栏 + 左侧栏 `bg-chrome`（`--color-chrome: #f7f7f8`，在 `styles.css` 的 @theme 里），
  主区保持纯白，**靠底色差分层、不加分割线**。灰底上选中态用**纯白浮起**
  （`bg-white + ring-1 ring-black/[0.04]` + 极淡阴影），不要再用 `bg-neutral-100`（显脏）；
  hover 用黑 4~7% 叠加而不是 neutral 色阶。
- **卡片头第一行统一 `h-8 items-center`**：CardHeader 是 flex-col，若某些卡的标题行里塞了
  32px 按钮/徽章而别的卡没有，items-center 会让两类标题差 7px。所有 CardHeader 第一行都包
  一层 `flex h-8 items-center`。
- 小字号中文与 14px 图标混排时一律加 `leading-none`，否则文字盒 1.5 行高会让文字比图标偏下。
- PageHeader 用 `items-center`（不是 items-start），右上按钮才与「标题+描述」块视觉居中。

## 目标检测模型（2026-10-22 晚，已落地）
- **模型是用户自己的**：`models/detect/yolo/` 下每个 `*.onnx` 都是可选项（`model.onnx` 是默认），
  另外 config 的 `detect_models: Vec<String>` 记外部绝对路径（**只记路径不复制文件**）。
- **类名从 ONNX metadata 读**：`sess.metadata()?.custom("names")`（ultralytics 会写
  `names`/`task`/`imgsz`/`description`）。`labels.txt` 只是兜底，别再要求用户手写。
  解析 Python dict repr 的坑：单双引号、`\'` 转义、乱序 key、值里带数字都要处理。
- **用 metadata 的 `task` 拦非检测模型**（segment/pose/obb/classify）：它们输出多 mask 系数
  或关键点通道，按检测解不会崩、只会给错类别的框。
- `EngineSpec` 的 yolo variant = 模型 id（文件名去扩展名）：`yolo` = 默认，`yolo:<id>` = 指定。
  `yolo:v8` 这类旧写法仍解析但落回默认。**外部模型的 id 不能「像版本号」**，否则被当旧写法吃掉。
- `/engine/list` 只扫目录**不加载模型**，所以那时读不到类名；加载过后由
  `AppState::engine_status_list()`（HTTP/IPC 共用）回填 classes 并把 note 换成「已加载 N 类」。
- 文件选择框用 Win32 `GetOpenFileNameW`（feature `Win32_UI_Controls_Dialogs`）。
  **`lpstrDefExt`/`lpstrTitle` 的 `vec.as_ptr()` 必须先绑定到变量**，否则指向已 drop 的临时 Vec。
- `save_config` 里要**保留后端维护的 `detect_models`**（前端传的是旧快照），同 `webview_compat`。
