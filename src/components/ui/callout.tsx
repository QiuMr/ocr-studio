import { cn } from "../../lib/utils";

type Variant = "info" | "warn" | "error" | "success";

const STYLE: Record<Variant, string> = {
  info: "bg-blue-50/70 text-blue-900",
  warn: "bg-amber-50 text-amber-900",
  error: "bg-red-50 text-red-800",
  success: "bg-emerald-50 text-emerald-800",
};

const ICON_STYLE: Record<Variant, string> = {
  info: "text-blue-600",
  warn: "text-amber-600",
  error: "text-red-600",
  success: "text-emerald-600",
};

interface Props {
  variant?: Variant;
  icon?: React.ReactNode;
  title?: string;
  children?: React.ReactNode;
  /** 右侧操作（如「重试」按钮） */
  action?: React.ReactNode;
  className?: string;
}

/**
 * 提示条。
 *
 * 不用边框，只用淡底色 —— 去掉分割线之后，边框会显得整屏都是线。
 */
export function Callout({ variant = "info", icon, title, children, action, className }: Props) {
  return (
    <div
      className={cn(
        "flex items-start gap-2.5 rounded-lg px-3.5 py-2.5 text-[12.5px] leading-5",
        STYLE[variant],
        className,
      )}
    >
      {icon && <span className={cn("mt-[3px] shrink-0", ICON_STYLE[variant])}>{icon}</span>}
      <div className="min-w-0 flex-1">
        {title && <div className="font-medium">{title}</div>}
        {children && <div className={cn(title && "mt-0.5 opacity-90")}>{children}</div>}
      </div>
      {action && <div className="shrink-0 self-center">{action}</div>}
    </div>
  );
}
