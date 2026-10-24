import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Activity,
  ArrowRight,
  Boxes,
  Check,
  Copy,
  Crosshair,
  FileSpreadsheet,
  Info,
  Loader2,
  Power,
  Square,
  Terminal,
  Type,
} from "lucide-react";
import { CodeBlock } from "../components/ui/code-block";
import { getConfig, getHealth, serverStart, serverStatus, serverStop } from "../lib/api";
import { isTauri } from "../lib/tauri";
import { cn } from "../lib/utils";
import {
  ENDPOINTS,
  ENDPOINT_PATH_COUNT,
  GROUP_DESC,
  GROUP_LABEL,
  GROUP_ORDER,
  groupOf,
  type EndpointGroup,
} from "../lib/endpoints";
import type { OcrTab } from "../App";
import type { AppConfig, Health } from "../types";

/*
 * 首页 = 服务台。
 *
 * 这个软件的本质是**一台跑在本机的接口服务**，桌面界面只是它的控制面板。
 * 所以首页第一眼必须回答三个问题：
 *   1. 服务开没开？
 *   2. 地址是什么？（能一键复制走）
 *   3. 能调什么、怎么调？（给现成代码）
 * 把「上传一张图看看识别效果」放第二位——那是验证手段，不是产品本身。
 */

/**
 * 每个组的图标和底色。
 *
 * 用 `Record<EndpointGroup, …>` 而不是数组：新增组时忘配图标会**编译报错**。
 * 原来这里是手写数组，加「文档」组时漏了一项，表格在首页概览里直接不显示。
 */
const GROUP_STYLE: Record<EndpointGroup, { icon: React.ReactNode; tint: string }> = {
  ocr: { icon: <Type className="h-4 w-4" />, tint: "bg-[#EFF4FF] text-[#2563EB]" },
  find: { icon: <Crosshair className="h-4 w-4" />, tint: "bg-[#F2EEFF] text-[#6D4AE6]" },
  vision: { icon: <Boxes className="h-4 w-4" />, tint: "bg-[#E9F7F1] text-[#0E9F6E]" },
  doc: { icon: <FileSpreadsheet className="h-4 w-4" />, tint: "bg-[#FFF7E8] text-[#B7791F]" },
  status: { icon: <Activity className="h-4 w-4" />, tint: "bg-[#F1F3F5] text-[#5B6472]" },
};

const LEVEL_TEXT: Record<string, string> = {
  tiny: "快",
  small: "均衡",
  medium: "准",
};

function fmtUptime(sec: number): string {
  if (sec <= 0) return "—";
  if (sec < 60) return `${sec} 秒`;
  if (sec < 3600) return `${Math.floor(sec / 60)} 分 ${sec % 60} 秒`;
  return `${Math.floor(sec / 3600)} 小时 ${Math.floor((sec % 3600) / 60)} 分`;
}

interface Props {
  /** 去试跑页；pick=true 时顺带弹出选图框 */
  onOpenPlayground: (tab: OcrTab, pick?: boolean) => void;
  onOpenApi: () => void;
  onOpenSettings: () => void;
}

