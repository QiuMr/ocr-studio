import { useState } from "react";
import { Check, ChevronDown, Copy } from "lucide-react";
import { CodeBlock } from "../components/ui/code-block";
import { cn } from "../lib/utils";
import {
  ENDPOINT_PATH_COUNT,
  GROUP_DESC,
  GROUP_LABEL,
  GROUP_ORDER,
  endpointsOf,
  type Endpoint,
  type EndpointGroup,
} from "../lib/endpoints";

/*
 * 接口页 = 这个产品的说明书。
 *
 * 排版照「API 文档」的惯例来：方法徽章 + 等宽路径，参数用表格，
 * 返回字段跟在后面。因为来看这页的人，脑子里想的是「我该怎么调」，
 * 不是「这个软件有什么功能」。
 *
 * 不做锚点导航：14 条一屏能扫完，加目录反而多一层。
 */

// 组顺序走 endpoints.ts 的 GROUP_ORDER（唯一事实源）。这里原来手写了一份
// 四组数组，加「文档」组时漏改，表格接口在接口页上静默消失过。
const ORDER: EndpointGroup[] = GROUP_ORDER;

const ERRORS = [
  { code: "0", text: "成功，结果在 data 里" },
  { code: "400", text: "参数有问题（缺 image、base64 解不开、图超过 3200 万像素）" },
  { code: "404", text: "路径不存在。msg 里会给出最接近的可用路径" },
  { code: "405", text: "路径对了但方法不对（这些接口基本都是 POST）" },
  { code: "429", text: "在途请求打满（默认 32 路）：响应带 Retry-After，退避后重试，别猛发" },
  { code: "501", text: "该能力还没实现" },
  { code: "503", text: "引擎没加载好：模型不全或 onnxruntime.dll 缺失" },
  { code: "1002", text: "这个引擎不支持该能力（比如用 OCR 引擎去目标检测）" },
  { code: "1003", text: "引擎名不认识（拼写错，或模型文件解析失败）" },
];

export function ApiPage() {
  const [host, setHost] = useState("127.0.0.1:8848");

  return (
    <div className="flex flex-1 flex-col">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-[26px] font-bold leading-none tracking-tight text-neutral-900">
            接口
          </h1>
          <p className="mt-2 text-[13px] font-medium leading-5 text-neutral-500">
            共 {ENDPOINT_PATH_COUNT} 个端点，全部走本机。返回统一是{" "}
            <code className="selectable rounded bg-neutral-100 px-1 py-0.5 font-mono text-[12px] text-neutral-700">
              {"{ code, msg, data }"}
            </code>
          </p>
          {/* 口径说明：下面每组的「N 个」数的是**卡片张数**，而 /vision/promptref
              一个端点三种方法拆成了三张卡。不解释的话，细心的用户把各组数字加起来
              会比端点数多 2，反而以为页面写错了。 */}
          <p className="mt-1 text-[12px] font-medium leading-4 text-neutral-400">
            /vision/promptref 是一个端点的三种方法，分开写了三张，所以各组的卡片数加起来比端点数多 2
          </p>
        </div>

        {/* 地址可改：换了端口之后，下面的示例要能跟着变 */}
        <label className="flex items-center gap-2 rounded-md border border-neutral-200 bg-white px-2.5 py-1.5 shadow-card">
          <span className="text-[12px] font-semibold text-neutral-500">地址</span>
          <input
            value={host}
            onChange={(e) => setHost(e.target.value)}
            spellCheck={false}
            className="selectable w-[132px] bg-transparent font-mono text-[12px] font-medium text-neutral-800 outline-none"
          />
        </label>
      </div>

      {/* ------------------------------------------------------------ 约定 */}
      <section className="mt-5 grid gap-3 lg:grid-cols-2">
        <div className="rounded-xl border border-neutral-200 bg-white p-4 shadow-card">
          <h2 className="text-[13px] font-bold text-neutral-900">请求怎么传</h2>
          <ul className="mt-2.5 flex flex-col gap-1.5 text-[12px] font-medium leading-5 text-neutral-600">
            <li>
              <b className="text-neutral-800">JSON</b>：{" "}
              <code className="font-mono text-[12px]">{"{\"image\":\"<base64>\"}"}</code>
              ，加{" "}
              <code className="font-mono text-[12px]">Content-Type: application/json</code>
            </li>
            <li>
              <b className="text-neutral-800">表单上传</b>：{" "}
              <code className="font-mono text-[12px]">multipart/form-data</code>，图片字段叫{" "}
              <code className="font-mono text-[12px]">image</code> 或{" "}
              <code className="font-mono text-[12px]">file</code>
            </li>
            <li>
              请求体上限 <b className="text-neutral-800">32 MB</b>，字段里的图片去掉{" "}
              <code className="font-mono text-[12px]">data:image/png;base64,</code> 前缀也能用
            </li>
          </ul>
        </div>

        <div className="rounded-xl border border-neutral-200 bg-white p-4 shadow-card">
          <h2 className="text-[13px] font-bold text-neutral-900">错误码</h2>
          <ul className="mt-2.5 flex flex-col gap-1">
            {ERRORS.map((e) => (
              <li key={e.code} className="flex items-baseline gap-2 text-[12px] font-medium">
                <code
                  className={cn(
                    "w-9 shrink-0 rounded px-1 text-center font-mono font-bold",
                    e.code === "0"
                      ? "bg-emerald-50 text-emerald-700"
                      : "bg-amber-50 text-amber-700",
                  )}
                >
                  {e.code}
                </code>
                <span className="text-neutral-600">{e.text}</span>
              </li>
            ))}
          </ul>
        </div>
      </section>

      {/* ------------------------------------------------------------ 清单 */}
      {ORDER.map((g) => {
        const list = endpointsOf(g);
        if (list.length === 0) return null;
        return (
          <section key={g} className="mt-8">
            <div className="flex items-baseline gap-2.5">
              <h2 className="text-[16px] font-bold tracking-tight text-neutral-900">
                {GROUP_LABEL[g]}
              </h2>
              <span className="text-[12px] font-semibold text-neutral-400">{list.length} 个</span>
              <span className="text-[12px] font-medium text-neutral-500">{GROUP_DESC[g]}</span>
            </div>

            <div className="mt-3 flex flex-col gap-2.5">
              {list.map((e) => (
                <EndpointCard key={e.path} ep={e} host={host} />
              ))}
            </div>
          </section>
        );
      })}

      <p className="mt-8 text-[12px] font-medium leading-5 text-neutral-400">
        所有端点只在本机监听，端口改了这里跟着改。想先确认服务通不通，先调{" "}
        <code className="font-mono">GET /ocr/health</code>。
      </p>
    </div>
  );
}

