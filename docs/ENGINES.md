# 多引擎架构（DDDDOCR / YOLO 扩展）

> 状态：**M3.0 / M3.1 已完成**——
> 抽象层 `src-tauri/src/engine/{mod.rs,catalog.rs,ddddocr.rs}`，
> `OcrEngine` 与 `DdddOcrEngine` 都实现了 `VisionEngine`，`/engine/list`、
> `list_engines`、以及 `/ocr/*` 的 `engine` 字段全部接通。
> M3.0 / M3.1 / M3.2 已完成（DDDDOCR、YOLO 检测均已实测落地）。
> 目的：在不破坏现有 PP-OCR 精度基线的前提下，为后续接入 DDDDOCR 与各版本 YOLO 留好位置。
> 时间：2026-10-22

## 1. 为什么现在就要抽象

当前 `OcrEngine`（`src-tauri/src/ocr/engine.rs`）把三件不同的事焊在一起：

1. **模型生命周期**：读 `models/<level>/*.onnx`、建 Session、DirectML→CPU 降级；
2. **流水线编排**：det → cls → rec；
3. **业务语义**：返回「文本行」。

要加 DDDDOCR，第 1、3 条都得变（它的目录结构、字典格式、输出语义都不同）；
要加 YOLO，第 2 条根本不适用（没有 rec），而输出是「带类别的框」。

如果直接 `if engine == "ddddocr" { ... }` 分支下去，三档 PP-OCR 的精度对齐测试
（`cargo test -- --ignored` 的 37 个用例）会变得无法定位问题——改一处不知道影响谁。

因此先切出三层，**每层只依赖下一层的 trait，不依赖具体模型**：

```
HTTP/Tauri 层
  └── EngineHandle（运行期句柄：Mutex<Box<dyn VisionEngine>>）
        └── trait VisionEngine（统一入口）
              ├── trait OcrPipeline     文本行 + 字框
              └── trait ObjectDetector  类别框（目标检测用）
                    └── 具体实现：PpOcr / DdddOcr / Yolo ...
```

## 2. 核心类型

定义在 `src-tauri/src/engine/mod.rs`（**已落地**）：

| 类型 | 含义 |
|---|---|
| `EngineKind` | `PpOcr` / `DdddOcr` / `Yolo`，决定目录与加载方式 |
| `yolo::YoloLayout` | **加载时探测**的模型结构：输出布局（`C×N` / `N×C`）、有无 objectness、输入边长、候选数、通道数。取代了早期的 `YoloVersion` 白名单 |
| `EngineSpec` | `<family>[:<variant>]` 的解析与规范化，`key()` 兼作缓存键 |
| `Capabilities` | 能力声明：`line_boxes` / `char_boxes` / `rotation_cls` / `detect` / `classes` / `batch` |
| `TextLine` | 一行文本：四点框 + 文本 + score + det_score + rotated + 可选逐字 |
| `CharBox` | 一个字：四点框 + 字 + score + CTC 帧区间（复用 `ocr::engine::CharBox`） |
| `Detection` | 一个目标：四点框 + `class_id` + `class_name` + score |
| `VisionEngine` | `spec()` / `capabilities()` / `status()` / `infer()` / `detect()` |
| `VisionError` | `BadRequest` / `Unsupported` / `UnknownEngine` / `NotReady` / `Inference` |

`detect()` 有默认实现返回 `Err(Unsupported)`，OCR 类引擎不实现即可——**不要在 `impl` 里
写个 `Err` 占位**，否则能力声明和实现容易长期不一致。调用方通过 `capabilities()` 提前判断，
不靠捕获错误来分支。

### 2.1 两个刻意的命名避让（踩过就知道多贵）

`OcrEngine` 上已有历史方法 `recognize(&img)` 与 `health()`。trait 若同名，会出现
「具体类型解析到 inherent、`dyn` 解析到 trait」的双份行为，同一行代码换个静态类型
结果就变，排查成本极高。因此 trait 用：

- `infer()` 而不是 `recognize()`；
- `status()` 而不是 `health()`（语义上也更宽：带能力声明与可用性）。

`OcrEngine` 的 `recognize` / `recognize_detailed` 原样保留，M3.0 只做「包一层」，
**推理路径一个字节都没动**，这是精度零变化的结构性保证。

## 3. 模型目录约定

新目录（推荐）：

