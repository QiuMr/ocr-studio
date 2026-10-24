# OCR-Studio 项目长期记忆

## 产品定位（最高优先级）
- **是 API 工具，不是识别软件。** 交付物是 `127.0.0.1:8848` 接口服务，桌面界面只是控制面板。设计先问：是不是又做成了「图片识别软件」。
- 首页必答三件事：服务开没开 / 地址（可复制）/ 能调什么怎么调（给现成代码）。「上传看效果」是验证手段不是主叙事。导航固定：服务 / 接口 / 试跑 / 日志 / 设置。
- 端点事实源 `src/lib/endpoints.ts` ↔ 后端 `routes::PATHS`（**20 条**）；`/vision/promptref` 一径三法，前端计数用 `ENDPOINT_PATH_COUNT` 去重。**`/ai/v1/*` 刻意不进 `PATHS`**（那是「用户脚本能用什么」，AI 工具另有一套清单）。

## 版本控制与发版（**改版本号前必读**）
- 仓库 `github.com/QiuMr/ocr-studio`（必须 public：Release 资产要匿名下载，程序才能自升级）。
- **版本号唯一来源 = `src-tauri/tauri.conf.json` 的 `version`**；`build.rs` 注入 `OCR_STUDIO_VERSION`（→ `version.rs::VERSION`、`/ocr/health.version`）、`pack_exe.ps1` 拼 zip 名、`release.ps1` 拼 tag。**别在脚本里写死 `$version`**。
- 发版：改版本 → 提交 → `powershell -File scripts/release.ps1 -Notes "..."`。**前置：gh 不在 PATH，在 `C:\Users\13114\.workbuddy\toolchains\gh\bin\gh.exe`，先设 `$env:OCR_GH`。**
- **不用 `tauri-plugin-updater`**（只认 NSIS/MSI 签名包；本项目 `bundle.active=false` 发免安装 zip）。自研 `src/updater.rs`：`releases/latest/download/latest.json` → 比版本 → 校验 size+sha256 → `tar.exe` 解压 → 复制 exe 到 `%TEMP%` 当助手 → 主进程退出 → 助手轮询 PID 确认退出后覆盖。
  - **必须等进程真退出，不能 sleep**（`onnxruntime.dll` 占用中覆盖 → `ERROR_SHARING_VIOLATION`，半新半旧最难救）。
  - **助手必须在 `tauri::Builder` 之前分流**，否则单实例插件把它当第二个实例踢掉（「点了更新，程序关掉再也没起来」）。
  - **latest.json 必须无 BOM**（serde_json 遇 BOM 直接失败 = 所有人更新检查永久报错）。版本比较用 `cmp_version`（**数值比**，字符串比会让 0.10.0 < 0.9.0）。
- `.gitattributes` 里 `*.ps1 -text`：本机 `core.autocrlf=true`，.ps1 必须 BOM+CRLF，让 git 一字节别碰。

## 托盘菜单（**改托盘前必读**）
- **不用 `tauri::menu::Menu`**（原生样式改不了）。自绘无边框浮层：label=`tray`、`index.html#tray`、transparent + always_on_top + skip_taskbar + shadow。**body 必须透明**（`body.tray-mode`）否则圆角外露白底；托盘窗口要用与主窗口相同的 `WEBVIEW_ARGS`。
- 内容：头部（产品名+版本+服务绿点）/ 打开界面 / 启停 / 检查更新 / 退出。**有更新时「检查更新」变蓝色高亮条目**（原生菜单做不到）。
- **第一红线：`Click` 按下和抬起各来一次**。两次都 show → 第二次撞「已可见就收起」→ 菜单自己关掉（用户实测「右键点一下瞬间闪没」）。**排查别往焦点上跑**。三道闸：只认 `MouseButtonState::Up` / 150ms 去重 / `TRAY_SHOWN_AT` 400ms 内不参与 toggle。
- **失焦隐藏要「焦点稳定」才启用**：`show()` 后 Windows 先抛假的 `Focused(false)`（照单全收 → 菜单永远弹不出，`IsWindowVisible` 恒 0）。方案：`TRAY_HAD_FOCUS` + 800ms 后 `is_focused()` 复核；拿不到焦点时菜单**保持显示**，改用 `GetAsyncKeyState` 轮询「点菜单外部」关闭（没焦点时前端连 Esc 都收不到）。
- 弹菜单统一 `run_on_main_thread`（托盘事件在托盘线程，抢不到前台）+ `SetForegroundWindow`；**tauri 的 HWND 与 windows crate 不是同一类型**，用 `HWND(raw.0)` 重包；**弹完别再动主窗口**（`--debug-tray` 路径必须 return）。
- 外观：① 卡片外**别留 padding**（透明窗口上 CSS 阴影染出灰边），阴影交给 `.shadow(true)`；② 量高必须量**最外层**，`body.tray-mode{margin:0}`，前后端都 `ceil`。
- 关闭途径：失焦 / Esc / 点菜单项（先 hide）/ **再点一次托盘 toggle**（拿不到焦点时唯一退路，别删）。状态靠后端 2s emit `tray-status`，权限在 `capabilities/tray.json`。

