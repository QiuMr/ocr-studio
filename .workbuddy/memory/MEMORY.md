# OCR-Studio 项目长期记忆

## 产品定位（最高优先级，2026-10-22）
- **OCR-Studio 是 API 工具，不是识别软件。** 交付物是跑在 `127.0.0.1:8848` 的接口服务，桌面界面只是控制面板。任何界面设计先问：是不是又把产品做成了「图片识别软件」。
- 首页必答三件事：服务开没开 / 地址（可复制）/ 能调什么怎么调（给现成代码）。「上传图片看效果」是验证手段，不是主叙事。
- 导航固定 5 项：服务 / 接口 / 试跑 / 日志 / 设置。
- 端点事实源 `src/lib/endpoints.ts`（首页概览与接口页同源），改后端路由必须同步。目前 **19 个端点**（认字 5 / 找图 9 / 看图 3 / 状态 2）；`/vision/promptref` 一个路径三种方法，前端计数用 `ENDPOINT_PATH_COUNT`（按路径去重）对齐后端 `PATHS`。

## 版本控制与发版（2026-10-23，**改版本号前必读**）
- 仓库 `github.com/QiuMr/ocr-studio`（**public**，因为 Release 资产要匿名可下载，程序才能自己升级；私有仓库拉资产要 token）。
- **版本号唯一来源 = `src-tauri/tauri.conf.json` 的 `version`。** 三处引用：`build.rs` 注入 `OCR_STUDIO_VERSION`（→ `version.rs::VERSION`、`/ocr/health.version`）、`pack_exe.ps1` 拼 zip 名、`release.ps1` 拼 tag。**别再往脚本里写 `$version = "x.y.z"`**（曾经有，漏改就导致 zip 名和程序内版本对不上）。`tauri.conf.json` 里没有的 `package.json` 版本没人读，不用管。
- 发版：改 tauri.conf.json → 提交 → `powershell -File scripts/release.ps1 -Notes "..."`。脚本会拦「工作区未提交 / tag 已存在 / 产物缺失」。
- **自动更新不用 `tauri-plugin-updater`**：它只认安装器（NSIS/MSI）的签名包，本项目 `bundle.active=false` 发免安装 zip。结论已钉死，别再试。
- 自研更新 `src/updater.rs`：`releases/latest/download/latest.json`（固定资产路径）→ 比对版本 → 下载校验 size+sha256 → `tar.exe` 解压 → 复制当前 exe 到 `%TEMP%` 当助手 → 主进程退出 → 助手轮询 PID 确认退出后覆盖。
  - **必须等进程真退出，不能 sleep**：`onnxruntime.dll` 被加载着，覆盖会 `ERROR_SHARING_VIOLATION`，目录半新半旧最难救。
  - **助手必须在 `tauri::Builder` 之前分流**，否则单实例插件把它当第二个实例踢掉（「点了更新，程序关掉再也没起来」）。
  - **latest.json 必须无 BOM**：serde_json 遇 BOM 直接解析失败，等于所有人更新检查永久报错。
  - 版本比较用 `version.rs::cmp_version`（**数值比**，字符串比会让 0.10.0 判成比 0.9.0 旧）。
- `.gitattributes` 里 `*.ps1 -text`：本机 `core.autocrlf=true`，而 `.ps1` 必须 BOM+CRLF，让 git 一字节都别碰。

