/** 与 Rust 侧 `api::dto` / `commands` 对应的前端类型。 */

export type ModelLevel = "tiny" | "small" | "medium";
export type Device = "directml" | "cpu";

export type Box4 = [[number, number], [number, number], [number, number], [number, number]];

export interface LineResult {
  text: string;
  /** 字段名是 `box`（与原产品一致），Rust 侧用原始标识符序列化而来 */
  box: Box4;
  score: number;
  det_score: number;
  rotated: boolean;
}

export interface RecognizeResult {
  text: string;
  lines: LineResult[];
  width: number;
  height: number;
  elapsed_ms: number;
}

// ---------------------------------------------------------------- 单字

/** 单个字的框与置信度 */
export interface CharResult {
  ch: string;
  box: Box4;
  score: number;
  index: number;
  /** CTC 起始帧（含） */
  t_start: number;
  /** CTC 结束帧（含） */
  t_end: number;
}

export interface DanziLine {
  text: string;
  box: Box4;
  score: number;
  det_score: number;
  rotated: boolean;
  /** rec 总帧数，字框宽度由帧区间比例决定 */
  frames: number;
  chars: CharResult[];
}

export interface DanziResult {
  text: string;
  lines: DanziLine[];
  char_count: number;
  width: number;
  height: number;
  elapsed_ms: number;
}

// ---------------------------------------------------------------- 区域 OCR

/** `/ocr/quyuocr`：只认指定矩形里的字，`lines` 的坐标已在原图坐标系 */
export interface RegionOcrResult {
  /** 实际生效的区域 `[x, y, w, h]`（请求值被钳到图内后的结果） */
  region: [number, number, number, number];
  /** 区域内全部文本按行用 `\n` 拼接 */
  text: string;
  /** 命中行的框，坐标已在原图坐标系 */
  lines: LineResult[];
  /** 原图宽（不是区域宽） */
  width: number;
  /** 原图高（不是区域高） */
  height: number;
  elapsed_ms: number;
}

// ---------------------------------------------------------------- 扫码

/** `/vision/qrcode`：一个识别到的二维码/条码 */
export interface BarcodeHit {
  /** 码里编的内容（原文，不做解析） */
  text: string;
  /** 码制名，ZXing 规范大写：`QR_CODE` / `CODE_128` / `EAN_13` … */
  format: string;
  /** 定位点/角点，原图坐标。QR 一般是 3 个定位图案 + 1 个校正点 */
  points: [number, number][];
  /** 定位点的轴对齐包围盒 `[x, y, w, h]` */
  x: number;
  y: number;
  width: number;
  height: number;
}

/** `/vision/qrcode` 出参 */
export interface BarcodeResult {
  count: number;
  codes: BarcodeHit[];
  /** 原图尺寸 */
  width: number;
  height: number;
  elapsed_ms: number;
}

export interface Health {
  status: "ok" | "no_model";
  /** 当前引擎标识，如 `ppocr:small` / `ddddocr:common` */
  engine: string;
  model: ModelLevel;
  device: Device;
  actual_device: Device | "-";
  det_loaded: boolean;
  rec_loaded: boolean;
  cls_loaded: boolean;
  dict_size: number;
  port: number;
  uptime_sec: number;
}

export interface AppConfig {
  level: ModelLevel;
  device: Device;
  port: number;
  template_threshold: number;
  /** 兼容渲染（软件合成）。由后端维护：WebView 白屏时自动置位，前端只读 */
  webview_compat?: boolean;
  /** 允许浏览器页面跨域调用（CORS）。默认 false，开启后需重启服务生效 */
  allow_browser?: boolean;
}

export const DEFAULT_CONFIG: AppConfig = {
  level: "small",
  device: "directml",
  port: 8848,
  template_threshold: 0.9,
};

// ---------------------------------------------------------------- 目标检测

/** 单个检测目标（`/vision/detect` 与 `vision_detect` 命令共用） */
export interface DetectHit {
  /** 轴对齐框转四点（左上/右上/右下/左下），与文字行共用画框逻辑 */
  box: Box4;
  class_id: number;
  /** labels.txt 缺失时后端回落 `class_{id}` */
  class_name: string;
  score: number;
}

export interface DetectResult {
  engine: string;
  detections: DetectHit[];
  count: number;
  width: number;
  height: number;
  elapsed_ms: number;
}

/** pipeline 的单区域：检测框 + 框内识别出的文字 */
export interface PipelineRegion {
  box: Box4;
  class_id: number;
  class_name: string;
  score: number;
  /** 框内各行文本按阅读顺序拼接（行间 \n） */
  text: string;
  /** 框内识别出的行（坐标已在原图上） */
  lines: LineResult[];
}

export interface PipelineResult {
  detect_engine: string;
  ocr_engine: string;
  regions: PipelineRegion[];
  count: number;
  width: number;
  height: number;
  elapsed_ms: number;
}

// ---------------------------------------------------------------- 找图

export type FindSource = "image" | "screen" | "window";
export type FindMode = "exact" | "loose";

/** 找图模式：模板找图（比像素） / 语义找图（模型认固定类别） / 提示找图（当场给目标） */
export type FindKind = "template" | "semantic" | "prompt";

/** 提示找图的两种给目标方式：给图（参考图 + 框）/ 给词 */
export type PromptMode = "visual" | "text";

export interface FindHit {
  x: number;
  y: number;
  width: number;
  height: number;
  center_x: number;
  center_y: number;
  /** 模板找图是相似度（NCC）；语义找图是检测置信度 */
  score: number;
  /** 语义找图才有：命中类别 id */
  class_id?: number;
  /** 语义找图才有：命中类别名 */
  class_name?: string;
}