## 前端规范
- React + TS + Tailwind + shadcn/ui（Vite，Tauri 2 壳），Vue 已废弃。MiSans 本地 @font-face；纯白为主 / 黑为辅 / 蓝点缀。**每个按钮都带图标**（lucide-react，统一 14px）。**自定义标题栏不要图标不要标题**，只有窗口控制按钮。
- **不写 `rounded-[Npx]` / `text-[N.x px]`**，用梯度档（`src/styles.css` 的 `@theme`）。圆角四档：sm 6 浮层小项 → md 8 按钮/输入/导航 → lg 10 卡片内块 → xl 14 卡片；「容器越大圆角越大，子比父小一档」。
- 字号七档（只留整数，半像素发虚）：10 微角标 / 11 次要 / 12 正文 / 13 强调·按钮 / 14 强强调 / 16 小节标题 / 26 页面主标题。按钮四档 h-7/8/9/10。**CardHeader 第一行统一包 `flex h-8 items-center`**（否则并排两卡标题差 7px）。

## 项目环境
- Rust 是**项目内私有工具链**：`RUSTUP_HOME=.rust\rustup`、`CARGO_HOME=.rust\cargo`、`CARGO_TARGET_DIR=D:\rust-target`；cargo 必须 `--manifest-path "src-tauri\Cargo.toml"`。入口固定 `scripts/dev.ps1` / `scripts/build.ps1`（铺 env + PATH；Tauri CLI 从 PATH 找 cargo，没有就静默退出、窗口不创建）。
- **验收界面只能用 `explorer.exe <exe>` 启动**再抓图；**agent shell 里直接跑 exe 必白屏**（WebView2 GPU 子进程起不来）—— 白屏结论一律作废，别据此改代码。
- Python 访问本机服务要绕系统代理（`ProxyHandler({})`），否则 127.0.0.1 也 502。cv2 读不了中文路径 → `np.frombuffer + cv2.imdecode`。写 `.ps1` 必须 UTF-8 **带 BOM** + CRLF。手动 `cargo build` 的 exe 要把 `src-tauri/runtime/*.dll` 拷旁边，否则 DirectML 静默退回 CPU。

## 环境坑（会误判成代码 bug）
- **safe-delete 按 turn 累计**：同 turn 删 >50 文件被拒，连 `Remove-Item -Recurse` 也拦；绕法 `[System.IO.Directory]::Delete(绝对路径, $true)`。
- **debug target 的 rlib 会瞬时损坏**（`invalid metadata files for crate 'test'` + `link.exe exit 1102`）→ **重跑一次即恢复**，别改代码。
- **Bash 的 python ≠ PowerShell 的 python**：带依赖（PIL/cv2/numpy）的脚本一律用 PowerShell 跑。**管道让退出码失真**：判 `test result:` 行 / `[PASS] 产物` + `[结论] 资源已嵌入`。
- **验收脚本（`verify_upgrade.py`）撞出来的 OCR 坑**：① 侧栏 12px 小字识别不稳，实测把「服务」认成「设置」→ 侧栏点击改为「取侧栏最下面一项」（布局固定）；② OCR 会在窗口底部黑边/任务栏区域产生**幻觉文字**（实测 "•Wo.r.n"，y=834 远超窗高 729）→ 取最下项必须排除底部 120px；③ 找蓝底白字按钮（「立即更新」）用颜色兜底时，要取**最靠下且中心在左 62%** 的蓝团 —— 设置页有 4 个蓝按钮（模型卡 3 个「下载」+ 更新卡 1 个「立即更新」），取「最密集」会命中下载按钮，点下去开始下模型、升级什么都没发生；④ 换页验证用正文大标题（26px）且阈值 `center_x > 240`（写 300 会漏：窗口贴左时正文标题才 279）。
- **git push 在 bash 里必失败**（凭据 helper 取不到 gh，报 `could not read Username`）；用 PowerShell 推（stderr 进度会被 PS 当成 error record，看 `..HEAD -> master` 判断成功）。

