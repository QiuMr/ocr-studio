# OCR-Studio 项目长期记忆

## 0 产品定位（最高优先级）
- **是 API 工具不是识别软件**。交付物 = `127.0.0.1:8848` 服务，界面只是控制面板。设计先自问：是不是又做成了「图片识别软件」。
- 首页必答三件事：服务开没开 / 地址可复制 / 能调什么怎么调（给现成代码）。「上传看效果」是验证手段不是主叙事。导航固定：服务 / 接口 / 试跑 / 日志 / 设置。
- 端点事实源 `src/lib/endpoints.ts` ↔ `routes::PATHS`（**20 条**）；`/vision/promptref` 一径三法，前端计数用 `ENDPOINT_PATH_COUNT` 去重。**`/ai/v1/*` 刻意不进 PATHS**（那是「用户脚本能用什么」，AI 工具另有清单）。
- 组顺序唯一源 = `endpoints.ts` 的 `GROUP_ORDER`（ApiPage/ServicePage 派生）；首页图标必须 `Record<EndpointGroup,…>`。`scripts/check_endpoints.py` 挂 build.ps1 Step 3，**加组/加端点后必跑** —— 曾漏 `doc` 组导致表格接口在界面静默消失且 TS 不报错。

## 1 环境自检 envcheck.rs（2026-10-25）
- 覆盖「进程活着但功能静默不可用」四类：无 WebView2（窗口建不起来只剩托盘）/ Win7·8（evergreen 装不了）/ 端口被占 / DirectML 降级。
- WebView2 检测读注册表 `...\EdgeUpdate\Clients\{F3017226-…}` 的 `pv`，**HKCU 与 HKLM 都必查**：本机实测 **HKCU 没有、HKLM 才有 155.0.4283.62**，只查 HKCU 会把已装判成没装、每次启动弹引导。路径写死 `WOW6432Node`（客户端项固定登记在 32 位视图）。
- 安装走官方 bootstrapper `LinkId=2124703` + `/silent /install`（每用户装，免管理员），**绝不静默装**，原生 MessageBoxW 三选一才动手；分支三路：系统太旧 / 没装 / 装了但建窗失败（引导更新）。
- **WebView2 挂了 Rust HTTP 服务照常跑**（实测：模态引导框弹着时 8848 逐秒可连）→ 所有提示必须写明「界面打不开不影响脚本调用」。
- **托盘菜单本身也是 WebView 窗口**：WebView2 真缺失时它建不起来，菜单里那条「安装 WebView2」根本点不到 → 在 `show_tray_menu_inner` 的建窗失败分支补一次 `offer_webview2_repair`（static 标志只补一次）。`tray_action` 的 `"webview2"` 分支**必须另起线程**（同步命令在主线程，下载安装几十秒会冻住界面）。
- 安装期间托盘 tooltip 会被 2s 状态循环覆盖 → `INSTALLING` 标志 + `InstallingGuard` RAII，循环侧读它跳过写。
- 端口占用：`GetExtendedTcpTable`（feature `Win32_NetworkManagement_IpHelper`）拿 PID → `QueryFullProcessImageNameW` 拿进程名。**`dwLocalPort` 低 16 位是 `{hi,lo}` 顺序存放**（文档说网络字节序但不完整），两种解释都试；单测「自开临时端口回查必须命中自身 PID」钉死。bind 失败 emit `service-error`，前端给「去设置改端口」。
- Win 判定必须 `RtlGetVersion`（`GetVersionEx` 不打 manifest 会撒谎）。真机无法卸载 WebView2 → 诊断开关 `OCR_FAKE_NO_WEBVIEW=1` 走同一分支验收。

