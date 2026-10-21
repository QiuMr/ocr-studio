# 多引擎架构（DDDDOCR / YOLO 扩展）

> 状态：**M3.0 已完成**——抽象层落在 `src-tauri/src/engine/{mod.rs,catalog.rs}`，
> `OcrEngine` 已实现 `VisionEngine`，`/engine/list` 与 `list_engines` 命令已接通。
> M3.1（DDDDOCR）与 M3.2（YOLO）待落，目录约定与能力声明已就位。
> 目的：在不破坏现有 PP-OCR 精度基线的前提下，为后续接入 DDDDOCR 与各版本 YOLO 留好位置。
> 时间：2026-10-21

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
              └── trait ObjectDetector  类别框（YOLO 用）
                    └── 具体实现：PpOcr / DdddOcr / YoloV8 ...
```

## 2. 核心类型

定义在 `src-tauri/src/engine/mod.rs`（**已落地**）：

| 类型 | 含义 |
|---|---|
| `EngineKind` | `PpOcr` / `DdddOcr` / `Yolo(YoloVersion)`，决定目录与加载方式 |
| `YoloVersion` | `V5` / `V7` / `V8` / `V11`，把「有无 objectness」「letterbox 填 0 还是 114」编码成方法，后处理不再散落 `if version` |
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
      v5/{model.onnx,labels.txt}
      v8/{model.onnx,labels.txt}
      v11/{model.onnx,labels.txt}
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
`<family>[:<variant>]`，例如 `ppocr:tiny`、`ddddocr:common`、`yolo:v8`。

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
| M3.1 | `ddddocr` 引擎（先 common 版）+ 引擎注册表（LRU 单活） | 同一张图上与 PP-OCR 的文本相似度 ≥ 0.95 才认为接入正确 |
| M3.2 | YOLO 通用检测（v8 优先，导出 ONNX opset 12+） | NMS 后处理与 ultralytics 对齐，IoU/置信度逐框比对 |
| M3.3 | `/vision/pipeline` 串联 | 复杂版面（表格、多列）上比纯 PP-OCR 明显更准 |

M3.0 顺带完成的部分：`catalog::discover()` 9 个候选引擎（ppocr×3 + ddddocr×2 + yolo×4）
的可用性探测、能力声明、缺失文件提示；前端设置页「识别引擎」卡片。
**注册表（`HashMap<key, Arc<Mutex<Box<dyn VisionEngine>>>>` + LRU）没有做**——
现在只有一种引擎在跑，多建一层缓存只会多一处不一致。等 M3.1 真有两个引擎了再落，
届时 `AppState.engine` 换成注册表，`ensure_engine` 的按参数重载逻辑一并搬过去。

### M3.1 参考实现要点（ddddocr 1.6.1，已读源码，模型已确认可取）

模型不用自己找：`pip download ddddocr --no-deps --no-binary :all:` 拿 sdist，
包内直接带 `common.onnx`（rec，54MB）、`common_det.onnx`（det，20MB）、
`common_old.onnx`（旧版 rec，13MB）与 `charsets.py`（字符集以 Python 列表字面量内嵌，
不是 txt，需要生成一次）。**注意这会让分发包再大 70MB+，分发时要单独说明。**

rec 预处理（`core/ocr_engine.py::_preprocess_image`，`use_import_onnx=False` 分支）：

1. 高固定 **64**，宽 = `int(w * 64 / h)`——**保持宽高比、不 pad、不裁**；
2. `PIL.Image.resize(..., LANCZOS)`。**这是与 PP-OCR 最大的差异点**：
   我们用 cv2 双线性，它用 LANCZOS。文字相似度验收足够，但别指望逐位对齐；
3. 转灰度 → `/255.0` → `[1, 1, 64, W]`（单通道，不是 3 通道）；
4. CTC 解码与 PP-OCR 同构，但 **charset[0] 是空串当 blank**，
   所以加载时**不能过滤空行**（我们的 `load_dict` 会过滤，ddddocr 要单独写一个保留下标 0 的加载器）。

det 侧另有 `core/detection_engine.py` 的 DB 后处理，参数需单独核对（不能直接套 PP-OCR 的
`DetConfig`）。字符集还带 `valid_indices` 范围限制机制，我们暂不需要，但读源码时要留意
它会把不在范围内的 idx 整个丢掉（不是替换成别的字）。

### YOLO 的已知坑（提前记）
1. **输出布局**：v5 是 `[1, N, 5+C]`（xywh+obj+cls），v8/v11 是 `[1, 4+C, N]`（无 obj，
   需转置）。两者不能共用一套后处理，必须按 `EngineKind` 分开。
2. **NMS**：ultralytics 默认 `conf=0.25, iou=0.7, class-agnostic=False`。要和它逐框对齐，
   就必须复刻同样的排序（按 score 降序）+ 同类内抑制。
3. **letterbox**：v8 用 114 灰边 padding，v5 用 0。像素值错了，置信度整体偏移 0.05+。
4. **anchors**：v5/v7 有 anchor，v8+ 无锚。模型目录里要标出 `version`，不能靠猜。

## 7. 不做的事（已评估，记录原因）

- **不做动态插件（加载任意 .dll / .so）**：安全面太大，而实际需求只是有限几种引擎。
  用编译期枚举 + `Box<dyn>` 已经够。
- **不做多引擎同时常驻**：DirectML 显存有限，两张 det 模型就可能 OOM。用 LRU 单活。
- **不改现有 `/ocr/*` 的默认输出结构**：已对接的脚本（易语言/按键精灵）依赖字段名，
  改一次就会全线崩，宁可加新端点。
