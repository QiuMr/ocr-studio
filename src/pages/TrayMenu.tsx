/**
 * 托盘菜单（自定义窗口，替代系统原生菜单）。
 *
 * 为什么自己画：`tauri::menu::Menu` 由系统绘制——白底黑字、没有图标、
 * 字体跟着系统走，跟主界面完全不是一个东西。原生菜单的样式是**改不了的**，
 * 要好看只能自己做窗口。换来的好处：能放图标、能放状态徽标、
 * 能把「有更新」做成一条蓝色高亮条目（原生菜单做不到）。
 *
 * 三个必须知道的约束：
 * - 窗口是 transparent + 无边框的，**内容要贴着窗口边缘**。别在卡片外面留
 *   padding：那圈透明区域会被 CSS 阴影染成一层灰边，看着像「菜单外面套了个
 *   框」（实测用户第一眼就问这是不是故意的）。阴影交给窗口自己的
 *   `shadow(true)`——它画在窗口矩形**外面**，不占内容区，也不会形成边框感。
 * - 高度不是固定的：有没有「有更新」那一条会差一行。渲染完要主动
 *   把实际高度回给后端重新定位（`tray_menu_height`），否则菜单会歪。
 * - 量高要量**最外层**（含 padding / border），量内层卡片会少算一圈，
 *   底部正好被切掉一截 —— 表现为「退出」那一行下面少一点。
 */

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { AppWindow, Download, LogOut, Play, RefreshCw, Square } from "lucide-react";
import { invoke } from "../lib/tauri";
import { readCachedUpdate, writeCachedUpdate, type CachedUpdate } from "../lib/update";
import type { UpdateCheck } from "../types";

/** 主窗口静默检查写在这里，托盘直接读，不用每次开菜单都联网。 */

