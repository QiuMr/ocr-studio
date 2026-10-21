import { CircleHelp } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "./tooltip";
import { cn } from "../../lib/utils";

interface Props {
  /** 解释文案。面向小白：先说「是什么」，再说「什么时候该改」 */
  children: React.ReactNode;
  /** 提示框方向，靠近窗口右缘时用 left */
  side?: "top" | "right" | "bottom" | "left";
  className?: string;
}

/**
 * 术语提示（问号图标）。
 *
 * 界面上 DirectML、cls、NCC、阈值这些词对小白是黑话，
 * 一律在旁边挂一个问号，不占版面又能随时查。
 */
export function Hint({ children, side = "top", className }: Props) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          aria-label="查看说明"
          className={cn(
            "inline-flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full text-neutral-300 hover:text-blue-600 focus-visible:text-blue-600",
            className,
          )}
        >
          <CircleHelp className="h-3.5 w-3.5" strokeWidth={2} />
        </button>
      </TooltipTrigger>
      <TooltipContent side={side} className="max-w-[260px] leading-relaxed">
        {children}
      </TooltipContent>
    </Tooltip>
  );
}
