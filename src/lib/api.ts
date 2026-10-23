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
  DetectModel,
  DetectResult,
  Device,
  EngineList,
  FindArgs,
  FindResult,
  Health,
  LogLine,
  ModelLevel,
  PipelineResult,
  PromptAssets,
  RecognizeResult,
  RegionOcrResult,
  BarcodeResult,
  PromptFindArgs,
  SemanticFindArgs,
  WindowInfo,
  AiStatus,
  UpdateCheck,
  UpdateProgress,
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

/**
 * 提示找图（YOLOE）资产状态：检测器 + 视觉提示编码器装没装。
 * 只扫目录不加载模型，设置页「模型」卡片用。
 */
export async function promptAssets(): Promise<PromptAssets> {
  if (!isTauri()) {
    return { detector: false, vpe: false, dir: "-" };
  }
  return invoke<PromptAssets>("prompt_assets_status");
}

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

// ---------------------------------------------------------------- 检测模型

/**
 * 可用的目标检测模型：`models/detect/yolo/` 下扫到的 + 设置里加的外部路径。
 *
 * 检测模型天生是「用户自己的」——不同人训的模型放在哪都可能，所以这里
 * 既支持目录约定，也支持任意路径（只记路径、不复制文件）。
 */
export async function detectModels(): Promise<DetectModel[]> {
  if (!isTauri()) return [];
  return invoke<DetectModel[]>("detect_models");
}

/** 弹系统文件选择框挑一个 .onnx；取消则原样返回当前列表 */
export async function pickDetectModel(): Promise<DetectModel[]> {
  if (!isTauri()) return [];
  return invoke<DetectModel[]>("pick_detect_model");
}

/** 按路径加一个模型（把 onnx 拖进窗口也走这里） */
export async function addDetectModel(path: string): Promise<DetectModel[]> {
  if (!isTauri()) return [];
  return invoke<DetectModel[]>("add_detect_model", { path });
}