export function TrayMenu() {
  const [running, setRunning] = useState(false);
  const [port, setPort] = useState(8848);
  const [version, setVersion] = useState("");
  const [cached, setCached] = useState<CachedUpdate | null>(null);
  const [busy, setBusy] = useState<"start" | "stop" | "check" | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  const act = useCallback(async (action: "open" | "start" | "stop" | "quit") => {
    try {
      await invoke<void>("tray_action", { action });
    } catch (e) {
      console.error("托盘动作失败", action, e);
    }
  }, []);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    void (async () => {
      try {
        const h = await invoke<{ version: string; port: number }>("get_health");
        setVersion(h.version ?? "");
        setPort(h.port || 8848);
        setCached(readCachedUpdate(h.version ?? ""));
      } catch {
        /* 引擎还没起来时 health 会失败 —— 菜单照样要能用 */
      }
      try {
        setRunning(await invoke<boolean>("server_status"));
      } catch {
        /* 同上 */
      }
    })();
  }, []);

  // Esc = 取消。菜单没有标题栏，按 Esc 关掉是最自然的手感。
  //
  // 为什么不能只靠失焦：万一某台机器上这个窗口压根没拿到焦点
  // （Windows 的前台锁），失焦事件永远不来，Esc 就是唯一的退路。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      void (async () => {
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().hide();
        } catch {
          /* 非 Tauri 环境（浏览器预览）不用管 */
        }
      })();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 服务状态由后端每 2 秒推一次（tray-status），省得菜单自己轮询
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let un: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      const f = await listen<{ running: boolean; port: number }>("tray-status", (e) => {
        setRunning(e.payload.running);
        setPort(e.payload.port);
      });
      if (disposed) f();
      else un = f;
    })();
    return () => {
      disposed = true;
      un?.();
    };
  }, []);

  // 渲染完把真实高度告诉后端，让它重新贴到托盘图标上。
  // useLayoutEffect 而不是 useEffect：晚一帧会看到菜单先歪一下再跳正。
  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    // 向上取整：小数高度在后端换算物理像素时会被截掉，底部就少一截。
    const h = Math.ceil(el.getBoundingClientRect().height);
    if (h > 0) {
      void invoke<void>("tray_menu_height", { height: h }).catch(() => {});
    }
  });

  const checkNow = async () => {
    setBusy("check");
    try {
      const r = await invoke<UpdateCheck>("update_check");
      const next: CachedUpdate = {
        has_update: r.has_update,
        latest: r.latest,
        current: r.current,
        ts: Date.now(),
      };
      writeCachedUpdate(r);
      setCached(next);
      setVersion(r.current);
      if (r.has_update) {
        // 有更新就回到界面去装（托盘里下载没有地方放进度条）
        await act("open");
      }
    } catch {
      /* 查不到就当没有更新，托盘里不弹任何东西 */
    } finally {
      setBusy(null);
    }
  };

  const hasUpdate = !!cached?.has_update;

  return (
    // ref 必须在**最外层**：量内层卡片会漏掉外层的 padding / border，
    // 后端按这个高度设窗口就会把底部切掉一截。
    <div ref={rootRef}>
      <div className="overflow-hidden rounded-xl border border-neutral-200 bg-white">
        {/* 头部：不是可点的菜单项，是状态说明 —— 别让它看起来像按钮 */}
        <div className="flex items-center justify-between gap-2 px-3 py-2.5">
          <span className="truncate text-[13px] font-semibold text-neutral-900">OCR Studio</span>
          {version && (
            <span className="shrink-0 rounded-sm bg-neutral-100 px-1.5 py-0.5 font-mono text-[10px] text-neutral-500">
              v{version}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1.5 px-3 pb-2.5">
          <span
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${running ? "bg-emerald-500" : "bg-neutral-300"}`}
          />
          <span className="truncate font-mono text-[11px] leading-none text-neutral-500">
            {running ? `服务运行中 · 127.0.0.1:${port}` : "服务已停止"}
          </span>
        </div>

        <div className="h-px bg-neutral-100" />

        <div className="p-1">
          <MenuItem icon={<AppWindow className="h-3.5 w-3.5" />} label="打开界面" onClick={() => void act("open")} />

          {running ? (
            <MenuItem
              icon={<Square className="h-3.5 w-3.5" />}
              label="停止服务"
              disabled={busy === "stop"}
              onClick={async () => {
                setBusy("stop");
                await act("stop");
                setBusy(null);
              }}
            />
          ) : (
            <MenuItem
              icon={<Play className="h-3.5 w-3.5" />}
              label="启动服务"
              disabled={busy === "start"}
              onClick={async () => {
                setBusy("start");
                await act("start");
                setBusy(null);
              }}
            />
          )}
        </div>

        <div className="h-px bg-neutral-100" />

        <div className="p-1">
          {hasUpdate ? (
            /*
             * 有更新：做成一条蓝色高亮条目。这是原生菜单永远做不出来的东西——
             * 它既放不了图标，也没办法让某一项「比别的重要」。
             */
            <MenuItem
              icon={<Download className="h-3.5 w-3.5" />}
              label={`有更新 v${cached?.latest ?? ""}`}
              hint="去设置页一键升级"
              tone="accent"
              badge
              onClick={() => void act("open")}
            />
          ) : (
            <MenuItem
              icon={<RefreshCw className={`h-3.5 w-3.5 ${busy === "check" ? "animate-spin" : ""}`} />}
              label="检查更新"
              disabled={busy === "check"}
              onClick={() => void checkNow()}
            />
          )}
        </div>

        <div className="h-px bg-neutral-100" />

        <div className="p-1">
          <MenuItem
            icon={<LogOut className="h-3.5 w-3.5" />}
            label="退出 OCR Studio"
            tone="danger"
            onClick={() => void act("quit")}
          />
        </div>
      </div>
    </div>
  );
}

function MenuItem({
  icon,
  label,
  hint,
  onClick,
  disabled,
  tone = "default",
  badge,
}: {
  icon: React.ReactNode;
  label: string;
  hint?: string;
  onClick: () => void;
  disabled?: boolean;
  tone?: "default" | "accent" | "danger";
  /** 右上角红点：用来表达「这里有个你还没处理的事」 */
  badge?: boolean;
}) {
  const toneCls =
    tone === "accent"
      ? "text-blue-700 hover:bg-blue-50"
      : tone === "danger"
        ? "text-neutral-700 hover:bg-red-50 hover:text-red-700"
        : "text-neutral-700 hover:bg-neutral-50";

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left text-[13px] leading-none transition-colors disabled:opacity-40 ${toneCls}`}
    >
      <span className="shrink-0 text-neutral-400">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {hint && <span className="shrink-0 text-[11px] leading-none text-neutral-400">{hint}</span>}
      {badge && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-blue-500" />}
    </button>
  );
}