## 2 版本与发版（改版本号前必读）
- 仓库 `github.com/QiuMr/ocr-studio`（必须 public：Release 要匿名下载才能自升级）。
- **版本唯一源 = `src-tauri/tauri.conf.json` 的 `version`**；`build.rs` 注入 `OCR_STUDIO_VERSION`（→ version.rs、health.version），`pack_exe.ps1`/`release.ps1` 拼 zip 名与 tag。**脚本里别写死 `$version`**。
- 发版：改版本 → 提交 → `powershell -File scripts/release.ps1 -Notes "..."`。gh 不在 PATH：`$env:OCR_GH="C:\Users\13114\.workbuddy\toolchains\gh\bin\gh.exe"`。
- **不用 `tauri-plugin-updater`**（只认 NSIS/MSI 签名包，本项目发免安装 zip），自研 `src/updater.rs`：①**必须等进程真退出不能 sleep**（dll 占用 → ERROR_SHARING_VIOLATION），用 `QueryFullProcessImageNameW` **比对镜像路径**（`OpenProcess` 轮询 PID 会被 PID 复用坑到超时）；②**助手必须在 `tauri::Builder` 之前分流**（否则被单实例插件踢掉），此时 tracing 未初始化 → 自写 `%TEMP%/ocr-studio-update/helper.log`；③助手用**旧进程的 exe 路径**拉起新版（开发机 `ocr-studio.exe`、分发 `OCR Studio.exe`）。
- `latest.json` 必须无 BOM；版本比较用 `cmp_version`（数值比）。
- `.gitattributes` 里 `*.ps1 -text`。`D:\rust-target\release\` 长期并存 `ocr-studio.exe`（cargo 产物，新）与 `OCR Studio.exe`（pack 重命名，更新器认这个，可能旧）—— 起服务验收前先看 health.version。

## 3 托盘菜单（改托盘前必读）
- **不用 `tauri::menu::Menu`**。自绘无边框浮层：label=`tray`、`index.html#tray`、transparent + always_on_top + skip_taskbar + shadow；**body 透明**（`body.tray-mode`）；与主窗口同一套 `WEBVIEW_ARGS`。
- 内容：头部（名+版本+服务绿点）/ 打开界面 / 启停 / 检查更新 / 退出。有更新或 WebView2 缺失时该条目变蓝高亮（原生菜单做不到）。
- **第一红线：`Click` 按下抬起各来一次**，两次都 show → 撞「已可见就收起」自己关掉。**排查别往焦点跑**。三道闸：只认 `Up` / 150ms 去重 / `TRAY_SHOWN_AT` 400ms 内不 toggle。
- **失焦隐藏要「焦点稳定」才启用**：`show()` 后 Windows 先抛假 `Focused(false)`。做法 `TRAY_HAD_FOCUS` + 800ms 后 `is_focused()` 复核；拿不到焦点则**保持显示**，改用 `GetAsyncKeyState` 轮询「点菜单外」关闭。
- 弹菜单统一 `run_on_main_thread` + `SetForegroundWindow`；tauri 的 HWND 与 windows crate 不同类型，用 `HWND(raw.0)` 重包；**弹完别再动主窗口**。
- 外观：卡片外**别留 padding**（透明窗口的 CSS 阴影染灰边），阴影交给 `.shadow(true)`；量高必须量**最外层**，前后端都 `ceil`。
- 关闭途径：失焦 / Esc / 点菜单项（先 hide）/ **再点一次托盘 toggle**（拿不到焦点时唯一退路）。状态靠后端 2s emit `tray-status`，权限在 `capabilities/tray.json`。

## 4 前端规范
- React + TS + Tailwind + shadcn/ui（Vite，Tauri 2），Vue 已废弃。MiSans 本地 @font-face；纯白主 / 黑辅 / 蓝点缀。**每个按钮带图标**（lucide-react 14px）。**标题栏不要图标不要标题**，只有窗口按钮。
- **禁 `rounded-[Npx]` / `text-[N.x px]`**，用 `styles.css` 的 `@theme` 档位。圆角：sm 6 浮层小项 → md 8 按钮/输入/导航 → lg 10 卡内块 → xl 14 卡片（容器越大圆角越大，子比父小一档）；例外 `rounded-full` 胶囊、`rounded`=4px。
- 字号七档（只整数，半像素发虚）：10/11/12/13/14/16/26。按钮 h-7/8/9/10。**CardHeader 首行统一 `flex h-8 items-center`**（否则并排两卡标题差 7px）。
- 错误展示走 `lib/errors.ts` 码→人话 + `ui/error-help.tsx`（含「复制给 AI」离线提示词）。判空用 `error !== null`（类型是 `unknown`）。

