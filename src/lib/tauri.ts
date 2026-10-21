/**
 * Tauri 环境探测与调用封装。
 *
 * 关键点：直接 import "@tauri-apps/api/core" 在纯浏览器下不会炸，但
 * `getCurrentWindow()` 会。因此所有窗口操作都先过 `isTauri()` 判断，
 * 让 `pnpm build` / 浏览器预览也能跑起来（Playground 自动降级到 HTTP）。
 */

export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in (window as unknown as Record<string, unknown>)
  );
}

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) {
    throw new Error("当前不在 Tauri 环境（浏览器预览模式不支持该操作）");
  }
  const { invoke: tauriInvoke } = await import("@tauri-apps/api/core");
  return tauriInvoke<T>(cmd, args);
}

/** 自定义标题栏的窗口控制。非 Tauri 环境下静默忽略。 */
export async function windowControl(action: "minimize" | "toggleMaximize" | "close") {
  if (!isTauri()) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const win = getCurrentWindow();
    if (action === "minimize") await win.minimize();
    else if (action === "toggleMaximize") await win.toggleMaximize();
    else await win.close();
  } catch (e) {
    console.error("窗口控制失败", e);
  }
}

export function listenWindowResize(handler: () => void): () => void {
  window.addEventListener("resize", handler);
  return () => window.removeEventListener("resize", handler);
}