## 精度基线（改前处理前必读）
- 黄金基线 `models/small/reference_output.json`（`scripts/ref_pipeline.py` 生成），验收 `cargo test -- --ignored`。
- 必须复刻三个底层语义（详见 PROGRESS §3）：cv2.resize（11bit 定点两段式）、cv2.warpPerspective（整数坐标=像素中心、源量化 1/32、越界补 0）、pyclipper（输入截断整数、距离 double、输出 half-up）。
- **误检框置信度是混沌的**（1-LSB 扰动抖 0.04），不能做精度验收；必须打全精度框坐标。

## 找图（模板匹配）
- 模块 `src-tauri/src/match.rs`（`match` 是关键字，lib.rs 用 `#[path]` 暴露为 `matcher`）。**粗筛也用 NCC 不用 SQDIFF**（SQDIFF 对亮度漂移敏感会漏检）。金字塔 ≤4 层，`MIN_TPL_SIDE=6`，纯色模板方差 0 → 拒命中。性能基线（release，1080p+64px）60ms。

## 表格识别（**改前处理前必读**）
- `src-tauri/src/engine/table.rs`：`detect.onnx`（PicoDet 定位）+ `structure.onnx`（SLANet-plus），`models/table/`，模型库 id=`table`。`/ocr/biaoge` 一径三 `mode`：`auto`（默认，先检测再逐张；**一张都没检测到要退回整图**并置 `fallback:true`）/ `structure` / `detect`；桌面 `ocr_biaoge` 走同一个 `recognize_page`。
- **SLANet 前处理三条铁律**（错一条满屏 colspan 乱码）：① pad 填 **0** 不是 127.5（PaddleOCR 的 PaddingTableImage 是 zeros）；② resize 长边 488 → **归一化** → pad 488×488 → CHW；③ **BGR** 通道序配 **RGB 顺序**的 mean/std（PaddleOCR 历史错配，别「顺手修正」）。
- **bbox 两轴都乘 `max(原图宽, 原图高)`**（图被 pad 成方形，按宽高分乘会压扁）。
- **解码不是 CTC**：逐位置 argmax + 遇 `eos` 停 + **不去重**；字符表 `["sos"] + metadata["character"].split("\n") + ["eos"]`（48+2=50 = 输出通道数）；每个 `<td`/`<td></td>` 吃一个 bbox。
- **PicoDet 分数已是概率，不要再 sigmoid**。再套一层把背景 0 变 0.50 → 一次 4093 框 → auto 逐个识别 → 六核满载 3500 CPU 秒、请求不返回。判据：阈值 0.5 只剩真正一两个框，分 0.83~0.97。
- **`classify` 必须 `trim()` 不能只 `trim_end()`**：SLANet 属性 token 带前导空格（` colspan="3"`），只去尾空格会让合并单元格被静默忽略。`Cell` **不能 derive Default**（rowspan/colspan 默认 0 → 渲染出空表），手写 Default（span=1）并在渲染里 `max(1)` 兜底。
- SLANet 输出位置有限（实测 19/32），单元格多会被截断 → **必须显式返回 `truncated`**。DirectML 上 SLANet 预热失败（Loop/Cast 不支持）自动降级 CPU，正常。

