import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import {
  AppWindow,
  Boxes,
  Brain,
  Check,
  ChevronDown,
  Copy,
  Crosshair,
  FileImage,
  Frame,
  Hand,
  Image as ImageIcon,
  Layers,
  Monitor,
  RefreshCw,
  ScanSearch,
  Settings2,
  Sparkles,
  Target,
  Type,
  Upload,
  X,
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
import {
  detectModels,
  fileToBase64,
  findImage,
  findPrompt,
  findSemantic,
  imageSize,
  listWindows,
  screenPreview,
} from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import { cn } from "../lib/utils";
import type {
  DetectModel,
  FindHit,
  FindKind,
  FindMode,
  FindResult,
  FindSource,
  PromptMode,
  WindowInfo,
} from "../types";

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

/**
 * 参考框（提示找图用）。坐标是**原图像素**，不是屏幕坐标——
 * 参考图只是「给模型看的样例」，跟搜索范围没有位置关系。
 */
interface RefBox {
  id: number;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

/** 参考框的面积下限（原图像素）。小于这个基本是手抖点了一下，不算画框。 */
const MIN_REF_BOX_AREA = 16;

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

const KIND_HINT: Record<FindKind, string> = {
  template:
    "比像素：拿一张小图（模板）去大图里找长得一样的位置。换个皮肤、按下态变了就找不到，但胜在「只要长得像就认得」。",
  semantic:
    "靠模型认目标：不用准备模板，模型认得「这是个按钮」就直接框出来。代价是只认得模型见过的类别，换目标得换模型。",
  prompt:
    "目标当场给：圈出参考图里要的东西，或者直接写几个词，模型就去找「跟这个像的 / 是这个的」。换目标只改这里的输入，不用换模型——这是它跟「语义找图」最大的区别。",
};

/** 提示找图的两个子模式：给图 / 给词。 */
const PROMPT_MODE_LABEL: Record<PromptMode, string> = {
  visual: "图找图",
  text: "语言找图",
};

/** 三种找图方式的顺序与文案（顺序 = 从「最省心」到「最灵活」）。 */
const KIND_META: { v: FindKind; label: string; blurb: string }[] = [
  { v: "template", label: "模板找图", blurb: "拿小图去大图里找" },
  { v: "semantic", label: "语义找图", blurb: "让模型自己认目标" },
  { v: "prompt", label: "提示找图", blurb: "圈一张参考图 / 写几个词" },
];

function KindIcon({ k, className }: { k: FindKind; className?: string }) {
  if (k === "template") return <ImageIcon className={className} />;
  if (k === "semantic") return <Brain className={className} />;
  return <Sparkles className={className} />;
}

/** `"0, 2 5"` → `[0,2,5]`；空串或全是垃圾 → undefined（= 不过滤） */
function parseIdList(s: string): number[] | undefined {
  const ids = s
    .split(/[,，\s]+/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => Number.parseInt(p, 10))
    .filter((n) => Number.isInteger(n) && n >= 0);
  return ids.length > 0 ? ids : undefined;
}

export function FindPanel({ inheritedImage }: Props) {
  const [kind, setKind] = useState<FindKind>("template");
  const [source, setSource] = useState<FindSource>("window");
  const [mode, setMode] = useState<FindMode>("exact");
  const [hay, setHay] = useState<Haystack | null>(null);
  const [tpl, setTpl] = useState<Template | null>(null);
  /** 模板掩码（可选）：白 = 参与匹配。尺寸必须与模板一致。 */
  const [mask, setMask] = useState<Template | null>(null);
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
  // ---- 语义找图专用 ----
  const [models, setModels] = useState<DetectModel[]>([]);
  const [modelEngine, setModelEngine] = useState("");
  const [conf, setConf] = useState("0.25");
  const [classIds, setClassIds] = useState("");
  // ---- 提示找图专用 ----
  const [promptMode, setPromptMode] = useState<PromptMode>("visual");
  /** 参考图：只是「给模型看的样例」，跟搜索范围无关 */
  const [refImg, setRefImg] = useState<Template | null>(null);
  const [refBoxes, setRefBoxes] = useState<RefBox[]>([]);
  const [words, setWords] = useState("");
  /** 左侧卡片显示哪一块：参考图（要框东西）/ 搜索范围（看命中） */
  const [leftTab, setLeftTab] = useState<"ref" | "hay">("ref");

  const hayInputRef = useRef<HTMLInputElement>(null);
  const tplInputRef = useRef<HTMLInputElement>(null);
  const maskInputRef = useRef<HTMLInputElement>(null);
  const refInputRef = useRef<HTMLInputElement>(null);
  const refBoxSeq = useRef(1);

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

  // 检测模型列表：只在切到语义找图时才拉（模板找图不需要它）
  useEffect(() => {
    if (kind !== "semantic" || models.length > 0) return;
    let cancelled = false;
    void detectModels()
      .then((list) => {
        if (cancelled) return;
        setModels(list);
        setModelEngine((cur) => cur || list[0]?.engine || "");
      })
      .catch((e) => setError(`读取检测模型失败: ${e}`));
    return () => {
      cancelled = true;
    };
  }, [kind, models.length]);

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
    // 换模板时掩码尺寸必然对不上，直接清掉，避免留下一个用不了的掩码
    setMask(null);
    setResult(null);
    setError(null);
  }, []);

  const applyMask = useCallback(async (b64: string, url: string, name: string) => {
    const dim = await imageSize(url);
    setMask({ url, b64, width: dim.width, height: dim.height, name });
    setResult(null);
    setError(null);
  }, []);

  const applyHaystack = useCallback(async (b64: string, url: string) => {
    const dim = await imageSize(url);
    setHay({ url, b64, width: dim.width, height: dim.height, origin: { x: 0, y: 0 } });
    setResult(null);
    setError(null);
  }, []);

  /** 换参考图：框是照着旧图的尺寸画的，必须一起清掉。 */
  const applyRefImage = useCallback(async (b64: string, url: string, name: string) => {
    const dim = await imageSize(url);
    setRefImg({ url, b64, width: dim.width, height: dim.height, name });
    setRefBoxes([]);
    setLeftTab("ref");
    setResult(null);
    setError(null);
  }, []);

  /** 拖进来的图：谁还缺就给谁。模板找图先补模板，图找图先补参考图，其余当搜索图。 */
  const onDrop = useCallback(
    (imgs: DroppedImage[]) => {
      const first = imgs[0];
      if (!first) return;
      if (kind === "template" && !tpl) void applyTemplate(first.b64, first.url, first.name);
      else if (kind === "prompt" && promptMode === "visual" && !refImg)
        void applyRefImage(first.b64, first.url, first.name);
      else void applyHaystack(first.b64, first.url);
    },
    [kind, promptMode, tpl, refImg, applyTemplate, applyRefImage, applyHaystack],
  );

  const { dragging } = useImageDrop(onDrop);

  /** 载入上传的图片（浏览器里选文件用）。 */
  const loadFileTo = async (file: File, target: "hay" | "tpl" | "mask" | "ref") => {
    try {
      const b64 = await fileToBase64(file);
      const url = `data:${file.type || "image/png"};base64,${b64}`;
      if (target === "hay") await applyHaystack(b64, url);
      else if (target === "mask") await applyMask(b64, url, file.name);
      else if (target === "ref") await applyRefImage(b64, url, file.name);
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

  /** 截整个屏幕当参考图（想找的东西就在手边屏幕上时最省事）。 */
  const shootRef = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = await screenPreview();
      await applyRefImage(p.image, `data:image/png;base64,${p.image}`, "屏幕截图");
    } catch (e) {
      setError(`截图失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const addRefBox = useCallback((b: Omit<RefBox, "id">) => {
    setRefBoxes((cur) => [...cur, { ...b, id: refBoxSeq.current++ }]);
    setResult(null);
  }, []);

  const removeRefBox = useCallback((id: number) => {
    setRefBoxes((cur) => cur.filter((b) => b.id !== id));
    setResult(null);
  }, []);

  const onFind = async () => {
    if (kind === "template" && !tpl) {
      setError("还没选模板图。模板 = 你要找的那个小图标/小按钮的截图。");
      return;
    }
    if (kind === "prompt" && promptMode === "visual") {
      if (!refImg) {
        setError("还没选参考图。参考图 = 一张「里面有你要找的东西」的图，随便截一张就行。");
        return;
      }
      if (refBoxes.length === 0) {
        setError("还没圈出要找的东西。在左边的参考图上按住拖一个框，把目标框住。");
        return;
      }
    }
    if (kind === "prompt" && promptMode === "text" && words.trim() === "") {
      setError('还没写提示词。填几个词，逗号分隔，比如「确认按钮,取消」或「person,bus」。');
      return;
    }
    if (kind === "semantic" && !modelEngine) {
      setError(
        models.length === 0
          ? "还没有检测模型。去「设置」页把 .onnx 放进模型目录，或点「添加模型」；YOLOE-26 这类开放词汇模型可用 scripts/fetch_yoloe.py 导出。"
          : "还没选模型。",
      );
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
    if (kind === "template" && mask && tpl && (mask.width !== tpl.width || mask.height !== tpl.height)) {
      setError(
        `掩码尺寸 ${mask.width}×${mask.height} 与模板 ${tpl.width}×${tpl.height} 不一致。掩码必须逐像素对应模板，请换一张同尺寸的掩码。`,
      );
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const parsed = Number.parseFloat(minScore);
      const parsedConf = Number.parseFloat(conf);
      const r =
        kind === "semantic"
          ? await findSemantic({
              source,
              image: source === "image" ? (hay?.b64 ?? undefined) : undefined,
              hwnd: source === "window" ? (hwnd ?? undefined) : undefined,
              engine: modelEngine || undefined,
              conf: Number.isFinite(parsedConf) ? parsedConf : undefined,
              class_filter: parseIdList(classIds),
              max_results: maxResults,
            })
          : kind === "prompt"
            ? await findPrompt({
                source,
                mode: promptMode,
                image: source === "image" ? (hay?.b64 ?? undefined) : undefined,
                hwnd: source === "window" ? (hwnd ?? undefined) : undefined,
                // 图找图：多个框当一个提示（同槽取并集）——用户圈几个样例时
                // 想要的是「这些都是一样的东西」，不是「分别找三样东西」。
                ref_image: promptMode === "visual" ? refImg?.b64 : undefined,
                ref_boxes:
                  promptMode === "visual"
                    ? refBoxes.map((b) => [b.x1, b.y1, b.x2, b.y2] as [number, number, number, number])
                    : undefined,
                words:
                  promptMode === "text"
                    ? words
                        .split(/[,，\n]+/)
                        .map((w) => w.trim())
                        .filter(Boolean)
                    : undefined,
                conf: Number.isFinite(parsedConf) ? parsedConf : undefined,
                max_results: maxResults,
              })
            : await findImage({
                source,
                mode,
                image: source === "image" ? (hay?.b64 ?? undefined) : undefined,
                template: tpl!.b64,
                mask: mask?.b64,
                hwnd: source === "window" ? (hwnd ?? undefined) : undefined,
                max_results: maxResults,
                min_score: Number.isFinite(parsed) ? parsed : undefined,
              });
      setResult(r);
      // 提示找图：搜完自动翻到「搜索范围」那一页，否则用户看不到框画在哪
      if (kind === "prompt" && promptMode === "visual") setLeftTab("hay");
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
      const what =
        kind === "semantic" ? "语义查找" : kind === "prompt" ? "提示查找" : "找图";
      setError(`${what}失败: ${e}`);
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
  /** 掩码与模板尺寸对不上时不能匹配（后端会直接报错，前端先提示） */
  const maskMismatch =
    mask != null && tpl != null && (mask.width !== tpl.width || mask.height !== tpl.height);
  /** 「要找什么」是否已经给全了——不全就不让点，免得白截一次屏 */
  const canRun =
    kind === "template"
      ? !!tpl
      : kind === "semantic"
        ? !!modelEngine
        : promptMode === "visual"
          ? !!refImg && refBoxes.length > 0
          : words.trim() !== "";
  /** 提示找图的图找图模式：左侧卡片显示参考图（要框东西）还是搜索范围（看命中） */
  const showRefPicker = kind === "prompt" && promptMode === "visual";
  /** 高级栏底部那句「当前用 X 在 Y 里找」里的 X */
  const whatLabel =
    kind === "template"
      ? tpl
        ? "模板"
        : "未选模板"
      : kind === "semantic"
        ? modelEngine || "未选模型"
        : promptMode === "visual"
          ? refBoxes.length > 0
            ? `参考图的 ${refBoxes.length} 个框`
            : "未圈目标"
          : words.trim()
            ? `提示词「${words.trim()}」`
            : "未写提示词";

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-3">
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-xl bg-amber-50/80 backdrop-blur-[1px]">
          <div className="flex flex-col items-center gap-2 text-amber-700">
            <Upload className="h-7 w-7" />
            <span className="text-[13px] font-medium">
              {kind === "semantic" || tpl ? "松手作为搜索图" : "松手作为模板图"}
            </span>
          </div>
        </div>
      )}

      {/* 找图方式：模板（比像素）/ 语义（模型认目标）。
          出参完全一样，区别只在「怎么定义要找的东西」。 */}
      <div className="flex flex-wrap items-center gap-2.5">
        <div className="flex items-center gap-0.5 rounded-lg bg-neutral-100 p-0.5">
          {KIND_META.map((m) => (
            <button
              key={m.v}
              type="button"
              onClick={() => {
                setKind(m.v);
                setResult(null);
                setError(null);
                setLeftTab("ref");
              }}
              className={cn(
                "flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12px] leading-none transition-colors",
                kind === m.v
                  ? "bg-white font-medium text-neutral-900 shadow-sm"
                  : "text-neutral-500 hover:text-neutral-700",
              )}
            >
              <KindIcon k={m.v} className="h-3.5 w-3.5" />
              {m.label}
            </button>
          ))}
        </div>
        <span className="flex items-center gap-1 text-[12px] text-neutral-500">
          {KIND_META.find((m) => m.v === kind)?.blurb}
          <Hint>{KIND_HINT[kind]}</Hint>
        </span>
      </div>

      {/* 三步式工具条 */}
      <div className="flex flex-wrap items-end gap-x-3 gap-y-2.5">
        {/* ① 要找什么：模板找图选小图，语义找图选模型 */}
        {kind === "template" ? (
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
                  <span className="font-mono text-[11px] text-neutral-500">
                    {tpl.width}×{tpl.height}
                  </span>
                </span>
              )}
              {tpl && (
                <>
                  <Button
                    size="sm"
                    variant={mask ? "secondary" : "outline"}
                    onClick={() => maskInputRef.current?.click()}
                    title="可选：只让掩码里涂白的区域参与匹配，忽略模板中会变化的部分（如按钮文字）"
                  >
                    <Layers className="h-3.5 w-3.5" />
                    {mask ? "换掩码" : "加掩码"}
                  </Button>
                  <input
                    ref={maskInputRef}
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={(e) => {
                      const f = e.target.files?.[0];
                      if (f) void loadFileTo(f, "mask");
                      e.target.value = "";
                    }}
                  />
                  {mask && (
                    <span
                      className={cn(
                        "flex items-center gap-1.5 rounded-md py-0.5 pl-1 pr-1.5",
                        maskMismatch
                          ? "bg-red-50 ring-1 ring-red-200"
                          : "bg-blue-50 ring-1 ring-blue-200",
                      )}
                      title={
                        maskMismatch
                          ? `掩码尺寸 ${mask.width}×${mask.height} 与模板 ${tpl.width}×${tpl.height} 不一致`
                          : mask.name
                      }
                    >
                      <img
                        src={mask.url}
                        alt="掩码"
                        className="h-6 w-6 rounded border border-neutral-200 bg-white object-contain"
                      />
                      <span
                        className={cn(
                          "font-mono text-[11px]",
                          maskMismatch ? "text-red-600" : "text-blue-600",
                        )}
                      >
                        {mask.width}×{mask.height}
                      </span>
                      <button
                        type="button"
                        onClick={() => setMask(null)}
                        title="移除掩码"
                        className="flex h-4 w-4 items-center justify-center rounded text-neutral-400 hover:bg-white hover:text-neutral-700"
                      >
                        <X className="h-3 w-3" />
                      </button>
                    </span>
                  )}
                </>
              )}
            </div>
          </div>
        ) : kind === "semantic" ? (
          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              ① 用哪个模型找
              <Hint>
                语义找图不用模板——「找什么」由模型决定。模型只认得它见过的类别，
                换目标要换模型（YOLOE-26 这类开放词汇模型可以按提示词重新导出，
                见 scripts/fetch_yoloe.py）。
              </Hint>
            </Label>
            <div className="flex items-center gap-1.5">
              {models.length === 0 ? (
                <span className="flex h-8 items-center gap-1.5 rounded-md border border-dashed border-neutral-300 px-2.5 text-[12px] text-neutral-500">
                  <Boxes className="h-3.5 w-3.5" />
                  没有检测模型 —— 去「设置」页添加 .onnx
                </span>
              ) : (
                <Select value={modelEngine} onValueChange={setModelEngine}>
                  <SelectTrigger className="h-8 w-[190px] text-[12px]">
                    <SelectValue placeholder="选择检测模型" />
                  </SelectTrigger>
                  <SelectContent>
                    {models.map((m) => (
                      <SelectItem key={m.engine} value={m.engine}>
                        <Boxes className="mr-1.5 inline h-3.5 w-3.5" />
                        {m.id}
                        {m.external ? "（外部）" : ""}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              ① 要找什么
              <Hint>{KIND_HINT.prompt}</Hint>
            </Label>
            <div className="flex items-center gap-1.5">
              {/* 图找图 / 语言找图：同一套权重，只是提示从哪来不一样 */}
              <div className="flex items-center gap-0.5 rounded-lg bg-neutral-100 p-0.5">
                {(["visual", "text"] as const).map((m) => (
                  <button
                    key={m}
                    type="button"
                    onClick={() => {
                      setPromptMode(m);
                      setResult(null);
                      setError(null);
                      setLeftTab(m === "visual" ? "ref" : "hay");
                    }}
                    className={cn(
                      "flex h-7 items-center gap-1 rounded-md px-2 text-[12px] leading-none transition-colors",
                      promptMode === m
                        ? "bg-white font-medium text-neutral-900 shadow-sm"
                        : "text-neutral-500 hover:text-neutral-700",
                    )}
                  >
                    {m === "visual" ? (
                      <Frame className="h-3.5 w-3.5" />
                    ) : (
                      <Type className="h-3.5 w-3.5" />
                    )}
                    {PROMPT_MODE_LABEL[m]}
                  </button>
                ))}
              </div>

              {promptMode === "visual" ? (
                <>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => refInputRef.current?.click()}
                  >
                    <Frame className="h-3.5 w-3.5" />
                    {refImg ? "换参考图" : "选参考图"}
                  </Button>
                  <input
                    ref={refInputRef}
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={(e) => {
                      const f = e.target.files?.[0];
                      if (f) void loadFileTo(f, "ref");
                      e.target.value = "";
                    }}
                  />
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void shootRef()}
                    disabled={busy}
                    title="直接截当前屏幕当参考图（想找的东西就在屏幕上时最省事）"
                  >
                    <Monitor className="h-3.5 w-3.5" />
                    截一张
                  </Button>
                  {refImg && (
                    <span
                      className="flex items-center gap-1.5 rounded-md bg-neutral-50 py-0.5 pl-1 pr-2"
                      title={refImg.name}
                    >
                      <img
                        src={refImg.url}
                        alt="参考图"
                        className="h-6 w-6 rounded border border-neutral-200 bg-white object-contain"
                      />
                      <span className="font-mono text-[11px] text-neutral-500">
                        {refImg.width}×{refImg.height}
                        {refBoxes.length > 0 ? ` · ${refBoxes.length} 个框` : ""}
                      </span>
                    </span>
                  )}
                </>
              ) : (
                <>
                  <Input
                    className="h-8 w-[240px] text-[12px]"
                    placeholder="确认按钮, 关闭"
                    value={words}
                    onChange={(e) => {
                      setWords(e.target.value);
                      setResult(null);
                    }}
                  />
                  <span className="text-[12px] text-neutral-400">逗号分隔</span>
                </>
              )}
            </div>
          </div>
        )}

        {/* ② 搜索范围 */}
        <div className="flex flex-col gap-1">
          <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
            ② 在哪里找
            <Hint>{SOURCE_HINT[source]}</Hint>
          </Label>
          <div className="flex items-center gap-1.5">
            <Select value={source} onValueChange={(v) => setSource(v as FindSource)}>
              <SelectTrigger className="h-8 w-[122px] text-[12px]">
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
                  <SelectTrigger className="h-8 w-[180px] text-[12px]">
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
          <Button size="sm" onClick={() => void onFind()} disabled={busy || !canRun}>
            <KindIcon k={kind} className="h-3.5 w-3.5" />
            {busy ? "查找中…" : kind === "semantic" ? "语义查找" : "开始找图"}
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
          {kind === "template" ? (
            <>
              <div className="flex flex-col gap-1">
                <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
                  匹配精度
                  <Hint>
                    精准模式要求相似度 ≥ 0.92，几乎只命中完全一样的像素；
                    容错模式放到 0.72，能容忍轻微压缩/抗锯齿差异，但更容易误命中。
                  </Hint>
                </Label>
                <Select value={mode} onValueChange={(v) => setMode(v as FindMode)}>
                  <SelectTrigger className="h-8 w-[130px] text-[12px]">
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
                  className="h-8 w-[86px] text-[12px]"
                  placeholder="跟随预设"
                  value={minScore}
                  onChange={(e) => setMinScore(e.target.value)}
                />
              </div>
            </>
          ) : (
            <>
              <div className="flex flex-col gap-1">
                <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
                  置信度下限
                  <Hint>
                    模型给每个框的把握，0~1，越大越严格。默认 0.25（与 ultralytics 一致）。
                    找不到就把这个调小，误检多就调大。
                  </Hint>
                </Label>
                <Input
                  className="h-8 w-[86px] text-[12px]"
                  placeholder="0.25"
                  value={conf}
                  onChange={(e) => setConf(e.target.value)}
                />
              </div>

              {kind === "semantic" && (
                <div className="flex flex-col gap-1">
                  <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
                    只保留类别 id
                    <Hint>
                      模型可能认得好几类东西。只想要其中几类时填类别 id，逗号分隔，如
                      「0,2」。留空 = 全都返回。类别 id 看命中结果里每条后面的编号。
                    </Hint>
                  </Label>
                  <Input
                    className="h-8 w-[120px] text-[12px]"
                    placeholder="留空 = 全部"
                    value={classIds}
                    onChange={(e) => setClassIds(e.target.value)}
                  />
                </div>
              )}
            </>
          )}

          <div className="flex flex-col gap-1">
            <Label className="flex items-center gap-1 text-[11px] text-neutral-500">
              最多返回
              <Hint>1 表示只找最像的那一个；找多个相同按钮时调大（上限 64）。</Hint>
            </Label>
            <Input
              className="h-8 w-[70px] text-[12px]"
              type="number"
              min={1}
              max={64}
              value={maxResults}
              onChange={(e) => setMaxResults(Math.max(1, Number(e.target.value) || 1))}
            />
          </div>

          <div className="pb-1 text-[12px] text-neutral-400">
            当前用「{whatLabel}」在「{SOURCE_LABEL[source]}」里找
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
            <div className="flex h-8 items-center gap-2">
              <Crosshair className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              {showRefPicker ? (
                // 图找图要配置的东西有两块（参考图上的框 / 去哪找），分两页免得挤在一起
                <div className="flex items-center gap-0.5 rounded-lg bg-neutral-100 p-0.5">
                  {(
                    [
                      { v: "ref" as const, label: "参考图 · 框住要找的", icon: <Frame className="h-3.5 w-3.5" /> },
                      { v: "hay" as const, label: "搜索范围", icon: <Crosshair className="h-3.5 w-3.5" /> },
                    ]
                  ).map((t) => (
                    <button
                      key={t.v}
                      type="button"
                      onClick={() => setLeftTab(t.v)}
                      className={cn(
                        "flex h-7 items-center gap-1.5 rounded-md px-2.5 text-[12px] leading-none transition-colors",
                        leftTab === t.v
                          ? "bg-white font-medium text-neutral-900 shadow-sm"
                          : "text-neutral-500 hover:text-neutral-700",
                      )}
                    >
                      {t.icon}
                      {t.label}
                    </button>
                  ))}
                </div>
              ) : (
                <CardTitle className="flex items-center gap-2 text-[13px] font-medium text-neutral-500">
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
              )}
              {showRefPicker && leftTab === "ref" && refImg && (
                <span className="ml-auto text-[12px] font-normal text-neutral-400">
                  在图上按住拖出方框 · 已有 {refBoxes.length} 个
                </span>
              )}
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {showRefPicker && leftTab === "ref" ? (
              <RefBoxPicker
                img={refImg}
                boxes={refBoxes}
                onAdd={addRefBox}
                onRemove={removeRefBox}
                onPick={() => refInputRef.current?.click()}
              />
            ) : (
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
                  description={
                    kind === "template"
                      ? "模板是「要找的小图」，搜索范围是「去哪张图里找」。两个都有了才能开始。"
                      : kind === "semantic"
                        ? "语义找图不用模板，但得先确定「去哪张图里找」。选好范围就能开始。"
                        : promptMode === "text"
                          ? "语言找图不用模板，但得先确定「去哪张图里找」。选好范围就能开始。"
                          : "搜索范围是「去哪张图里找」。在左边那页圈好参考图，这里选好范围，就能开始。"
                  }
                  steps={
                    kind === "template"
                      ? [
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
                        ]
                      : kind === "semantic"
                        ? [
                            {
                              icon: <Brain className="h-3.5 w-3.5" />,
                              text: "① 选一个检测模型（决定能找什么）",
                              detail: "模型只认它见过的类别，没有模型先去设置页加",
                            },
                            {
                              icon: <AppWindow className="h-3.5 w-3.5" />,
                              text: "② 选一个窗口，或整个屏幕",
                              detail: "点「语义查找」时会自动截当前画面",
                            },
                            {
                              icon: <Brain className="h-3.5 w-3.5" />,
                              text: "③ 点「语义查找」",
                              detail: "命中的目标会连类别一起列在右边",
                            },
                          ]
                        : [
                            {
                              icon: promptMode === "visual" ? (
                                <Frame className="h-3.5 w-3.5" />
                              ) : (
                                <Type className="h-3.5 w-3.5" />
                              ),
                              text:
                                promptMode === "visual"
                                  ? "① 选参考图，在上面拖框圈住要找的东西"
                                  : "① 写几个提示词，逗号分隔",
                              detail:
                                promptMode === "visual"
                                  ? "去左边「参考图」那页圈；多个框会当成同一个目标"
                                  : "比如「确认按钮, 关闭」",
                            },
                            {
                              icon: <AppWindow className="h-3.5 w-3.5" />,
                              text: "② 选一个窗口，或整个屏幕",
                              detail: "点「开始找图」时会自动截当前画面",
                            },
                            {
                              icon: <Sparkles className="h-3.5 w-3.5" />,
                              text: "③ 点「开始找图」",
                              detail: "换目标只改①②，不用换模型",
                            },
                          ]
                  }
                />
              }
            />
            )}
          </CardContent>
        </Card>

        <Card className="flex min-h-[340px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex h-8 items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[13px] font-medium text-neutral-500">
                <Target className="h-3.5 w-3.5 text-blue-600" />
                命中结果
              </CardTitle>
              {result && (
                <Badge variant={result.found ? "warning" : "secondary"}>
                  {result.found ? `命中 ${result.matched}` : "未找到"} · {result.elapsed_ms} ms
                  {result.engine ? ` · ${result.engine}` : ""}
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
                            <span className="font-mono text-[13px] text-neutral-900">
                              ({h.x}, {h.y})
                            </span>
                            {h.class_name && (
                              <span className="rounded bg-neutral-100 px-1.5 py-0.5 text-[11px] text-neutral-600">
                                {h.class_name}
                                {h.class_id != null ? ` #${h.class_id}` : ""}
                              </span>
                            )}
                            <span className="ml-auto font-mono text-[11px] text-neutral-400">
                              {h.class_id != null ? "置信" : "相似"} {h.score.toFixed(3)}
                            </span>
                          </div>
                          <div className="mt-0.5 font-mono text-[11px] text-neutral-400">
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
                    ? kind === "semantic"
                      ? "模型没在这个画面里认出目标。先把「置信度下限」调小试试；还不行就是模型不认得这类目标，得换模型。"
                      : kind === "prompt"
                        ? promptMode === "visual"
                          ? "没找到跟参考框里那块像的东西。先把「置信度下限」调小试试；还不行就把参考框圈得更贴一点，或者换一张更清楚的参考图。"
                          : "没找到符合这些词的东西。先把「置信度下限」调小试试；还不行就换个说法——用具体的名词，别用「按钮」这种太泛的词。"
                        : "可能是画面里没有这个图，或者相似度不够。切到「容错」再试一次。"
                    : kind === "semantic"
                      ? "模型和搜索范围都选好后，点「语义查找」。"
                      : kind === "prompt"
                        ? promptMode === "visual"
                          ? "参考框和搜索范围都弄好后，点「开始找图」。"
                          : "提示词和搜索范围都写好后，点「开始找图」。"
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

/**
 * 参考图取框器：在图上按住拖，框出「要找的东西长什么样」。
 *
 * 为什么让用户自己拖而不是给个固定九宫格：模型要的是「这块像素长这样」的
 * 视觉提示，框得越贴，提示越干净。让用户自己圈是唯一能表达「我要的是这块、
 * 不是旁边那块」的方式。
 *
 * 多个框 = 同一个目标的多张样例（后端把它们并成一个提示），所以这里不做
 * 「第几号目标」的区分，只标序号方便数数。
 */
function RefBoxPicker({
  img,
  boxes,
  onAdd,
  onRemove,
  onPick,
}: {
  img: Template | null;
  boxes: RefBox[];
  onAdd: (b: Omit<RefBox, "id">) => void;
  onRemove: (id: number) => void;
  onPick: () => void;
}) {
  const imgRef = useRef<HTMLImageElement>(null);
  const [draft, setDraft] = useState<{ x1: number; y1: number; x2: number; y2: number } | null>(
    null,
  );

  if (!img) {
    return (
      <EmptyState
        icon={<Frame className="h-5 w-5" />}
        title="还没有参考图"
        description="参考图 = 一张「里面有你要找的东西」的图。截当前屏幕最快，也可以直接拖一张图进来。"
        steps={[
          { icon: <Monitor className="h-3.5 w-3.5" />, text: "点上方「截一张」抓当前屏幕" },
          { icon: <Frame className="h-3.5 w-3.5" />, text: "在图上按住拖一个框，把目标圈住" },
          { icon: <Sparkles className="h-3.5 w-3.5" />, text: "点「开始找图」", detail: "换目标只改这个框" },
        ]}
        action={
          <Button size="sm" variant="outline" onClick={onPick}>
            <FileImage className="h-3.5 w-3.5" />
            选参考图
          </Button>
        }
      />
    );
  }

  /** 客户端坐标 → 原图像素坐标（图片是等比缩放的，要按显示尺寸换算） */
  const toImage = (e: ReactPointerEvent<HTMLDivElement>) => {
    const el = imgRef.current;
    if (!el) return { x: 0, y: 0 };
    const r = el.getBoundingClientRect();
    const sx = r.width > 0 ? img.width / r.width : 1;
    const sy = r.height > 0 ? img.height / r.height : 1;
    const clamp = (v: number, hi: number) => Math.min(Math.max(v, 0), hi);
    return {
      x: clamp((e.clientX - r.left) * sx, img.width),
      y: clamp((e.clientY - r.top) * sy, img.height),
    };
  };

  const onDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const p = toImage(e);
    setDraft({ x1: p.x, y1: p.y, x2: p.x, y2: p.y });
  };

  const onMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!draft) return;
    const p = toImage(e);
    setDraft((d) => (d ? { ...d, x2: p.x, y2: p.y } : d));
  };

  const onUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    if (draft) {
      const b = {
        x1: Math.min(draft.x1, draft.x2),
        y1: Math.min(draft.y1, draft.y2),
        x2: Math.max(draft.x1, draft.x2),
        y2: Math.max(draft.y1, draft.y2),
      };
      if ((b.x2 - b.x1) * (b.y2 - b.y1) >= MIN_REF_BOX_AREA) onAdd(b);
    }
    setDraft(null);
  };

  const pct = (v: number, total: number) => `${(v / total) * 100}%`;
  const rectStyle = (b: { x1: number; y1: number; x2: number; y2: number }) => ({
    left: pct(Math.min(b.x1, b.x2), img.width),
    top: pct(Math.min(b.y1, b.y2), img.height),
    width: pct(Math.abs(b.x2 - b.x1), img.width),
    height: pct(Math.abs(b.y2 - b.y1), img.height),
  });

  return (
    <div className="flex h-full min-h-[260px] w-full items-center justify-center overflow-hidden rounded-xl bg-neutral-50/70 p-2">
      {/* inline-block 的容器会被图片撑到实际显示尺寸，覆盖层 inset-0 就精确贴着图片 */}
      <div className="relative inline-block max-h-full max-w-full">
        <img
          ref={imgRef}
          src={img.url}
          alt="参考图"
          draggable={false}
          className="max-h-full max-w-full select-none rounded-lg border border-neutral-200 bg-white object-contain"
          style={{ maxHeight: "min(52vh, 420px)" }}
        />
        <div
          className="absolute inset-0 cursor-crosshair touch-none"
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={() => setDraft(null)}
        >
          {boxes.map((b, i) => (
            <div
              key={b.id}
              className="pointer-events-none absolute rounded-[3px] border-2 border-amber-500 bg-amber-400/20"
              style={rectStyle(b)}
            >
              <span className="absolute -left-0.5 -top-0.5 flex h-4 min-w-4 items-center justify-center rounded-br bg-amber-500 px-1 text-[10px] font-semibold leading-none text-white">
                {i + 1}
              </span>
              <button
                type="button"
                title="删掉这个框"
                onPointerDown={(e) => e.stopPropagation()}
                onClick={() => onRemove(b.id)}
                className="pointer-events-auto absolute -right-2 -top-2 flex h-4.5 w-4.5 items-center justify-center rounded-full bg-white text-neutral-500 shadow ring-1 ring-black/10 hover:text-red-600"
                style={{ width: 18, height: 18 }}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          ))}
          {draft && (
            <div
              className="pointer-events-none absolute rounded-[3px] border-2 border-dashed border-blue-500 bg-blue-400/20"
              style={rectStyle(draft)}
            />
          )}
        </div>
      </div>
    </div>
  );
}
