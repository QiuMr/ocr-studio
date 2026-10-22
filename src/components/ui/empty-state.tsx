import { cn } from "../../lib/utils";

interface Step {
  icon: React.ReactNode;
  /** 这一步要做什么，动词开头 */
  text: string;
  /** 补充说明，可选 */
  detail?: string;
  done?: boolean;
}

interface Props {
  icon?: React.ReactNode;
  title: string;
  description?: string;
  /** 「接下来这么做」的步骤列表——小白最需要的就是这个 */
  steps?: Step[];
  action?: React.ReactNode;
  className?: string;
}

/**
 * 空状态。
 *
 * 原则：不留一句干巴巴的「暂无数据」，而是告诉用户下一步点哪里。
 */
export function EmptyState({ icon, title, description, steps, action, className }: Props) {
  return (
    <div
      className={cn(
        "flex h-full min-h-[260px] flex-col items-center justify-center rounded-xl bg-neutral-50/70 px-6 py-8 text-center",
        className,
      )}
    >
      {icon && (
        <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-xl bg-white text-neutral-400 shadow-sm">
          {icon}
        </div>
      )}
      <div className="text-[13px] font-medium text-neutral-700">{title}</div>
      {description && (
        <p className="mt-1 max-w-[420px] text-[13px] leading-5 text-neutral-500">{description}</p>
      )}

      {steps && steps.length > 0 && (
        <ol className="mt-4 flex w-full max-w-[380px] flex-col gap-1.5 text-left">
          {steps.map((s, i) => (
            <li
              key={i}
              className="flex items-start gap-2.5 rounded-lg bg-white/80 px-3 py-2 text-[13px] text-neutral-600 shadow-[0_1px_2px_rgba(0,0,0,0.03)]"
            >
              <span
                className={cn(
                  "mt-[1px] flex h-4.5 w-4.5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold",
                  s.done
                    ? "bg-emerald-100 text-emerald-700"
                    : "bg-neutral-100 text-neutral-500",
                )}
                style={{ width: 18, height: 18 }}
              >
                {i + 1}
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-1.5">
                  <span className="text-neutral-400">{s.icon}</span>
                  <span>{s.text}</span>
                </span>
                {s.detail && (
                  <span className="mt-0.5 block text-[12px] text-neutral-400">{s.detail}</span>
                )}
              </span>
            </li>
          ))}
        </ol>
      )}

      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}
