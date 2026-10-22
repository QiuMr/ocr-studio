# OCR-Studio 项目长期记忆

## 前端技术规范（2026-10-21 用户确定，必须遵守）
- 技术栈：**React + TypeScript + Tailwind CSS + shadcn/ui**（Vite，Tauri 2 壳不变）。
  原 Vue 3 + shadcn-vue 方案已废弃，`src/` 下 Vue 骨架需整体替换。
- 字体：**MiSans**（小米），`@font-face` 本地引入并设为全局默认；商用授权需确认后再随包分发。
- 配色：**纯白为主，黑色为辅，蓝色点缀**。
- **每个按钮都必须带图标**（用 `lucide-react`，shadcn/ui 默认图标源）。
- **自定义标题栏：不要图标、不要标题**，仅窗口控制按钮。
  `tauri.conf.json` 已设 `decorations: false`，前端用 `getCurrentWindow()` 自绘。

## 产品定位（2026-10-22 用户纠正，**最高优先级**）
- **OCR-Studio 是一个 API 工具，不是识别软件。** 交付物是跑在 `127.0.0.1:8848` 的接口服务，
  桌面界面只是它的控制面板。任何界面设计先问：这是不是又把产品做成了「图片识别软件」。
- 首页必须回答三件事：服务开没开 / 地址是什么（可复制）/ 能调什么怎么调（给现成代码）。
  「上传图片看效果」是验证手段，不是主叙事。
- 导航固定 5 项：**服务 / 接口 / 试跑 / 日志 / 设置**。
- 端点事实源：`src/lib/endpoints.ts`（首页概览与接口页同源），改后端路由必须同步。

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
- **启动入口固定两个脚本**（2026-10-22 晚用户报「界面又起不来」后定）：
  开发启动 `scripts/dev.ps1`、编译启动 `scripts/build.ps1`。
  真因是 Tauri CLI 从 PATH 找 `cargo`，而本项目 Rust 是**项目内私有工具链**
  （平时都用全路径调，所以裸 `cargo build` 从不暴露），PATH 没有就当场退出、
  **窗口根本不创建**。脚本负责铺 `RUSTUP_HOME`/`CARGO_HOME`/`CARGO_TARGET_DIR`
  并把 `.rust\cargo\bin` 前置进 PATH。
- **验收界面只能用 `explorer.exe <exe>` 启动**，再 `scripts/shot_window.py` 抓图。
  **agent shell 里直接跑 exe 必白屏**（该 shell 让 WebView2 的 GPU 子进程起不来），
  这是**环境问题不是产品缺陷**，同一 exe 用资源管理器启动就正常。
  agent shell 里的白屏/「前端未报到」结论**一律作废，不要据此改代码**
  —— 本轮就因此把已知环境问题当成看门狗 bug 改了一轮又回退（见 PROGRESS 0.1 / 7.7）。

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

## 托盘常驻 + 单实例（2026-10-22，**产品语义，别改回去**）
- **关窗口 = 隐藏，不是退出**：`WindowEvent::CloseRequested` → `prevent_close()` + `hide()`。
  交付物是 API 服务，界面只是控制面板，服务必须能在后台活着。
- 托盘菜单：首行只读状态（`● 运行中 · 127.0.0.1:端口` / `○ 已停止`，ticker 每 2s 刷新、
  **变了才更新**）+ 打开界面 / 启动服务 / 停止服务 / 退出。**只有「退出」是真正退出。**
- **托盘必须显式给图标**：`TrayIconBuilder` 在 tauri-2.12.4 里**不会**回退到
  `default_window_icon()`，不设也能 build 成功、只是图标不可见。写
  `.icon(app.default_window_icon().cloned())`。
- **`Menu::get` 参数是 `"id"` 不是 `&"id"`**（`get<'a, I>(&'a I)` + `MenuId: PartialEq<&'a I>`）；
  返回 `MenuItemKind`，要 `.as_menuitem().cloned()` 才有 `set_text`/`set_enabled`；
  这些更新必须 `app.run_on_main_thread(...)`。
- **`tauri-plugin-single-instance` 必须是第一个注册的插件**。没有它，第二次双击会另起
  进程抢 8848，失败后界面显示「服务已停止」，用户会以为程序坏了。
- **进程名是 `OCR Studio`（带空格）**，不是 `ocr-studio`。杀进程/数实例都按后者写会一个
  都匹配不到（本轮因此「以为杀了其实没杀」）。
- 托盘图标可见性**没做过截图验证**（Win11 折叠进溢出面板，合成 `mouse_event` 点不开，
  试三次都落到输入法图标上）。行为用 `scripts/verify_tray.py` 验，图标请人工看任务栏。

## 后台任务退出码（2026-10-22）
- `cargo test ... | Select-Object -Last N` 之类的**管道会让退出码失真**：任务报 failed，
  实际 `test result: ok. 82 passed`。判基线看 `test result:` 行或单独取 `$LASTEXITCODE`。


## 目标检测模型（2026-10-22 晚，已落地）
- **模型是用户自己的**：`models/detect/yolo/` 下每个 `*.onnx` 都是可选项，**默认模型 =
  同名级 onnx 按文件名排序的第一个**（早期硬编码 `model.onnx` 优先，已删）。
  另外 config 的 `detect_models: Vec<String>` 记外部绝对路径（**只记路径不复制文件**）。
- **出厂目录为空**（只有 `README.txt`）：程序不预置任何检测模型。**`pack_exe.ps1` 必须
  把 `models/detect/yolo/` 里的模型从分发包剔掉**，否则开发机上的模型跟着发出去，
  又变回「内置模型」（用户明确反对这件事）。
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

## 语义找图（2026-10-22 深夜，提交 4a6ec9f，**卖点，别削弱**）
- 端点 `/ocr/yuyizhaotu`（第 15 个）。来源由 body 的 `source` 给（image/screen/window，
  复用 `acquire_haystack`），阈值 `conf`（0.25），`class_filter` 按类别 id。
- **出参与模板找图刻意同构**：`assemble_find_data()` 是唯一组装点，
  `to_find_data`（模板）/ `to_find_data_from_dets`（语义）都走它。
  新字段 `engine`/`class_id`/`class_name` 带 `skip_serializing_if`，
  **模板找图出参一个字节没变**。已有脚本换路径即可，不许改成新格式。
- **不暴露 `iou`**（NMS-free 模型不做 NMS）；**不按来源分三个端点**（用 `source`）。
- `OutputLayout::EndToEnd`（`[1,N,6+nm]`）判据是 **`anchors < grid && channels >= 6`**：
  只看通道数会误吃 v5 系 6 通道 raw head。`class_channels()` 的 base
  **`EndToEnd` 是 5 不是 6**（4 坐标 + 1 类别下标列），写错会让分割模型类别整体错位。
- YOLOE-26 导出 `scripts/fetch_yoloe.py`：**本机只能走视觉提示**（文本提示要 ultralytics
  的 CLIP fork，拉 git 子模块在本机 MSYS 必失败 `0xC0000142`）。必须打补丁绕开
  ultralytics 8.4.13 的 end2end fuse bug（先置空 cv3/cv4 再 fuse）；补丁内要 `import torch`。
  配 `-seg` 模型要用 `YOLOEVPSegPredictor`。权重缓存到 `models/_fixtures/weights/`。