```
models/
  ocr/
    ppocr/
      tiny/{det.onnx,rec.onnx,cls.onnx,keys.txt}
      small/{...}
      medium/{...}
    ddddocr/
      common/{det.onnx,rec.onnx,charset.txt}
      old/{det.onnx,rec.onnx,charset.txt}
  detect/
    yolo/
      {model.onnx,labels.txt}          ← 唯一约定的位置（不分版本）
      <任意名>/{model.onnx,labels.txt}  ← 兼容旧的 v8/ v11/ 分层，自动下探一层
```

兼容规则：**旧路径 `models/{tiny,small,medium}/` 继续有效**——`catalog::ppocr_dir`
先看新路径下有没有 `det.onnx`，没有再回退旧路径。这样老用户的模型不用搬家
（30MB 文件让人手动挪目录是没必要的摩擦）。

YOLO 的 `labels.txt` 每行一个类名，行号即 `class_id`；缺失时 `classes` 为空、
类名回落 `class_{id}` 并打 warn——**没有 labels 只是名字不可读，不该判定引擎不可用**。
字典名各家打包不一致，ddddocr 的字典认 `charset.txt` / `keys.txt` / `dict.txt` 三个名字。

### 3.1 探测与加载必须分开

`catalog::discover()` **只读文件系统，绝不加载模型**：加载一次 det+rec 要几百毫秒
到几秒、还吃显存，拿来做「本机能开哪些引擎」的探测完全不合适。所以
`available: true` 只代表「文件齐」，能不能真跑要加载后看 `/ocr/health` 的
`actual_device`（DirectML 注册成功但推理失败会静默降级 CPU，只有预热推理才知道）。


## 4. 接口面（HTTP）

原则：**旧的 `/ocr/*` 一个都不动**，新能力全部走新路径 + 新增可选字段。

### 4.1 选择引擎（向后兼容）

在现有请求体里加可选字段 `engine`：

```json
{ "image": "...", "engine": "ppocr:small" }
```

缺省 = 当前配置里的默认引擎，行为与今天完全一致。`engine` 的语法：
`<family>[:<variant>]`，例如 `ppocr:tiny`、`ddddocr:common`、`yolo`（别名 `detect`）。
目标检测没有 variant 这一层——模型结构在加载时探测。

### 4.2 新增端点

| 端点 | 用途 | 备注 |
|---|---|---|
| `POST /ocr/danzijson` | 单字 JSON | **已实现**（CTC 帧对齐 + 逆透视） |
| `GET /engine/list` | 列出可用引擎与能力 | **已实现**（`list_engines` 命令同源） |
| `POST /vision/detect` | 通用目标检测 | 请求带 `engine`，返回 `Detection[]` |
| `POST /vision/pipeline` | 检测 + 识别串联 | YOLO 找区域 → PP-OCR 认字 |

`/vision/*` 与 `/ocr/*` 共用同一份 `AppState`，**不会各加载一份模型**：
句柄按 `engine` 键缓存在 `HashMap<String, Arc<Mutex<Box<dyn VisionEngine>>>>` 里，
带引用计数，超过 2 个未使用时按 LRU 释放（显存有限，不能无限堆）。

### 4.3 错误码

沿用现有 `code`：
- `1001` 引擎未就绪
- `1002` 引擎不支持该能力（新增）
- `1003` 未知引擎名（新增）

## 5. 前端影响

- 设置页增加「OCR 引擎」「检测引擎」两个下拉，选项来自 `GET /engine/list`；
- 测试台按 `capabilities()` 决定显示：
  - `line_boxes` → 「逐行」tab
  - `char_boxes` → 「逐字」tab（DDDDOCR 也支持，但字框精度语义不同，需在 UI 注明）
  - `detect` → 新增「目标检测」tab，显示类别 + 置信度，框色按类别分
- **未实现能力的 tab 直接隐藏**，不要显示禁用按钮——小白看到灰按钮只会困惑。

## 6. 实施顺序

| 阶段 | 内容 | 验收 |
|---|---|---|
| M3.0 ✅ | 抽象层 + `OcrEngine` 实现 `VisionEngine` + `/engine/list` | 8 个新用例全绿；48 通过 / 1 忽略；推理路径零改动 |
| M3.1 ✅ | `ddddocr` 引擎（common / old）+ 共享 PP-OCR det + `/ocr/*` 的 `engine` 字段 | 见 6.1 的实测验收（7 行逐字与官方 Python 完全一致） |
| M3.2 ✅ | YOLO 通用检测（v8 优先，导出 ONNX opset 12+） | 与 ultralytics 逐框比对：8/8 框 IoU=1.0000、conf 差=0.0000（见 6.2） |
| M3.3 ✅ | `/vision/pipeline` 串联 | bus.png 实测：5 区域 8 行文字（bus 框内 7 行），UI「框内识字」可用（见 6.3） |

