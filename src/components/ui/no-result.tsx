import { Lightbulb } from "lucide-react";
import { cn } from "../../lib/utils";

interface Props {
  /** 结论，一句话。例如「这张图里没认出文字」 */
  title: string;
  /** 可能的原因 / 可以试的办法。2~3 条，动词开头，别写成说明书 */
  tips: string[];
  /** 可选的下一步按钮（比如「去装个模型」） */
  action?: React.ReactNode;
  className?: string;
}

/**
 * 「做完了，但什么都没找到」的引导。
 *
 * 和错误提示的区别：**这不是出错**。接口跑成功了，只是结果为空。
 * 所以语气是「接下来可以试试」，不是「出错了」；也不该给「复制给 AI」——
 * 那只会让用户以为程序坏了。
 *
 * 为什么单独一个组件而不复用 `EmptyState`：那个是整块区域的大空状态
 * （最小高 260px），这里是结果列表里的一行——塞进去会把版面撑开。
 */
export function NoResult({ title, tips, action, className }: Props) {
  return (
    <div
      className={cn(
        "rounded-lg bg-neutral-50 px-3.5 py-3 text-center",
        className,
      )}
    >
      <div className="text-[13px] font-medium text-neutral-600">{title}</div>
      {tips.length > 0 && (
        <ul className="mx-auto mt-1.5 flex max-w-[340px] flex-col gap-1 text-left">
          {tips.map((t) => (
            <li key={t} className="flex items-start gap-1.5 text-[12px] leading-4 text-neutral-500">
              <Lightbulb className="mt-[1px] h-3 w-3 shrink-0 text-amber-500" />
              <span>{t}</span>
            </li>
          ))}
        </ul>
      )}
      {action && <div className="mt-2.5 flex justify-center">{action}</div>}
    </div>
  );
}