## 托盘菜单（2026-10-23 重做，**改托盘前必读**）
- **不用 `tauri::menu::Menu`**（系统原生菜单，白底黑字无图标、样式改不了）。改成自己画的无边框浮层窗口：label=`tray`，URL `index.html#tray`（`main.tsx` 按 hash 切 `TrayMenu`），transparent + always_on_top + skip_taskbar + shadow。**body 必须透明**（`body.tray-mode`），否则圆角外露白底、比原生菜单还丑。
- 菜单内容：头部（产品名 + 版本徽标 + 服务状态绿点）、打开界面、启停服务、检查更新、退出。**有更新时「检查更新」变蓝色高亮条目**（`有更新 v0.1.3` + 蓝点）—— 这是原生菜单做不到的。
- **第一红线：`Click` 事件按下和抬起各来一次**（2026-10-24 用户实测「右键点一下瞬间闪没」）。
  两次都调 show → 第二次撞上「已可见就收起」的 toggle 分支 → 菜单自己把自己关掉。
  **排查方向别跑到焦点上去**，我第一次就是被带偏的。三道闸：只认
  `MouseButtonState::Up` / 150ms 去重 / `TRAY_SHOWN_AT` 400ms 内不参与 toggle。
  双击不再单独处理（双击前必然已有 Click(Up)）。
- **失焦隐藏要「焦点稳定」才启用**：show 后 800ms 复核 `is_focused()`，
  还在才允许失焦关闭。老逻辑「拿到过焦点就算」会被 `Focused(true)`→`Focused(false)`
  的抖动击穿。判定拿不到焦点时菜单**保持显示**，改用 `GetAsyncKeyState`
  轮询「点击菜单外部」关闭 —— 没焦点时前端连 Esc 都收不到（键盘消息只给前台窗口）。
- 弹菜单统一 `run_on_main_thread`（托盘事件在托盘线程，抢不到前台）+ Win32 `SetForegroundWindow`。
  **tauri 的 HWND 与本项目 windows crate 的不是同一类型**，用 `HWND(raw.0)` 重新包。
- 外观两条：① 卡片外面**别留 padding**（透明窗口上 CSS 阴影会染出一圈灰边，
  看着像套了个框），阴影交给 `.shadow(true)`（画在窗口矩形外，不占内容区）；
  ② 量高必须量**最外层**（含 padding/border），量内层会切掉底部一截，
  `body.tray-mode` 还要 `margin:0`，前后端都 `ceil`。
  1. **失焦隐藏不能照单全收**：`show()` 后 Windows 先抛一个假的 `Focused(false)`，直接隐藏会让菜单永远弹不出来（窗口建好、位置也对，但 `IsWindowVisible` 恒为 0）。现方案：`TRAY_HAD_FOCUS` 标记（先拿到过焦点才允许失焦隐藏）+ 600ms 宽限后用 `is_focused()` 复核。
  2. **弹菜单后别再动主窗口**：`--debug-tray` 路径弹完菜单必须 return，否则 single-instance 回调接着 show/set_focus 主窗口，菜单立刻失焦被收。
  3. **托盘窗口要跟上主窗口的渲染参数**（`WEBVIEW_ARGS`），否则主窗口切软件合成后托盘窗口会白。
- 关闭途径：失焦 / Esc（前端）/ 点菜单项（`tray_action` 里先 hide）/ **再点一次托盘 toggle**（这条是「菜单拿不到焦点」时的唯一退路，别删）。
- 状态刷新：后端 2 秒 emit `tray-status` 事件，菜单订阅；不再写菜单项文字（那是原生菜单时代的做法）。
- 权限在 `src-tauri/capabilities/tray.json`（windows: ["tray"]），别忘同步。

## 前端技术规范（2026-10-21 用户确定）
- 栈：React + TS + Tailwind + shadcn/ui（Vite，Tauri 2 壳）。Vue 方案已废弃。
- 字体 MiSans（本地 @font-face）；配色纯白为主、黑为辅、蓝点缀。
- **每个按钮都必须带图标**（lucide-react）。**自定义标题栏不要图标不要标题**，仅窗口控制按钮（`decorations:false`）。

