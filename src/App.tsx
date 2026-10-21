import { useState } from "react";
import { LayoutDashboard, ScanText, Settings as SettingsIcon, Square } from "lucide-react";
import { TitleBar } from "./components/TitleBar";
import { Button } from "./components/ui/button";
import { TooltipProvider, Tooltip, TooltipTrigger, TooltipContent } from "./components/ui/tooltip";
import { Dashboard } from "./pages/Dashboard";
import { Playground } from "./pages/Playground";
import { SettingsPage } from "./pages/SettingsPage";
import { cn } from "./lib/utils";
import { isTauri } from "./lib/tauri";

type PageKey = "dashboard" | "playground" | "settings";

const NAV: { key: PageKey; label: string; icon: React.ReactNode }[] = [
  { key: "dashboard", label: "仪表盘", icon: <LayoutDashboard className="h-4 w-4" /> },
  { key: "playground", label: "测试台", icon: <ScanText className="h-4 w-4" /> },
  { key: "settings", label: "设置", icon: <SettingsIcon className="h-4 w-4" /> },
];

export default function App() {
  const [page, setPage] = useState<PageKey>("dashboard");

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex h-full flex-col overflow-hidden bg-white">
        <TitleBar />
        <div className="flex min-h-0 flex-1">
          {/* 侧边导航 */}
          <aside className="flex w-52 shrink-0 flex-col border-r border-neutral-200 bg-white">
            <nav className="flex flex-col gap-1 p-3">
              {NAV.map((item) => (
                <Tooltip key={item.key}>
                  <TooltipTrigger asChild>
                    <Button
                      variant={page === item.key ? "secondary" : "ghost"}
                      className="justify-start"
                      onClick={() => setPage(item.key)}
                    >
                      {item.icon}
                      {item.label}
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="right">{item.label}</TooltipContent>
                </Tooltip>
              ))}
            </nav>

            <div className="mt-auto p-3">
              {!isTauri() && (
                <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[11px] leading-4 text-amber-800">
                  浏览器预览模式：仅测试台的 HTTP 降级可用
                </div>
              )}
              <div
                className={cn(
                  "mt-2 flex items-center gap-2 px-1 text-[11px] text-neutral-400",
                )}
              >
                <Square className="h-2.5 w-2.5 fill-blue-600 text-blue-600" />
                OCR Studio v0.1
              </div>
            </div>
          </aside>

          {/* 主区 */}
          <main className="min-w-0 flex-1 overflow-y-auto p-6">
            {page === "dashboard" && <Dashboard />}
            {page === "playground" && <Playground />}
            {page === "settings" && <SettingsPage />}
          </main>
        </div>
      </div>
    </TooltipProvider>
  );
}
