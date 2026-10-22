import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "../../lib/utils";

interface Props {
  code: string;
  /** 右上角标签，比如 sh / python */
  lang?: string;
  className?: string;
}

/**
 * 代码块。
 *
 * 用深色底：一是和首页那块深色服务卡呼应，二是代码在浅色页面里本来就是
 * 「另一类内容」，颜色分开比加边框更省地方也更好认。
 *
 * 复制按钮做成**整块右上角可点**，不是只有 16px 的图标 —— 用户第一次
 * 想复制的时候不该需要瞄准。
 */
export function CodeBlock({ code, lang, className }: Props) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      /* 剪贴板不可用：代码本身可选中，用户能手动复制 */
    }
  };

  return (
    <div
      className={cn(
        "group relative overflow-hidden rounded-[12px] bg-[#0e1016] ring-1 ring-white/[0.06]",
        className,
      )}
    >
      <div className="flex items-center justify-between border-b border-white/[0.06] px-3 py-1.5">
        <span className="font-mono text-[10.5px] font-medium uppercase tracking-wider text-white/35">
          {lang ?? "code"}
        </span>
        <button
          type="button"
          onClick={() => void copy()}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] font-medium transition-colors",
            copied
              ? "text-emerald-400"
              : "text-white/40 hover:bg-white/[0.07] hover:text-white/90",
          )}
        >
          {copied ? (
            <>
              <Check className="animate-pop h-3 w-3" strokeWidth={3} />
              已复制
            </>
          ) : (
            <>
              <Copy className="h-3 w-3" />
              复制
            </>
          )}
        </button>
      </div>
      <pre className="selectable overflow-x-auto px-3.5 py-3 font-mono text-[11.5px] leading-[1.75] text-white/85">
        {code}
      </pre>
    </div>
  );
}