M3.0 顺带完成的部分：`catalog::discover()` 9 个候选引擎（ppocr×3 + ddddocr×2 + yolo×4）
的可用性探测、能力声明、缺失文件提示；前端设置页「识别引擎」卡片。
**注册表（`HashMap<key, Arc<Mutex<Box<dyn VisionEngine>>>>` + LRU）没有做**——
`AppState` 里改用「PP-OCR 一个 + 额外引擎单槽」：PP-OCR 的配置/重载语义已被现有测试依赖，
不动它；额外引擎只可能同时开一个（ddddocr common 就 54MB），单槽等价于 LRU 上限 1。
真需要多引擎并行（例如 YOLO + PP-OCR 同时用于 pipeline）时，再把 `alt` 换成 HashMap。

### 6.1 M3.1 实现实录（ddddocr 1.6.1，全部实测，别再凭源码猜）

**模型与字典**

- `scripts/fetch_ddddocr.ps1` 从 PyPI sdist 取，**字典与模型同源同版本**：
  - `common` ← `common.onnx` + **`CHARSET_BETA`**（54MB rec）
  - `old` ← `common_old.onnx` + **`CHARSET_OLD`**（13.6MB rec）
  - 两套字典都是 8210 项，`[0] == ""` 就是 blank。
- **配对关系错一次就整行错位**：`common.onnx` 配 `CHARSET_OLD` 会把「测试样本」解成
  「婊︽幏鐚滄」这类乱码，而且不报错。当时差点误判成「上游字符集损坏」，
  真实原因是 **PowerShell 控制台按 GBK 显示 UTF-8 输出**造成的假象——
  查这类问题必须先把 `[Console]::OutputEncoding` 设成 UTF-8（或把结果写进文件再读）。
- 别忘了一条：ddddocr **默认走的其实是 `common_old`**（`load_ocr_model` 里
  `old=False,beta=False` 两个分支都指向 common_old），只有 `beta=True` 才用 common.onnx。

**预处理（`core/ocr_engine.py::_preprocess_image`）**

1. 高固定 **64**，宽 = `int(w * 64 / h)`——截断，**不 pad、不裁、不保 32 对齐**；
2. `PIL.Image.resize(..., LANCZOS)`，**先 resize 再转灰度**（顺序反了有 1~2 灰阶差）；
3. 灰度用 **ITU-R 601-2**（0.299/0.587/0.114），不是 `image` crate 的 Rec.709；
4. 只 `/255.0`，**不做 mean/std 归一化**；输入 `[1, 1, 64, W]` 单通道。

**输出与解码**

- 输出 `[T, 1, C]`，且是 **logits**（实测单帧和可到 -34，softmax 后才归 1）；
- blank 就在 `charset[0]`，映射是 `chars[idx]`；**不能**把字典直接丢给
  `ocr::rec::ctc_decode_spans`（那个按 PP-OCR 约定取 `chars[idx-1]`，会整体错位一位）；
- 置信度只对「被采纳的帧」算 softmax：`p = 1 / Σexp(v - v_max)`，
  T×8210 次 exp 换成几十帧×8210，快一个数量级，且数学上精确。

**行检测**

ddddocr 自带 `common_det.onnx` 是 **YOLOX 目标检测**（416×416、strides 8/16/32、
`demo_postprocess`、class-agnostic NMS、BGR 0~255 不归一化），训练目标是验证码字符块，
**不是 DB 文本行检测**。所以本引擎的行检测复用了 PP-OCR 的 `det.onnx`
（新增 `ocr::detector::LineDetector`，只做 det 不加载 rec，避免为了一个 det 而
背上整个 PP-OCR）。这一条同时解释了为什么 `/engine/list` 里 ddddocr 的可用性
要求「PP-OCR det 存在」。

**验收数据**（`cargo test --test ddddocr_parity -- --ignored`）

| 项目 | 结果 |
|---|---|
| 7 张裁剪行图 vs 官方 Python 实现 | 文本**逐字完全一致**（含中文、英文、数字混排） |
| 整图 det+rec 链路 | 7 行全部返回，框点落在图内，逐字框数 = 文本字数 |
| 与 PP-OCR 的差异 | ddddocr 会丢空格与短横线（`SO-2026-1021-8848` → `SO202610218848`），中文偶有错字；PP-OCR 在同一张图上是逐字全对 |