export interface FindResult {
  found: boolean;
  /** 首个命中的左上角（屏幕绝对坐标；source=image 时是图内坐标） */
  x?: number;
  y?: number;
  score?: number;
  center_x?: number;
  center_y?: number;
  /** 模板尺寸；语义找图是首个命中框的尺寸，未命中为 0 */
  width: number;
  height: number;
  matched: number;
  matches: FindHit[];
  source: FindSource;
  /** 搜索图左上角在屏幕上的坐标（source=image 时为 0） */
  origin_x: number;
  origin_y: number;
  /** 语义找图才有：实际使用的检测引擎 */
  engine?: string;
  elapsed_ms: number;
}

export interface FindArgs {
  source: FindSource;
  mode: FindMode;
  image?: string;
  template?: string;
  template_file?: string;
  /** 模板掩码 base64（可省略）。**白（≥ 阈值）= 参与匹配**，尺寸须与模板一致 */
  mask?: string;
  /** 掩码文件绝对路径（与 `mask` 二选一） */
  mask_file?: string;
  /** 掩码二值化阈值，缺省 128 */
  mask_threshold?: number;
  hwnd?: number;
  title?: string;
  region?: [number, number, number, number];
  min_score?: number;
  max_results?: number;
}

/** 语义找图参数：没有模板字段，多了检测参数 */
export interface SemanticFindArgs {
  source: FindSource;
  image?: string;
  hwnd?: number;
  title?: string;
  region?: [number, number, number, number];
  /** `yolo`（缺省 = 目录里第一个模型）/ `yolo:<模型id>` */
  engine?: string;
  /** 检测置信度下限，默认 0.25 */
  conf?: number;
  /** 只保留这些类别 id；空 = 全部 */
  class_filter?: number[];
  max_results?: number;
}

/**
 * 提示找图参数：目标由调用方**当场给**，不是模型固化的类别。
 *
 * - `mode=visual`：给参考图 + 一个（或多个）框，找「跟框里这块像的」。
 * - `mode=text`：给几个词，找「语义上是的」。
 *
 * 与 `SemanticFindArgs` 的核心差别就是「换目标要不要换模型」：语义找图要，
 * 这里不要。
 */
export interface PromptFindArgs {
  source: FindSource;
  /** `visual`（缺省）或 `text` */
  mode?: PromptMode;
  image?: string;
  hwnd?: number;
  title?: string;
  region?: [number, number, number, number];
  /** 参考图 base64（`mode=visual` 必填） */
  ref_image?: string;
  /** 参考框 `[x1,y1,x2,y2]`，原图像素坐标（`mode=visual` 必填） */
  ref_boxes?: [number, number, number, number][];
  /** 每个参考框归属的「槽」，同槽取并集。缺省全 0 */
  ref_classes?: number[];
  /** 槽的名字（仅回显用） */
  ref_names?: string[];
  /** 提示词列表（`mode=text` 必填） */
  words?: string[];
  /** 置信度下限，默认 0.25 */
  conf?: number;
  /** 只保留这些提示槽（不是模型类别 id） */
  class_filter?: number[];
  max_results?: number;
}

export interface WindowInfo {
  hwnd: number;
  title: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

// ---------------------------------------------------------------- AI 操控

/** 与 Rust 侧 commands::AiStatusOut 对应。 */
export interface AiStatus {
  enabled: boolean;
  port: number;
  base_url: string;
  token: string;
  prompt: string;
  running: boolean;
}

// ---------------------------------------------------------------- 引擎

/**
 * 引擎能力声明。前端据此决定显示哪些入口——
 * **不支持的能力直接隐藏**，不要显示灰色禁用按钮（小白看到灰按钮只会困惑）。
 */
export interface EngineCapabilities {
  line_boxes: boolean;
  char_boxes: boolean;
  rotation_cls: boolean;
  detect: boolean;
  /** 检测类别名。空 = 不做检测，或没给 labels.txt */
  classes: string[];
  batch: boolean;
}

/** 与 Rust 侧 `EngineItemDto` 一一对应 */
export interface EngineItem {
  /** 规范化名字，如 `ppocr:small`、`yolo` */
  name: string;
  family: "ppocr" | "ddddocr" | "yolo" | string;
  variant?: string | null;
  capabilities: EngineCapabilities;
  /** 模型文件齐全、可以加载。不代表已在内存里 */
  available: boolean;
  /** 只有当前已加载的那个引擎才有值 */
  actual_device?: string | null;
  /** 不可用的原因，或可用时的补充说明 */
  note?: string | null;
}

export interface EngineList {
  models_root: string;
  default_engine: string;
  engines: EngineItem[];
}

/**
 * 一个可用的目标检测模型。
 *
 * 检测模型是「用户自己的」：既可以是 `models/detect/yolo/` 下扫到的，
 * 也可以是设置里从任意目录「添加」进来的（只记路径，不复制文件）。
 */
export interface DetectModel {
  /** 模型 id（`yolo:<id>` 的后半段） */
  id: string;
  /** 引擎标识，可直接传给 detectImage / recognize */
  engine: string;
  /** onnx 完整路径 */
  path: string;
  file_name: string;
  /** true = 从任意目录加的（配置里记着路径） */
  external: boolean;
}

// ---------------------------------------------------------------- 日志

export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

/** 与 Rust 侧 `logbuf::LogLine` 一一对应 */
export interface LogLine {
  /** Unix 毫秒时间戳（前端格式化成当地时间） */
  ts_ms: number;
  level: LogLevel;
  /** 产生日志的模块路径，如 `ocr_studio::api::routes` */
  target: string;
  message: string;
}
