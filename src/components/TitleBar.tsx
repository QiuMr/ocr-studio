import { PanelLeft, PanelLeftClose, Minus, Square, X } from "lucide-react";
import { windowControl } from "../lib/tauri";

interface Props {
  /** 侧边栏当前是否收起 */
  collapsed: boolean;
  onToggleSidebar: () => void;
}

/**
 * 自定义标题栏。
 *
 * 规范（2026-10-21）：**不要图标、不要标题、不要底部分割线**，只保留窗口控制按钮；
 * 侧边栏折叠按钮固定在左上角（用户要求）。
 * `tauri.conf.json` 已设 `decorations: false`，原生标题栏被移除。
 *
 * 整条都是 `data-tauri-drag-region`，按住可拖动窗口；按钮要阻止冒泡，
 * 否则点击会被当成拖拽起点——双击最大化也会被吞掉。
 */
export function TitleBar({ collapsed, onToggleSidebar }: Props) {
  return (
    <div
      data-tauri-drag-region
      className="flex h-11 shrink-0 select-none items-center bg-white px-2"
    >
      {/* 左上角：侧边栏折叠开关 */}
      <button
        type="button"
        title={collapsed ? "展开侧边栏" : "收起侧边栏"}
        aria-label={collapsed ? "展开侧边栏" : "收起侧边栏"}
        aria-expanded={!collapsed}
        onClick={onToggleSidebar}
        onMouseDown={(e) => e.stopPropagation()}
        className="flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 active:bg-neutral-200"
      >
        {collapsed ? (
          <PanelLeft className="h-4 w-4" strokeWidth={1.75} />
        ) : (
          <PanelLeftClose className="h-4 w-4" strokeWidth={1.75} />
        )}
      </button>

      {/* 中段留空，仅作为拖动区 */}
      <div className="h-full flex-1" />

      {/* 右上角：窗口控制。内缩圆角，不再贴边平铺，视觉更轻 */}
      <div className="flex items-center gap-1">
        <WinButton label="最小化" onClick={() => windowControl("minimize")}>
          <Minus className="h-4 w-4" strokeWidth={2} />
        </WinButton>
        <WinButton label="最大化 / 还原" onClick={() => windowControl("toggleMaximize")}>
          <Square className="h-3 w-3" strokeWidth={2} />
        </WinButton>
        <WinButton label="关闭" danger onClick={() => windowControl("close")}>
          <X className="h-4 w-4" strokeWidth={2} />
        </WinButton>
      </div>
    </div>
  );
}

function WinButton({
  children,
  label,
  onClick,
  danger,
}: {
  children: React.ReactNode;
  label: string;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      onMouseDown={(e) => e.stopPropagation()}
      className={
        danger
          ? "flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 hover:bg-red-500 hover:text-white active:bg-red-600"
          : "flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 hover:bg-neutral-100 hover:text-neutral-900 active:bg-neutral-200"
      }
    >
      {children}
    </button>
  );
}
