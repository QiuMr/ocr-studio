import { useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { buildSample, sceneNote, type Scene, type SceneOpts } from "../../lib/codegen";
import { useServerPort } from "../../lib/use-server-port";
import { cn } from "../../lib/utils";

interface Props {
  /** 这一步对应哪个接口 */
  scene: Scene;
  /** 场景参数（不用给 endpoint，组件自己拿当前端口） */
  opts?: Omit<SceneOpts, "endpoint">;
  className?: string;
}

/**
 * 「这一步怎么用脚本调」的代码面板。
 *
 * 抽成组件而不是每个面板抄一遍：语言切换、端口、复制状态这些逻辑
 * 在四处重复，迟早会出现「有的面板记住了语言、有的没有」这种不一致。
 *
 * 端口由组件自己拉（`useServerPort`），调用方不用管——写死会在用户
 * 改过端口之后给出一份连不上的示例代码。
 */
export function CallCodePanel({ scene, opts, className }: Props) {
  const port = useServerPort();
  const [lang, setLang] = useState<"python" | "curl">("python");
  const [copied, setCopied] = useState(false);

  const code = useMemo(
    () => buildSample(scene, { endpoint: `127.0.0.1:${port}`, ...opts })[lang],
    [scene, opts, port, lang],
  );

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      /* 代码本身可选中，用户能手动复制 */
    }
  };

  return (
    <div className={cn("shrink-0 rounded-lg bg-neutral-50/80 p-2.5", className)}>
      <div className="mb-1.5 flex items-start justify-between gap-2">
        <span className="text-[11px] leading-4 text-neutral-500">{sceneNote(scene)}</span>
        <div className="flex shrink-0 items-center gap-0.5 rounded-md bg-neutral-200/60 p-0.5">
          {(
            [
              { v: "python" as const, label: "Python" },
              { v: "curl" as const, label: "curl" },
            ]
          ).map((it) => (
            <button
              key={it.v}
              type="button"
              onClick={() => setLang(it.v)}
              className={cn(
                "h-6 rounded px-2 text-[11px] font-medium leading-none transition-colors",
                lang === it.v
                  ? "bg-white text-neutral-900 shadow-sm"
                  : "text-neutral-500 hover:text-neutral-800",
              )}
            >
              {it.label}
            </button>
          ))}
        </div>
      </div>
      <div className="relative">
        <pre className="selectable max-h-[200px] overflow-auto rounded-lg bg-[#0e1016] px-3 py-2.5 font-mono text-[11px] leading-[1.7] text-white/85">
          {code}
        </pre>
        <button
          type="button"
          onClick={() => void copy()}
          className={cn(
            "absolute right-2 top-2 flex items-center gap-1 rounded-md bg-white/10 px-2 py-1 text-[11px] font-medium backdrop-blur transition-colors",
            copied ? "text-emerald-400" : "text-white/60 hover:bg-white/20 hover:text-white",
          )}
        >
          {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
          {copied ? "已复制" : "复制"}
        </button>
      </div>
    </div>
  );
}
