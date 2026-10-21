# 多引擎架构（DDDDOCR / YOLO 扩展）

> 状态：**设计已定，骨架已落（`src-tauri/src/engine/`），尚未接入路由。**
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

定义在 `src-tauri/src/engine/mod.rs`：

| 类型 | 含义 |
|---|---|
| `EngineKind` | `PpOcr` / `DdddOcr` / `Yolo { version }`，决定目录与加载方式 |
| `Capabilities` | 能力声明：`line_boxes` / `char_boxes` / `rotation_cls` / `classes` / `batch` |
| `TextLine` | 一行文本：四点框 + 文本 + score + det_score + rotated + 可选逐字 |
| `CharBox` | 一个字：四点框 + 字 + score + CTC 帧区间 |
| `Detection` | 一个目标：四点或轴对齐框 + `class_id` + `class_name` + score |
| `VisionEngine` | `kind()` / `capabilities()` / `health()` / `recognize()` / `detect()` |

`detect()` 默认返回 `Err(Unsupported)`，`recognize()` 同理——某个实现不支持某能力时
不实现对应方法即可，调用方通过 `capabilities()` 提前判断，不靠捕获错误来分支。

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

兼容规则：**旧路径 `models/{tiny,small,medium}/` 继续有效**（`resolve_model_dir` 先找
新路径，找不到再回退旧路径）。这样老用户的模型不用搬家。

YOLO 的 `labels.txt` 每行一个类名，行号即 `class_id`；缺失时用 `class_{id}` 兜底并打 warn。

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
| `POST /vision/detect` | 通用目标检测 | 请求带 `engine`，返回 `Detection[]` |
| `POST /vision/pipeline` | 检测 + 识别串联 | YOLO 找区域 → PP-OCR 认字 |
| `GET /engine/list` | 列出可用引擎与能力 | 前端据此决定显示哪些面板 |

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
| M3.0 | 抽象层 + `OcrEngine` 实现 `VisionEngine`，路由不变 | 37 个测试全绿，精度零变化 |
| M3.1 | `ddddocr` 引擎（先 common 版） | 同一张图上与 PP-OCR 的文本相似度 ≥ 0.95 才认为接入正确 |
| M3.2 | YOLO 通用检测（v8 优先，导出 ONNX opset 12+） | NMS 后处理与 ultralytics 对齐，IoU/置信度逐框比对 |
| M3.3 | `/vision/pipeline` 串联 | 复杂版面（表格、多列）上比纯 PP-OCR 明显更准 |

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