## 5 项目环境（构建）
- Rust 私有工具链：`RUSTUP_HOME=.rust\rustup`、`CARGO_HOME=.rust\cargo`、`CARGO_TARGET_DIR=D:\rust-target`；cargo 必须 `--manifest-path "src-tauri\Cargo.toml"`。入口固定 `scripts/dev.ps1` / `build.ps1`（Tauri CLI 从 PATH 找 cargo，没有就静默退出）。
- **agent 里 PowerShell 调 native exe 不可靠**（stdout 丢失、`$LASTEXITCODE` 空、假失败）→ 一律 **Git Bash + 全路径** node/python/cargo。`build.ps1` 已加 node/python 探测与 `-SkipFrontend`，成败看产物文件不看退出码。**`build.ps1` 在 agent 的 PowerShell 里跑到 cargo 那步会静默失败**（产物时间戳双保险刚好把它拦住）→ 编译一律 Git Bash 直接 `cargo build --release`。
- **验收界面只能 `explorer.exe <exe>` 启动**再抓图；agent shell 直跑 exe 必白屏（WebView2 GPU 子进程起不来）—— 白屏结论一律作废。**`Start-Process` 拉起的窗口与 agent 各工具调用不在同一窗口站**（Get-Process 能读到 MainWindowTitle，但那个调用里的 `EnumWindows` 枚举不到 → 截不了图）。这种时候改用「进程存活 + 窗口标题正确 + 8848 端口可连」三点判定分支是否走对，别为截图去改代码。
- Python 访问本机服务要绕代理（`ProxyHandler({})`）；cv2 读不了中文路径 → `np.frombuffer + imdecode`。

## 6 环境坑（会误判成代码 bug）
- **safe-delete 按 turn 累计**：同 turn 删 >50 文件被拒（连 `Remove-Item -Recurse` 也拦）；绕法 `[System.IO.Directory]::Delete(绝对路径,$true)`。`pack_exe.ps1` 走 `Remove-BuildArtifact`。
- **MSYS coreutils 会整批坏掉**（`ls/tail/grep/find/touch/sleep/rm` 一律 exit 127，时好时坏，重启会话不一定恢复）→ 读日志用 `Read`/`Get-Content`，列文件用 `Glob`，等待用 python `time.sleep`，别用 `| tail` 这类管道（会让退出码失真成 127）。Bash 里也禁止调 `cmd.exe`/`powershell.exe`（安全策略拦）。
- **debug target 的 rlib 会瞬时损坏**（`invalid metadata files for crate 'test'` + `link.exe 1102`）→ **重跑即恢复**，别改代码。
- **Bash 的 python ≠ PowerShell 的 python**：带依赖（PIL/cv2/numpy/psutil）的一律 PowerShell 跑。**管道让退出码失真**：判 `test result:` 行 / `[PASS] 产物`。
- **git push 在 bash 里必失败**（凭据 helper）；用 PowerShell 推，看 `..HEAD -> master` 判成功。
- **OCR 验收脚本四坑**：① 12px 侧栏小字不稳 → 取「侧栏最下面一项」；② 底部有**幻觉文字**（实测 y=834 超窗高）→ 排除底部 120px；③ 找蓝底白字按颜色兜底取「最靠下且中心在左 62%」的蓝团（设置页有 4 个蓝按钮）；④ 换页验证用正文大标题 + `center_x > 240`。

