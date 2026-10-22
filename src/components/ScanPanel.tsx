import { useCallback, useEffect, useRef, useState } from "react";
import {
  Clipboard,
  Copy,
  Check,
  FileImage,
  ImagePlus,
  Keyboard,
  QrCode,
  RefreshCw,
  ScanLine,
} from "lucide-react";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { Label } from "./ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { Switch } from "./ui/switch";
import { ScrollArea } from "./ui/scroll-area";
import { EmptyState } from "./ui/empty-state";
import { Callout } from "./ui/callout";
import { Hint } from "./ui/hint";
import { ImageViewer } from "./ImageViewer";
import { fileToBase64, imageSize, scanBarcode } from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import { cn } from "../lib/utils";
import type { BarcodeResult, LineResult } from "../types";

/**
 * 码制预设。
 *
 * 用固定预设而不是让用户手打码制名：多数人只想说「这是二维码还是条形码」，
 * 让他去查 `CODE_128` 怎么拼是纯粹的负担。真需要细控的走接口文档。
 */
const PRESETS: { value: string; label: string; formats: string[] }[] = [
  { value: "all", label: "全部码制", formats: [] },
  { value: "qr", label: "只找二维码", formats: ["qr"] },
  { value: "1d", label: "只找条形码", formats: ["1d"] },
  { value: "qr+1d", label: "二维码 + 条形码", formats: ["qr", "1d"] },
  { value: "datamatrix", label: "DataMatrix", formats: ["datamatrix"] },
  { value: "pdf417", label: "PDF417", formats: ["pdf417"] },
];

/** 码制名 → 中文显示名（列表里给人看）。 */
function formatLabel(f: string): string {
  const map: Record<string, string> = {
    QR_CODE: "二维码",
    MICRO_QR_CODE: "微型二维码",
    DATA_MATRIX: "DataMatrix",
    AZTEC: "Aztec",
    PDF_417: "PDF417",
    MAXICODE: "MaxiCode",
    CODE_39: "Code 39",
    CODE_93: "Code 93",
    CODE_128: "Code 128",
    EAN_8: "EAN-8",
    EAN_13: "EAN-13",
    UPC_A: "UPC-A",
    UPC_E: "UPC-E",
    ITF: "ITF",
    CODABAR: "Codabar",
  };
  return map[f] ?? f;
}

