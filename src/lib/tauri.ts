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

/** 自定义标题栏的窗口控制。非 Tauri 环境下静默忽略。
 *
 * 需要 `capabilities/default.json` 里的 `core:window:allow-*` 权限；
 * 少了权限不会抛到界面上，只会静默失败——这也是之前「点关闭没反应」的根因。
 */
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

/** 当前是否最大化（用于切换标题栏按钮图标）。 */
export async function isWindowMaximized(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    return await getCurrentWindow().isMaximized();
  } catch {
    return false;
  }
}

/**
 * 订阅窗口尺寸变化并回报最大化状态。
 *
 * 不用 `onResized` + 自己比较尺寸：最大化/还原、贴边分屏都会改变尺寸，
 * 直接问 `isMaximized()` 才不会猜错。
 */
export function listenWindowMaximize(handler: (maximized: boolean) => void): () => void {
  if (!isTauri()) return () => {};
  let unlisten: (() => void) | undefined;
  let disposed = false;
  void (async () => {
    try {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      handler(await win.isMaximized());
      const un = await win.onResized(async () => {
        handler(await win.isMaximized());
      });
      if (disposed) un();
      else unlisten = un;
    } catch (e) {
      console.error("监听窗口状态失败", e);
    }
  })();
  return () => {
    disposed = true;
    unlisten?.();
  };
}

export function listenWindowResize(handler: () => void): () => void {
  window.addEventListener("resize", handler);
  return () => window.removeEventListener("resize", handler);
}
