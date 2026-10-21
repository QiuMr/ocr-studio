# OCR Studio 项目进度记录

> 本文档记录「现在做到哪里了」。每次有实质进展后更新，旧结论不删除、只标注状态。
> 最后更新：2026-10-21（晚）

---

## 0. 一句话现状

**M1 已完成**：Rust 推理核心与 Python 基线逐位对齐，HTTP 接口层（4 个端点 + 7 个明确 501）、
Tauri 命令层、前端（React + Tailwind v4 + shadcn/ui 风格 + MiSans + 自定义标题栏）全部落地，
**打包产物（exe/MSI/NSIS）已实测可启动并响应 /ocr/health**。
M2（截图找图 / 单字 JSON / 日志页）未开始。前端技术栈见第 7 节。

---

## 1. 验证状态（可复现）

```
cargo test                       # 33 个单元测试
cargo test -- --ignored          # + 11 个重量级集成测试（需模型与 DLL）
pnpm build                       # 前端 tsc --noEmit + vite build
pnpm tauri build --debug         # 全链路打包（exe + MSI + NSIS）
```

| 测试 | 结果 |
|---|---|
| 单元测试（geometry / det / rec / preprocess / resize fixture） | **33 / 33 通过** |
| `pipeline_match::rust_matches_python_reference` | **通过**：文本不一致 0、框偏差 >2px 0、置信度偏差 >0.05 0 |
| `tensor_diff::rec_tensor_matches_python_dump` | 通过（rec 输入张量与 Python dump 一致） |
| `tensor_diff::warp_perspective_matches_python_crop` | 通过（透视裁剪 **0/3069 逐位一致**） |
| `tensor_diff::rec_probe_matches_python_on_same_tensor` | 通过（同一张量下 Rust/Python 逐帧 argmax 一致） |
| `api_smoke`（7 个） | **7 / 7 通过**：health、shibie(JSON/multipart)、zhaozi(子串/正则)、501 占位、400 错误路径 |

打包产物实测（2026-10-21）：
```
D:\rust-target\debug\ocr-studio.exe                 启动 8s 不退出
curl http://127.0.0.1:8848/ocr/health               {"status":"ok","model":"small",
                                                     "actual_device":"directml","dict_size":18708}
bundle/msi/OCR Studio_0.1.0_x64_en-US.msi          25.52 MiB
bundle/nsis/OCR Studio_0.1.0_x64-setup.exe         19.80 MiB
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
- **7 个明确 501**（M2）：`/ocr/danzijson`、`/ocr/tuzhaotu1|2`、`/ocr/jubingzhaotu1|2`、`/ocr/pingmuzhaotu1|2`。
  返回 `code=501` + 具体原因，不假装成功。
- 引擎按请求参数热切换：`level` / `device` / `use_cls` 与当前引擎不一致时自动重载。

### Tauri 命令层（`src-tauri/src/commands.rs`）
`ocr_recognize` / `get_health` / `get_config` / `save_config`（热生效，端口变更自动重启服务）/
`set_level` / `set_device` / `server_start` / `server_stop` / `server_status` / `models_root`。
UI 走 invoke，外部脚本走 HTTP，两条路汇到同一个 `AppState`，不会各加载一份模型。

### 前端（`src/`，2026-10-21 从 Vue 迁移到 React）
```
src/App.tsx                   布局：TitleBar + 侧边导航 + 三页
src/components/TitleBar.tsx   自定义标题栏（无图标无标题，仅窗口控制）
src/components/ImageViewer.tsx  canvas 框叠加（按原图像素坐标等比缩放）
src/components/ui/*.tsx       shadcn/ui 风格：button/card/input/label/badge/select/
                              tabs/switch/slider/separator/scroll-area/progress/tooltip
src/pages/                    Dashboard / Playground / SettingsPage
src/lib/api.ts                invoke 封装 + 浏览器降级到 http://127.0.0.1:8848
```
- 图标库 `lucide-react`，**每个按钮均带图标**。
- Playground 支持拖入 / Ctrl+V 粘贴 / 选文件，结果分「文本 / 逐行 / JSON」三个 Tab。
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
| **App 内 UI 交互未验证** | 本环境无 GUI 会话，Dashboard/Playground 的 invoke 链路只验证了底层（打包 exe 启动 + HTTP 层 7/7），页面渲染与按钮回调未经真机点击 | 环境限制——需在有桌面的机器上过一遍 |
| ort `Environment` 进程级单例 | 已修：`OcrEngine::load` 复用全局 `OnceLock<Environment>` | 曾是真 bug——运行时切档位重载引擎会直接报 "only one environment is allowed per process" |
| 两套 `ModelLevel`/`Device` 类型 | 已修：统一定义在 `crate::config`，`ocr` 模块重新导出 | 曾是编译错误源 |

**区分说明**：前两项是工程代价问题（投入即可继续压缩）；第四项是原理性天花板，
堆精度救不回来，验收标准必须建立在真实文本框上。

---

## 5. 待办（按依赖顺序）

| # | 任务 | 状态 |
|---|---|---|
| 1 | `api/routes.rs` + `api/dto.rs`：`/ocr/*` 端点 | **已完成**（4 个实现 + 7 个明确 501） |
| 2 | `commands.rs`：Tauri invoke 命令 | **已完成** |
| 3 | 前端测试台接真实后端 | **已完成**（代码 + 构建 + 打包验证；App 内点击未验证，见第 4 节） |
| 4 | `model_manager.rs`：三档模型懒加载缓存 | 部分达成——`AppState` 已支持按请求热切换重载，但无 LRU 缓存，切档会整份重载 |
| 5 | `cls_should_rotate` 阈值语义对齐 RapidOCR | 待办（需读 RapidOCR 源码确认 label 顺序） |
| 6 | 截图/句柄找图（`windows` crate PrintWindow/BitBlt）+ 模板匹配 NCC → 7 个找图端点 | **M2 主线，未开始** |
| 7 | 单字 JSON：CTC 帧对齐 + 逆透视映射得到字符级 box | **M2，未开始** |
| 8 | 日志页：tracing → `app.emit` → 前端实时日志 | **M2，未开始** |
| 9 | MiSans 商用授权口径确认 | 待用户确认（随包分发前） |
| 10 | release 打包（`pnpm tauri build`，LTO + strip）与安装包实测 | debug 已通；release 未跑 |

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
scripts/models.manifest.json      # 三档模型 URL + SHA256
scripts/fetch_models.ps1          # 下载校验模型
scripts/copy_runtime.ps1          # 拷贝 ort DLL 到 src-tauri/runtime/
models/small/                     # small 档模型 + reference_output.json + dump/
src-tauri/src/ocr/                # Rust 推理核心
src-tauri/src/api/                # HTTP 层（dto / state / routes）
src-tauri/src/commands.rs         # Tauri invoke 命令
src-tauri/tests/                  # pipeline_match / tensor_diff / api_smoke
src/assets/fonts/misans/          # MiSans 切片（fetch_misans_font.py 生成，勿手改）
```