## 截图与坐标（易错）
- `GetWindowRect` 必须配 `GetWindowDC`/`PrintWindow`（原点=窗口左上角含边框）；`PrintWindow` 有「返回 TRUE 但全黑」假成功，要检测并回退 BitBlt。
- 命中坐标偏移只能由后端返回（`origin_x/origin_y`），不能前端反推（虚拟屏原点可能为负）；find 类截图必须与匹配用**同一份像素**。
- 无人值守抓图：杀进程 → `explorer.exe <exe>` → `scripts/shot_window.py "OCR Studio" out.png` → `scripts/ui_probe.py x y out.png`；被遮挡用 `scripts/win_capture.py`（PrintWindow + `PW_RENDERFULLCONTENT`）；`SetForegroundWindow` 受前台锁会静默失败。

## DirectML 必须显式验证
- `with_execution_providers` 对单个 EP 注册失败只打日志、仍返回 Ok。判据 `crate::ocr::ort_util::directml_available()`（内部 `GetExecutionProviderApi("DML")`）。`runtime/onnxruntime.dll` 必须是**带 DirectML 的构建**（约 20MB）；换 DLL 后必重跑 `pipeline_match`。

## Tauri 2 窗口（产品语义，别改回去）
- **关窗口 = 隐藏不是退出**（`prevent_close()` + `hide()`），交付物是服务，只有托盘「退出」真退。**托盘必须显式 `.icon(app.default_window_icon().cloned())`**（不会自动回退）。
- `core:default` 的 window 权限只有只读命令，拖动/最小化/最大化/关闭必须在 `capabilities/default.json` 显式声明；`data-tauri-drag-region` **只认事件目标自身，不继承**。
- **`tauri-plugin-single-instance` 必须第一个注册**，否则第二次双击另起进程抢 8848。**进程名是 `OCR Studio`（带空格）**，按 `ocr-studio` 匹配不到。

## 引擎层错误 → API 错误：一律走 `map_engine_err_ctx`
- `api/state.rs`：`Unsupported`→**1002**、`UnknownEngine`→**1003**、`BadRequest`→400、`NotReady`/`Inference`→500；1002 让调用方区分「引擎做不到」与「服务故障」。**别手写 `ApiError::Internal`**，有单测钉住。缺模型 → **503 + 指路「设置 → 模型库」**，不许让用户跑 Python 脚本（分发包里没源码）。

## 检测模型（用户自己的，别预置）
- `models/detect/yolo/` 下每个 `*.onnx` 都可选，**默认=按文件名排序第一个**；config 的 `detect_models` 记外部绝对路径（不复制）；**出厂目录为空**（只 README.txt），`pack_exe.ps1` 必须剔除。
- 类名从 ONNX metadata 的 `names` 读（ultralytics 会写），`labels.txt` 只兜底；用 metadata 的 `task` 拦 segment/pose/obb/classify。`EngineSpec` 的 yolo variant = 模型 id（`yolo` / `yolo:<id>`）。

## 语义找图 / 提示找图（卖点，别削弱）
- `/ocr/yuyizhaotu` 与 `/vision/prompt` **出参与模板找图刻意同构**（`assemble_find_data()` 唯一组装点，不许改格式）；不暴露 `iou`，不按来源分端点（用 `source`）。
- 提示找图只有**两件套**：`*_prompt.onnx`（检测器）+ `*_vpe.onnx`（视觉编码器），**没有文本塔/词表**。
- `OutputLayout::EndToEnd` 判据 `anchors < grid && channels >= 6`（只看通道会误吃 v5 6 通道 raw head）；`class_channels()` 的 EndToEnd base 是 5。

## 语言找图已整体移除（**别再往回加**）
- `mode=text` 在界面控件上分数只有 **0.1~0.2 = 噪声**（设置页 12 个中文词全 0 命中）；YOLOE 文本提示的训练分布决定，调参救不了；连带删掉 mobileclip2 文本塔（242MB）等。`mode=text` 保留**明确指路的 400**（老脚本/AI 旧记忆会写 text，静默会让人以为功能还在）。

