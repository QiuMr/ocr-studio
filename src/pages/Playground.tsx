import { useCallback, useEffect, useRef, useState } from "react";
import {
  Braces,
  Clipboard,
  Copy,
  Crosshair,
  Eye,
  EyeOff,
  FileImage,
  ImagePlus,
  Keyboard,
  List,
  Loader2,
  RefreshCw,
  ScanText,
  ScanLine,
  Settings2,
  Sparkles,
  Type,
} from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Label } from "../components/ui/label";
import { Switch } from "../components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../components/ui/tabs";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { ScrollArea } from "../components/ui/scroll-area";
import { EmptyState } from "../components/ui/empty-state";
import { Callout } from "../components/ui/callout";
import { Hint } from "../components/ui/hint";
import { ImageViewer } from "../components/ImageViewer";
import { FindPanel } from "../components/FindPanel";
import { ScanPanel } from "../components/ScanPanel";
import {
  fileToBase64,
  detectImage,
  imageSize,
  listEngines,
  pipeline,
  recognize,
  recognizeChars,
  regionOcr,
} from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import { cn } from "../lib/utils";
import type { OcrTab } from "../App";
import type {
  DanziResult,
  DetectResult,
  EngineItem,
  LineResult,
  PipelineResult,
  RecognizeResult,
  RegionOcrResult,
} from "../types";

/** 引擎在下拉里的显示名。名字本身就要告诉用户「什么时候该选它」。 */
function engineLabel(e: EngineItem): string {
  if (e.family === "ddddocr") return `验证码识别（DDDDOCR）`;
  // 目标检测可以挂多个模型（用户自己的）：默认那条不带 variant，
  // 其余用模型名区分——`variant` 就是 onnx 文件名去掉扩展名
  if (e.family === "yolo") {
    return e.variant ? `找物体 · ${e.variant}` : "找物体（默认模型）";
  }
  return e.name;
}

/** 上次选的引擎：检测模型是用户自己加的，每次开 App 都要重选很烦 */
const ENGINE_KEY = "ocr-studio:engine";
/** 换图后要不要自动识别。默认开——小白最常问的就是「图放进去了怎么没反应」 */
const AUTO_KEY = "ocr-studio:auto-run";

export function Playground({
  tab,
  onTabChange,
  pickToken,
}: {
  tab: OcrTab;
  onTabChange: (t: OcrTab) => void;
  /** 每次自增 = 请把选图框弹出来（首页那个大按钮触发的） */
  pickToken: number;
}) {
  // 识别页载入的图会传下来，切到找图页可以直接当搜索图
  const [b64, setB64] = useState<string | null>(null);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="mb-4 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-[26px] font-bold leading-none tracking-tight text-neutral-900">
            试跑
          </h1>
          <p className="mt-2 text-[13px] font-medium leading-5 text-neutral-500">
            {tab === "ocr"
              ? "丢一张图进去，看 /ocr/shibie 返回什么。放进来就自动跑，不用点按钮。"
              : tab === "scan"
                ? "丢一张图进去，把里面的二维码、条形码内容扫出来。放进来就自动扫。"
                : // 两种找图模式的「要找什么」不一样（小图 vs 模型），这里别写死
                  // 某一个端点路径——语义找图走的是 /ocr/yuyizhaotu。
                  "给一张大图，再给一张小图或一个检测模型，看找不找得到位置。"}
          </p>
        </div>

        {/* 用普通按钮而不是 Tab 条：三个功能差别很大，分开显示标题更清楚 */}
        <div className="flex items-center gap-1 rounded-[10px] bg-neutral-100 p-1">
          {(
            [
              { v: "ocr" as OcrTab, label: "文字识别", icon: <Type className="h-3.5 w-3.5" /> },
              { v: "find" as OcrTab, label: "找图定位", icon: <Crosshair className="h-3.5 w-3.5" /> },
              { v: "scan" as OcrTab, label: "扫码", icon: <ScanLine className="h-3.5 w-3.5" /> },
            ]
          ).map((it) => (
            <button
              key={it.v}
              type="button"
              onClick={() => onTabChange(it.v)}
              className={cn(
                "flex h-7 items-center gap-1.5 rounded-lg px-3 text-[12.5px] leading-none transition-colors",
                tab === it.v
                  ? "shadow-card bg-white font-semibold text-neutral-900"
                  : "font-medium text-neutral-500 hover:text-neutral-800",
              )}
            >
              {it.icon}
              {it.label}
            </button>
          ))}
        </div>
      </div>

      {tab === "ocr" ? (
        <OcrPanel onImageLoaded={setB64} pickToken={pickToken} />
      ) : tab === "scan" ? (
        <ScanPanel />
      ) : (
        <FindPanel inheritedImage={b64} />
      )}
    </div>
  );
}