## 视觉规范：圆角与字号（2026-10-23，**改界面前必读**）
- **不写 `rounded-[Npx]` / `text-[N.x px]`**，用梯度档。梯度在 `src/styles.css` 的 `@theme` 覆盖 Tailwind 默认（`--radius-sm/md/lg/xl`、`--text-xs/sm`）。
- 圆角四档：sm 6px（浮层小项/行内码）→ md 8px（按钮/输入/触发器/tab/导航/窗口按钮）→ lg 10px（卡片内块：分段容器/滚动区/提示条/搜索框）→ xl 14px（卡片/面板/大容器）。规则「容器越大圆角越大；子比父小一档」。例外：`rounded-full` 胶囊、`rounded`=4px 微图标。
- 字号七档（只留整数，半像素发虚）：10 微角标 / 11 次要·表头 / 12 正文 / 13 强调·卡片标题·按钮 / 14 强强调 / 16 小节标题 / 26 页面主标题。
- 按钮四档：xs h-7 text-12 / sm h-8 text-12 / default h-9 text-13 / lg h-10 text-13；icon-sm h-8 w-8、icon-xs h-7 w-7。**图标统一 14px**（`h-3.5 w-3.5`）。
- 手写控件也要落档：h-7/h-8 → `rounded-md`+`text-[12px]`；分段控件「容器 rounded-lg + 项 rounded-md」（8+2=10 同心）。
- **CardHeader 第一行统一包 `flex h-8 items-center`**，否则并排两卡标题差 ~7px。小字中文配 14px 图标加 `leading-none`。PageHeader 用 `items-center`。

## 项目环境约定
- Rust 是**项目内私有工具链**：`RUSTUP_HOME=...\.rust\rustup`、`CARGO_HOME=...\.rust\cargo`、`CARGO_TARGET_DIR=D:\rust-target`；cargo 必须带 `--manifest-path "src-tauri\Cargo.toml"`。
- **启动入口固定两个脚本**：开发 `scripts/dev.ps1`、编译 `scripts/build.ps1`（它们铺 env + 把 `.rust\cargo\bin` 前置进 PATH；Tauri CLI 从 PATH 找 cargo，没有就静默退出、窗口不创建）。
- **验收界面只能用 `explorer.exe <exe>` 启动**再抓图。**agent shell 里直接跑 exe 必白屏**（WebView2 GPU 子进程起不来），是环境问题不是产品缺陷 —— 白屏/「前端未报到」结论一律作废，不要据此改代码。
- Python 访问本机服务必须绕系统代理（`ProxyHandler({})`），否则 127.0.0.1 也拿 502。
- cv2 读不了中文路径 → `np.frombuffer + cv2.imdecode`。
- 写 `.ps1` 必须 UTF-8 **带 BOM** + CRLF（PS 5.1 按 GBK 解码无 BOM 文件）。`pack_exe.ps1` 内嵌说明文件，改完必须重加 BOM。
- 手动 `cargo build` 出的 exe 必须把 `src-tauri/runtime/*.dll` 拷到旁边，否则 DirectML 静默退回 CPU。

## 环境坑（会误判成代码 bug）
- **safe-delete 按 turn 累计**：同一 turn 删 >50 文件被拒（`SAFE_DELETE_BULK_CONFIRM_REQUIRED`），连 `Remove-Item -Recurse` 也拦。绕法 `[System.IO.Directory]::Delete("<绝对路径>", $true)`。
- **debug target 的 rlib 会瞬时损坏**（`invalid metadata files for crate 'test'` + `link.exe exit 1102`）→ **重跑一次即恢复**，别改代码。
- **Bash 里的 `python` ≠ PowerShell 里的**：带依赖（qrcode/PIL/cv2/numpy）的脚本一律用 PowerShell 跑。
- **管道让退出码失真**：`cargo test | Select-Object` 报 failed 其实成功，判 `test result:` 行。
- `build.ps1` 同理，判成功看 `[PASS] 产物` + `check_embedded_assets.py` 的 `[结论] 资源已嵌入`。

## 精度基线（改动前后处理前必读）
- 黄金基线 `models/small/reference_output.json`（`scripts/ref_pipeline.py` 生成）；验收 `cargo test -- --ignored`。
- 必须复刻的三个底层语义（详见 PROGRESS §3）：cv2.resize（11bit 定点两段式）、cv2.warpPerspective（整数坐标=像素中心、源量化 1/32、越界补 0）、pyclipper（输入截断整数、距离 double、输出 half-up）。
- **误检框置信度是混沌的**（1-LSB 扰动能抖 0.04），不能用它做精度验收；必须打**全精度**框坐标。

