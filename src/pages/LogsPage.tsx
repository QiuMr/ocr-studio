import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownToLine,
  Copy,
  Pause,
  Play,
  ScrollText,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { PageHeader } from "../components/ui/page-header";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { EmptyState } from "../components/ui/empty-state";
import { Callout } from "../components/ui/callout";
import { getLogs, subscribeLogs } from "../lib/api";
import { isTauri } from "../lib/tauri";
import { cn } from "../lib/utils";
import type { LogLevel, LogLine } from "../types";

const LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

const LEVEL_STYLE: Record<LogLevel, string> = {
  trace: "bg-neutral-100 text-neutral-500",
  debug: "bg-neutral-100 text-neutral-600",
  info: "bg-blue-50 text-blue-700",
  warn: "bg-amber-100 text-amber-800",
  error: "bg-red-100 text-red-700",
};

/** 级别过滤的下限：选 info 就显示 info/warn/error，和 tracing 的语义一致。 */
const SEVERITY: Record<LogLevel, number> = {
  trace: 0,
  debug: 1,
  info: 2,
  warn: 3,
  error: 4,
};

const MAX_KEEP = 3000;

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

export function LogsPage() {
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [minLevel, setMinLevel] = useState<LogLevel>("info");
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState(false);
  // 暂停时新日志不能直接丢——先攒着，恢复时一次性补上
  const [pending, setPending] = useState<LogLine[]>([]);
  const [follow, setFollow] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const listRef = useRef<HTMLDivElement>(null);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const followRef = useRef(follow);
  followRef.current = follow;

  const append = useCallback((incoming: LogLine[]) => {
    if (incoming.length === 0) return;
    if (pausedRef.current) {
      setPending((prev) => [...prev, ...incoming].slice(-MAX_KEEP));
      return;
    }
    setLogs((prev) => {
      const next = prev.concat(incoming);
      return next.length > MAX_KEEP ? next.slice(next.length - MAX_KEEP) : next;
    });
  }, []);

  // 首次：先订阅（不漏新行），再拉历史（拿到订阅之前的）
  useEffect(() => {
    let alive = true;
    let unsub: (() => void) | undefined;

    void (async () => {
      if (!isTauri()) {
        setError("浏览器预览模式拿不到后端日志，请在 App 内查看。");
        return;
      }
      try {
        unsub = await subscribeLogs((lines) => {
          if (alive) append(lines);
        });
        const history = await getLogs();
        if (alive) setLogs(history.slice(-MAX_KEEP));
      } catch (e) {
        if (alive) setError(`读取日志失败: ${e}`);
      }
    })();

    return () => {
      alive = false;
      unsub?.();
    };
  }, [append]);

  // 恢复时把暂停期间攒的补进去
  useEffect(() => {
    if (paused || pending.length === 0) return;
    setLogs((prev) => {
      const next = prev.concat(pending);
      return next.length > MAX_KEEP ? next.slice(next.length - MAX_KEEP) : next;
    });
    setPending([]);
  }, [paused, pending]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const floor = SEVERITY[minLevel];
    return logs.filter((l) => {
      if (SEVERITY[l.level] < floor) return false;
      if (!q) return true;
      return (
        l.message.toLowerCase().includes(q) ||
        l.target.toLowerCase().includes(q) ||
        l.level.includes(q)
      );
    });
  }, [logs, minLevel, query]);

  // 跟随滚动：只有用户没手动上滚时才自动到底
  useEffect(() => {
    if (!follow) return;
    const el = listRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [filtered.length, follow]);

  const onScroll = () => {
    const el = listRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    setFollow(atBottom);
  };

  const copyAll = async () => {
    const text = filtered
      .map((l) => `${fmtTime(l.ts_ms)} ${l.level.toUpperCase().padEnd(5)} ${l.target} ${l.message}`)
      .join("\n");
    await navigator.clipboard.writeText(text);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };

  const counts = useMemo(() => {
    const c: Record<string, number> = {};
    for (const l of logs) c[l.level] = (c[l.level] ?? 0) + 1;
    return c;
  }, [logs]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PageHeader
        icon={<ScrollText className="h-[18px] w-[18px]" />}
        title="运行日志"
        description="引擎、HTTP 服务、找图的实际执行记录。遇到问题时先在这里看有没有红色条目。"
        actions={
          <>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPaused((p) => !p)}
              title={paused ? "恢复接收新日志" : "暂停接收，方便翻看"}
            >
              {paused ? <Play className="h-3.5 w-3.5" /> : <Pause className="h-3.5 w-3.5" />}
              {paused ? `已暂停${pending.length > 0 ? ` (${pending.length})` : ""}` : "暂停"}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void copyAll()}
              disabled={filtered.length === 0}
            >
              {copied ? <Copy className="h-3.5 w-3.5 text-emerald-600" /> : <Copy className="h-3.5 w-3.5" />}
              {copied ? "已复制" : "复制"}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={() => {
                setLogs([]);
                setPending([]);
              }}
              disabled={logs.length === 0}
              title="只清空界面上的记录，磁盘日志文件不动"
            >
              <Trash2 className="h-3.5 w-3.5" />
              清屏
            </Button>
          </>
        }
      />

      {error && (
        <Callout variant="warn" icon={<X className="h-4 w-4" />} className="mb-3">
          {error}
        </Callout>
      )}

      {/* 过滤栏 */}
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="flex items-center gap-1 rounded-lg bg-neutral-100 p-0.5">
          {LEVELS.map((lv) => (
            <button
              key={lv}
              type="button"
              onClick={() => setMinLevel(lv)}
              className={cn(
                "rounded-md px-2.5 py-1 text-[12px] font-medium uppercase tracking-wide",
                minLevel === lv
                  ? "bg-white text-neutral-900 shadow-sm"
                  : "text-neutral-500 hover:text-neutral-800",
              )}
              title={lv === minLevel ? "当前下限" : `只看 ${lv} 及以上`}
            >
              {lv}
              {counts[lv] ? (
                <span className="ml-1 font-normal text-neutral-400">{counts[lv]}</span>
              ) : null}
            </button>
          ))}
        </div>

        <div className="relative min-w-[200px] flex-1">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-neutral-400" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索内容或模块，例如 find / 找图 / 8848"
            className="h-8 pl-8 text-[13px]"
          />
        </div>

        <button
          type="button"
          onClick={() => setFollow((f) => !f)}
          className={cn(
            "flex h-8 items-center gap-1.5 rounded-md px-2.5 text-[12px]",
            follow
              ? "bg-blue-50 text-blue-700"
              : "text-neutral-500 hover:bg-neutral-100 hover:text-neutral-800",
          )}
          title="关闭后新日志不会把视图拽到底部，方便往回翻"
        >
          <ArrowDownToLine className="h-3.5 w-3.5" />
          {follow ? "跟随最新" : "已固定"}
        </button>

        <span className="text-[12px] text-neutral-400">
          共 {filtered.length} 条 / 缓冲 {logs.length}
        </span>
      </div>

      {/* 日志本体 */}
      <div
        ref={listRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 overflow-auto rounded-xl bg-neutral-50/70 px-1 py-1"
      >
        {filtered.length === 0 ? (
          <EmptyState
            className="h-full bg-transparent"
            icon={<ScrollText className="h-5 w-5" />}
            title={logs.length === 0 ? "还没有日志" : "没有符合过滤条件的日志"}
            description={
              logs.length === 0
                ? "启动引擎或跑一次识别后，日志会实时出现在这里。"
                : "试着降低级别下限，或清空搜索关键字。"
            }
          />
        ) : (
          <ul className="flex flex-col">
            {filtered.map((l, i) => (
              <li
                key={`${l.ts_ms}-${i}`}
                className="group flex items-start gap-2 rounded-md px-2 py-[3px] font-mono text-[12px] leading-[1.55] hover:bg-white"
              >
                <span className="shrink-0 tabular-nums text-neutral-400">{fmtTime(l.ts_ms)}</span>
                <span
                  className={cn(
                    "shrink-0 rounded px-1 text-[10px] font-semibold uppercase leading-[1.5]",
                    LEVEL_STYLE[l.level],
                  )}
                >
                  {l.level}
                </span>
                <span
                  className="hidden shrink-0 max-w-[190px] truncate text-neutral-400 lg:inline"
                  title={l.target}
                >
                  {l.target}
                </span>
                <span className="selectable min-w-0 flex-1 whitespace-pre-wrap break-words text-neutral-800">
                  {l.message}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