结论：**ddddocr 定位为「验证码 / 短英文数字」引擎，长文本仍然用 PP-OCR**。
这不是工程问题，是训练分布决定的（它的字典与模型都是为短串设计的）。


### 6.2 M3.2 实现实录（yolov8n，全部实测，别再凭源码猜）

**模型获取**：`scripts/fetch_yolo.ps1` = 下载官方 yolov8n.pt → ultralytics 导出
ONNX（`opset=12, imgsz=640, dynamic=False, simplify=True`）→ 重命名 `model.onnx`，
**labels.txt 从权重元数据同源生成**（COCO 80 类），不手工维护。前提是本机 Python
装了 ultralytics（含 torch CPU）。

**预处理必须逐条复刻 ultralytics 的 LetterBox（auto=False, center=True）**：

1. `scale = min(640/w, 640/h)`；`new = round(w*scale), round(h*scale)`——
   Python 的 `round()` 是**银行家舍入**，Rust 要用 `round_ties_even()`，
   `f64::round()` 会在 .5 边界差一个像素；
2. 缩放用 `cv2.resize(INTER_LINEAR)`——直接复用 `ocr::preprocess::resize_bilinear`
   （PP-OCR 精度基线验证过的 OpenCV 11bit 定点逐位复刻）；
3. 填充值统一 **114**（ultralytics 全系与 yolov5 官方 letterbox 默认色都是 114），
   上下 `round(dh±0.1)` 均分；早期写成「v5 用 0」是错的，已纠正；
4. 输入 RGB `/255`（ultralytics 是 BGR 读入、letterbox 后 `[...,::-1]` 转 RGB，等效）。

**后处理**：v8 输出 `[1, 4+C, 8400]`（导出图已带 sigmoid，值就是概率，不要再次
sigmoid）；conf 过滤 0.25 → 按 conf 降序（**稳定排序**，并列时先出现者先，与
torchvision 一致）→ 同类内贪心抑制 IoU 0.7（ultralytics 的「类别 × 7680 坐标偏移
+ torchvision NMS」与同类贪心等价）→ max_det 300 → **先 NMS 后还原原图坐标**
（除 scale、减 pad），最后 clip 到闭区间 `[0,w]×[0,h]`（不是 w-1）。

**验收方法（重要）**：`tests/yolo_parity.rs` + `scripts/ref_yolo.py`，参考值用
ultralytics 跑**同一份导出的 onnx**（不是 .pt），差异才纯粹是我们的代码。
**测试图必须用 PNG**：第一版用官方 bus.jpg 逐框比对，IoU 只有 0.97~0.999、
conf 差 0.001~0.011，一度以为是 NMS/还原坐标写错；实际是 **JPEG 解码器差异**
（cv2 与 image crate 的 IDCT 有 ±1 抖动）经回归放大。转 PNG 后 8/8 框
IoU=1.0000、conf 差=0.0000，证明整条链路（letterbox/解码/NMS/坐标还原）与
ultralytics 逐位一致。真实场景的 jpg 输入自带这点解码噪声，属正常现象。

**v5/v7 系**：`decode()` 的 `[1,N,5+C]` obj×cls 路径已实现，结构探测也有合成单测，
但**没有真模型实测**——哪天拿到 v5/v7 的 onnx，先跑一遍 parity 再宣布可用。
（2026-10-22 之后引擎不再区分版本，放进去就自动按 `N×C` + objectness 解码。）

**本机环境坑（复现时必看）**：本机装的是 `onnxruntime-directml`，而 ultralytics
按发行版名找 `onnxruntime`，找不到就**自动用 uv 装最新版**——装到一半 dll 被锁
失败，把新版 wrapper 盖在旧版 native 上（症状：`InferenceSession` 没有
`is_webgpu_graph_capture_enabled` 属性，且每次跑都重新破坏）。已改为安装
`onnxruntime==1.24.3`（CPU 版）根治；代价是 RapidOCR 这类本机脚本退回 CPU 推理。


### 6.3 M3.3 实现实录（/vision/pipeline）

- 流程：`detect_engine`（默认 `yolo`）找区域 → 轴对齐裁剪 → PP-OCR 逐区域认字 →
  行坐标加框偏移平移回原图。两把引擎锁**顺序持有、绝不嵌套**（detect 用 alt 锁、
  识别用 engine 锁，各自独立的 spawn_blocking）。