/** 扫码面板：丢一张图进去，把图里所有码列出来。 */
export function ScanPanel() {
  const [src, setSrc] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [result, setResult] = useState<BarcodeResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showBoxes, setShowBoxes] = useState(true);
  const [preset, setPreset] = useState("all");
  const [tryHarder, setTryHarder] = useState(true);
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);

  const b64Ref = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  /** 竞态保护：连续换图时只认最后一次的返回 */
  const runSeq = useRef(0);

  const runScan = useCallback(
    async (override?: string) => {
      const b64 = override ?? b64Ref.current;
      if (!b64) {
        setError("请先载入图片");
        return;
      }
      const seq = ++runSeq.current;
      setBusy(true);
      setError(null);
      try {
        const formats = PRESETS.find((p) => p.value === preset)?.formats ?? [];
        const r = await scanBarcode(b64, formats, tryHarder);
        if (seq !== runSeq.current) return;
        setResult(r);
      } catch (e) {
        if (seq === runSeq.current) {
          setError(`扫码失败：${e}`);
          setResult(null);
        }
      } finally {
        if (seq === runSeq.current) setBusy(false);
      }
    },
    [preset, tryHarder],
  );

  const applyImage = useCallback(
    async (b64: string, url: string, dim: { width: number; height: number }, label: string) => {
      b64Ref.current = b64;
      setSrc(url);
      setSize(dim);
      setName(label);
      setResult(null);
      setError(null);
      void runScan(b64);
    },
    [runScan],
  );

  const loadFile = useCallback(
    async (file: File) => {
      try {
        const b64 = await fileToBase64(file);
        const url = `data:${file.type || "image/png"};base64,${b64}`;
        const dim = await imageSize(url);
        await applyImage(b64, url, dim, file.name);
      } catch (e) {
        setError(`加载图片失败：${e}`);
      }
    },
    [applyImage],
  );

  const onDrop = useCallback(
    (imgs: DroppedImage[]) => {
      const first = imgs[0];
      if (!first) return;
      void applyImage(
        first.b64,
        first.url,
        { width: first.width, height: first.height },
        first.name,
      );
    },
    [applyImage],
  );

  const { dragging } = useImageDrop(onDrop);

  // Ctrl+V 粘贴
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
        i.type.startsWith("image/"),
      );
      const file = item?.getAsFile();
      if (file) void loadFile(file);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [loadFile]);

  const copyText = async (text: string, i: number) => {
    await navigator.clipboard.writeText(text);
    setCopiedIdx(i);
    window.setTimeout(() => setCopiedIdx(null), 1400);
  };

  const codes = result?.codes ?? [];
  /**
   * 图上画框。
   *
   * 用**轴对齐包围盒**而不是 rxing 给的定位点：那些点是 QR 的 3 个定位图案
   * 加 1 个校正点，连起来是个歪的四边形，直接画出来会像画错了。
   */
  const lines: LineResult[] = codes.map((c) => ({
    text: formatLabel(c.format),
    box: [
      [c.x, c.y],
      [c.x + c.width, c.y],
      [c.x + c.width, c.y + c.height],
      [c.x, c.y + c.height],
    ],
    score: 1,
    det_score: 1,
    rotated: false,
  }));

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-3">
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-xl bg-blue-50/85 backdrop-blur-[1px]">
          <div className="flex flex-col items-center gap-2 text-blue-700">
            <ImagePlus className="h-8 w-8" />
            <span className="text-[14px] font-bold">松手就开始扫码</span>
          </div>
        </div>
      )}

      {/* 工具栏 */}
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={() => inputRef.current?.click()}>
          <FileImage className="h-3.5 w-3.5" />
          {src ? "换一张图" : "打开图片"}
        </Button>
        <input
          ref={inputRef}
          type="file"
          accept="image/*"
          className="hidden"
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void loadFile(f);
            e.target.value = "";
          }}
        />

        <div className="flex items-center gap-1.5">
          <Label className="text-[12px] text-neutral-500">找什么码</Label>
          <Select
            value={preset}
            onValueChange={(v) => {
              setPreset(v);
              setResult(null);
            }}
          >
            <SelectTrigger className="h-8 w-[150px] text-[12px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PRESETS.map((p) => (
                <SelectItem key={p.value} value={p.value}>
                  {p.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Hint>
            限定码制能更快、也更少误报。拿不准就用「全部码制」——
            一张图里同时有二维码和条形码时它会都找出来。
          </Hint>
        </div>

        <div className="flex items-center gap-1.5">
          <Switch id="try-harder" checked={tryHarder} onCheckedChange={setTryHarder} />
          <Label
            htmlFor="try-harder"
            className="flex cursor-pointer items-center gap-1 text-[12px] font-normal text-neutral-600"
          >
            仔细找
            <Hint>
              打开后会多花点时间尝试（默认开）。条形码在缩放过、有点模糊的图上
              容易漏，关掉它漏得更多。
            </Hint>
          </Label>
        </div>

        <Button
          variant="outline"
          size="sm"
          onClick={() => void runScan()}
          disabled={!src || busy}
        >
          <RefreshCw className={cn("h-3.5 w-3.5", busy && "animate-spin")} />
          {busy ? "扫描中…" : "重新扫一次"}
        </Button>

        <div className="ml-auto flex items-center gap-2">
          {result && (
            <Badge variant={result.count > 0 ? "default" : "secondary"}>
              {result.count > 0 ? `找到 ${result.count} 个码` : "没找到码"} ·{" "}
              {result.elapsed_ms} ms
            </Badge>
          )}
          <span className="hidden items-center gap-1 text-[12px] text-neutral-400 sm:flex">
            <Keyboard className="h-3 w-3" />
            Ctrl+V
          </span>
        </div>
      </div>

      {error && (
        <Callout variant="error" icon={<QrCode className="h-4 w-4" />}>
          {error}
        </Callout>
      )}

      {/* 主体：左图右结果 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[1.55fr_1fr]">
        <Card className="flex min-h-[360px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex h-8 items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[13px] font-medium leading-none text-neutral-500">
                <FileImage className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                {src ? "原图" : "待扫描的图片"}
                {size.width > 0 && (
                  <span className="font-normal text-neutral-400">
                    {name && <span className="mr-1.5 text-neutral-500">{name}</span>}
                    {size.width} × {size.height}
                  </span>
                )}
              </CardTitle>
              {src && (
                <div className="flex items-center gap-1.5">
                  <Switch id="scan-boxes" checked={showBoxes} onCheckedChange={setShowBoxes} />
                  <Label
                    htmlFor="scan-boxes"
                    className="cursor-pointer text-[12px] font-normal text-neutral-500"
                  >
                    显示位置
                  </Label>
                </div>
              )}
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {src ? (
              <ImageViewer
                src={src}
                width={size.width}
                height={size.height}
                lines={lines}
                showBoxes={showBoxes}
              />
            ) : (
              <div className="flex h-full min-h-[280px] flex-col items-center justify-center rounded-xl border-2 border-dashed border-neutral-200 bg-neutral-50/50 px-6 py-8 text-center">
                <div className="flex h-14 w-14 items-center justify-center rounded-xl bg-white text-blue-500 shadow-card">
                  <QrCode className="h-6 w-6" strokeWidth={1.8} />
                </div>
                <p className="mt-4 text-[16px] font-bold text-neutral-800">把图片拖到这里</p>
                <p className="mt-1 text-[13px] text-neutral-500">
                  二维码、条形码都行，放进来就开始扫
                </p>
                <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
                  <span className="flex items-center gap-1.5 rounded-full bg-white px-3 py-1.5 text-[12px] text-neutral-600 ring-1 ring-neutral-200/80">
                    <Clipboard className="h-3.5 w-3.5 text-neutral-400" />
                    按 Ctrl+V 粘贴截图
                  </span>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  className="mt-4"
                  onClick={() => inputRef.current?.click()}
                >
                  <FileImage className="h-3.5 w-3.5" />
                  或者从文件夹里选
                </Button>
              </div>
            )}
          </CardContent>
        </Card>

        <Card className="flex min-h-[360px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex h-8 items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[13px] font-medium leading-none text-neutral-500">
                <ScanLine className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                扫出来的内容
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {codes.length > 0 ? (
              <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                <ul className="divide-y divide-neutral-200/60">
                  {codes.map((c, i) => (
                    <li key={i} className="group flex items-start gap-2.5 px-3 py-2.5">
                      <span
                        className="mt-0.5 flex shrink-0 items-center justify-center rounded bg-blue-600 text-[10px] font-semibold text-white"
                        style={{ width: 18, height: 18 }}
                      >
                        {i + 1}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="rounded bg-neutral-100 px-1.5 py-0.5 text-[11px] font-medium text-neutral-600">
                            {formatLabel(c.format)}
                          </span>
                          <span className="font-mono text-[11px] text-neutral-400">
                            {c.width}×{c.height} @({c.x}, {c.y})
                          </span>
                        </div>
                        <div className="selectable mt-1 break-all font-mono text-[13px] leading-5 text-neutral-900">
                          {c.text || "（空内容）"}
                        </div>
                      </div>
                      <button
                        type="button"
                        onClick={() => void copyText(c.text, i)}
                        title="复制内容"
                        className="mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded text-neutral-300 hover:bg-white hover:text-neutral-600 group-hover:text-neutral-400"
                      >
                        {copiedIdx === i ? (
                          <Check className="h-3.5 w-3.5 text-emerald-600" />
                        ) : (
                          <Copy className="h-3.5 w-3.5" />
                        )}
                      </button>
                    </li>
                  ))}
                </ul>
              </ScrollArea>
            ) : (
              <EmptyState
                className="bg-neutral-50/40"
                icon={<QrCode className="h-5 w-5" />}
                title={result ? "这张图里没扫到码" : "扫出来的内容会显示在这里"}
                description={
                  result
                    ? "图里可能确实没有码，也可能码太小/太模糊/被裁掉了一部分。先确认码在画面里、没被裁切，再试着打开「仔细找」。"
                    : "二维码、条形码都支持，一个图里多个码会一起列出来。"
                }
              />
            )}
          </CardContent>
        </Card>
      </div>

      {codes.length > 0 && (
        <div className="flex items-center gap-1.5 text-[12px] text-neutral-400">
          图上的蓝框是每个码的位置，编号与右侧列表一一对应。点右侧的复制按钮拿走内容。
        </div>
      )}
    </div>
  );
}
