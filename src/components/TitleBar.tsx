import { useEffect, useState } from "react";
import { PanelLeft, PanelLeftClose, Minus, Square, Copy, X } from "lucide-react";
import { isWindowMaximized, listenWindowMaximize, windowControl } from "../lib/tauri";

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
 * 两个必须记住的坑（2026-10-22 修）：
 * 1. **权限**：拖动与窗口按钮走的是 Tauri 2 的 IPC，`core:default` **不包含**
 *    `allow-start-dragging` / `allow-minimize` / `allow-toggle-maximize` / `allow-close`，
 *    必须在 `capabilities/default.json` 里显式声明。少了权限的表现是「点了没反应」，
 *    因为前端 `catch` 把 rejection 吞掉了，既不报错也不提示。
 * 2. **拖动区只认事件目标自身**：`data-tauri-drag-region` 不会从父元素继承。
 *    所以中间的占位 div 必须自己带一份，否则只有按钮之间的缝隙能拖。
 */
export function TitleBar({ collapsed, onToggleSidebar }: Props) {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    void isWindowMaximized().then(setMaximized);
    return listenWindowMaximize(setMaximized);
  }, []);

  return (
    <div
      data-tauri-drag-region
      className="flex h-10 shrink-0 select-none items-center bg-chrome px-2"
    >
      {/* 左上角：侧边栏折叠开关 */}
      <button
        type="button"
        title={collapsed ? "展开侧边栏" : "收起侧边栏"}
        aria-label={collapsed ? "展开侧边栏" : "收起侧边栏"}
        aria-expanded={!collapsed}
        onClick={onToggleSidebar}
        onMouseDown={(e) => e.stopPropagation()}
        className="flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 hover:bg-black/[0.06] hover:text-neutral-900 active:bg-black/[0.1]"
      >
        {collapsed ? (
          <PanelLeft className="h-4 w-4" strokeWidth={1.75} />
        ) : (
          <PanelLeftClose className="h-4 w-4" strokeWidth={1.75} />
        )}
      </button>

      {/* 中段：拖动区。必须自己带 data-tauri-drag-region，不能指望继承 */}
      <div data-tauri-drag-region className="h-full flex-1" />

      {/* 右上角：窗口控制。内缩圆角，不再贴边平铺，视觉更轻 */}
      <div className="flex items-center gap-1">
        <WinButton label="最小化" onClick={() => windowControl("minimize")}>
          <Minus className="h-4 w-4" strokeWidth={2} />
        </WinButton>
        <WinButton
          label={maximized ? "向下还原" : "最大化"}
          onClick={() => windowControl("toggleMaximize")}
        >
          {maximized ? (
            <Copy className="h-3.5 w-3.5" strokeWidth={1.9} />
          ) : (
            <Square className="h-3 w-3" strokeWidth={2} />
          )}
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
          : "flex h-8 w-8 items-center justify-center rounded-md text-neutral-500 hover:bg-black/[0.06] hover:text-neutral-900 active:bg-black/[0.1]"
      }
    >
      {children}
    </button>
  );
}
