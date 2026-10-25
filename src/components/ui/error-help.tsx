import { useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  Check,
  ChevronDown,
  Copy,
  Info,
  RefreshCw,
  Sparkles,
  XCircle,
} from "lucide-react";
import { Callout } from "./callout";
import { Button } from "./button";
import { cn } from "../../lib/utils";
import {
  buildAiHelpPrompt,
  describeError,
  errorText,
  type ErrorActionKind,
} from "../../lib/errors";

const ACTION_ICON: Record<ErrorActionKind, React.ReactNode> = {
  "open-models": <ArrowRight className="h-3.5 w-3.5" />,
  "open-ai": <Sparkles className="h-3.5 w-3.5" />,
  "open-service": <ArrowRight className="h-3.5 w-3.5" />,
  "open-settings": <ArrowRight className="h-3.5 w-3.5" />,
  retry: <RefreshCw className="h-3.5 w-3.5" />,
};

const TONE_ICON = {
  error: <XCircle className="h-4 w-4" />,
  warn: <AlertTriangle className="h-4 w-4" />,
  info: <Info className="h-4 w-4" />,
};

interface Props {
  /** `catch` 到的原始错误。人话翻译在组件内部做，页面不许自己拼文案 */
  error: unknown;
  /** 点「下一步」按钮时回调；不传就不显示动作按钮 */
  onAction?: (kind: ErrorActionKind) => void;
  /** 「我刚才在做什么」，复制给 AI 时会一起带上 */
  context?: string;
  className?: string;
}

/**
 * 错误的统一展示：人话标题 + 为什么 + 下一步 + 复制给 AI。
 *
 * 为什么集中成一个组件：错误提示散在各页面时，每个页面各写一份文案，
 * 改一处漏三处，最后同一个 503 在四个页面四种说法。现在翻译只有
 * `lib/errors.ts` 一份，这里只负责渲染。
 *
 * 「复制给 AI」是刻意留的出口：总有些错误是这张表没覆盖到的，
 * 与其显示一句用户看不懂的原文，不如打包成提示词让他粘给任意 AI。
 */
export function ErrorHelp({ error, onAction, context, className }: Props) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const help = describeError(error);
  const raw = errorText(error);

  const copyForAi = async () => {
    try {
      await navigator.clipboard.writeText(buildAiHelpPrompt(error, { hint: context }));
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {
      /* 剪贴板被拒就算了，不值得为它报错 */
    }
  };

  return (
    <Callout
      variant={help.tone}
      icon={TONE_ICON[help.tone]}
      title={help.title}
      className={className}
      action={
        help.action && onAction ? (
          <Button
            size="xs"
            variant="outline"
            onClick={() => onAction(help.action!.kind)}
            className="shrink-0"
          >
            {ACTION_ICON[help.action.kind]}
            {help.action.label}
          </Button>
        ) : undefined
      }
    >
      <div className="space-y-1">
        {help.why && <div>{help.why}</div>}
        {help.next && <div className="font-medium">下一步：{help.next}</div>}
      </div>

      {/* 原文默认收起：它写给开发者看，露出来只会让小白更慌 */}
      {raw && (
        <div className="mt-2">
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            className="inline-flex items-center gap-1 text-[11px] opacity-70 hover:opacity-100"
          >
            <ChevronDown className={cn("h-3 w-3 transition-transform", open && "rotate-180")} />
            {open ? "收起详情" : "看原始报错"}
          </button>
          {open && (
            <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap break-all rounded-md bg-black/5 px-2.5 py-2 text-[11px] leading-4 opacity-80">
              {raw}
            </pre>
          )}
        </div>
      )}

      <div className="mt-2 flex items-center gap-2">
        <Button size="xs" variant="ghost" onClick={copyForAi} className="px-2">
          {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          {copied ? "已复制" : "复制给 AI"}
        </Button>
        <span className="text-[10px] opacity-60">粘给任意 AI，它会告诉你怎么修</span>
      </div>
    </Callout>
  );
}
