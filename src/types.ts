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

export interface Health {
  status: "ok" | "no_model";
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
