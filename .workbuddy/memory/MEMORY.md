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
- M3.0/M3.1/M3.2 完成：DDDDOCR（共享 PP-OCR det 行检测）+ YOLO v8 检测均已实测落地并提交。
  - M3.1：`engine/ddddocr.rs`，`engine` 参数贯通 HTTP/Tauri，AltEngine 单槽；提交 e67699c。
  - M3.2：`engine/yolo.rs` + `/vision/detect` + 错误码 1002/1003；提交 c78b006。
- **跨引擎对齐测试图必须用 PNG**：JPEG 各家解码器 IDCT 有 ±1 抖动，会把 YOLO 置信度
  推动 0.001~0.01、大框 IoU 压到 0.97，全是噪声不是 bug。换 PNG 后 yolo_parity
  8/8 框 IoU=1.0000、conf 差=0。
- **letterbox 复刻要点**：Python `round()` = 银行家舍入 → Rust `round_ties_even()`；
  缩放复用 `ocr::preprocess::resize_bilinear`（cv2 逐位复刻）；v8 填 114；先 NMS 后还原坐标。
- **ultralytics 会按发行版名自动装 `onnxruntime` 最新版**（本机原是 onnxruntime-directml），
  半途失败会混装出新 wrapper+旧 native 的坏包。已改装 onnxruntime==1.24.3（CPU）根治，
  本机 Python 的 RapidOCR 退回 CPU 推理。
- 无人值守 UI 验收：`scripts/ui_probe.py`（激活→按键→点击→同进程立刻截图）、
  `scripts/ui_paste.py`（CF_DIB 剪贴板 + Ctrl+V；**GlobalAlloc/GlobalLock 必须显式声明
  64 位 restype，否则句柄截断 access violation**）。
- 测试基线：`cargo test --lib` 63 通过；`--ignored` 21 项全绿（PP-OCR 基线 pipeline_match 逐字全对）。
- M3 剩余：M3.3 `/vision/pipeline`（YOLO 找区域 → PP-OCR 认字）；v5/v7 缺模型未实测；
  pack_exe.ps1 未复验（zip 已含 ddddocr 54MB + yolo 19MB）。
- 详细记录见 `docs/PROGRESS.md`；多引擎契约与实录见 `docs/ENGINES.md` 6.1/6.2。
