import { Minus, Copy, X } from "lucide-react";
import { windowControl } from "../lib/tauri";

/**
 * 自定义标题栏。
 *
 * 规范（2026-10-21）：**不要图标、不要标题**，只保留窗口控制按钮。
 * `tauri.conf.json` 已设 `decorations: false`，原生标题栏被移除。
 *
 * 整条都是 `data-tauri-drag-region`，按住可拖动窗口；按钮要阻止冒泡，
 * 否则点击会被当成拖拽起点。
 */
export function TitleBar() {
  return (
    <div
      data-tauri-drag-region
      className="flex h-9 shrink-0 select-none items-center justify-end border-b border-neutral-200 bg-white"
    >
      <div className="flex h-full items-center">
        <button
          type="button"
          title="最小化"
          onClick={() => windowControl("minimize")}
          onMouseDown={(e) => e.stopPropagation()}
          className="flex h-full w-11 items-center justify-center text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-900"
        >
          <Minus className="h-4 w-4" strokeWidth={1.75} />
        </button>
        <button
          type="button"
          title="最大化 / 还原"
          onClick={() => windowControl("toggleMaximize")}
          onMouseDown={(e) => e.stopPropagation()}
          className="flex h-full w-11 items-center justify-center text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-900"
        >
          <Copy className="h-3.5 w-3.5" strokeWidth={1.75} />
        </button>
        <button
          type="button"
          title="关闭"
          onClick={() => windowControl("close")}
          onMouseDown={(e) => e.stopPropagation()}
          className="flex h-full w-11 items-center justify-center text-neutral-500 transition-colors hover:bg-red-500 hover:text-white"
        >
          <X className="h-4 w-4" strokeWidth={1.75} />
        </button>
      </div>
    </div>
  );
}
