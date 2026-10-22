import { useCallback, useEffect, useRef, useState } from "react";
import { Braces, FlaskConical, ScrollText, Server, Settings as SettingsIcon } from "lucide-react";
import { TitleBar } from "./components/TitleBar";
import { TooltipProvider, Tooltip, TooltipTrigger, TooltipContent } from "./components/ui/tooltip";
import { ServicePage } from "./pages/ServicePage";
import { ApiPage } from "./pages/ApiPage";
import { Playground } from "./pages/Playground";
import { LogsPage } from "./pages/LogsPage";
import { SettingsPage } from "./pages/SettingsPage";
import { cn } from "./lib/utils";
import { invoke, isTauri } from "./lib/tauri";

export type PageKey = "service" | "api" | "playground" | "logs" | "settings";
/** 试跑页的三个 tab：认字 / 找图 / 扫码 */
export type OcrTab = "ocr" | "find" | "scan";

/**
 * 导航文案原则：**用用户想干的事，不用程序里的模块名**。
 *
 * 这是一个「把识别能力变成本地接口」的工具，所以第一项就是**服务**本身 ——
 * 把服务开起来、拿到地址，才是用户真正要的；认不认识字是我们内部的事。
 * 「仪表盘」「测试台」这类词是开发者视角，用户不会那样说话。
 */
const NAV: { key: PageKey; label: string; hint: string; icon: React.ReactNode }[] = [
  {
    key: "service",
    label: "服务",
    hint: "把本地接口开起来，脚本就能调",
    icon: <Server className="h-[18px] w-[18px]" strokeWidth={2} />,
  },
  {
    key: "api",
    label: "接口",
    hint: "每个接口干什么、怎么传、返回什么",
    icon: <Braces className="h-[18px] w-[18px]" strokeWidth={2} />,
  },
  {
    key: "playground",
    label: "试跑",
    hint: "丢一张图进去，验证接口通不通",
    icon: <FlaskConical className="h-[18px] w-[18px]" strokeWidth={2} />,
  },
  {
    key: "logs",
    label: "日志",
    hint: "每次调用都有记录，排查用",
    icon: <ScrollText className="h-[18px] w-[18px]" strokeWidth={2} />,
  },
  {
    key: "settings",
    label: "设置",
    hint: "模型、显卡加速、端口",
    icon: <SettingsIcon className="h-[18px] w-[18px]" strokeWidth={2} />,
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
  const [page, setPage] = useState<PageKey>("service");
  const [ocrTab, setOcrTab] = useState<OcrTab>("ocr");
  /**
   * 每次自增 = 「请试跑页把文件选择框弹出来」。
   *
   * 用户在服务台点「传张图试试」，意思是「我现在就要跑一次」，
   * 不该让他跳到试跑页之后自己再找一次入口。用计数器而不是布尔量，
   * 是因为连续点两次也要能再弹一次。
   */
  const [pickToken, setPickToken] = useState(0);
  const scrollRef = useRef<HTMLDivElement>(null);

  /*
   * 切页回到顶部。
   *
   * 滚动容器是共用的，不重置的话从长页面（接口文档）切到短页面时，
   * 用户会直接落在新页面的中间 —— 看起来像"这页怎么没有标题"。
   */
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [page]);

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

  /** 去试跑页；`pick` 为真时顺带把选图框弹出来 */
  const openPlayground = useCallback((tab: OcrTab, pick = false) => {
    setOcrTab(tab);
    setPage("playground");
    if (pick) setPickToken((t) => t + 1);
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
      {/*
       * 外壳（标题栏 + 侧栏）统一浅灰，主内容区是**压在灰底上的白色圆角块**。
       *
       * 转角之所以是圆的，靠的就是 rounded-tl：白色块的左上角按 14px 收进去，
       * 露出来的那一点灰就是外壳色。以前外壳是纯白、内容也是纯白，
       * 只能用一条 1px 分割线去分界，既生硬又躲不开方角。
       */}
      <div className="flex h-full flex-col overflow-hidden bg-chrome">
        <TitleBar collapsed={collapsed} onToggleSidebar={toggleSidebar} />

        <div className="flex min-h-0 flex-1">
          <aside
            className={cn(
              "ease-sidebar flex shrink-0 flex-col overflow-hidden",
              collapsed ? "w-[64px]" : "w-[188px]",
            )}
          >
            <nav className={cn("flex flex-col gap-0.5 px-2.5 pt-0.5", collapsed && "px-2.5")}>
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

            <div className="mt-auto px-2.5 pb-3">
              {!isTauri() && !collapsed && (
                <div className="mb-2 rounded-lg bg-amber-50 px-2.5 py-2 text-[11px] font-medium leading-4 text-amber-800">
                  浏览器预览：接口调用不了后端
                </div>
              )}
              {/*
               * 不用 Tooltip 包这一行：Radix 的 Trigger 会把子节点变成 inline 元素，
               * 在 flex 里量出来的可用宽度变成 0，文字直接被 truncate 成省略号
               * （实测只剩一个小圆点）。原生 title 足够。
               */}
              <div
                title="OCR Studio v0.1 · MIT"
                className={cn(
                  "flex items-center gap-2 rounded-lg px-2 py-1.5 text-[11px] font-medium leading-none text-neutral-400",
                  collapsed && "justify-center px-0",
                )}
              >
                <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-neutral-300" />
                {!collapsed && <span className="truncate">v0.1</span>}
              </div>
            </div>
          </aside>

          {/* 主区：白色圆角块。滚动条由它自己管，切页面时不会带着别页的滚动位置 */}
          <main className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-tl-xl bg-white">
            <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
              <div
                key={page}
                className="animate-fade-rise mx-auto flex min-h-full w-full max-w-[1180px] flex-col px-8 py-6"
              >
                {page === "service" && (
                  <ServicePage
                    onOpenPlayground={openPlayground}
                    onOpenApi={() => setPage("api")}
                    onOpenSettings={() => setPage("settings")}
                  />
                )}
                {page === "api" && <ApiPage />}
                {page === "playground" && (
                  <Playground tab={ocrTab} onTabChange={setOcrTab} pickToken={pickToken} />
                )}
                {page === "logs" && <LogsPage />}
                {page === "settings" && <SettingsPage />}
              </div>
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
        "group flex h-9 items-center rounded-md text-[13px] leading-none",
        collapsed ? "w-9 justify-center" : "w-full gap-2.5 px-2.5",
        // 浅灰外壳上，选中项用**纯白浮起**（VS Code / Notion 的做法）：
        // 在 #f7f7f8 底上加深灰块会显得脏，白色 + 极淡描边 + 微阴影更干净
        active
          ? "shadow-card bg-white font-semibold text-neutral-900 ring-1 ring-black/[0.04]"
          : "font-medium text-neutral-600 hover:bg-black/[0.05] hover:text-neutral-900 active:bg-black/[0.08]",
      )}
    >
      <span
        className={cn(
          "flex h-[18px] w-[18px] shrink-0 items-center justify-center transition-all duration-200 group-hover:scale-110",
          active ? "text-neutral-900" : "text-neutral-400 group-hover:text-neutral-600",
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