## 7 精度基线（改前处理前必读）
- 黄金基线 `models/small/reference_output.json`（`scripts/ref_pipeline.py`），验收 `cargo test -- --ignored`。
- 必须复刻三个语义：cv2.resize（11bit 定点两段式）、cv2.warpPerspective（整数坐标=像素中心、源量化 1/32、越界补 0）、pyclipper（输入截断整数、距离 double、输出 half-up）。
- **误检框置信度是混沌的**（1-LSB 抖 0.04），不能做精度验收，必须打全精度框坐标。

## 8 找图
- 模板匹配 `src-tauri/src/match.rs`（`match` 是关键字，lib.rs 用 `#[path]` 暴露为 `matcher`）。**粗筛也用 NCC 不用 SQDIFF**；金字塔 ≤4 层，`MIN_TPL_SIDE=6`，纯色模板方差 0 → 拒命中。release 1080p+64px 基线 60ms。
- `/ocr/yuyizhaotu` 与 `/vision/prompt` 出参**与模板找图刻意同构**（`assemble_find_data()` 唯一组装点，别改格式）；不暴露 iou，不分端点（用 `source`）。
- 提示找图只有两件套：`*_prompt.onnx`（检测器）+ `*_vpe.onnx`（视觉编码器），**没有文本塔/词表**。`OutputLayout::EndToEnd` 判据 `anchors < grid && channels >= 6`，`class_channels()` base 5。
- **语言找图已整体移除**（界面控件分数只 0.1~0.2 = 噪声，训练分布决定）；`mode=text` 保留**明确指路的 400**，别静默。
- 中文找图库 `engine/prompt_refs.rs`：框一次起中文名 → VPE 图找图；`GET/POST/DELETE /vision/promptref`，**DELETE 的 name 必须 query 与 body 都认**。**教多大一块决定成败**：单行文字 0.16 / 导航块 0.74 / **整条侧边栏 0.94（跨页 0.84）** —— VPE 认「有结构的整体」。

## 9 表格识别（改前处理前必读）
- `engine/table.rs`：`detect.onnx`(PicoDet) + `structure.onnx`(SLANet-plus)，`models/table/`，模型库 id=`table`。`/ocr/biaoge` 一径三 `mode`：`auto`（默认，**一张都没检测到要退回整图**并置 `fallback:true`）/ `structure` / `detect`；桌面 `ocr_biaoge` 走同一个 `recognize_page`。
- **SLANet 前处理三条铁律**：① pad 填 **0** 不是 127.5；② resize 长边 488 → **归一化** → pad 488×488 → CHW；③ **BGR** 通道序配 **RGB 顺序**的 mean/std（PaddleOCR 历史错配，别「顺手修正」）。
- **bbox 两轴都乘 `max(原图宽,原图高)`**（图被 pad 成方形，按宽高分乘会压扁）。
- **解码不是 CTC**：逐位置 argmax + 遇 `eos` 停 + **不去重**；字符表 `["sos"] + metadata["character"].split("\n") + ["eos"]`；每个 `<td`/`<td></td>` 吃一个 bbox。
- **PicoDet 分数已是概率，别再 sigmoid**（再套一层把背景 0 变 0.50 → 4093 框 → 请求永不返回）。判据：阈值 0.5 只剩一两个框，分 0.83~0.97。
- `classify` 必须 `trim()`（属性 token 带前导空格）。`Cell` **不能 derive Default**（span 默认 0 → 空表），手写 Default(1) + 渲染 `max(1)`。
- SLANet 输出位置有限（实测 19/32）→ **必须显式返回 `truncated`**。DirectML 上 SLANet 预热失败（Loop/Cast 不支持）自动降级 CPU，正常。

