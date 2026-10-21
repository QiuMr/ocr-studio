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
}

export const DEFAULT_CONFIG: AppConfig = {
  level: "small",
  device: "directml",
  port: 8848,
  template_threshold: 0.9,
};