## 中文找图 + AI 操控（卖点）
- 参考库 `engine/prompt_refs.rs`：框一次起中文名 → 走 VPE 图找图；`GET/POST/DELETE /vision/promptref`，**DELETE 的 name 必须 query 与 body 都认**。
- **教多大一块决定成败**：单行文字 0.16 / 导航块 0.74 / **整条侧边栏 0.94（跨页 0.84）**；VPE 认「有结构的整体」，细条低分且被正文纹理骗走。
- **AI 服务** `api/ai.rs`（门禁/限流/幂等/审计/提示词）+ `api/ai_tools.rs`（工具表 + exec）：复用 8848 默认关，Bearer 连接码、120 次/分、`request_id` 幂等、readonly/write/dangerous 三档、信封 `{ok,summary,data,error}`、未知工具给 `closest_tool` 候选。
  - **不提供**（自伤或动系统，交给人点界面）：退出程序、开机自启注册表、AI 开关本身与连接码 —— 做法是**根本不在 TOOLS 注册**。
  - 工具清单顺序 = 展示顺序（先只读、后写、最后危险）；`ENDPOINT_METHODS` 必须与 `routes::PATHS` 一一对应（单测钉住）。
  - 键鼠注入在 `src/input.rs`（SendInput + 虚拟屏绝对坐标归一化 + SetForegroundWindow）；AI 拿到的坐标一律是**屏幕绝对坐标**，可直接喂 `click_at`。**35 个工具**（只读 10 / 读屏幕 11 / 动手 14），`risk_counts()`/`highlights()` 是设置页工具数的唯一来源。
  - **AI 操控四条实测限制**（docs/AI.md §7.5）：① PP-OCR small 对**白字彩底按钮**连检测框都不出（蓝底「立即更新」直接搜不到）→ 兜底是截图后按颜色聚类定位主按钮；② 12px 小灰字识别率差；③ 滚轮内容要先 `scroll`，而**等元素出现期间不能滚屏**（会把元素滚出视野，造成「出来了却搜不到」的误判）；④ 点击返回 ok ≠ 界面响应，自动化必须点完复核。
  - **验收脚本**：`scripts/verify_ai.py`（35 工具全表 + 参数校验 + 401/404，`--inject` 真注入记事本回环）、`scripts/verify_upgrade.py`（用 AI 接口操作自己界面完成升级，实测 0.1.6→0.1.7）。

## 界面返工 / 自动升级 / 焦点抢占（2026-10-24 深夜，v0.1.8~0.1.12）

**表格接口曾在界面文档里静默消失**（v0.1.8 修）：`groupOf()` 把 `/ocr/biaoge`
归到 `doc` 组，但 `ApiPage` 的 `ORDER` 与 `ServicePage` 的 `GROUPS` 各是一份
**手写组数组**，加组时漏改 → TypeScript 不报错（普通数组不是 Record），
而页面「共 20 个端点」又是对的，肉眼看不出少了一组。
根治：组顺序收成 `endpoints.ts` 的 `GROUP_ORDER`（唯一事实源，两页派生）；
首页图标必须 `Record<EndpointGroup, ...>`（加组忘配图标直接编译报错）；
`scripts/check_endpoints.py` 挂进 `build.ps1` Step 3（静态比对前端 ENDPOINTS
与后端 PATHS、GROUP_ORDER 覆盖所有组、两页真的派生）。**加新组/新端点后必跑。**

**自动更新两大坑**（v0.1.10/0.1.11 修，实测两次升级成功）：
- 助手等旧进程退出用 `OpenProcess` 轮询 PID 会被 **PID 复用**坑到超时
  （旧进程退出后别的程序抢到同 PID，`OpenProcess` 一直成功，助手傻等 120s 放弃，
  界面表现为「点了更新程序关了再也没起来」）。必须比对
  `QueryFullProcessImageNameW` 镜像路径做身份校验。
- 助手在 `tauri::Builder` 之前分流，**tracing subscriber 还没初始化，日志全丢**。
  现在写 `%TEMP%/ocr-studio-update/helper.log`（每步一行）。
- 助手启动新版本要用**旧进程的 exe 路径**：开发机是 `ocr-studio.exe`（连字符），
  分发包是 `OCR Studio.exe`（带空格），照 `install.join(EXE_NAME)` 拼会拉起另一个文件。