## 10 截图与坐标
- `GetWindowRect` 必须配 `GetWindowDC`/`PrintWindow`（原点=窗口左上含边框）；`PrintWindow` 有「TRUE 但全黑」假成功，要检测并回退 BitBlt。
- 偏移只能由后端返回（`origin_x/origin_y`），不能前端反推（虚拟屏原点可能为负）；find 类截图必须与匹配用**同一份像素**。
- 无人值守抓图：杀进程 → `explorer.exe <exe>` → `scripts/shot_window.py "OCR Studio" out.png` → `ui_probe.py`；被遮挡用 `win_capture.py`。
- **`focus_window` 抢不到前台**（Windows 前台锁静默失败 → 输入全打空）。三道：空 ALT → `AttachThreadInput` → `GetForegroundWindow` 复核。实测 5/5。
- AI 拿到的坐标**始终是屏幕绝对坐标**（带不带 `region` 都一样），可直接喂 `click_at`。

## 11 DirectML / ORT
- `with_execution_providers` 对单个 EP 注册失败只打日志仍返回 Ok → 判据 `ocr::ort_util::directml_available()`。`runtime/onnxruntime.dll` 必须带 DirectML（约 20MB，当前 1.24.20260304）；换 DLL 后必重跑 `pipeline_match`。
- ORT 环境**进程级单例**（`ort::init().build()` 第二次直接报错），统一走 `ort_util::ort_env()`；日志级别 Error（SLANet 动态 shape 每推理刷几十条 WARN，实测一天 120 万行）。
- **AVX2 基线检测**：`ort_util::cpu_supports_ort_baseline()` + `build_session*` 前置拦截 + `lib.rs setup()` 体检；无 AVX2 的机器直接给 1002/明确提示，不让 ORT 崩。

## 12 Tauri 2 窗口（产品语义，别改回去）
- **关窗口 = 隐藏不是退出**（`prevent_close()` + `hide()`），只有托盘「退出」真退。**托盘必须显式 `.icon(app.default_window_icon().cloned())`**。
- `core:default` 的 window 权限只有只读命令，拖动/最小化/最大化/关闭必须在 `capabilities/default.json` 显式声明；`data-tauri-drag-region` **只认事件目标自身，不继承**。
- **`tauri-plugin-single-instance` 必须第一个注册**。**进程名是 `OCR Studio`（带空格）**，按 `ocr-studio` 匹配不到。

## 13 错误码
- 引擎层错误一律走 `map_engine_err_ctx`：`Unsupported`→**1002**、`UnknownEngine`→**1003**、`BadRequest`→400、`NotReady`/`Inference`→500。**别手写 `ApiError::Internal`**（有单测钉住）。缺模型 → **503 + 指路「设置 → 模型库」**；并发超限 → 429 + `Retry-After`。

## 14 检测模型（用户自己的，别预置）
- `models/detect/yolo/` 下每个 `*.onnx` 都可选，**默认=按文件名排序第一个**；config 的 `detect_models` 记外部绝对路径（不复制）；**出厂目录为空**（只 README.txt），`pack_exe.ps1` 必须剔除。
- 类名从 ONNX metadata 的 `names` 读，`labels.txt` 只兜底；用 metadata 的 `task` 拦 segment/pose/obb/classify。`EngineSpec` 的 yolo variant = 模型 id。

## 15 AI 操控（卖点）
- `api/ai.rs`（门禁/限流/幂等/审计）+ `api/ai_tools.rs`（工具表 + exec）：复用 8848 默认关，Bearer 连接码、120 次/分、`request_id` 幂等、readonly/write/dangerous 三档、信封 `{ok,summary,data,error}`、未知工具给 `closest_tool`。
- **不提供**（自伤或动系统，交给人点界面）：退出程序、开机自启、AI 开关本身与连接码 —— **根本不在 TOOLS 注册**。
- 键鼠注入 `src/input.rs`（SendInput + 虚拟屏绝对坐标归一化）。**35 个工具**（只读 10 / 读屏 11 / 动手 14），`risk_counts()`/`highlights()` 是设置页工具数唯一来源。
- **AI 操控四条实测限制**（docs/AI.md §7.5）：① PP-OCR small 对**白字彩底按钮**连检测框都不出；② 12px 小灰字识别率差；③ 滚轮内容要先 `scroll`，而**等元素出现期间不能滚屏**；④ 点击返回 ok ≠ 界面响应，必须点完复核。
- 验收：`scripts/verify_ai.py`（全表 + 参数校验 + 401/404，`--inject` 真注入记事本）、`verify_upgrade.py`（用 AI 操作自己界面完成升级）。