## 找图（模板匹配）
- 模块 `src-tauri/src/match.rs`（`match` 是关键字，lib.rs 用 `#[path]` 暴露为 `matcher`）。
- **粗筛也用 NCC 不用 SQDIFF**（SQDIFF 对亮度漂移敏感会漏检）。金字塔 ≤4 层，`MIN_TPL_SIDE=6`。纯色模板方差 0 → 拒命中。
- 性能基线（release，1080p + 64px 模板）：60ms。

## 截图与坐标（易错）
- `GetWindowRect` 必须配 `GetWindowDC`/`PrintWindow`（原点=窗口左上角含边框）；`PrintWindow` 有「返回 TRUE 但全黑」假成功，要检测回退 BitBlt。
- 命中坐标换算偏移只能由后端返回（`origin_x/origin_y`），不能前端反推（多显示器虚拟屏原点可能为负）。
- find 类接口截图必须与匹配用**同一份像素**（`find_image` 返回 `preview`）。
- 无人值守截图流程：杀进程 → `explorer.exe <exe>` → `scripts/shot_window.py "OCR Studio" out.png` → `scripts/ui_probe.py x y out.png`。被遮挡用 `scripts/win_capture.py`（PrintWindow + `PW_RENDERFULLCONTENT`）；`SetForegroundWindow` 受前台锁会静默失败。

## DirectML 必须显式验证
- `with_execution_providers` 对单个 EP 注册失败只打日志然后跳过、仍返回 Ok。「建会话成功」不能证明 DML 生效。
- 判据 `crate::ocr::ort_util::directml_available()`（内部 `GetExecutionProviderApi("DML")`）。
- `runtime/onnxruntime.dll` 必须是**带 DirectML 的构建**（约 20MB）；换 DLL 后必须重跑 `pipeline_match`。

## Tauri 2 窗口 / 托盘（产品语义，别改回去）
- **关窗口 = 隐藏不是退出**（`prevent_close()` + `hide()`），交付物是服务。只有托盘「退出」真退。
- 托盘菜单首行只读状态（ticker 2s、变了才更新）+ 打开界面/启动/停止/退出。**托盘必须显式 `.icon(app.default_window_icon().cloned())`**（不会自动回退）。
- `core:default` 的 window 权限只有只读命令，拖动/最小化/最大化/关闭必须在 `capabilities/default.json` 显式声明。
- `data-tauri-drag-region` **只认事件目标自身，不继承**。
- **`tauri-plugin-single-instance` 必须第一个注册**，否则第二次双击另起进程抢 8848。
- **进程名是 `OCR Studio`（带空格）**，杀进程按 `ocr-studio` 匹配不到。

## 引擎层错误 → API 错误：一律走 `map_engine_err_ctx`（别绕）
- `src-tauri/src/api/state.rs`。映射：`Unsupported`→**1002**、`UnknownEngine`→**1003**、`BadRequest`→**400**、`NotReady`/`Inference`→500。
- 1002 的意义是让调用方区分「引擎做不到」与「服务故障」，都拍 500 就没法写自动降级。新写「调引擎」的入口用 `map_engine_err` / `map_engine_err_ctx`，**别手写 `ApiError::Internal`**。有单测 `engine_error_mapping_*` 钉住。

## 检测模型（用户自己的，别预置）
- `models/detect/yolo/` 下每个 `*.onnx` 都是可选项，**默认 = 按文件名排序第一个**。config 的 `detect_models` 记外部绝对路径（只记路径不复制）。
- **出厂目录为空**（只有 README.txt）；`pack_exe.ps1` 必须把 detect 模型从分发包剔掉。
- 类名从 ONNX metadata 的 `names` 读（ultralytics 会写），`labels.txt` 只是兜底；用 metadata 的 `task` 拦非检测模型（segment/pose/obb/classify）。
- `EngineSpec` 的 yolo variant = 模型 id：`yolo`=默认、`yolo:<id>`=指定。

