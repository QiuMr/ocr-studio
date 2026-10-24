import { useCallback, useEffect, useRef, useState } from "react";
import {
  AlertTriangle,
  Check,
  Copy,
  FileImage,
  ImagePlus,
  Loader2,
  RefreshCw,
  ScanText,
  Table2,
  Type,
} from "lucide-react";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Callout } from "./ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { EmptyState } from "./ui/empty-state";
import { fileToBase64, imageSize, recognizeTable } from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import { cn } from "../lib/utils";
import type { TableItem, TableResult } from "../types";

/** 三种分支。用卡片而不是下拉：说清「什么时候用哪个」比省空间重要。 */
const MODES = [
  {
    v: "auto" as const,
    label: "自动",
    desc: "整页文档，自己找表格",
    icon: <ScanText className="h-3.5 w-3.5" />,
  },
  {
    v: "structure" as const,
    label: "整图当表格",
    desc: "图就是一张裁好的表格",
    icon: <Table2 className="h-3.5 w-3.5" />,
  },
  {
    v: "detect" as const,
    label: "只找位置",
    desc: "只要框，不认结构",
    icon: <Type className="h-3.5 w-3.5" />,
  },
];

/**
 * 表格识别面板（`/ocr/biaoge`）。
 *
 * 三个模式对应三种真实用法，所以做成并列的按钮而不是藏在下拉里——
 * 「我有张文档照片」和「我已经裁好表格了」的人要找的东西不一样。
 */