export function ServicePage({ onOpenPlayground, onOpenApi, onOpenSettings }: Props) {
  const [cfg, setCfg] = useState<AppConfig | null>(null);
  const [health, setHealth] = useState<Health | null>(null);
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [lang, setLang] = useState<"python" | "curl" | "eyuyan">("python");

  /** 运行时长基准：后端给的是「查那一刻」的秒数，本地按秒往上加 */
  const [base, setBase] = useState<{ sec: number; at: number } | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const tauri = isTauri();
  const port = cfg?.port ?? 8848;
  const endpoint = `127.0.0.1:${port}`;

  const refresh = useCallback(async () => {
    if (!tauri) return;
    try {
      const [c, h, s] = await Promise.all([getConfig(), getHealth(), serverStatus()]);
      setCfg(c);
      setHealth(h);
      setRunning(s);
      setBase({ sec: h.uptime_sec, at: Date.now() });
      setError(null);
    } catch (e) {
      // 服务没起来时 getHealth 走的是 HTTP，会连接失败——这不是错误，只是状态
      setRunning(false);
      setError(null);
      void e;
    }
  }, [tauri]);

  useEffect(() => {
    void refresh();
    // 5 秒对一次：服务可能在别处被停掉，状态不能只靠按钮自己改
    const id = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(id);
  }, [refresh]);

  // 每秒走一次，让「已运行」这个数字是活的（同时也表明服务在跑）
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, []);

  const uptimeSec = base ? base.sec + Math.floor((now - base.at) / 1000) : 0;
  const gpu = health?.actual_device === "directml";

  const onToggle = async () => {
    if (!tauri) {
      setError("浏览器预览模式不能控制服务，请在桌面 App 里操作。");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (running) {
        await serverStop();
        setRunning(false);
        setBase(null);
      } else {
        await serverStart();
        setRunning(true);
        // 刚启动时立刻拉一次，把真实端口与运行时长接上
        window.setTimeout(() => void refresh(), 300);
      }
    } catch (e) {
      setError(`操作失败：${e}`);
    } finally {
      setBusy(false);
    }
  };

  const copyEndpoint = async () => {
    try {
      await navigator.clipboard.writeText(`http://${endpoint}`);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      /* 地址本身可选中，忽略 */
    }
  };

  /** 三种语言的「整图识别」示例，端口跟着设置走 */
  const samples = useMemo(
    () => ({
      python: `import base64, requests

b64 = base64.b64encode(open("shot.png", "rb").read()).decode()
r = requests.post("http://${endpoint}/ocr/shibie",
                  json={"image": b64}).json()
print(r["data"]["text"])`,
      curl: `curl -X POST http://${endpoint}/ocr/shibie \\
  -H "Content-Type: application/json" \\
  -d "{\\"image\\":\\"<图片base64>\\"}"`,
      eyuyan: `1. 图片文件 → Base64 字符串（去掉前缀）
2. POST http://${endpoint}/ocr/shibie
   Content-Type: application/json
   请求体：{"image":"<base64>"}
3. 取返回 JSON 里的 data.text`,
    }),
    [endpoint],
  );

  const tauriMissing = !tauri;

  return (
    <div className="flex flex-1 flex-col">
      {/* ------------------------------------------------------------ 标题 */}
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-[26px] font-bold leading-none tracking-tight text-neutral-900">
            服务
          </h1>
          <p className="mt-2 text-[13px] font-medium leading-5 text-neutral-500">
            把本地接口开起来，你的脚本就能调识别、找图和看图
          </p>
        </div>
        <button
          type="button"
          onClick={() => onOpenPlayground("ocr", true)}
          className="flex h-8 items-center gap-1.5 rounded-md border border-neutral-200 bg-white px-3 text-[12px] font-semibold text-neutral-700 shadow-card transition-colors hover:border-neutral-300 hover:text-neutral-900"
        >
          <Activity className="h-3.5 w-3.5 text-neutral-400" />
          传张图试试
        </button>
      </div>

      {/* -------------------------------------------------------- 服务主卡 */}
      <section className="panel-dark mt-5 overflow-hidden rounded-xl px-6 py-5 text-white">
        <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-4">
          <div className="min-w-0">
            {/* 状态：点 + 文字。运行时外圈有水波，静止时不动 */}
            <div className="flex items-center gap-2.5">
              <span className="relative flex h-2.5 w-2.5 shrink-0">
                {running && (
                  <span className="animate-ping-ring absolute inset-0 rounded-full bg-emerald-400" />
                )}
                <span
                  className={cn(
                    "relative h-2.5 w-2.5 rounded-full",
                    running
                      ? "animate-soft-pulse bg-emerald-400"
                      : tauriMissing
                        ? "bg-amber-400"
                        : "bg-white/25",
                  )}
                />
              </span>
              <span className="text-[13px] font-bold tracking-wide text-white/90">
                {running ? "服务运行中" : tauriMissing ? "需要在桌面 App 里使用" : "服务已停止"}
              </span>
            </div>

            {/* 地址：整页最大的一行字，因为这是用户最想拿走的东西 */}
            <div className="mt-3.5 flex flex-wrap items-center gap-2.5">
              <code
                className={cn(
                  "tnum font-mono text-[26px] font-bold leading-none tracking-tight transition-colors",
                  running ? "text-white" : "text-white/35",
                )}
              >
                {endpoint}
              </code>
              <button
                type="button"
                onClick={() => void copyEndpoint()}
                title="复制地址"
                className={cn(
                  "flex h-7 items-center gap-1.5 rounded-md px-2 text-[12px] font-semibold transition-colors",
                  copied
                    ? "bg-emerald-400/15 text-emerald-300"
                    : "bg-white/[0.08] text-white/60 hover:bg-white/[0.14] hover:text-white",
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

            <p className="mt-2.5 text-[12px] font-medium text-white/40">
              只监听本机 127.0.0.1，别的电脑连不进来 · 端口在设置里改
            </p>
          </div>

          {/* 启停：这是整页唯一的主动作，所以做成实心大按钮 */}
          <button
            type="button"
            onClick={() => void onToggle()}
            disabled={busy}
            className={cn(
              "flex h-11 shrink-0 items-center gap-2 rounded-md px-5 text-[13px] font-bold transition-all duration-200",
              running
                ? "bg-white/[0.08] text-white/85 ring-1 ring-inset ring-white/15 hover:bg-white/[0.14] hover:text-white active:scale-[0.98]"
                : "btn-brand text-white",
              busy && "cursor-wait opacity-70",
            )}
          >
            {busy ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : running ? (
              <Square className="h-3.5 w-3.5" strokeWidth={2.5} />
            ) : (
              <Power className="h-4 w-4" strokeWidth={2.5} />
            )}
            {running ? "停止服务" : "启动服务"}
          </button>
        </div>

        {/* 运行统计：三个短指标，用细线隔开，不做成卡片（卡片会削弱主卡的整体感） */}
        <div className="mt-5 grid grid-cols-3 divide-x divide-white/[0.08] border-t border-white/[0.08] pt-4">
          <MiniStat label="已运行" value={running ? fmtUptime(uptimeSec) : "—"} live={running} />
          <MiniStat
            label="推理设备"
            value={running ? (gpu ? "显卡加速" : "处理器") : "—"}
          />
          <MiniStat
            label="识别精度"
            value={running && health ? (LEVEL_TEXT[health.model] ?? health.model) : "—"}
          />
        </div>
      </section>

      {error && (
        <div className="mt-3 rounded-lg bg-amber-50 px-3.5 py-2.5 text-[13px] font-medium text-amber-900">
          {error}
        </div>
      )}

      {/* 关窗口 ≠ 停服务。这是最容易误解的一点，直接写在状态卡下面。
          托盘图标在任务栏右下角（可能被折叠进「隐藏的图标」里）。 */}
      <p className="mt-3 flex items-start gap-2 text-[12px] font-medium leading-relaxed text-neutral-500">
        <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-neutral-400" strokeWidth={2} />
        <span>
          关掉窗口不会停服务：它会缩到任务栏右下角的托盘图标里继续跑。
          要真正退出，右键托盘图标选「退出 OCR Studio」。
        </span>
      </p>

      {/* ------------------------------------------------------ 能力概览 */}
      <div className="mt-8 flex items-end justify-between gap-3">
        <div>
          <h2 className="text-[16px] font-bold leading-none tracking-tight text-neutral-900">
            能调的接口
          </h2>
          <p className="mt-1.5 text-[12px] font-medium text-neutral-500">
            共 {ENDPOINT_PATH_COUNT} 个。点任意一类看完整参数和示例。
          </p>
        </div>
        <button
          type="button"
          onClick={onOpenApi}
          className="group flex shrink-0 items-center gap-1 text-[13px] font-bold text-[#2563EB] hover:underline"
        >
          全部接口
          <ArrowRight className="h-3.5 w-3.5 transition-transform group-hover:translate-x-0.5" />
        </button>
      </div>

      <div className="mt-3 grid gap-3 md:grid-cols-3">
        {GROUP_ORDER.map((key) => {
          const list = ENDPOINTS.filter((e) => groupOf(e.path) === key);
          if (list.length === 0) return null;
          const style = GROUP_STYLE[key];
          return (
            <button
              key={key}
              type="button"
              onClick={onOpenApi}
              className="card-lift group flex flex-col rounded-xl border border-neutral-200 bg-white p-4 text-left shadow-card"
            >
              <div className="flex items-center gap-2.5">
                <span
                  className={cn(
                    "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg",
                    style.tint,
                  )}
                >
                  {style.icon}
                </span>
                <span className="text-[14px] font-bold text-neutral-900">{GROUP_LABEL[key]}</span>
                <span className="ml-auto rounded-full bg-neutral-100 px-2 py-0.5 text-[11px] font-bold text-neutral-500">
                  {list.length} 个
                </span>
              </div>
              <p className="mt-2.5 text-[12px] font-medium leading-5 text-neutral-500">
                {GROUP_DESC[key]}
              </p>
              <ul className="mt-3 flex flex-col gap-1.5 border-t border-neutral-100 pt-2.5">
                {list.slice(0, 2).map((e) => (
                  <li key={e.path} className="flex items-baseline gap-2 text-[12px]">
                    <code className="shrink-0 font-mono font-medium text-neutral-400">
                      {e.path.replace("/ocr/", "").replace("/vision/", "")}
                    </code>
                    <span className="truncate font-medium text-neutral-600">{e.title}</span>
                  </li>
                ))}
              </ul>
              {list.length > 2 && (
                <p className="mt-2 text-[11px] font-semibold text-neutral-400">
                  还有 {list.length - 2} 个 →
                </p>
              )}
            </button>
          );
        })}
      </div>

      {/* ------------------------------------------------------ 接入指引 */}
      <section className="mt-8 rounded-xl border border-neutral-200 bg-neutral-50/70 p-5">
        <div className="flex items-center gap-2">
          <Terminal className="h-4 w-4 text-neutral-400" />
          <h2 className="text-[16px] font-bold tracking-tight text-neutral-900">接进你的脚本</h2>
        </div>

        <div className="mt-4 grid gap-5 lg:grid-cols-[268px_1fr]">
          {/* 三步 */}
          <ol className="flex flex-col gap-3">
            {[
              { t: "启动服务", d: "上面的按钮点一下，状态变成运行中" },
              { t: "复制地址", d: `http://${endpoint}` },
              { t: "把图转成 Base64", d: "POST 过去，取 data.text 就是文字" },
            ].map((s, i) => (
              <li key={s.t} className="flex gap-3">
                <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-white text-[12px] font-bold text-neutral-500 shadow-card ring-1 ring-neutral-200/70">
                  {i + 1}
                </span>
                <div className="min-w-0">
                  <p className="text-[13px] font-bold leading-6 text-neutral-800">{s.t}</p>
                  <p
                    className={cn(
                      "truncate text-[12px] font-medium text-neutral-500",
                      i === 1 && "font-mono text-[11px] text-neutral-600",
                    )}
                    title={s.d}
                  >
                    {s.d}
                  </p>
                </div>
              </li>
            ))}
          </ol>

          {/* 代码 + 语言切换 */}
          <div className="min-w-0">
            <div className="mb-2 flex items-center gap-1">
              {(
                [
                  { v: "python" as const, label: "Python" },
                  { v: "curl" as const, label: "curl" },
                  { v: "eyuyan" as const, label: "易语言 / 按键精灵" },
                ]
              ).map((it) => (
                <button
                  key={it.v}
                  type="button"
                  onClick={() => setLang(it.v)}
                  className={cn(
                    "h-7 rounded-md px-2.5 text-[12px] leading-none transition-colors",
                    lang === it.v
                      ? "bg-white font-bold text-neutral-900 shadow-card ring-1 ring-neutral-200/70"
                      : "font-medium text-neutral-500 hover:text-neutral-800",
                  )}
                >
                  {it.label}
                </button>
              ))}
            </div>
            <CodeBlock
              code={samples[lang]}
              lang={lang === "eyuyan" ? "易语言 / 按键精灵" : lang === "curl" ? "sh" : "python"}
            />
            <p className="mt-2 text-[12px] font-medium leading-5 text-neutral-400">
              返回是统一的 <code className="font-mono">{"{code, msg, data}"}</code>：
              <code className="font-mono">code</code> 为 0 才是成功，结果都在
              <code className="font-mono">data</code> 里。
            </p>
          </div>
        </div>
      </section>

      {/* 底部：引擎状态的收口，技术细节都指向设置 */}
      <div className="mt-auto flex flex-wrap items-center justify-between gap-3 border-t border-neutral-100 pt-4">
        <div className="flex items-center gap-2">
          <span
            className={cn(
              "h-1.5 w-1.5 rounded-full",
              health?.status === "ok" ? "bg-emerald-500" : "bg-amber-400",
            )}
          />
          <span className="text-[12px] font-medium text-neutral-500">
            {health?.status === "ok"
              ? `识别引擎已就绪 · ${health.actual_device === "directml" ? "显卡" : "处理器"} · 字典 ${health.dict_size.toLocaleString()} 字`
              : "识别引擎还没就绪，接口会返回 503"}
          </span>
        </div>
        <button
          type="button"
          onClick={onOpenSettings}
          className="flex items-center gap-1 text-[12px] font-bold text-neutral-500 hover:text-neutral-900"
        >
          引擎设置
          <ArrowRight className="h-3 w-3" />
        </button>
      </div>
    </div>
  );
}

/** 主卡里的一个小指标。label 在上（小、淡），value 在下（大、亮）。 */
function MiniStat({ label, value, live }: { label: string; value: string; live?: boolean }) {
  return (
    <div className="px-4 first:pl-0 last:pr-0">
      <p className="text-[11px] font-semibold tracking-wide text-white/35">{label}</p>
      <p
        key={value}
        className={cn(
          "tnum mt-1 text-[16px] font-bold leading-none",
          live ? "animate-num text-white" : "text-white/70",
        )}
      >
        {value}
      </p>
    </div>
  );
}