## 语义找图 / 提示找图（卖点，别削弱）
- `/ocr/yuyizhaotu`（语义）与 `/vision/prompt`（提示）**出参与模板找图刻意同构**：`assemble_find_data()` 是唯一组装点，模板找图出参一个字节没变，不许改成新格式。不暴露 `iou`；不按来源分端点（用 `source`）。
- **提示找图只有两件套**（2026-10-23 起）：`*_prompt.onnx`（检测器，双输入 images+pe）、`*_vpe.onnx`（视觉提示编码器）。**没有文本塔、没有 texthead、没有词表**——语言找图整条已删（见下）。
- `OutputLayout::EndToEnd` 判据 `anchors < grid && channels >= 6`（只看通道会误吃 v5 6 通道 raw head）；`class_channels()` 的 EndToEnd base 是 5 不是 6。

## 语言找图已整体移除（2026-10-23 用户拍板，**别再往回加**）
- `mode=text` 实测在界面控件上分数只有 **0.1~0.2 = 噪声**（设置页截图 12 个中文词全 0 命中，「按钮」放宽到 conf=0.10 才 1 命中 0.207）。是 YOLOE 文本提示的训练分布决定的，调参救不了。
- 连带删除：`mobileclip2_b.ts`(242MB)、`tokenizer*.json`、`*_texthead.*`、`zh_words.rs`、`text_pe_cache.rs`、`bin/bake_text_pe.rs`、`mobileclip2-b-rs` 依赖、2 项文本塔 parity。**原「`text_pe` 逐位复刻 + 中文只放 `pe_for_words`」的架构红线随之作废。**
- `mode=text` 保留**明确指路的 400**（不是静默、不是 404）：老脚本/AI 旧记忆都会写 `text`，静默会让人以为「功能还在只是没找到」。
- 分发包 345.3MB → **224.6MB**；`verify_packaged.py` 有体积防线，文本塔文件回流即 FAIL。

## 中文找图（只剩参考库）+ AI 操控服务（卖点，别削弱）
- `engine/prompt_refs.rs` 中文视觉参考库：框一次起中文名 → 走 VPE 图找图。`GET/POST/DELETE /vision/promptref`，**DELETE 的 name 必须 query 与 body 都认**（`?name=中文` 是 AI 最自然写法）。
- 找的时候用 **`mode=ref`**（`pe_for_refs(names, refs)` 只查参考库）；未命中的名字明确报错 + 列出已有名字 + 提示先教。
- **教多大一块决定成败**（实测首位命中）：单行文字 0.16 / 整行 0.16 / 导航块 0.74 但跨页掉出前 3 / **整条侧边栏 0.94、跨页 0.84**。VPE 认「一块有结构的整体」，细条既低分又会被正文纹理骗走。
- **AI 服务** `api/ai.rs` + `api/ai_tools.rs`（复用 8848，默认关）：Bearer 连接码、120 次/分限流、`request_id` 幂等、写操作审计、readonly/write/dangerous 三档；信封 `{ok,summary,data,error}`；未知工具给 `closest_tool` 候选；`/ai/v1/*` **刻意不进 `PATHS`**（那是用户脚本清单）。工具 `find_target`/`teach_target`/`list_targets`（原名 `find_word` 已改）。设置页「AI 控制」面板管理开关/连接码/提示词。

## 打包与分发
- **主包只带 PP-OCRv6 small，体积基线 50MB**（2026-10-24 起）。其余模型全部走设置页
  「模型库」按需下载。zips 从 225.4MB 砍到 50.3MB。**不要再往回加模型**——
  `pack_exe.ps1` 只拷 `models/small`（5 个文件）+ `detect/yolo/README.txt`。
- 可选模型：`scripts/build_models.py` 一条命令打包 + 生成清单 + 上传（`--upload`）。
  模型挂固定 tag `models` 的 Release，**必须 `--prerelease`**：正式 release 会抢走
  `releases/latest`，`latest.json` 404，自动更新全线报废。
