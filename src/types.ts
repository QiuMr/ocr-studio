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

export interface FindHit {
  x: number;
  y: number;
  width: number;
  height: number;
  center_x: number;
  center_y: number;
  score: number;
}

export interface FindResult {
  found: boolean;
  /** 首个命中的左上角（屏幕绝对坐标；source=image 时是图内坐标） */
  x?: number;
  y?: number;
  score?: number;
  center_x?: number;
  center_y?: number;
  /** 模板尺寸（未命中时也有，便于算点击偏移） */
  width: number;
  height: number;
  matched: number;
  matches: FindHit[];
  source: FindSource;
  /** 搜索图左上角在屏幕上的坐标（source=image 时为 0） */
  origin_x: number;
  origin_y: number;
  elapsed_ms: number;
}

export interface FindArgs {
  source: FindSource;
  mode: FindMode;
  image?: string;
  template?: string;
  template_file?: string;
  hwnd?: number;
  title?: string;
  region?: [number, number, number, number];
  min_score?: number;
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