export function TablePanel() {
  const [src, setSrc] = useState<string | null>(null);
  const [b64, setB64] = useState<string>("");
  const [fileName, setFileName] = useState("");
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [mode, setMode] = useState<"auto" | "structure" | "detect">("auto");
  const [withText, setWithText] = useState(true);
  const [result, setResult] = useState<TableResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showBoxes, setShowBoxes] = useState(true);
  const [copied, setCopied] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const run = useCallback(
    async (b: string, m: typeof mode, wt: boolean) => {
      if (!b) return;
      setBusy(true);
      setError(null);
      try {
        setResult(await recognizeTable(b, { mode: m, withText: wt }));
      } catch (e) {
        setResult(null);
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  const load = useCallback(
    async (file: File) => {
      try {
        const b = await fileToBase64(file);
        const url = `data:${file.type || "image/png"};base64,${b}`;
        const dim = await imageSize(url);
        setSrc(url);
        setB64(b);
        setFileName(file.name);
        setSize(dim);
        await run(b, mode, withText);
      } catch (e) {
        setError(`加载图片失败：${e}`);
      }
    },
    [mode, withText, run],
  );

  const onDrop = useCallback(
    (imgs: DroppedImage[]) => {
      // 一次只处理一张：表格识别没有「批量」的语义
      const first = imgs[0];
      if (!first) return;
      setSrc(first.url);
      setB64(first.b64);
      setFileName(first.name);
      setSize({ width: first.width, height: first.height });
      void run(first.b64, mode, withText);
    },
    [mode, withText, run],
  );
  const { dragging } = useImageDrop(onDrop);

  // Ctrl+V 粘贴
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
        i.type.startsWith("image/"),
      );
      if (!item) return;
      const f = item.getAsFile();
      if (f) void load(f);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [load]);

  const switchMode = (m: typeof mode) => {
    setMode(m);
    if (b64) void run(b64, m, withText);
  };

  const copy = async (text: string, tag: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(tag);
    window.setTimeout(() => setCopied(null), 1400);
  };

  const total = result?.count ?? 0;

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-4">
      {/* ------------------------------------------------------------- 左：图 */}
      <Card className="flex min-h-0 flex-col">
        <CardHeader className="flex h-8 flex-row items-center justify-between space-y-0 pb-0">
          <CardTitle className="flex items-center gap-1.5">
            <FileImage className="h-3.5 w-3.5 text-neutral-400" />
            图片
          </CardTitle>
          <div className="flex items-center gap-1">
            {src && (
              <Button
                variant="ghost"
                size="icon-sm"
                title="换一张"
                onClick={() => fileRef.current?.click()}
              >
                <RefreshCw className="h-3.5 w-3.5" />
              </Button>
            )}
            <Button size="sm" onClick={() => fileRef.current?.click()}>
              <ImagePlus className="h-3.5 w-3.5" />
              选图
            </Button>
          </div>
        </CardHeader>

        <CardContent className="flex min-h-0 flex-1 flex-col pt-3">
          <input
            ref={fileRef}
            type="file"
            accept="image/*"
            className="hidden"
            onChange={(e) => {
              const f = e.target.files?.[0];
              if (f) void load(f);
              e.target.value = "";
            }}
          />

          {!src ? (
            <div
              className={cn(
                "flex min-h-0 flex-1 items-center justify-center rounded-lg border-2 border-dashed",
                dragging ? "border-[#2563EB] bg-[#EFF4FF]" : "border-neutral-200",
              )}
            >
              <EmptyState
                icon={<Table2 className="h-5 w-5" />}
                title="把表格图片拖进来"
                description="整页文档、截图、手机拍的表格都行。也可以 Ctrl+V 粘贴。"
                steps={[
                  { icon: <ImagePlus className="h-3.5 w-3.5" />, text: "选一张图" },
                  {
                    icon: <ScanText className="h-3.5 w-3.5" />,
                    text: "自动找表格并识别",
                    detail: "图里没有表格时会退回「整图当一张表格」",
                  },
                  {
                    icon: <Copy className="h-3.5 w-3.5" />,
                    text: "复制成 Excel 能用的文本",
                  },
                ]}
              />
            </div>
          ) : (
            <div className="flex min-h-0 flex-1 flex-col gap-2">
              {/*
                叠加框必须贴着**图片实际显示的矩形**，不能贴容器：
                object-contain 会在容器里留白，按容器百分比算就整体偏移
                （第一版就是这么错的，框跑到图片外面去了）。
                所以用一层只包住 img 的 relative wrapper，让它的尺寸由图片决定。
              */}
              <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-lg bg-neutral-50">
                <div className="relative flex max-h-full max-w-full">
                  <img
                    src={src}
                    alt={fileName}
                    className="block max-h-full max-w-full object-contain"
                    draggable={false}
                  />
                  {showBoxes && result && size.width > 0 && (
                    <div className="pointer-events-none absolute inset-0">
                      {result.tables.map((t, ti) =>
                        t.cells.length > 0 ? (
                          t.cells.map((c, ci) => (
                            <div
                              key={`${ti}-${ci}`}
                              className="absolute border border-[#2563EB] bg-[#2563EB]/10"
                              style={{
                                left: `${(c.x / size.width) * 100}%`,
                                top: `${(c.y / size.height) * 100}%`,
                                width: `${(c.width / size.width) * 100}%`,
                                height: `${(c.height / size.height) * 100}%`,
                              }}
                            />
                          ))
                        ) : (
                          <div
                            key={ti}
                            className="absolute border-2 border-[#2563EB]"
                            style={{
                              left: `${(t.x / size.width) * 100}%`,
                              top: `${(t.y / size.height) * 100}%`,
                              width: `${(t.width / size.width) * 100}%`,
                              height: `${(t.height / size.height) * 100}%`,
                            }}
                          />
                        ),
                      )}
                    </div>
                  )}
                </div>
              </div>
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-[11px] text-neutral-400">
                  {fileName} · {size.width}×{size.height}
                </span>
                <button
                  type="button"
                  onClick={() => setShowBoxes((v) => !v)}
                  className={cn(
                    "shrink-0 text-[11px] font-medium transition-colors",
                    showBoxes ? "text-[#2563EB]" : "text-neutral-400 hover:text-neutral-600",
                  )}
                >
                  {showBoxes ? "隐藏格子" : "显示格子"}
                </button>
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* ----------------------------------------------------------- 右：结果 */}
      <Card className="flex min-h-0 flex-col">
        <CardHeader className="flex h-8 flex-row items-center justify-between space-y-0 pb-0">
          <CardTitle className="flex items-center gap-1.5">
            <Table2 className="h-3.5 w-3.5 text-neutral-400" />
            识别结果
          </CardTitle>
          <div className="flex items-center gap-1.5">
            {busy && <Loader2 className="h-3.5 w-3.5 animate-spin text-neutral-400" />}
            {total > 0 && (
              <Badge variant="secondary">
                {total} 张表
                {result ? ` · ${result.elapsed_ms}ms` : ""}
              </Badge>
            )}
          </div>
        </CardHeader>

        <CardContent className="flex min-h-0 flex-1 flex-col gap-3 pt-3">
          <div className="flex items-center gap-1 rounded-lg bg-neutral-100 p-1">
            {MODES.map((m) => (
              <button
                key={m.v}
                type="button"
                onClick={() => switchMode(m.v)}
                title={m.desc}
                className={cn(
                  "flex h-7 flex-1 items-center justify-center gap-1.5 rounded-md text-[12px] leading-none transition-colors",
                  mode === m.v
                    ? "shadow-card bg-white font-semibold text-neutral-900"
                    : "font-medium text-neutral-500 hover:text-neutral-800",
                )}
              >
                {m.icon}
                {m.label}
              </button>
            ))}
          </div>

          <label className="flex cursor-pointer items-center gap-2 text-[12px] text-neutral-600">
            <input
              type="checkbox"
              checked={withText}
              onChange={(e) => {
                setWithText(e.target.checked);
                if (b64) void run(b64, mode, e.target.checked);
              }}
              className="h-3.5 w-3.5 accent-[#2563EB]"
            />
            识别格子里的文字（关掉只出结构，快一倍）
          </label>

          {error && (
            <Callout
              variant="error"
              icon={<AlertTriangle className="h-3.5 w-3.5" />}
              title="识别失败"
            >
              {error}
            </Callout>
          )}

          {result?.fallback && (
            <Callout variant="warn" icon={<AlertTriangle className="h-3.5 w-3.5" />}>
              这张图里没检测到独立的表格，已按「整张图就是一个表格」处理。
              如果结果不对，把模式切到「整图当表格」再试一次。
            </Callout>
          )}

          {!result && !error && (
            <div className="flex min-h-0 flex-1 items-center justify-center rounded-lg border border-dashed border-neutral-200">
              <p className="px-6 text-center text-[12px] leading-5 text-neutral-400">
                选一张图就会自动识别。
                <br />
                第一次用需要先在「设置 → 模型库」下载「表格识别」（约 13MB）。
              </p>
            </div>
          )}

          {result && result.tables.length === 0 && !error && (
            <div className="flex min-h-0 flex-1 items-center justify-center rounded-lg border border-dashed border-neutral-200">
              <p className="text-[12px] text-neutral-400">这张图里没有找到表格</p>
            </div>
          )}

          {result && result.tables.length > 0 && (
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">
              {result.tables.map((t, i) => (
                <TableCard
                  key={i}
                  index={i}
                  item={t}
                  copied={copied}
                  onCopy={copy}
                  detectOnly={result.mode === "detect"}
                />
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/** 一张表的卡片：结构摘要 + 渲染出来的表格 + 复制。 */
function TableCard({
  index,
  item,
  copied,
  onCopy,
  detectOnly,
}: {
  index: number;
  item: TableItem;
  copied: string | null;
  onCopy: (text: string, tag: string) => void;
  detectOnly: boolean;
}) {
  return (
    <div className="rounded-lg border border-neutral-200">
      <div className="flex items-center justify-between gap-2 border-b border-neutral-100 px-3 py-2">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="text-[12px] font-semibold text-neutral-800">表 {index + 1}</span>
          <span className="truncate font-mono text-[11px] text-neutral-400">
            {item.x},{item.y} {item.width}×{item.height}
          </span>
        </div>
        {detectOnly ? (
          <Badge variant="secondary">检测 {item.detect_score.toFixed(2)}</Badge>
        ) : (
          <Badge variant="secondary">
            {item.rows}×{item.cols} · {(item.structure_score * 100).toFixed(0)}%
          </Badge>
        )}
      </div>

      {item.truncated && (
        <div className="border-b border-neutral-100 bg-amber-50 px-3 py-1.5 text-[11px] text-amber-800">
          格子太多，这张表没识别完。把表格拆小一点（比如按行分段）再试。
        </div>
      )}

      {detectOnly ? (
        <p className="px-3 py-2 text-[12px] text-neutral-500">
          只做了定位。「整图当表格」或「自动」才会识别结构。
        </p>
      ) : (
        <>
          {/* HTML 已由后端转义（& < >），这里渲染是安全的 */}
          <div
            className="table-render max-h-64 overflow-auto px-3 py-2"
            dangerouslySetInnerHTML={{ __html: item.html }}
          />
          <div className="flex items-center justify-end gap-1 border-t border-neutral-100 px-3 py-1.5">
            <Button
              variant="ghost"
              size="sm"
              onClick={() => onCopy(item.text, `text-${index}`)}
            >
              {copied === `text-${index}` ? (
                <Check className="h-3.5 w-3.5 text-emerald-600" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
              复制文本
            </Button>
            <Button variant="ghost" size="sm" onClick={() => onCopy(item.html, `html-${index}`)}>
              {copied === `html-${index}` ? (
                <Check className="h-3.5 w-3.5 text-emerald-600" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
              复制 HTML
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
