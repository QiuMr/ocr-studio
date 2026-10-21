/**
 * 前端 API 层。
 *
 * 双通道设计：
 * - Tauri 环境：走 invoke（类型安全、无 base64 网络开销）
 * - 纯浏览器（`pnpm dev` 调试 UI）：降级到 http://127.0.0.1:8848/ocr/*
 *   前提是 Rust 侧服务已经在跑（`cargo run` 或已启动的 App）
 */

import { invoke, isTauri } from "./tauri";
import type { AppConfig, Device, Health, ModelLevel, RecognizeResult } from "../types";

export async function getHealth(): Promise<Health> {
  return invoke<Health>("get_health");
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
export async function recognize(image: string, useCls: boolean): Promise<RecognizeResult> {
  if (isTauri()) {
    return invoke<RecognizeResult>("ocr_recognize", { image, use_cls: useCls });
  }
  const res = await fetch("http://127.0.0.1:8848/ocr/shibie", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ image, use_cls: useCls }),
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