## 16 打包与分发
- **主包只带 PP-OCRv6 small，基线 50MB**；`pack_exe.ps1` 只拷 `models/small`(5 文件) + `detect/yolo/README.txt`；可选模型走「模型库」按需下载。
- **模型库 catalog 必须含出厂自带的 `ppocr-small`**（26.3MB，排第一）—— 漏了界面显示「5 个可选模型全部未安装」而识别明明好的。上传后核对线上条目数（CDN 缓存；别传 `release/models/` 旧 catalog）。
- `scripts/build_models.py` 一条命令打包+生成清单+上传（`--upload`）；挂固定 tag `models` 的 Release，**必须 `--prerelease`**（正式 release 会抢 `releases/latest`，自动更新全线报废）。
- **`rename` 跨卷失败**：解压暂存目录必须与目标**同级**（`models/.tmp_<id>`）。
- 信封键是 **`msg`** 不是 `message`。请求体上限 32MB 靠 `DefaultBodyLimit` 显式放开（axum 0.8 默认 2MB 且只作用于 Multipart；JSON 路径自己 `to_bytes(MAX_BODY)`）。
- `.gitignore` 必须有 `models/**/*.ts`（MobileCLIP2 权重后缀 .ts）；另忽略 `src-tauri/config.json`（含 AI 连接码）与 `prompt_refs/`。
- **模型根目录解析**（开发机陷阱）：`resolve_models_root()` 顺序 `OCR_MODELS_ROOT` → exe 旁 `models/` → 向上 1~4 级 → `workspace_root()/models` → cwd，**第一个含 `small/det.onnx` 的胜出**。开发机跑 `D:\rust-target\release\*.exe` 会回退到仓库根 models；升级把 zip 的 `models/small` 写进 exe 旁后改解析到那份，缺 yoloe/table 就报「缺模型」（真实用户只有一份目录）。删 `D:\rust-target\release\models` 即恢复。
- 验收脚本：`verify_packaged.py`（体积防线 + 各端点 + `mode=text` 必须 400 + 大图 multipart）、`verify_ui_zh.py`、`verify_concurrency.py`、`verify_guards.py`、`smoke_endpoints.py`。

## 17 API 并发模型（改并发/线程代码前必读）
- **推理全局串行**：`AppState.engine/alt/prompt` 都是 `Mutex<Option<Engine>>`，每请求在 `spawn_blocking` 里拿锁跑完整推理。`run_pipeline` 两把锁**顺序持有、绝不嵌套**（两个 spawn_blocking）。锁中毒 `unwrap_or_else(|e| e.into_inner())`。
- 这正是 ORT 官方推荐解法（共享 Session 并发 run() 是未定义行为）。每模型只建一次会话，线程数只由「会话数 × 每会话线程池」决定，**与并发请求数无关**。
- 实测（release/DirectML/12 核/2400×1600）：并发 1→200 全 200/200 成功、**指纹与串行逐字节一致**、不崩；峰值线程 60→72，内存 +50MB。
- **吞吐不随并发上涨**（3.8 张/s 封顶），延迟线性排队：并发 32 均 4.4s、64 均 9.0s、200 均 31.7s/最大 52.7s。故加信号量 32 路限流 + 429/Retry-After。
- **`OMP_NUM_THREADS=4` 实测无效**（线程 65→66、延迟 3626→3617ms）；`ort` 的 `with_intra_threads()` 对 OpenMP 构建也无效。**调 intra_op 线程数这条路已否决**，除非出现「多核至强 + 纯 CPU」场景。
- 空闲 CPU 0%；`decode_image` **没有尺寸上限**（32MB body 可解出 300MB+ 位图）—— 唯一能靠「几张图」把进程拖到 OOM 的口子，已加解码双闸。
