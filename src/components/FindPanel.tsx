import { useCallback, useEffect, useRef, useState } from "react";
import {
  AppWindow,
  Crosshair,
  Download,
  FileImage,
  Monitor,
  RefreshCw,
  ScanSearch,
  Target,
  Upload,
} from "lucide-react";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { Input } from "./ui/input";
import { Label } from "./ui/label";
import { ScrollArea } from "./ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";
import { Separator } from "./ui/separator";
import { Switch } from "./ui/switch";
import { ImageViewer } from "./ImageViewer";
import { fileToBase64, findImage, imageSize, listWindows, screenPreview } from "../lib/api";
import type { FindHit, FindMode, FindResult, FindSource, WindowInfo } from "../types";

/** 搜索图：既可能是用户上传的，也可能是内部截的屏幕/窗口。 */
interface Haystack {
  /** 可直接给 <img src> 的 data URL */
  url: string;
  /** 上传来源时的原始 base64（发给后端当 image 字段） */
  b64?: string;
  width: number;
  height: number;
  /** 该图左上角在屏幕上的坐标。屏幕/窗口截图非零，上传图为 0。 */
  origin: { x: number; y: number };
}

interface Props {
  /** 从「识别」页带过来的图片，切到找图页时可直接当模板候选 */
  inheritedImage?: string | null;
}

const SOURCE_LABEL: Record<FindSource, string> = {
  image: "上传图片",
  screen: "整个屏幕",
  window: "指定窗口",
};

