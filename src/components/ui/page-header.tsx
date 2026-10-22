import { cn } from "../../lib/utils";

interface Props {
  /** 页面标题，一句话说清这页干什么 */
  title: string;
  /** 副标题：给小白看的解释，避免只有名词没有上下文 */
  description?: string;
  /** 标题左侧图标，用蓝色点缀色 */
  icon?: React.ReactNode;
  /** 右侧操作区 */
  actions?: React.ReactNode;
  className?: string;
}

/**
 * 页面头部。
 *
 * 去掉了全局分割线之后，页面层级靠标题字号 + 留白建立，
 * 所以每个页面的第一屏都必须有这个头部，否则内容会像「贴在窗口顶上」。
 */
export function PageHeader({ title, description, icon, actions, className }: Props) {
  return (
    <div className={cn("mb-5 flex items-center justify-between gap-4", className)}>
      <div className="min-w-0">
        <h1 className="flex items-center gap-2 text-[17px] font-semibold leading-tight tracking-tight text-neutral-900">
          {icon && <span className="flex h-[18px] w-[18px] items-center justify-center text-blue-600">{icon}</span>}
          {title}
        </h1>
        {description && (
          <p className="mt-1 text-[12.5px] leading-5 text-neutral-500">{description}</p>
        )}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}