function EndpointCard({ ep, host }: { ep: Endpoint; host: string }) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState<"path" | "code" | null>(null);

  const copy = async (what: "path" | "code") => {
    const text =
      what === "path"
        ? `http://${host}${ep.path}`
        : ep.method === "GET"
          ? `curl http://${host}${ep.path}`
          : `curl -X POST http://${host}${ep.path} \\\n  -H "Content-Type: application/json" \\\n  -d '{"image":"<图片base64>"}'`;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(what);
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      /* 忽略 */
    }
  };

  return (
    <div className="group overflow-hidden rounded-xl border border-neutral-200 bg-white shadow-card transition-colors hover:border-neutral-300">
      {/* 头行：整行可点展开，只有两个复制按钮单独拦事件 */}
      <div className="flex items-center gap-2.5 px-4 py-3">
        <span
          className={cn(
            "shrink-0 rounded-md px-1.5 py-0.5 font-mono text-[10px] font-bold leading-4 tracking-wide",
            ep.method === "GET"
              ? "bg-emerald-50 text-emerald-700"
              : "bg-[#EFF4FF] text-[#2563EB]",
          )}
        >
          {ep.method}
        </span>

        <button
          type="button"
          onClick={() => setOpen((o) => !o)}
          className="flex min-w-0 flex-1 items-baseline gap-2.5 text-left"
          aria-expanded={open}
        >
          <code className="selectable shrink-0 font-mono text-[13px] font-bold text-neutral-900">
            {ep.path}
          </code>
          <span className="truncate text-[12px] font-medium text-neutral-500">{ep.title}</span>
        </button>

        <button
          type="button"
          onClick={() => void copy("path")}
          title="复制完整地址"
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-neutral-300 transition-colors hover:bg-neutral-100 hover:text-neutral-700"
        >
          {copied === "path" ? (
            <Check className="animate-pop h-3.5 w-3.5 text-emerald-600" strokeWidth={3} />
          ) : (
            <Copy className="h-3.5 w-3.5" />
          )}
        </button>

        <ChevronDown
          className={cn(
            "h-4 w-4 shrink-0 text-neutral-300 transition-transform duration-200",
            open && "rotate-180",
          )}
        />
      </div>

      {/* 详情 */}
      {open && (
        <div className="animate-fade-rise border-t border-neutral-100 px-4 pb-4 pt-3">
          {ep.note && (
            <p className="text-[12px] font-medium leading-5 text-neutral-600">{ep.note}</p>
          )}

          {ep.params && ep.params.length > 0 && (
            <div className="mt-3">
              <p className="mb-1.5 text-[11px] font-bold uppercase tracking-wider text-neutral-400">
                参数
              </p>
              <div className="overflow-hidden rounded-lg bg-neutral-50">
                {ep.params.map((p, i) => (
                  <div
                    key={p.name}
                    className={cn(
                      "flex items-baseline gap-3 px-3 py-1.5 text-[12px]",
                      i > 0 && "border-t border-neutral-100",
                    )}
                  >
                    <code className="w-[92px] shrink-0 font-mono font-bold text-neutral-800">
                      {p.name}
                      {p.required && <span className="ml-0.5 text-red-500">*</span>}
                    </code>
                    <span className="font-medium text-neutral-600">{p.desc}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {ep.returns && (
            <div className="mt-3">
              <p className="mb-1 text-[11px] font-bold uppercase tracking-wider text-neutral-400">
                返回
              </p>
              <p className="font-mono text-[12px] font-medium leading-5 text-neutral-600">
                {ep.returns}
              </p>
            </div>
          )}

          <div className="mt-3">
            <CodeBlock
              lang={ep.method === "GET" ? "请求示例" : "请求示例 · 换掉 <图片base64>"}
              code={
                ep.method === "GET"
                  ? `curl http://${host}${ep.path}`
                  : `curl -X POST http://${host}${ep.path} \\\n  -H "Content-Type: application/json" \\\n  -d '{"image":"<图片base64>"}'`
              }
            />
          </div>
        </div>
      )}
    </div>
  );
}