- **`rename` 跨卷会失败**（`ERROR_NOT_SAME_DEVICE`）：解压暂存目录必须和目标**同级**
  （`models/.tmp_<id>`）。放 `%APPDATA%` 再 rename 到 D: 会把「包下好了、校验过了、
  解压也成了」全卡在最后一步，界面停在「正在校验」不动。
- 一个可选条目 = 一个 zip（PP-OCRv6 一档 5 个文件，逐个下发无法判断「装没装全」）。
- 清单三层：网络 → 本地缓存(1h) → 内置 `src-tauri/assets/catalog.json`（`include_str!`）。
- `scripts/pack_exe.ps1` 产 `release/OCR-Studio-<ver>-win64.zip`（免安装）。剔掉：`*.pt`、`*.bak`、`ref_*.json`、`reference_*.json`、`inspect.json`。**新增夹具目录要同步加进来**。
- `.gitignore` 必须有 `models/**/*.ts`（MobileCLIP2-B 权重后缀 `.ts` 会被当 TS 源码，253MB）。不能裸写 `*.ts`。另需忽略 `src-tauri/config.json`（运行期配置，含 AI 连接码）与 `prompt_refs/`。
- 信封键是 **`msg`** 不是 `message`（`ApiResp { code, msg, data }`）。
- **验收脚本 `scripts/verify_packaged.py`**：从解压目录起服务跑「不该带什么」体积防线 + health/shibie/quyuocr/qrcode + prompt 相关（模型没装时标 `[SKIP]`，靠 `PROMPT_OK` 判断）+ `mode=text` 必须 400 指路 + >2MB 大图 multipart + AI 未授权。改打包流程或模型清单后必须复跑。
- **用户可见的错误文案不许让用户去跑 Python 脚本**（分发包里没有源码）。缺模型一律指路「设置 → 模型库」。
- **界面验收脚本 `scripts/verify_ui_zh.py`**：explorer 拉起 exe → **从截图里量左侧导航 5 项行位置**（不写死比例，写死过一次导致点在正文区、两张图同页、被误读成「找图不准」）→ 教「左侧侧边栏」→ 同图 + 跨页按名字找。调用 400 响应必须当正常响应读（urllib 默认抛 HTTPError）。
- **请求体上限 32MB 靠 `DefaultBodyLimit` 显式放开**（`routes.rs` 的 `build_router_with`）：
  axum 0.8 默认只给 **2MB**，且只作用于 `Multipart` extractor（JSON 路径自己用
  `to_bytes(body, MAX_BODY)`，不受影响）。**漏了它，用户 multipart 传 3MB 截图就 400**
  「Error parsing multipart/form-data request」，同一张图走 JSON 却正常 —— 极难排查。
- 分发包现约 **224.6MB**（PP-OCR tiny/small/medium 三档是主要体积；文本塔移除后省 242MB）。
- `pack_exe.ps1` 删构建产物必须走 `Remove-BuildArtifact`（.NET 直删）——safe-delete trash 对暂存目录抛 fail-closed。

## 当前状态（2026-10-23 晚）
- 全部大需求已落地：区域OCR、蒙版模板匹配、二维码/条码、语义找图、提示找图（图找图 + 按中文名找）、AI 操控服务、界面精修、全面检查。端点 **19**（认字 5 / 找图 9 / 看图 3 / 状态 2）——语言找图移除**不减少端点数**（那是 `mode` 的一个取值，不是独立路径）。
- 测试基线：`cargo test --lib` **127 passed**；`api_smoke` **17 passed + 10 ignored**；`ai_service` **7 passed**；`--ignored` **27 项**（逐 target 跑；`capture_live::window_capture_produces_non_blank_image` 在无头/远程会话里必黑，属环境限制不属缺陷）。
- 详细记录见 `docs/PROGRESS.md`（§7.11 = 移除语言提示找图）；引擎契约见 `docs/ENGINES.md` §6.7。
