import { useCallback, useEffect, useRef, useState } from "react";
import {
  AppWindow,
  Check,
  ChevronDown,
  Copy,
  Crosshair,
  FileImage,
  Hand,
  Image as ImageIcon,
  Monitor,
  RefreshCw,
  ScanSearch,
  Settings2,
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
import { Switch } from "./ui/switch";
import { EmptyState } from "./ui/empty-state";
import { Callout } from "./ui/callout";
import { Hint } from "./ui/hint";
import { ImageViewer } from "./ImageViewer";
import { fileToBase64, findImage, imageSize, listWindows, screenPreview } from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import { cn } from "../lib/utils";
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

interface Template {
  url: string;
  b64: string;
  width: number;
  height: number;
  name: string;
}

interface Props {
  /** 从「识别」页带过来的图片，切到找图页时可直接当模板候选 */
  inheritedImage?: string | null;
}

const SOURCE_LABEL: Record<FindSource, string> = {
  image: "上传的图片",
  screen: "整个屏幕",
  window: "某个窗口",
};

const SOURCE_HINT: Record<FindSource, string> = {
  image: "在上传的图片里搜。适合离线验证模板准不准。",
  screen: "截取整个屏幕来搜。适合做主屏幕上的自动化点击。",
  window: "只截指定窗口。窗口被移动、遮挡影响更小，推荐用这个。",
};

export function FindPanel({ inheritedImage }: Props) {
  const [source, setSource] = useState<FindSource>("window");
  const [mode, setMode] = useState<FindMode>("exact");
  const [hay, setHay] = useState<Haystack | null>(null);
  const [tpl, setTpl] = useState<Template | null>(null);
  const [result, setResult] = useState<(FindResult & { preview?: string }) | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showHits, setShowHits] = useState(true);
  const [advanced, setAdvanced] = useState(false);
  const [maxResults, setMaxResults] = useState(1);
  const [minScore, setMinScore] = useState(""); // 留空 = 用端点预设
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [hwnd, setHwnd] = useState<number | null>(null);
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);

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

  // 从识别页继承的图：只用来当模板（大概率就是用户刚从截图里裁出来的东西）
  useEffect(() => {
    if (!inheritedImage || tpl) return;
    let cancelled = false;
    const url = `data:image/png;base64,${inheritedImage}`;
    void imageSize(url).then((d) => {
      if (cancelled) return;
      setTpl({ url, b64: inheritedImage, width: d.width, height: d.height, name: "来自识别页" });
    });
    return () => {
      cancelled = true;
    };
  }, [inheritedImage, tpl]);

  const applyTemplate = useCallback(async (b64: string, url: string, name: string) => {
    const dim = await imageSize(url);
    setTpl({ url, b64, width: dim.width, height: dim.height, name });
    setResult(null);
    setError(null);
  }, []);

  const applyHaystack = useCallback(async (b64: string, url: string) => {
    const dim = await imageSize(url);
    setHay({ url, b64, width: dim.width, height: dim.height, origin: { x: 0, y: 0 } });
    setResult(null);
    setError(null);
  }, []);

  /** 拖进来的图：还没有模板就当模板，否则当搜索图。 */
  const onDrop = useCallback(
    (imgs: DroppedImage[]) => {
      const first = imgs[0];
      if (!first) return;
      if (!tpl) void applyTemplate(first.b64, first.url, first.name);
      else void applyHaystack(first.b64, first.url);
    },
    [tpl, applyTemplate, applyHaystack],
  );

  const { dragging } = useImageDrop(onDrop);

  /** 载入上传的图片（浏览器里选文件用）。 */
  const loadFileTo = async (file: File, target: "hay" | "tpl") => {
    try {
      const b64 = await fileToBase64(file);
      const url = `data:${file.type || "image/png"};base64,${b64}`;
      if (target === "hay") await applyHaystack(b64, url);
      else await applyTemplate(b64, url, file.name);
    } catch (e) {
      setError(`加载图片失败: ${e}`);
    }
  };

  /** 截一张屏幕/窗口预览当搜索图。 */
  const shootScreen = async (target?: WindowInfo | null) => {
    setBusy(true);
    setError(null);
    try {
      const region: [number, number, number, number] | undefined = target
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

  const onFind = async () => {
    if (!tpl) {
      setError("还没选模板图。模板 = 你要找的那个小图标/小按钮的截图。");
      return;
    }
    if (source === "image" && !hay) {
      setError("还没载入搜索图。点「选搜索图」，或直接把图片拖进来。");
      return;
    }
    if (source === "window" && hwnd == null) {
      setError("还没选窗口。先在上面选一个要搜的窗口。");
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
        template: tpl.b64,
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

  const copyCoord = async (h: FindHit, i: number) => {
    // 给自动化脚本用：绝对屏幕坐标 = 图内坐标 + 截图原点
    const ox = hay?.origin.x ?? 0;
    const oy = hay?.origin.y ?? 0;
    await navigator.clipboard.writeText(`${h.center_x + ox},${h.center_y + oy}`);
    setCopiedIdx(i);
    window.setTimeout(() => setCopiedIdx(null), 1400);
  };

  const hits: FindHit[] = result?.matches ?? [];
  const currentWindow = windows.find((w) => w.hwnd === hwnd) ?? null;

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-3">
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-xl bg-amber-50/80 backdrop-blur-[1px]">
          <div className="flex flex-col items-center gap-2 text-amber-700">
            <Upload className="h-7 w-7" />
            <span className="text-[13px] font-medium">
              {tpl ? "松手作为搜索图" : "松手作为模板图"}
            </span>
          </div>
        </div>
      )}

      {/* 三步式工具条 */}
      <div className="flex flex-wrap items-end gap-x-3 gap-y-2.5">
        {/* ① 模板 */}
        <div className="flex flex-col gap-1">
          <Label className="text-[11px] text-neutral-500">① 要找什么（模板）</Label>
          <div className="flex items-center gap-1.5">
            <Button size="sm" variant="outline" onClick={() => tplInputRef.current?.click()}>
              <ImageIcon className="h-3.5 w-3.5" />
              {tpl ? "换模板" : "选模板图"}
            </Button>
            <input
              ref={tplInputRef}
              type="file"
              accept="image/*"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) void loadFileTo(f, "tpl");
                e.target.value = "";
              }}
            />
            {tpl && (
              <span
                className="flex items-center gap-1.5 rounded-md bg-neutral-50 py-0.5 pl-1 pr-2"
                title={tpl.name}
              >
                <img
                  src={tpl.url}
                  alt="模板"
                  className="h-6 w-6 rounded border border-neutral-200 bg-white object-contain"
                />
                <span className="font-mono text-[10.5px] text-neutral-500">
                  {tpl.width}×{tpl.height}
                </span>
              </span>
            )}
          </div>
        </div>

        {/* ② 搜索范围 */}
        <div className="flex flex-col gap-1">
          <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
            ② 在哪里找
            <Hint>{SOURCE_HINT[source]}</Hint>
          </Label>
          <div className="flex items-center gap-1.5">
            <Select value={source} onValueChange={(v) => setSource(v as FindSource)}>
              <SelectTrigger className="h-8 w-[122px] text-[12.5px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="window">
                  <AppWindow className="mr-1.5 inline h-3.5 w-3.5" />
                  某个窗口
                </SelectItem>
                <SelectItem value="screen">
                  <Monitor className="mr-1.5 inline h-3.5 w-3.5" />
                  整个屏幕
                </SelectItem>
                <SelectItem value="image">
                  <FileImage className="mr-1.5 inline h-3.5 w-3.5" />
                  上传的图片
                </SelectItem>
              </SelectContent>
            </Select>

            {source === "window" && (
              <>
                <Select
                  value={hwnd == null ? "" : String(hwnd)}
                  onValueChange={(v) => setHwnd(Number(v))}
                >
                  <SelectTrigger className="h-8 w-[180px] text-[12.5px]">
                    <SelectValue placeholder="选择窗口" />
                  </SelectTrigger>
                  <SelectContent>
                    {windows.length === 0 && (
                      <SelectItem value="__none" disabled>
                        没有可用窗口
                      </SelectItem>
                    )}
                    {windows.map((w) => (
                      <SelectItem key={w.hwnd} value={String(w.hwnd)}>
                        {w.title.length > 30 ? `${w.title.slice(0, 30)}…` : w.title}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8"
                  onClick={() => void refreshWindows()}
                  title="刷新窗口列表（新开的窗口要点这里才会出现）"
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                </Button>
              </>
            )}

            {source === "screen" && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void shootScreen(null)}
                disabled={busy}
              >
                <Monitor className="h-3.5 w-3.5" />
                先截一张
              </Button>
            )}

            {source === "image" && (
              <>
                <Button variant="outline" size="sm" onClick={() => hayInputRef.current?.click()}>
                  <FileImage className="h-3.5 w-3.5" />
                  {hay ? "换搜索图" : "选搜索图"}
                </Button>
                <input
                  ref={hayInputRef}
                  type="file"
                  accept="image/*"
                  className="hidden"
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (f) void loadFileTo(f, "hay");
                    e.target.value = "";
                  }}
                />
              </>
            )}
          </div>
        </div>

        {/* ③ 执行 */}
        <div className="flex flex-col gap-1">
          <Label className="text-[11px] text-neutral-500">③ 开始</Label>
          <Button size="sm" onClick={() => void onFind()} disabled={busy || !tpl}>
            <ScanSearch className="h-3.5 w-3.5" />
            {busy ? "查找中…" : "开始找图"}
          </Button>
        </div>

        <div className="ml-auto flex items-center gap-2">
          <div className="flex items-center gap-1.5">
            <Switch id="show-hits" checked={showHits} onCheckedChange={setShowHits} />
            <Label
              htmlFor="show-hits"
              className="flex cursor-pointer items-center gap-1 text-[12px] font-normal text-neutral-600"
            >
              <Target className="h-3.5 w-3.5" />
              显示命中框
            </Label>
          </div>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setAdvanced((a) => !a)}
            title="匹配精度、阈值、返回数量"
          >
            <Settings2 className="h-3.5 w-3.5" />
            高级
            <ChevronDown className={cn("h-3 w-3 transition-transform", advanced && "rotate-180")} />
          </Button>
        </div>
      </div>

      {/* 高级选项，默认收起，避免一上来一屏都是参数 */}
      {advanced && (
        <div className="flex flex-wrap items-end gap-3 rounded-lg bg-neutral-50/80 px-3 py-2.5">
          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              匹配精度
              <Hint>
                精准模式要求相似度 ≥ 0.92，几乎只命中完全一样的像素；
                容错模式放到 0.72，能容忍轻微压缩/抗锯齿差异，但更容易误命中。
              </Hint>
            </Label>
            <Select value={mode} onValueChange={(v) => setMode(v as FindMode)}>
              <SelectTrigger className="h-8 w-[130px] text-[12.5px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="exact">精准（≥0.92）</SelectItem>
                <SelectItem value="loose">容错（≥0.72）</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              自定义阈值
              <Hint>
                相似度下限，0~1，越大越严格。留空则跟随上面的精度预设。
                不确定就留空，先用精准模式试。
              </Hint>
            </Label>
            <Input
              className="h-8 w-[86px] text-[12.5px]"
              placeholder="跟随预设"
              value={minScore}
              onChange={(e) => setMinScore(e.target.value)}
            />
          </div>

          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              最多返回
              <Hint>1 表示只找最像的那一个；找多个相同按钮时调大（上限 64）。</Hint>
            </Label>
            <Input
              className="h-8 w-[70px] text-[12.5px]"
              type="number"
              min={1}
              max={64}
              value={maxResults}
              onChange={(e) => setMaxResults(Math.max(1, Number(e.target.value) || 1))}
            />
          </div>

          <div className="pb-1 text-[11.5px] text-neutral-400">
            当前在「{SOURCE_LABEL[source]}」里找
            {source === "window" && currentWindow ? ` · ${currentWindow.title.slice(0, 20)}` : ""}
          </div>
        </div>
      )}

      {error && (
        <Callout variant="error" icon={<Crosshair className="h-4 w-4" />}>
          {error}
        </Callout>
      )}

      {/* 主体 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[1.55fr_1fr]">
        <Card className="flex min-h-[340px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium text-neutral-500">
              <Crosshair className="h-3.5 w-3.5 text-blue-600" />
              {hay ? SOURCE_LABEL[source] : "搜索范围预览"}
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
              empty={
                <EmptyState
                  icon={<Hand className="h-5 w-5" />}
                  title="还没确定搜索范围"
                  description="模板是「要找的小图」，搜索范围是「去哪张图里找」。两个都有了才能开始。"
                  steps={[
                    {
                      icon: <ImageIcon className="h-3.5 w-3.5" />,
                      text: "① 选一张模板图（要找到的小图标）",
                    },
                    {
                      icon: <AppWindow className="h-3.5 w-3.5" />,
                      text: "② 选一个窗口，或整个屏幕",
                      detail: "点「开始找图」时会自动截当前画面",
                    },
                    {
                      icon: <ScanSearch className="h-3.5 w-3.5" />,
                      text: "③ 点「开始找图」",
                      detail: "命中的位置会用琥珀色框标出来",
                    },
                  ]}
                />
              }
            />
          </CardContent>
        </Card>

        <Card className="flex min-h-[340px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium text-neutral-500">
                <Target className="h-3.5 w-3.5 text-blue-600" />
                命中结果
              </CardTitle>
              {result && (
                <Badge variant={result.found ? "warning" : "secondary"}>
                  {result.found ? `命中 ${result.matched}` : "未找到"} · {result.elapsed_ms} ms
                </Badge>
              )}
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {result && result.found && result.matches.length > 0 ? (
              <>
                <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                  <ul className="divide-y divide-neutral-200/60">
                    {result.matches.map((h, i) => (
                      <li key={i} className="group flex items-start gap-2.5 px-3 py-2">
                        <span
                          className="mt-0.5 flex shrink-0 items-center justify-center rounded bg-amber-600 text-[10px] font-semibold text-white"
                          style={{ width: 18, height: 18 }}
                        >
                          {i + 1}
                        </span>
                        <div className="min-w-0 flex-1">
                          <div className="flex items-center gap-2">
                            <span className="font-mono text-[12.5px] text-neutral-900">
                              ({h.x}, {h.y})
                            </span>
                            <span className="ml-auto font-mono text-[10.5px] text-neutral-400">
                              相似 {h.score.toFixed(3)}
                            </span>
                          </div>
                          <div className="mt-0.5 font-mono text-[10.5px] text-neutral-400">
                            中心 ({h.center_x.toFixed(1)}, {h.center_y.toFixed(1)}) · {h.width}×
                            {h.height}
                          </div>
                        </div>
                        <button
                          type="button"
                          onClick={() => void copyCoord(h, i)}
                          title="复制屏幕绝对坐标（中心点），可直接喂给点击脚本"
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
                <p className="mt-1.5 text-[11px] text-neutral-400">
                  点右侧复制按钮可拿到屏幕绝对坐标（中心点）。
                </p>
              </>
            ) : (
              <EmptyState
                className="bg-neutral-50/40"
                icon={<Target className="h-5 w-5" />}
                title={result ? "没找到匹配" : "还没有结果"}
                description={
                  result
                    ? "可能是画面里没有这个图，或者相似度不够。切到「容错」再试一次。"
                    : "模板和搜索范围都选好后，点「开始找图」。"
                }
              />
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
