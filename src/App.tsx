import { useCallback, useEffect, useState } from "react";
import {
  LayoutDashboard,
  ScanText,
  Settings as SettingsIcon,
  ScrollText,
} from "lucide-react";
import { TitleBar } from "./components/TitleBar";
import { TooltipProvider, Tooltip, TooltipTrigger, TooltipContent } from "./components/ui/tooltip";
import { Dashboard } from "./pages/Dashboard";
import { Playground } from "./pages/Playground";
import { LogsPage } from "./pages/LogsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { cn } from "./lib/utils";
import { invoke, isTauri } from "./lib/tauri";

export type PageKey = "dashboard" | "playground" | "logs" | "settings";

const NAV: { key: PageKey; label: string; hint: string; icon: React.ReactNode }[] = [
  {
    key: "dashboard",
    label: "仪表盘",
    hint: "引擎状态、模型档位、本地服务开关",
    icon: <LayoutDashboard className="h-[18px] w-[18px]" strokeWidth={1.9} />,
  },
  {
    key: "playground",
    label: "测试台",
    hint: "上传或截图，试文字识别与找图",
    icon: <ScanText className="h-[18px] w-[18px]" strokeWidth={1.9} />,
  },
  {
    key: "logs",
    label: "运行日志",
    hint: "实时日志，排查问题用",
    icon: <ScrollText className="h-[18px] w-[18px]" strokeWidth={1.9} />,
  },
  {
    key: "settings",
    label: "设置",
    hint: "端口、默认档位、设备",
    icon: <SettingsIcon className="h-[18px] w-[18px]" strokeWidth={1.9} />,
  },
];

const SIDEBAR_KEY = "ocr-studio:sidebar-collapsed";

export default function App() {
  // 记住上次的折叠状态：每次开 App 都要重新点一次很烦
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(SIDEBAR_KEY) === "1";
    } catch {
      return false;
    }
  });
  const [page, setPage] = useState<PageKey>("dashboard");

  const toggleSidebar = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev;
      try {
        localStorage.setItem(SIDEBAR_KEY, next ? "1" : "0");
      } catch {
        /* 隐私模式下 localStorage 可能不可写，忽略 */
      }
      return next;
    });
  }, []);

  // 报到：告诉后端「界面已经渲染出来了」。
  // 后端有个看门狗在等这个信号——WebView2 的 GPU 进程崩溃时 JS 根本不会执行，
  // 收不到报到就自动切软件合成重建窗口（见 src-tauri/src/lib.rs）。
  useEffect(() => {
    void invoke("ui_ready").catch(() => {
      /* 浏览器预览模式下没有这个命令，忽略 */
    });
  }, []);

  return (
    <TooltipProvider delayDuration={200}>
      <div className="flex h-full flex-col overflow-hidden bg-white">
        <TitleBar collapsed={collapsed} onToggleSidebar={toggleSidebar} />

        <div className="flex min-h-0 flex-1">
          {/* 侧边导航：与标题栏同色（浅灰外壳），靠底色差与白色主区分层，不加边框 */}
          <aside
            className={cn(
              "ease-sidebar flex shrink-0 flex-col overflow-hidden bg-chrome",
              collapsed ? "w-[60px]" : "w-[196px]",
            )}
          >
            <nav className={cn("flex flex-col gap-0.5", collapsed ? "px-3" : "px-3")}>
              {NAV.map((item) => (
                <NavItem
                  key={item.key}
                  label={item.label}
                  hint={item.hint}
                  icon={item.icon}
                  active={page === item.key}
                  collapsed={collapsed}
                  onClick={() => setPage(item.key)}
                />
              ))}
            </nav>

            <div className="mt-auto px-3 pb-3">
              {!isTauri() && !collapsed && (
                <div className="mb-2 rounded-md bg-amber-50 px-2.5 py-2 text-[11px] leading-4 text-amber-800">
                  浏览器预览：仅测试台的 HTTP 降级可用
                </div>
              )}
              <Tooltip>
                <TooltipTrigger asChild>
                  <div
                    className={cn(
                      "flex items-center gap-2 rounded-md px-2 py-1.5 text-[11px] leading-none text-neutral-400",
                      collapsed && "justify-center px-0",
                    )}
                  >
                    <span className="relative flex h-2 w-2 shrink-0">
                      <span className="absolute inline-flex h-full w-full rounded-full bg-neutral-400 opacity-40" />
                      <span className="relative inline-flex h-2 w-2 rounded-full bg-neutral-500" />
                    </span>
                    {!collapsed && <span className="truncate">OCR Studio v0.1</span>}
                  </div>
                </TooltipTrigger>
                <TooltipContent side="right">OCR Studio v0.1 · MIT</TooltipContent>
              </Tooltip>
            </div>
          </aside>

          {/* 主区：内容自己做左右留白，主区本身不加边框 */}
          <main className="min-w-0 flex-1 overflow-y-auto">
            <div
              key={page}
              className="animate-fade-rise mx-auto flex min-h-full w-full max-w-[1240px] flex-col px-7 py-5"
            >
              {page === "dashboard" && <Dashboard onNavigate={setPage} />}
              {page === "playground" && <Playground />}
              {page === "logs" && <LogsPage />}
              {page === "settings" && <SettingsPage />}
            </div>
          </main>
        </div>
      </div>
    </TooltipProvider>
  );
}

function NavItem({
  label,
  hint,
  icon,
  active,
  collapsed,
  onClick,
}: {
  label: string;
  hint: string;
  icon: React.ReactNode;
  active: boolean;
  collapsed: boolean;
  onClick: () => void;
}) {
  const btn = (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={cn(
        // leading-none：13px 文字默认行高 1.5（19.5px），配 18px 图标会看起来偏下
        "group flex h-9 items-center rounded-lg text-[13px] leading-none",
        collapsed ? "w-9 justify-center" : "w-full gap-2.5 px-2.5",
        // 浅灰外壳上，选中项用**纯白浮起**（VS Code / Notion 的做法）：
        // 在 #f7f7f8 底上加深灰块会显得脏，白色 + 极淡描边 + 微阴影更干净
        active
          ? "bg-white font-medium text-neutral-900 shadow-[0_1px_2px_rgba(16,24,40,0.06)] ring-1 ring-black/[0.04]"
          : "font-normal text-neutral-600 hover:bg-black/[0.04] hover:text-neutral-900 active:bg-black/[0.07]",
      )}
    >
      <span
        className={cn(
          "flex h-[18px] w-[18px] shrink-0 items-center justify-center",
          active ? "text-neutral-800" : "text-neutral-400 group-hover:text-neutral-600",
        )}
      >
        {icon}
      </span>
      {!collapsed && <span className="truncate">{label}</span>}
    </button>
  );

  return (
    <Tooltip>
      <TooltipTrigger asChild>{btn}</TooltipTrigger>
      <TooltipContent side="right" className="max-w-[220px]">
        <div className="font-medium">{label}</div>
        <div className="mt-0.5 text-[11px] opacity-70">{hint}</div>
      </TooltipContent>
    </Tooltip>
  );
}
