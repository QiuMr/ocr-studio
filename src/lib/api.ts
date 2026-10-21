/**
 * 前端 API 层。
 *
 * 双通道设计：
 * - Tauri 环境：走 invoke（类型安全、无 base64 网络开销）
 * - 纯浏览器（`pnpm dev` 调试 UI）：降级到 http://127.0.0.1:8848/ocr/*
 *   前提是 Rust 侧服务已经在跑（`cargo run` 或已启动的 App）
 */

import { invoke, isTauri } from "./tauri";
import type {
  AppConfig,
  DanziResult,
  DetectResult,
  Device,
  EngineList,
  FindArgs,
  FindResult,
  Health,
  LogLine,
  ModelLevel,
  RecognizeResult,
  WindowInfo,
} from "../types";

/** 浏览器调试时的兜底地址（Rust 服务默认端口）。 */
const HTTP_BASE = "http://127.0.0.1:8848";

export async function getHealth(): Promise<Health> {
  return invoke<Health>("get_health");
}

/**
 * 列出本机可用的引擎与能力（只扫目录，不加载模型）。
 *
 * 浏览器调试模式下没有 HTTP 对应实现——`/engine/list` 有，但调试 UI 时
 * 引擎面板没有意义（数据来自本机文件系统），故直接返回空列表让界面走空状态。
 */
export async function listEngines(): Promise<EngineList> {
  if (!isTauri()) {
    return { models_root: "-", default_engine: "ppocr:small", engines: [] };
  }
  return invoke<EngineList>("list_engines");
}

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export async function saveConfig(cfg: AppConfig): Promise<AppConfig> {
  return invoke<AppConfig>("save_config", { cfg });
}

export async function setLevel(level: ModelLevel): Promise<Health> {
  return invoke<Health>("set_level", { level });
}

export async function setDevice(device: Device): Promise<Health> {
  return invoke<Health>("set_device", { device });
}

export async function serverStatus(): Promise<boolean> {
  return invoke<boolean>("server_status");
}

export async function serverStart(): Promise<number> {
  return invoke<number>("server_start");
}

export async function serverStop(): Promise<void> {
  return invoke<void>("server_stop");
}

export async function modelsRoot(): Promise<string> {
  return invoke<string>("models_root");
}

/** 识别一张图片。`image` 为 base64（不含 data URL 前缀也可）。 */
export async function recognize(
  image: string,
  useCls: boolean,
  engine?: string,
): Promise<RecognizeResult> {
  if (isTauri()) {
    return invoke<RecognizeResult>("ocr_recognize", {
      image,
      use_cls: useCls,
      engine: engine || null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/ocr/shibie`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ image, use_cls: useCls, ...(engine ? { engine } : {}) }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  const d = json.data;
  return {
    text: d.text,
    lines: d.lines,
    width: d.width,
    height: d.height,
    elapsed_ms: d.elapsed_ms,
  };
}

/**
 * 逐字识别：除整行框外，额外返回每个字的四边形。
 *
 * 字框是「CTC 帧区间按比例切分行宽 + 单应矩阵投回原图」算出来的，
 * 所以字间距不均时边界会有几像素误差。需要精确字框的场景应自行校验。
 */
export async function recognizeChars(
  image: string,
  useCls: boolean,
  engine?: string,
): Promise<DanziResult> {
  if (isTauri()) {
    return invoke<DanziResult>("ocr_danzijson", {
      image,
      use_cls: useCls,
      engine: engine || null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/ocr/danzijson`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ image, use_cls: useCls, ...(engine ? { engine } : {}) }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as DanziResult;
}

/**
 * 通用目标检测（`yolo:v8` 等检测引擎）。
 *
 * 只有 `capabilities.detect === true` 的引擎能走这条；OCR 引擎会收到
 * code 1002（不支持该能力）。
 */
export async function detectImage(
  image: string,
  engine?: string,
  opts?: { conf?: number; iou?: number },
): Promise<DetectResult> {
  if (isTauri()) {
    return invoke<DetectResult>("vision_detect", {
      image,
      engine: engine || null,
      conf: opts?.conf ?? null,
      iou: opts?.iou ?? null,
      class_filter: null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/vision/detect`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      image,
      ...(engine ? { engine } : {}),
      ...(opts?.conf != null ? { conf: opts.conf } : {}),
      ...(opts?.iou != null ? { iou: opts.iou } : {}),
    }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as DetectResult;
}

/**
 * 找图。
 *
 * Tauri 环境走 `find_image` 命令；浏览器调试时按来源映射到对应 HTTP 端点。
 * App 内 `source != image` 时额外返回 `preview`（本次匹配实际看到的画面），
 * 用它当背景可保证命中框与图像严格对齐。
 */
export async function findImage(args: FindArgs): Promise<FindResult & { preview?: string }> {
  if (isTauri()) {
    return invoke<FindResult & { preview?: string }>("find_image", { args });
  }
  const suffix = args.mode === "loose" ? "2" : "1";
  const path =
    args.source === "screen"
      ? `/ocr/pingmuzhaotu${suffix}`
      : args.source === "window"
        ? `/ocr/jubingzhaotu${suffix}`
        : `/ocr/tuzhaotu${suffix}`;
  const res = await fetch(`${HTTP_BASE}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as FindResult;
}

/** 截图预览（含区域原点，用于把屏幕绝对坐标换算回图内坐标）。 */
export interface ScreenPreview {
  image: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** 截一张屏幕预览图（PNG base64 + 区域原点），`region` 缺省为整屏。 */
export async function screenPreview(
  region?: [number, number, number, number],
): Promise<ScreenPreview> {
  if (isTauri()) {
    return invoke<ScreenPreview>("screen_preview", { region: region ?? null });
  }
  throw new Error("屏幕预览只在 App 内可用（浏览器调试请用上传图片）");
}

/** 枚举可见窗口（仅 App 内可用）。 */
export async function listWindows(): Promise<WindowInfo[]> {
  if (isTauri()) {
    return invoke<WindowInfo[]>("list_windows");
  }
  return [];
}

/** 读文件为 base64（去掉 data URL 前缀）。 */
export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const s = String(reader.result);
      const i = s.indexOf(",");
      resolve(i >= 0 ? s.slice(i + 1) : s);
    };
    reader.onerror = () => reject(reader.error ?? new Error("读取文件失败"));
    reader.readAsDataURL(file);
  });
}

/** 取图片自然尺寸。 */export function imageSize(src: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => reject(new Error("无法解析图片尺寸"));
    img.src = src;
  });
}

// ---------------------------------------------------------------- 日志

/** 拉取后端环形缓冲里的最近日志（首次进页面用）。 */
export async function getLogs(): Promise<LogLine[]> {
  return invoke<LogLine[]>("get_logs");
}

/**
 * 订阅实时日志。
 *
 * 后端把 tracing 事件同时写进环形缓冲和 Tauri 事件，这是第二条路。
 * 事件 payload 是**数组**（后端按批次攒着发，避免每条日志一次 IPC）。
 * 返回取消订阅函数；浏览器模式下直接给个空函数，页面不用判断环境。
 */
export async function subscribeLogs(onLines: (lines: LogLine[]) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<LogLine[]>("log-lines", (e) => onLines(e.payload));
}