- **验收口径要写清**：COCO 类别与文本区域基本不对齐，yolov8n+coco 跑文档截图
  检出的是人/车不是文字块——这是预期，不是 bug。pipeline 面向**自训检测模型**
  （按钮/标题/表格检测器）。bus.png 只用来验证链路：实测 5 区域 8 行
  （bus 框内 7 行：3ATDOM/MADRID/eléctricamEMTe/cero/emisiones...）。
- **踩过的坑（一度以为链路断了）**：直接对巴士裁剪图跑 `/ocr/shibie` 有 7 行字，
  pipeline 却报 0——根因是 `crop_imm(x0, y0, x1 - x0, y1 - y1)`，高度算成 `y1-y1=0`
  的手误，0 高图 infer 静默返回空。**裁剪后必须断言宽高 > 0**（现已在测试里锁）。


### 目标检测的已知坑

1. **输出布局**：v5/v7 是 `[1, N, 5+C]`（xywh+obj+cls），v8/v10/v11/v12/v26 是 `[1, 4+C, N]`
   （无 obj，需转置）。两者不能共用一套后处理——现在由 [`YoloLayout`] 决定，**不再靠版本号**。
2. **NMS**：ultralytics 默认 `conf=0.25, iou=0.7, class-agnostic=False`。要和它逐框对齐，
   就必须复刻同样的排序（按 score 降序）+ 同类内抑制。
3. **letterbox**：全系统一 114 灰边。填错会让置信度整体偏移 0.05+，肉眼看不出来但阈值一改就翻。
4. **怎么判断有没有 objectness**：`C == 4 + 类别数` 是没有，`C == 5 + 类别数` 是有。
   类别数来自 `labels.txt`；没有 labels 时用候选数对锚点网格兜底（8400 = anchor-free，
   25200 = 3 anchors × 8400）。
5. **两类导出不受支持**，必须明确报错而不是硬跑：v3/v4 的多输出 head（3 个输出张量）、
   自带 NMS 的端到端导出（`[1,300,6]`，候选数远小于锚点网格）。


### 6.4 目标检测去版本化（2026-10-22，用户要求）

**动机**：让用户去选 `v5/v7/v8/v11` 是错的责任划分——他手上只有一个 `model.onnx`，
版本号既不是他填的、也决定不了怎么后处理；真正决定后处理的是那个文件的输出形状。
白名单还必然落后（v10/v12/v26 已经在用，v3 的多 head 又是另一套）。

**做法**：删除 `YoloVersion`，`EngineKind::Yolo` 变成无参数；加载时从 ONNX 读
`inputs[0].dims`（定输入边长）与 `outputs[0].dims`（定布局与通道数），再用 `labels.txt`
的类别数定有无 objectness，得到 `YoloLayout`。`decode()` 直接吃 `YoloLayout`。

**兼容**：`yolo:v8` / `detect` 这类写法继续解析，版本部分被忽略（但会校验「像不像版本号」，
`yolo:abc` 直接报错，不把拼写错误咽下去）。目录 `detect/yolo/v8/` 仍能扫到。

**验收**：v8n 重跑 `tests/yolo_parity.rs`，**8/8 框 IoU=1.0000、conf 差=0.0000 不变**；
日志打出探测结果：`输出 [1,84,8400] / C×N 布局 / 无 objectness / 640x640 输入 · 类别 80 个`。

**顺带修正的假象**：`SessionBuilder::with_execution_providers` 对单个 EP 注册失败**只打
ERROR 日志然后跳过**，整体仍返回 Ok。所以「会话建成功」不代表 DirectML 生效——
本轮发现界面写着 GPU/DirectML、日志里四条 `DirectML is not supported in this build`，
实际一直在 CPU 上跑。现在 `ort_util::directml_available()` 先查
`ep::directml::api()`，不可用就直接走 CPU 并如实上报 `actual_device = Cpu`。
同时把 `runtime/onnxruntime.dll` 换回 1.24.3 的 **DirectML 构建**（20.1MB，
之前那份是 1.31.0 的 CPU 构建），日志里那条 ERROR 消失。

## 7. 不做的事（已评估，记录原因）

- **不做动态插件（加载任意 .dll / .so）**：安全面太大，而实际需求只是有限几种引擎。
  用编译期枚举 + `Box<dyn>` 已经够。
- **不做多引擎同时常驻**：DirectML 显存有限，两张 det 模型就可能 OOM。用 LRU 单活。
- **不改现有 `/ocr/*` 的默认输出结构**：已对接的脚本（易语言/按键精灵）依赖字段名，
  改一次就会全线崩，宁可加新端点。