interface OcrPanelProps {
  /** 把载入的图同步出去，切到找图页时可直接复用 */
  onImageLoaded: (b64: string) => void;
  pickToken: number;
}

function OcrPanel({ onImageLoaded, pickToken }: OcrPanelProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [name, setName] = useState<string>("");
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [result, setResult] = useState<RecognizeResult | null>(null);
  /** 检测引擎（yolo 等）的结果；与 result 互斥 */
  const [detResult, setDetResult] = useState<DetectResult | null>(null);
  /** 检测+识别串联（框内识字）的结果 */
  const [pipeResult, setPipeResult] = useState<PipelineResult | null>(null);
  /** 区域 OCR：只认图上框选的那一块 */
  const [regionMode, setRegionMode] = useState(false);
  const [selection, setSelection] = useState<[number, number, number, number] | null>(null);
  const [regionResult, setRegionResult] = useState<RegionOcrResult | null>(null);
  /** 检测模式下：识别时是否顺带在框内认字 */
  const [ocrInBoxes, setOcrInBoxes] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showBoxes, setShowBoxes] = useState(true);
  const [useCls, setUseCls] = useState(true);
  /** 空字符串 = 用当前 PP-OCR 配置（历史行为）；其它值为 `family:variant` */
  const [engine, setEngine] = useState<string>(() => {
    try {
      return localStorage.getItem(ENGINE_KEY) ?? "";
    } catch {
      return "";
    }
  });
  const [engines, setEngines] = useState<EngineItem[]>([]);
  const [copied, setCopied] = useState(false);
  const [tab, setTab] = useState("text");
  /** 逐字结果：按需加载，切到「逐字」tab 时才请求 */
  const [detail, setDetail] = useState<DanziResult | null>(null);
  const [charBusy, setCharBusy] = useState(false);
  /** 「更多设置」浮层 */
  const [moreOpen, setMoreOpen] = useState(false);
  /** 换图后自动识别 */
  const [autoRun, setAutoRun] = useState<boolean>(() => {
    try {
      return localStorage.getItem(AUTO_KEY) !== "0";
    } catch {
      return true;
    }
  });

  const b64Ref = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const moreRef = useRef<HTMLDivElement>(null);
  /**
   * 识别请求序号。
   *
   * 自动识别意味着「拖第二张图」可能发生在第一次还没算完的时候。
   * 只认最后一次的返回，早先的结果直接丢 —— 否则会出现
   * 「图是新的、框是旧图的」这种最难解释的现象。
   */
  const runSeq = useRef(0);

  /**
   * 可选引擎列表。
   *
   * 只列**已经可用**的非 PP-OCR 引擎：PP-OCR 的档位由设置页管理，
   * 在识别面板里再选一次会出现「两处都能改、以谁为准」的歧义。
   * 加载失败（比如模型目录被搬走）时静默降级为只有默认项，不打断识别主流程。
   */
  useEffect(() => {
    let alive = true;
    void listEngines()
      .then((l) => {
        if (!alive) return;
        setEngines(l.engines.filter((e) => e.available && e.family !== "ppocr"));
      })
      .catch(() => {
        if (alive) setEngines([]);
      });
    return () => {
      alive = false;
    };
  }, []);

  /** 当前选中的引擎条目；空字符串 = PP-OCR 默认 */
  const engineItem = engines.find((e) => e.name === engine);
  /** 检测引擎（yolo）没有行框/文字能力，界面切换成「找物体」形态 */
  const isDetector = engineItem?.capabilities.detect === true && !engineItem.capabilities.line_boxes;

  /**
   * 真正跑一次推理。
   *
   * 抽成独立函数是为了让「载入图片」能直接调用它 —— 小白不该需要
   * 「先放图，再点开始识别」这两步，放进去就应该有反应。
   */
  const runRecognize = useCallback(
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
        if (isDetector && ocrInBoxes) {
          const r = await pipeline(b64, engine);
          if (seq !== runSeq.current) return;
          setPipeResult(r);
          setResult(null);
          setDetResult(null);
        } else if (isDetector) {
          const r = await detectImage(b64, engine);
          if (seq !== runSeq.current) return;
          setDetResult(r);
          setResult(null);
          setPipeResult(null);
        } else {
          const r = await recognize(b64, useCls, engine);
          if (seq !== runSeq.current) return;
          setResult(r);
          setDetResult(null);
          setPipeResult(null);
        }
        setDetail(null); // 档位/参数可能变了，旧的逐字结果作废
        setRegionResult(null);
        setTab("text");
      } catch (e) {
        if (seq === runSeq.current) setError(`识别失败：${e}`);
      } finally {
        if (seq === runSeq.current) setBusy(false);
      }
    },
    [isDetector, ocrInBoxes, engine, useCls],
  );

  /**
   * 区域 OCR：只识别框选出来的那一块。
   *
   * 与整图识别的取舍：大图上只关心一小块时（比如游戏里的某个数值栏），
   * 整图识别再自己过滤既慢又会混进区域外的噪声文字。
   */
  const runRegion = useCallback(
    async (region: [number, number, number, number]) => {
      const b64 = b64Ref.current;
      if (!b64) return;
      const seq = ++runSeq.current;
      setBusy(true);
      setError(null);
      try {
        const r = await regionOcr(b64, region, useCls, engine);
        if (seq !== runSeq.current) return;
        setRegionResult(r);
        setResult(null);
        setDetResult(null);
        setPipeResult(null);
        setDetail(null);
        setTab("text");
      } catch (e) {
        if (seq === runSeq.current) setError(`区域识别失败：${e}`);
      } finally {
        if (seq === runSeq.current) setBusy(false);
      }
    },
    [useCls, engine],
  );

  /** 框选回调：有选区就立刻识别，拖得太小（null）只清结果不报错。 */
  const onSelectRegion = useCallback(
    (r: [number, number, number, number] | null) => {
      setSelection(r);
      setRegionResult(null);
      if (r) void runRegion(r);
    },
    [runRegion],
  );

  /** 所有入口（选文件 / 拖入 / 粘贴）最终都走这里，状态更新只写一遍。 */
  const applyImage = useCallback(
    async (b64: string, url: string, dim: { width: number; height: number }, label: string) => {
      b64Ref.current = b64;
      setSrc(url);
      setSize(dim);
      setName(label);
      setResult(null);
      setDetResult(null);
      setPipeResult(null);
      setRegionResult(null);
      setSelection(null);
      setDetail(null);
      setTab("text");
      setError(null);
      onImageLoaded(b64);
      // 框选模式下不自动整图识别——用户要的是「先在图上拖一个框」
      if (autoRun && !regionMode) void runRecognize(b64);
    },
    [onImageLoaded, autoRun, runRecognize, regionMode],
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

  // 首页那个大按钮：跳过来就把选图框弹出来，不用用户再找一次入口
  const lastPick = useRef(0);
  useEffect(() => {
    if (pickToken === lastPick.current) return;
    lastPick.current = pickToken;
    inputRef.current?.click();
  }, [pickToken]);

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

  // 点浮层外面收起来
  useEffect(() => {
    if (!moreOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!moreRef.current?.contains(e.target as Node)) setMoreOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [moreOpen]);

  /** 懒加载逐字结果：只有真的切到「逐字」tab 才多跑一次推理。 */
  const loadChars = async () => {
    if (!b64Ref.current || detail) return;
    setCharBusy(true);
    setError(null);
    try {
      setDetail(await recognizeChars(b64Ref.current, useCls, engine));
    } catch (e) {
      setError(`逐字定位失败：${e}`);
    } finally {
      setCharBusy(false);
    }
  };

  const onTabChange = (v: string) => {
    setTab(v);
    if (v === "chars") void loadChars();
  };

  const copyJson = async () => {
    if (pipeResult) {
      await navigator.clipboard.writeText(JSON.stringify(pipeResult.regions, null, 2));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
      return;
    }
    if (detResult) {
      await navigator.clipboard.writeText(JSON.stringify(detResult.detections, null, 2));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
      return;
    }
    if (!result) return;
    await navigator.clipboard.writeText(JSON.stringify(result.lines, null, 2));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  };

  // 图上画什么框：区域识别用区域内的行框，pipeline 模式画区域框
  // （文字=框内识别出的内容），检测模式画类别框
  const lines: LineResult[] = regionResult
    ? regionResult.lines
    : pipeResult
    ? pipeResult.regions.map((r) => ({
        text: r.text ? `${r.class_name} · ${r.text.split("\n")[0]}` : r.class_name,
        box: r.box,
        score: r.score,
        det_score: r.score,
        rotated: false,
      }))
    : detResult
      ? // 检测框复用行框的画图逻辑：文本位置显示「类别 + 置信度」
        detResult.detections.map((d) => ({
          text: `${d.class_name} ${(d.score * 100).toFixed(0)}%`,
          box: d.box,
          score: d.score,
          det_score: d.score,
          rotated: false,
        }))
      : (result?.lines ?? []);

  const hasResult = !!(result || detResult || pipeResult || regionResult);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-3">
      {/* 拖拽遮罩 */}
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-[14px] bg-blue-50/85 backdrop-blur-[1px]">
          <div className="flex flex-col items-center gap-2 text-blue-700">
            <ImagePlus className="h-8 w-8" />
            <span className="text-[14px] font-bold">松手就能识别</span>
          </div>
        </div>
      )}

      {/* 工具栏：只留「换图」和「更多」，其余概念都收起来 */}
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

        {busy && (
          <span className="flex items-center gap-1.5 text-[12.5px] text-neutral-500">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            正在识别…
          </span>
        )}

        {/* 有第二个可用引擎时才出现下拉，否则小白只会多一个看不懂的选择 */}
        {engines.length > 0 && (
          <div className="flex items-center gap-1">
            <Select
              value={engine || "default"}
              onValueChange={(v) => {
                const next = v === "default" ? "" : v;
                setEngine(next);
                try {
                  if (next) localStorage.setItem(ENGINE_KEY, next);
                  else localStorage.removeItem(ENGINE_KEY);
                } catch {
                  /* 隐私模式下 localStorage 可能不可写，忽略 */
                }
                // 换引擎后旧结果一定作废，别让用户拿 A 的框对着 B 的图
                setResult(null);
                setDetResult(null);
                setPipeResult(null);
                setDetail(null);
                if (b64Ref.current) void runRecognize();
              }}
            >
              <SelectTrigger className="h-8 w-[176px] text-[12.5px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="default">识别文字（默认）</SelectItem>
                {engines.map((e) => (
                  <SelectItem key={e.name} value={e.name}>
                    {engineLabel(e)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Hint>
              不同模型擅长的东西不一样：默认那套适合成段文字（截图、扫描件），
              DDDDOCR 训练在验证码上，短英文数字更快；「找物体」会框出物体类别，
              用哪个模型在设置页里管。换完会自动重新识别一次。
            </Hint>
          </div>
        )}

        <div className="ml-auto flex items-center gap-2">
          {result && (
            <Badge variant="secondary">
              {result.lines.length} 行 · {result.elapsed_ms} ms
            </Badge>
          )}
          {detResult && (
            <Badge variant="secondary">
              {detResult.count} 个 · {detResult.elapsed_ms} ms
            </Badge>
          )}
          {pipeResult && (
            <Badge variant="secondary">
              {pipeResult.regions.length} 区域 · {pipeResult.count} 行 · {pipeResult.elapsed_ms} ms
            </Badge>
          )}
          {regionResult && (
            <Badge variant="secondary">
              区域 {regionResult.region[2]}×{regionResult.region[3]} · {regionResult.lines.length} 行 ·{" "}
              {regionResult.elapsed_ms} ms
            </Badge>
          )}
          <span className="hidden items-center gap-1 text-[11.5px] text-neutral-400 sm:flex">
            <Keyboard className="h-3 w-3" />
            Ctrl+V
          </span>

          {/* 更多设置：显示框、方向分类这些不是每天都动的东西 */}
          <div className="relative" ref={moreRef}>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setMoreOpen((o) => !o)}
              aria-expanded={moreOpen}
            >
              <Settings2 className="h-3.5 w-3.5" />
              更多
            </Button>

            {moreOpen && (
              <div className="absolute right-0 top-full z-30 mt-1.5 w-[268px] rounded-[12px] border border-neutral-200 bg-white p-3 shadow-[0_12px_32px_rgba(16,24,40,0.14)]">
                <ToggleRow
                  id="show-boxes"
                  checked={showBoxes}
                  onCheckedChange={setShowBoxes}
                  label="在图上画框"
                  hint="结果框会直接叠在图片上"
                  icon={showBoxes ? <Eye className="h-3.5 w-3.5" /> : <EyeOff className="h-3.5 w-3.5" />}
                />

                {!isDetector && (
                  <ToggleRow
                    id="region-mode"
                    checked={regionMode}
                    onCheckedChange={(v) => {
                      setRegionMode(v);
                      setSelection(null);
                      setRegionResult(null);
                    }}
                    label="只识别一块区域"
                    hint="打开后在图上拖一个框，只认框里的字"
                    icon={<Crosshair className="h-3.5 w-3.5" />}
                  />
                )}

                {isDetector ? (
                  <ToggleRow
                    id="ocr-in-boxes"
                    checked={ocrInBoxes}
                    onCheckedChange={setOcrInBoxes}
                    label="框里也认字"
                    hint="每个框内再识别一次文字，慢一些"
                  />
                ) : (
                  <ToggleRow
                    id="use-cls"
                    checked={useCls}
                    onCheckedChange={setUseCls}
                    label="自动翻正倒字"
                    hint="倒着 180° 的字会先翻正再认"
                  />
                )}

                <ToggleRow
                  id="auto-run"
                  checked={autoRun}
                  onCheckedChange={(v) => {
                    setAutoRun(v);
                    try {
                      localStorage.setItem(AUTO_KEY, v ? "1" : "0");
                    } catch {
                      /* 忽略 */
                    }
                  }}
                  label="换图后自动识别"
                  hint="关掉就每次自己点「重新识别」"
                />

                <div className="my-2 h-px bg-neutral-100" />

                <Button
                  variant="ghost"
                  size="sm"
                  className="w-full justify-start"
                  onClick={() => {
                    setMoreOpen(false);
                    void runRecognize();
                  }}
                  disabled={!src || busy}
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  重新识别一次
                </Button>
              </div>
            )}
          </div>
        </div>
      </div>

      {error && (
        <Callout variant="error" icon={<ScanText className="h-4 w-4" />}>
          {error}
        </Callout>
      )}

      {/* 主体：左图右结果 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[1.55fr_1fr]">
        <Card className="flex min-h-[360px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            {/* h-8：与右侧卡片头（含「复制 JSON」按钮）保持同一行高，
                否则左标题会比右标题高 7px —— 一眼就能看出的错位 */}
            <div className="flex h-8 items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium leading-none text-neutral-500">
                <FileImage className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                {src ? "原图" : "待识别的图片"}
                {size.width > 0 && (
                  <span className="font-normal text-neutral-400">
                    {name && <span className="mr-1.5 text-neutral-500">{name}</span>}
                    {size.width} × {size.height}
                  </span>
                )}
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {src ? (
              <>
                <ImageViewer
                  src={src}
                  width={size.width}
                  height={size.height}
                  lines={lines}
                  showBoxes={showBoxes}
                  chars={tab === "chars" ? detail?.lines.flatMap((l) => l.chars) : undefined}
                  selectable={regionMode}
                  selection={selection}
                  onSelect={onSelectRegion}
                />
                {regionMode && (
                  <p className="mt-1.5 flex items-center gap-1.5 text-[11.5px] text-sky-600">
                    <Crosshair className="h-3.5 w-3.5" />
                    {selection
                      ? `已选 ${selection[2]}×${selection[3]} @(${selection[0]}, ${selection[1]})，重新拖可换区域`
                      : "在图上按住拖一个框，松手就开始只识别框里的字"}
                  </p>
                )}
              </>
            ) : (
              <DropZone onPick={() => inputRef.current?.click()} />
            )}
          </CardContent>
        </Card>

        <Card className="flex min-h-[360px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex h-8 items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium leading-none text-neutral-500">
                <ScanText className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                {regionResult
                  ? "区域识别结果"
                  : pipeResult
                    ? "检测 + 识别结果"
                    : detResult
                      ? "检测结果"
                      : result
                        ? "识别结果"
                        : "识别结果会显示在这里"}
              </CardTitle>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void copyJson()}
                disabled={!hasResult}
              >
                {copied ? (
                  <Copy className="h-3.5 w-3.5 text-emerald-600" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {copied ? "已复制" : "复制 JSON"}
              </Button>
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {regionResult ? (
              <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                <div className="p-3">
                  <pre className="selectable whitespace-pre-wrap font-sans text-[13px] leading-6 text-neutral-800">
                    {regionResult.text || "（框里没识别到文字）"}
                  </pre>
                  {regionResult.lines.length > 0 && (
                    <ul className="mt-3 divide-y divide-neutral-200/60 border-t border-neutral-200/70">
                      {regionResult.lines.map((l, i) => (
                        <li key={i} className="flex items-start gap-2.5 py-2">
                          <span
                            className="mt-0.5 flex shrink-0 items-center justify-center rounded bg-blue-600 text-[10px] font-semibold text-white"
                            style={{ width: 18, height: 18 }}
                          >
                            {i + 1}
                          </span>
                          <div className="min-w-0 flex-1">
                            <div className="selectable truncate text-[12.5px] text-neutral-900">
                              {l.text}
                            </div>
                            <div className="mt-0.5 font-mono text-[10.5px] text-neutral-400">
                              置信 {l.score.toFixed(3)} · 位置 ({l.box[0][0].toFixed(0)},{" "}
                              {l.box[0][1].toFixed(0)})
                            </div>
                          </div>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>
              </ScrollArea>
            ) : pipeResult ? (
              <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                <ul className="divide-y divide-neutral-200/60">
                  {pipeResult.regions.map((r, i) => (
                    <li key={i} className="flex items-start gap-2.5 px-3 py-2">
                      <span
                        className="mt-0.5 flex shrink-0 items-center justify-center rounded text-[10px] font-semibold text-white"
                        style={{
                          width: 18,
                          height: 18,
                          background: `hsl(${(r.class_id * 47) % 360} 70% 45%)`,
                        }}
                      >
                        {r.class_id}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="selectable truncate text-[12.5px] font-medium text-neutral-900">
                            {r.class_name}
                          </span>
                          <span className="font-mono text-[10.5px] text-neutral-400">
                            置信 {r.score.toFixed(3)}
                          </span>
                        </div>
                        {r.text ? (
                          <div className="selectable mt-0.5 whitespace-pre-wrap text-[12.5px] leading-5 text-neutral-700">
                            {r.text}
                          </div>
                        ) : (
                          <div className="mt-0.5 text-[11.5px] text-neutral-400">
                            （框内没识别出文字）
                          </div>
                        )}
                      </div>
                    </li>
                  ))}
                  {pipeResult.regions.length === 0 && (
                    <li className="px-3 py-8 text-center text-[12.5px] text-neutral-400">
                      （没有检出区域）
                    </li>
                  )}
                </ul>
              </ScrollArea>
            ) : detResult ? (
              <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                <ul className="divide-y divide-neutral-200/60">
                  {detResult.detections.map((d, i) => (
                    <li key={i} className="flex items-start gap-2.5 px-3 py-2">
                      <span
                        className="mt-0.5 flex shrink-0 items-center justify-center rounded text-[10px] font-semibold text-white"
                        style={{
                          width: 18,
                          height: 18,
                          background: `hsl(${(d.class_id * 47) % 360} 70% 45%)`,
                        }}
                      >
                        {d.class_id}
                      </span>
                      <div className="min-w-0 flex-1">
                        <div className="selectable truncate text-[12.5px] text-neutral-900">
                          {d.class_name}
                        </div>
                        <div className="mt-0.5 font-mono text-[10.5px] text-neutral-400">
                          置信 {d.score.toFixed(3)} · 位置 ({d.box[0][0].toFixed(0)},{" "}
                          {d.box[0][1].toFixed(0)}) - ({d.box[2][0].toFixed(0)},{" "}
                          {d.box[2][1].toFixed(0)})
                        </div>
                      </div>
                    </li>
                  ))}
                  {detResult.detections.length === 0 && (
                    <li className="px-3 py-8 text-center text-[12.5px] text-neutral-400">
                      （没有检出目标，可以试着把置信度调低）
                    </li>
                  )}
                </ul>
              </ScrollArea>
            ) : result ? (
              <Tabs value={tab} onValueChange={onTabChange} className="flex h-full flex-col">
                <TabsList className="self-start">
                  <TabsTrigger value="text">
                    <Type />
                    文本
                  </TabsTrigger>
                  <TabsTrigger value="lines">
                    <List />
                    逐行
                  </TabsTrigger>
                  <TabsTrigger value="chars">
                    <Braces />
                    逐字
                  </TabsTrigger>
                  <TabsTrigger value="json">
                    <Braces />
                    JSON
                  </TabsTrigger>
                </TabsList>

                <TabsContent value="text" className="mt-2 min-h-0 flex-1">
                  <ScrollArea className="h-full rounded-lg bg-neutral-50/70 p-3">
                    <pre className="selectable whitespace-pre-wrap font-sans text-[13px] leading-6 text-neutral-800">
                      {result.text || "（没识别到文字）"}
                    </pre>
                  </ScrollArea>
                </TabsContent>

                <TabsContent value="lines" className="mt-2 min-h-0 flex-1">
                  <ScrollArea className="h-full rounded-lg bg-neutral-50/70">
                    <ul className="divide-y divide-neutral-200/60">
                      {lines.map((l, i) => (
                        <li key={i} className="flex items-start gap-2.5 px-3 py-2">
                          <span
                            className="mt-0.5 flex shrink-0 items-center justify-center rounded bg-blue-600 text-[10px] font-semibold text-white"
                            style={{ width: 18, height: 18 }}
                          >
                            {i + 1}
                          </span>
                          <div className="min-w-0 flex-1">
                            <div className="selectable truncate text-[12.5px] text-neutral-900">
                              {l.text}
                            </div>
                            <div className="mt-0.5 font-mono text-[10.5px] text-neutral-400">
                              置信 {l.score.toFixed(3)} · 检测 {l.det_score.toFixed(3)}
                              {l.rotated ? " · 已翻转" : ""}
                            </div>
                          </div>
                        </li>
                      ))}
                      {lines.length === 0 && (
                        <li className="px-3 py-8 text-center text-[12.5px] text-neutral-400">
                          （没识别到文字）
                        </li>
                      )}
                    </ul>
                  </ScrollArea>
                </TabsContent>

                <TabsContent value="chars" className="mt-2 min-h-0 flex-1">
                  <ScrollArea className="h-full rounded-lg bg-neutral-50/70 p-3">
                    {charBusy && (
                      <div className="py-6 text-center text-[12.5px] text-neutral-400">
                        正在做逐字定位…
                      </div>
                    )}
                    {!charBusy && !detail && (
                      <div className="py-6 text-center text-[12.5px] text-neutral-400">
                        没有逐字数据
                      </div>
                    )}
                    {!charBusy && detail && (
                      <div className="flex flex-col gap-3">
                        <p className="text-[11.5px] leading-5 text-neutral-500">
                          共 {detail.char_count} 字。图中青色细框就是一个字的范围 ——
                          它是按识别模型的帧对齐推算出来的，字距不均时会有几像素偏差。
                        </p>
                        {detail.lines.map((l, li) => (
                          <div key={li}>
                            <div className="mb-1 flex items-center gap-2 text-[11px] text-neutral-400">
                              <span className="font-mono">第 {li + 1} 行</span>
                              <span>· {l.chars.length} 字</span>
                              <span>· {l.frames} 帧</span>
                            </div>
                            <div className="flex flex-wrap gap-1">
                              {l.chars.map((c) => {
                                const cx = (c.box[0][0] + c.box[2][0]) / 2;
                                const cy = (c.box[0][1] + c.box[2][1]) / 2;
                                return (
                                  <span
                                    key={`${li}-${c.index}`}
                                    title={`(${cx.toFixed(1)}, ${cy.toFixed(1)}) 置信 ${c.score.toFixed(3)} · 帧 ${c.t_start}-${c.t_end}`}
                                    className="flex h-7 min-w-7 cursor-default items-center justify-center rounded border border-sky-200 bg-white px-1 text-[13px] text-neutral-800"
                                  >
                                    {c.ch}
                                  </span>
                                );
                              })}
                            </div>
                          </div>
                        ))}
                      </div>
                    )}
                  </ScrollArea>
                </TabsContent>

                <TabsContent value="json" className="mt-2 min-h-0 flex-1">
                  <ScrollArea className="h-full rounded-lg bg-neutral-50/70 p-3">
                    <pre className="selectable font-mono text-[11px] leading-5 text-neutral-700">
                      {JSON.stringify(result.lines, null, 2)}
                    </pre>
                  </ScrollArea>
                </TabsContent>
              </Tabs>
            ) : busy ? (
              <div className="flex h-full min-h-[260px] flex-col items-center justify-center gap-2 rounded-xl bg-neutral-50/50 text-neutral-400">
                <Loader2 className="h-6 w-6 animate-spin" />
                <span className="text-[12.5px]">正在识别，稍等一下…</span>
              </div>
            ) : (
              <EmptyState
                className="bg-neutral-50/40"
                icon={<Sparkles className="h-5 w-5" />}
                title="结果会出现在这里"
                description="左边放一张图，识别完成后这里会显示整段文字、逐行明细和原始 JSON。"
              />
            )}
          </CardContent>
        </Card>
      </div>

      {/* 底部提示：为什么框和字对不上之类的小白困惑 */}
      {(detResult || pipeResult) &&
        (detResult?.detections.length ?? 0) + (pipeResult?.regions.length ?? 0) > 0 &&
        showBoxes && (
          <div className="flex items-center gap-1.5 text-[11.5px] text-neutral-400">
            {pipeResult
              ? "框上的文字是「类别 · 框内识别出的第一行」，右侧列表有完整文字。"
              : "框上的文字是「类别 + 置信度」，右侧列表的编号就是类别 id。"}
          </div>
        )}
      {!detResult && !pipeResult && result && result.lines.length > 0 && showBoxes && (
        <div className="flex items-center gap-1.5 text-[11.5px] text-neutral-400">
          图上的蓝框就是每一行的位置，编号与「逐行」列表一一对应。
        </div>
      )}
    </div>
  );
}

/** 浮层里的一行开关：左边标题 + 一行解释，右边 Switch。 */
function ToggleRow({
  id,
  checked,
  onCheckedChange,
  label,
  hint,
  icon,
}: {
  id: string;
  checked: boolean;
  onCheckedChange: (v: boolean) => void;
  label: string;
  hint: string;
  icon?: React.ReactNode;
}) {
  return (
    <div className="flex items-start gap-2.5 py-1.5">
      <div className="min-w-0 flex-1">
        <Label
          htmlFor={id}
          className="flex cursor-pointer items-center gap-1.5 text-[12.5px] leading-none text-neutral-800"
        >
          {icon && <span className="text-neutral-400">{icon}</span>}
          {label}
        </Label>
        <p className="mt-1 text-[11px] leading-4 text-neutral-400">{hint}</p>
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onCheckedChange} className="mt-0.5" />
    </div>
  );
}

/**
 * 没图时的引导区。
 *
 * 小白最短路径是「把手里的图丢进来」，所以这里不是一句"暂无数据"，
 * 而是一个**看起来就能扔东西进去**的虚线大框，下面把另外两条路也写清楚。
 */
function DropZone({ onPick }: { onPick: () => void }) {
  return (
    <div className="flex h-full min-h-[280px] flex-col items-center justify-center rounded-[14px] border-2 border-dashed border-neutral-200 bg-neutral-50/50 px-6 py-8 text-center">
      <div className="flex h-14 w-14 items-center justify-center rounded-[16px] bg-white text-blue-500 shadow-card">
        <ImagePlus className="h-6 w-6" strokeWidth={1.8} />
      </div>
      <p className="mt-4 text-[15px] font-bold text-neutral-800">把图片拖到这里</p>
      <p className="mt-1 text-[12.5px] text-neutral-500">放进来就自动识别，不用再点别的</p>

      <div className="mt-5 flex flex-wrap items-center justify-center gap-2">
        <span className="flex items-center gap-1.5 rounded-full bg-white px-3 py-1.5 text-[11.5px] text-neutral-600 ring-1 ring-neutral-200/80">
          <Clipboard className="h-3.5 w-3.5 text-neutral-400" />
          按 Ctrl+V 粘贴截图
        </span>
        <span className="flex items-center gap-1.5 rounded-full bg-white px-3 py-1.5 text-[11.5px] text-neutral-600 ring-1 ring-neutral-200/80">
          <Keyboard className="h-3.5 w-3.5 text-neutral-400" />
          Win+Shift+S 截图后直接粘
        </span>
      </div>

      <Button variant="outline" size="sm" className="mt-4" onClick={onPick}>
        <FileImage className="h-3.5 w-3.5" />
        或者从文件夹里选
      </Button>
    </div>
  );
}