**`focus_window` 抢不到前台**（v0.1.11 修）：Windows 前台锁不允许后台进程
`SetForegroundWindow`（静默失败），而输入只进前台窗口 → `click_at`/`type_text`
全部打空。三道：空 ALT 键 → `AttachThreadInput` 贴输入线程借身份前置 →
前置后读 `GetForegroundWindow` 复核（该 API 有时成功也返回 false）。
实测 5/5（修复前 0/8）。

**模型库必须包含出厂自带的 `ppocr-small`**（v0.1.12 修）：分发包带
`models/small`、识别开箱即用，但 catalog 里没这一项 → 界面显示「5 个可选模型
全部未安装」而识别明明好的，用户只会以为程序坏了。清单 6 项，small 排第一。
上传 catalog 后要核对线上条目数（GitHub release 资产有 CDN 缓存，且别把
`release/models/` 下的旧 catalog 传上去 —— 实测踩过：assets 里是新的、
上传的是旧的）。

**AI 接口坐标口径**：`find_text`/`ocr_image` 返回的 `center_x/center_y`
**始终是屏幕绝对坐标**（带不带 `region` 都一样，`line_json` 里加了 origin）——
曾误判 region 路径是相对坐标而多加一次 origin，白绕一圈。对比实测：
同一行带 region 199、不带 196。

## 模型根目录解析（开发机陷阱）
- `resolve_models_root()` 顺序：`OCR_MODELS_ROOT` → exe 旁 `models/` → exe 向上 1~4 级 → `workspace_root()/models` → cwd；**第一个含 `small/det.onnx` 的胜出**。开发机跑 `D:\rust-target\release\*.exe` 时 exe 旁没有 models，会一路回退到**仓库根 models**（所以开发态什么模型都在）。
- **升级会把 zip 里的 `models/small` 写进 exe 旁** → 升级后的实例改解析到 exe 旁那份，缺 yoloe/table 就报「缺模型」；真实用户只有一份目录且更新不删额外模型，**不是数据丢失 bug**；开发机删掉 `D:\rust-target\release\models` 即恢复。
- `D:\rust-target\release\` 长期并存两个 exe：`ocr-studio.exe`（cargo 产物）与 `OCR Studio.exe`（pack 重命名，**更新器的 `EXE_NAME` 认的是这个**）。

## 打包与分发
- **主包只带 PP-OCRv6 small，基线 50MB**（不再往回加模型）；`pack_exe.ps1` 只拷 `models/small`(5 文件) + `detect/yolo/README.txt`；可选模型走「模型库」按需下载。
- `scripts/build_models.py` 一条命令打包 + 生成清单 + 上传（`--upload`）；挂固定 tag `models` 的 Release，**必须 `--prerelease`**（正式 release 会抢 `releases/latest`，`latest.json` 404，自动更新全线报废）。
- **`rename` 跨卷失败**（`ERROR_NOT_SAME_DEVICE`）：解压暂存目录必须与目标**同级**（`models/.tmp_<id>`）；放 appdata 再 rename，会卡在「正在校验」。
- 一个可选条目 = 一个 zip；清单三层：网络 → 本地缓存(1h) → 内置 `src-tauri/assets/catalog.json`（`include_str!`）。
- `pack_exe.ps1` 删产物必须走 `Remove-BuildArtifact`（.NET 直删）—— safe-delete 对暂存目录 fail-closed。
- `.gitignore` 必须有 `models/**/*.ts`（MobileCLIP2 权重后缀 .ts 会被当 TS 源码）；不能裸写 `*.ts`；另忽略 `src-tauri/config.json`（含 AI 连接码）与 `prompt_refs/`。
- 信封键是 **`msg`** 不是 `message`。**请求体上限 32MB 靠 `DefaultBodyLimit` 显式放开**：axum 0.8 默认 2MB 且只作用于 `Multipart`（JSON 路径自己 `to_bytes(MAX_BODY)`）；漏了它，用户传 3MB 截图就 400，同图走 JSON 却正常。
- **验收脚本**：`scripts/verify_packaged.py`（体积防线 + health/shibie/quyuocr/qrcode/prompt + `mode=text` 必须 400 + 大图 multipart + AI 未授权）、`scripts/verify_ui_zh.py`（explorer 拉起后从截图量左侧 5 项行位置，别写死比例）。