export function FindPanel({ inheritedImage }: Props) {
  const [source, setSource] = useState<FindSource>("image");
  const [mode, setMode] = useState<FindMode>("exact");
  const [hay, setHay] = useState<Haystack | null>(null);
  const [tplUrl, setTplUrl] = useState<string | null>(null);
  const [tplB64, setTplB64] = useState<string | null>(null);
  const [tplSize, setTplSize] = useState({ width: 0, height: 0 });
  const [result, setResult] = useState<(FindResult & { preview?: string }) | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showHits, setShowHits] = useState(true);
  const [maxResults, setMaxResults] = useState(1);
  const [minScore, setMinScore] = useState(""); // 留空 = 用端点预设
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [hwnd, setHwnd] = useState<number | null>(null);

  const hayInputRef = useRef<HTMLInputElement>(null);
  const tplInputRef = useRef<HTMLInputElement>(null);

  // 刷新窗口列表
  const refreshWindows = useCallback(async () => {
    try {
      const list = await listWindows();
      setWindows(list);
      setHwnd((cur) => cur ?? list[0]?.hwnd ?? null);
    } catch (e) {
      setError(`枚举窗口失败: ${e}`);
    }
  }, []);

  useEffect(() => {
    if (source === "window" && windows.length === 0) void refreshWindows();
  }, [source, windows.length, refreshWindows]);

  // 从识别页继承的图：直接当搜索图（source=image 时才有意义）
  useEffect(() => {
    if (!inheritedImage || hay) return;
    let cancelled = false;
    const url = `data:image/png;base64,${inheritedImage}`;
    void imageSize(url).then((d) => {
      if (cancelled) return;
      setHay({ url, b64: inheritedImage, width: d.width, height: d.height, origin: { x: 0, y: 0 } });
    });
    return () => {
      cancelled = true;
    };
  }, [inheritedImage, hay]);

  /** 载入上传的搜索图。 */
  const loadHayFile = async (file: File) => {
    try {
      const b64 = await fileToBase64(file);
      const url = URL.createObjectURL(file);
      const dim = await imageSize(url);
      setHay({ url, b64, width: dim.width, height: dim.height, origin: { x: 0, y: 0 } });
      setResult(null);
      setError(null);
    } catch (e) {
      setError(`加载搜索图失败: ${e}`);
    }
  };

  /** 截一张屏幕/窗口预览当搜索图。 */
  const shootScreen = async (target?: WindowInfo | null) => {
    setBusy(true);
    setError(null);
    try {
      const region:
        | [number, number, number, number]
        | undefined = target
        ? [target.x, target.y, target.width, target.height]
        : undefined;
      const p = await screenPreview(region);
      setHay({
        url: `data:image/png;base64,${p.image}`,
        width: p.width,
        height: p.height,
        origin: { x: p.x, y: p.y },
      });
      setResult(null);
    } catch (e) {
      setError(`截图失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  /** 载入模板图。 */
  const loadTplFile = async (file: File) => {
    try {
      const b64 = await fileToBase64(file);
      const url = URL.createObjectURL(file);
      const dim = await imageSize(url);
      setTplUrl(url);
      setTplB64(b64);
      setTplSize(dim);
      setResult(null);
      setError(null);
    } catch (e) {
      setError(`加载模板失败: ${e}`);
    }
  };

  /** Ctrl+V 粘贴：没有模板时优先当模板（模板通常很小）。 */
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const item = Array.from(e.clipboardData?.items ?? []).find((i) =>
        i.type.startsWith("image/"),
      );
      const file = item?.getAsFile();
      if (!file) return;
      if (!tplB64) void loadTplFile(file);
      else void loadHayFile(file);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  });

  const onFind = async () => {
    if (!tplB64) {
      setError("请先选择模板图");
      return;
    }
    if (source === "image" && !hay?.b64) {
      setError("请先载入搜索图（上传图片或粘贴）");
      return;
    }
    if (source === "window" && hwnd == null) {
      setError("请先选择一个窗口");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const parsed = Number.parseFloat(minScore);
      const r = await findImage({
        source,
        mode,
        image: source === "image" ? (hay?.b64 ?? undefined) : undefined,
        template: tplB64,
        hwnd: source === "window" ? (hwnd ?? undefined) : undefined,
        max_results: maxResults,
        min_score: Number.isFinite(parsed) ? parsed : undefined,
      });
      setResult(r);
      // 后端回传了它实际看到的画面：用它当背景，框与图必然对齐。
      // origin 由后端直接给出（屏幕/窗口截图的左上角），不靠前端反推。
      if (r.preview) {
        const url = `data:image/png;base64,${r.preview}`;
        const dim = await imageSize(url);
        setHay({
          url,
          width: dim.width,
          height: dim.height,
          origin: { x: r.origin_x, y: r.origin_y },
        });
      }
    } catch (e) {
      setError(`找图失败: ${e}`);
      setResult(null);
    } finally {
      setBusy(false);
    }
  };

  const hits: FindHit[] = result?.matches ?? [];

  return (
    <div className="flex h-full flex-col gap-4">
      {/* 工具栏 */}
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1.5">
          <Label className="text-[11px] text-neutral-500">搜索范围</Label>
          <Select value={source} onValueChange={(v) => setSource(v as FindSource)}>
            <SelectTrigger className="w-[150px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="image">
                <FileImage className="mr-1.5 inline h-3.5 w-3.5" />
                上传图片
              </SelectItem>
              <SelectItem value="screen">
                <Monitor className="mr-1.5 inline h-3.5 w-3.5" />
                整个屏幕
              </SelectItem>
              <SelectItem value="window">
                <AppWindow className="mr-1.5 inline h-3.5 w-3.5" />
                指定窗口
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label className="text-[11px] text-neutral-500">匹配精度</Label>
          <Select value={mode} onValueChange={(v) => setMode(v as FindMode)}>
            <SelectTrigger className="w-[130px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="exact">精准（≥0.92）</SelectItem>
              <SelectItem value="loose">容错（≥0.72）</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label className="text-[11px] text-neutral-500">阈值 / 数量</Label>
          <div className="flex items-center gap-2">
            <Input
              className="w-[76px]"
              placeholder="预设"
              value={minScore}
              onChange={(e) => setMinScore(e.target.value)}
            />
            <Input
              className="w-[62px]"
              type="number"
              min={1}
              max={64}
              value={maxResults}
              onChange={(e) => setMaxResults(Math.max(1, Number(e.target.value) || 1))}
            />
          </div>
        </div>

        <Separator orientation="vertical" className="mx-1 h-9" />

        <Button variant="outline" onClick={() => tplInputRef.current?.click()}>
          <Upload className="h-4 w-4" />
          {tplB64 ? "换模板" : "选模板"}
        </Button>
        <input
          ref={tplInputRef}
          type="file"
          accept="image/*"
          className="hidden"
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void loadTplFile(f);
            e.target.value = "";
          }}
        />

        {source === "image" && (
          <>
            <Button variant="outline" onClick={() => hayInputRef.current?.click()}>
              <FileImage className="h-4 w-4" />
              选搜索图
            </Button>
            <input
              ref={hayInputRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) void loadHayFile(f);
                e.target.value = "";
              }}
            />
          </>
        )}

        {source === "screen" && (
          <Button variant="outline" onClick={() => void shootScreen(null)} disabled={busy}>
            <Monitor className="h-4 w-4" />
            截取屏幕
          </Button>
        )}

        {source === "window" && (
          <>
            <Select
              value={hwnd == null ? "" : String(hwnd)}
              onValueChange={(v) => setHwnd(Number(v))}
            >
              <SelectTrigger className="w-[240px]">
                <SelectValue placeholder="选择窗口" />
              </SelectTrigger>
              <SelectContent>
                {windows.map((w) => (
                  <SelectItem key={w.hwnd} value={String(w.hwnd)}>
                    {w.title.length > 26 ? `${w.title.slice(0, 26)}…` : w.title}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              variant="outline"
              size="icon"
              onClick={() => void refreshWindows()}
              title="刷新窗口列表"
            >
              <RefreshCw className="h-4 w-4" />
            </Button>
            <Button
              variant="outline"
              onClick={() => void shootScreen(windows.find((w) => w.hwnd === hwnd) ?? null)}
              disabled={busy || hwnd == null}
            >
              <AppWindow className="h-4 w-4" />
              截取窗口
            </Button>
          </>
        )}

        <Button onClick={() => void onFind()} disabled={busy || !tplB64}>
          <ScanSearch className="h-4 w-4" />
          {busy ? "查找中…" : "开始找图"}
        </Button>

        <div className="ml-auto flex items-center gap-2">
          <div className="flex items-center gap-2">
            <Switch id="show-hits" checked={showHits} onCheckedChange={setShowHits} />
            <Label htmlFor="show-hits" className="cursor-pointer">
              <Target className="mr-1 inline h-3.5 w-3.5" />
              显示命中框
            </Label>
          </div>
          {result && (
            <Badge variant={result.found ? "default" : "secondary"}>
              {result.found ? `命中 ${result.matched}` : "未找到"} · {result.elapsed_ms} ms
            </Badge>
          )}
        </div>
      </div>

      {error && (
        <div className="rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {/* 主体 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 lg:grid-cols-[3fr_2fr]">
        <Card className="flex min-h-[420px] flex-col overflow-hidden">
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-sm">
              <Crosshair className="h-4 w-4 text-blue-600" />
              {SOURCE_LABEL[source]}
              {hay && (
                <span className="font-normal text-neutral-400">
                  {hay.width} × {hay.height}
                  {hay.origin.x !== 0 || hay.origin.y !== 0
                    ? ` @(${hay.origin.x}, ${hay.origin.y})`
                    : ""}
                </span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            <ImageViewer
              src={hay?.url ?? null}
              width={hay?.width ?? 0}
              height={hay?.height ?? 0}
              lines={[]}
              showBoxes={showHits}
              hits={hits}
              hitsOffset={hay?.origin}
            />
          </CardContent>
        </Card>

        <Card className="flex min-h-[420px] flex-col overflow-hidden">
          <CardHeader className="pb-3">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-sm">
                <Target className="h-4 w-4 text-blue-600" />
                命中结果
              </CardTitle>
              <Button
                variant="ghost"
                size="sm"
                disabled={!result}
                onClick={() =>
                  void navigator.clipboard.writeText(JSON.stringify(result, null, 2))
                }
              >
                <Download className="h-3.5 w-3.5" />
                复制 JSON
              </Button>
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            <ScrollArea className="h-full rounded-md border border-neutral-200">
              {result && result.found ? (
                <ul className="divide-y divide-neutral-100">
                  {result.matches.map((h, i) => (
                    <li key={i} className="px-3 py-2.5">
                      <div className="flex items-center gap-2">
                        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded bg-amber-600 text-[11px] font-semibold text-white">
                          {i + 1}
                        </span>
                        <span className="font-mono text-sm text-neutral-900">
                          ({h.x}, {h.y})
                        </span>
                        <span className="ml-auto font-mono text-[11px] text-neutral-500">
                          {h.score.toFixed(4)}
                        </span>
                      </div>
                      <div className="mt-1 pl-7 font-mono text-[11px] text-neutral-400">
                        center ({h.center_x.toFixed(1)}, {h.center_y.toFixed(1)}) ·{" "}
                        {h.width}×{h.height}
                      </div>
                    </li>
                  ))}
                </ul>
              ) : (
                <div className="px-3 py-10 text-center text-sm text-neutral-400">
                  {result ? "未找到匹配" : "（尚未查找）"}
                </div>
              )}
            </ScrollArea>
          </CardContent>
        </Card>
      </div>

      {/* 模板预览 */}
      {tplUrl && (
        <Card>
          <CardContent className="flex items-center gap-4 py-3">
            <span className="text-xs text-neutral-500">模板</span>
            <img
              src={tplUrl}
              alt="模板"
              className="max-h-16 rounded border border-neutral-200 bg-neutral-50 object-contain p-1"
            />
            <span className="font-mono text-[11px] text-neutral-400">
              {tplSize.width} × {tplSize.height}
            </span>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