/** 从列表移除一个外部模型（只删记录，不动磁盘上的文件） */
export async function removeDetectModel(path: string): Promise<DetectModel[]> {
  if (!isTauri()) return [];
  return invoke<DetectModel[]>("remove_detect_model", { path });
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

/** 开机自启是否已开启（状态只在系统里，不在 config.json）。 */
export async function getAutostart(): Promise<boolean> {
  return invoke<boolean>("get_autostart");
}

/** 打开/关闭开机自启，返回操作后的真实状态。 */
export async function setAutostart(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart", { enabled });
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
 * 区域 OCR：只识别 `region`（原图像素坐标 `[x,y,w,h]`）里的文字。
 *
 * 返回的 `lines[].box` 已经平移回**原图**坐标系，调用方不用再做换算。
 * 大图里只关心一小块时用它，比整图识别再自己过滤快得多，也不会把区域外的
 * 噪声文字混进来。
 */
export async function regionOcr(
  image: string,
  region: [number, number, number, number],
  useCls: boolean,
  engine?: string,
): Promise<RegionOcrResult> {
  if (isTauri()) {
    return invoke<RegionOcrResult>("ocr_quyuocr", {
      image,
      region,
      use_cls: useCls,
      engine: engine || null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/ocr/quyuocr`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ image, region, use_cls: useCls, ...(engine ? { engine } : {}) }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as RegionOcrResult;
}

/**
 * 扫码：找出图里所有二维码 / 条码。
 *
 * 与 OCR、检测引擎都无关（纯 CPU），所以引擎没就绪时也能用。
 * `formats` 空 = 全找；`tryHarder` 缺省 true（一维码更容易漏，默认多花点时间）。
 * 「一个都没找到」返回 `count: 0`，不是错误。
 */
export async function scanBarcode(
  image: string,
  formats?: string[],
  tryHarder?: boolean,
): Promise<BarcodeResult> {
  if (isTauri()) {
    return invoke<BarcodeResult>("vision_qrcode", {
      image,
      barcode_formats: formats && formats.length > 0 ? formats : null,
      try_harder: tryHarder ?? null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/vision/qrcode`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      image,
      ...(formats && formats.length > 0 ? { barcode_formats: formats } : {}),
      ...(tryHarder === undefined ? {} : { try_harder: tryHarder }),
    }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as BarcodeResult;
}

/**
 * 通用目标检测（`yolo` 检测引擎）。
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
 * 检测 + 识别串联：检测引擎找区域，PP-OCR 在每个区域内认字。
 * 测试台「框内识字」开关打开时走这条。
 */
export async function pipeline(
  image: string,
  detectEngine?: string,
  opts?: { conf?: number; iou?: number },
): Promise<PipelineResult> {
  if (isTauri()) {
    return invoke<PipelineResult>("vision_pipeline", {
      image,
      detect_engine: detectEngine || null,
      conf: opts?.conf ?? null,
      iou: opts?.iou ?? null,
      class_filter: null,
    });
  }
  const res = await fetch(`${HTTP_BASE}/vision/pipeline`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      image,
      ...(detectEngine ? { detect_engine: detectEngine } : {}),
      ...(opts?.conf != null ? { conf: opts.conf } : {}),
      ...(opts?.iou != null ? { iou: opts.iou } : {}),
    }),
  });
  const json = await res.json();
  if (json.code !== 0) {
    throw new Error(json.msg || `HTTP ${res.status}`);
  }
  return json.data as PipelineResult;
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

/**
 * 语义找图：用目标检测模型直接找目标，**不需要模板图**。
 *
 * 出参与 `findImage` 同构（`found`/`x`/`y`/`center_x`/`center_y`/`matches`…），
 * 额外多 `engine` 与命中里的 `class_id`/`class_name`。
 * 只认得模型见过的类别——换目标要换模型，不是换模板。
 */
export async function findSemantic(
  args: SemanticFindArgs,
): Promise<FindResult & { preview?: string }> {
  if (isTauri()) {
    return invoke<FindResult & { preview?: string }>("find_semantic", { args });
  }
  const res = await fetch(`${HTTP_BASE}/ocr/yuyizhaotu`, {
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

/**
 * 提示找图：目标由调用方当场给——给参考图 + 框（图找图），或给教过的名字（按名字找）。
 *
 * 与 `findSemantic` 的差别是**换目标不用换模型**：语义找图的提示在导出时就
 * 烘焙进权重了，这里是把提示当额外输入。出参与 `findImage` 完全同构，
 * 额外多 `engine`（固定 `yoloe-prompt`）与命中里的 `class_id`/`class_name`
 * （按名字找时类名就是那个名字）。
 */
export async function findPrompt(
  args: PromptFindArgs,
): Promise<FindResult & { preview?: string }> {
  if (isTauri()) {
    return invoke<FindResult & { preview?: string }>("find_prompt", { args });
  }
  const res = await fetch(`${HTTP_BASE}/vision/prompt`, {
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

/** 取图片自然尺寸。 */
export function imageSize(src: string): Promise<{ width: number; height: number }> {
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

// ---------------------------------------------------------------- AI 操控

/** AI 接口当前状态（含可复制的提示词）。 */
export async function aiStatus(): Promise<AiStatus> {
  return invoke<AiStatus>("ai_status");
}

/** 开关 AI 接口。不用重启服务，改完立刻生效。 */
export async function aiSetEnabled(enabled: boolean): Promise<AiStatus> {
  return invoke<AiStatus>("ai_set_enabled", { enabled });
}

/** 重置连接码，旧码立刻失效。 */
export async function aiResetToken(): Promise<AiStatus> {
  return invoke<AiStatus>("ai_reset_token");
}

/** 中文参考库里已经教过的目标名。 */
export async function aiTargets(): Promise<string[]> {
  return invoke<string[]>("ai_targets");
}

// ---------------------------------------------------------------- 版本与更新

/**
 * 检查是否有新版本。
 *
 * 网络不通不会抛错，原因放在 `error` 里 —— 更新检查是旁路动作，
 * 没网不该让界面报错。
 */
export async function updateCheck(): Promise<UpdateCheck> {
  return invoke<UpdateCheck>("update_check");
}

/**
 * 下载并安装更新。
 *
 * **成功后本进程会退出**（由临时目录里的助手替换文件并拉起新版），
 * 所以调用后不要期待 Promise 正常 resolve 之外还有后续逻辑。
 */
export async function updateInstall(): Promise<void> {
  return invoke<void>("update_install");
}

/** 订阅下载进度。返回取消订阅函数；浏览器模式给空函数。 */
export async function subscribeUpdateProgress(
  onProgress: (p: UpdateProgress) => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<UpdateProgress>("update-progress", (e) => onProgress(e.payload));
}

