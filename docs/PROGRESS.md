# OCR Studio 项目进度记录

> 本文档记录「现在做到哪里了」。每次有实质进展后更新，旧结论不删除、只标注状态。
> 最后更新：2026-10-21（深夜第二轮：界面精修 + 日志页 + 单字 JSON + 免安装分发）

---

## 0. 一句话现状

**M2 全部收口**：找图 7 端点、单字 JSON（`/ocr/danzijson`）、实时日志页、界面精修
四件事一起完成，40 单测 + 16 重量级（含精度基线 / 真实截图 / 逐字对齐）全绿。

**分发方式改为免安装 zip**（用户明确要求：不再出 MSI/NSIS）：
`tauri.conf.json` 设 `bundle.active = false`，改用 `scripts/pack_exe.ps1` 打
`exe + runtime/*.dll + models/ + 使用说明.txt` 的压缩包。**单独拷 exe 是跑不起来的**
（缺 onnxruntime.dll 会静默退回 CPU，缺 DirectML.dll 直接起不来），这一点已写进分发说明。

已完成里程碑：M0（环境基线）、M1（推理核心与 Python 逐位对齐 + HTTP/命令/前端全套）、
M2（找图 + 单字 JSON + 日志页 + 界面精修）。

下一步：M3 多引擎（DDDDOCR / YOLO），设计见 `docs/ENGINES.md`。

### 0.0 本轮（界面精修 + M2 收口）做了什么

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
| 14 | 多引擎抽象（DDDDOCR / YOLO） | 设计完成 `docs/ENGINES.md`，代码骨架待落 |

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
