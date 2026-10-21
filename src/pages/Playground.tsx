import { useCallback, useEffect, useRef, useState } from "react";
import {
  Braces,
  Clipboard,
  Copy,
  Crosshair,
  Eye,
  EyeOff,
  FileImage,
  GripHorizontal,
  Hand,
  Keyboard,
  List,
  ScanText,
  Sparkles,
  Type,
  Upload,
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
import { PageHeader } from "../components/ui/page-header";
import { EmptyState } from "../components/ui/empty-state";
import { Callout } from "../components/ui/callout";
import { Hint } from "../components/ui/hint";
import { ImageViewer } from "../components/ImageViewer";
import { FindPanel } from "../components/FindPanel";
import { fileToBase64, imageSize, listEngines, recognize, recognizeChars } from "../lib/api";
import { useImageDrop, type DroppedImage } from "../lib/use-file-drop";
import type { DanziResult, EngineItem, LineResult, RecognizeResult } from "../types";

/** 引擎在下拉里的显示名。名字本身就要告诉用户「什么时候该选它」。 */
function engineLabel(e: EngineItem): string {
  if (e.family === "ddddocr") return `DDDDOCR（${e.variant ?? "common"}·验证码快）`;
  if (e.family === "yolo") return `YOLO ${e.variant ?? ""}（目标检测）`;
  return e.name;
}

export function Playground() {
  // 识别页载入的图会传下来，切到找图页可以直接当搜索图
  const [b64, setB64] = useState<string | null>(null);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <PageHeader
        icon={<ScanText className="h-[18px] w-[18px]" />}
        title="测试台"
        description="不用写代码就能试能力。先把图放进来，再点按钮；识别框会直接画在图上。"
      />

      <Tabs defaultValue="ocr" className="flex min-h-0 flex-1 flex-col gap-3">
        <TabsList className="self-start">
          <TabsTrigger value="ocr">
            <Type />
            文字识别
          </TabsTrigger>
          <TabsTrigger value="find">
            <Crosshair />
            找图定位
          </TabsTrigger>
        </TabsList>
        <TabsContent value="ocr" className="mt-0 flex min-h-0 flex-1 flex-col">
          <OcrPanel onImageLoaded={setB64} />
        </TabsContent>
        <TabsContent value="find" className="mt-0 flex min-h-0 flex-1 flex-col">
          <FindPanel inheritedImage={b64} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

interface OcrPanelProps {
  /** 把载入的图同步出去，切到找图页时可直接复用 */
  onImageLoaded: (b64: string) => void;
}

function OcrPanel({ onImageLoaded }: OcrPanelProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [name, setName] = useState<string>("");
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [result, setResult] = useState<RecognizeResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showBoxes, setShowBoxes] = useState(true);
  const [useCls, setUseCls] = useState(true);
  /** 空字符串 = 用当前 PP-OCR 配置（历史行为）；其它值为 `family:variant` */
  const [engine, setEngine] = useState("");
  const [engines, setEngines] = useState<EngineItem[]>([]);
  const [copied, setCopied] = useState(false);
  const [tab, setTab] = useState("text");
  /** 逐字结果：按需加载，切到「逐字」tab 时才请求 */
  const [detail, setDetail] = useState<DanziResult | null>(null);
  const [charBusy, setCharBusy] = useState(false);
  const b64Ref = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  /** 所有入口（选文件 / 拖入 / 粘贴）最终都走这里，状态更新只写一遍。 */
  const applyImage = useCallback(
    async (b64: string, url: string, dim: { width: number; height: number }, label: string) => {
      b64Ref.current = b64;
      setSrc(url);
      setSize(dim);
      setName(label);
      setResult(null);
      setDetail(null);
      setTab("text");
      setError(null);
      onImageLoaded(b64);
    },
    [onImageLoaded],
  );

  const loadFile = useCallback(
    async (file: File) => {
      try {
        const b64 = await fileToBase64(file);
        const url = `data:${file.type || "image/png"};base64,${b64}`;
        const dim = await imageSize(url);
        await applyImage(b64, url, dim, file.name);
      } catch (e) {
        setError(`加载图片失败: ${e}`);
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

  const onRecognize = async () => {
    if (!b64Ref.current) {
      setError("请先载入图片");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setResult(await recognize(b64Ref.current, useCls, engine));
      setDetail(null); // 档位/参数可能变了，旧的逐字结果作废
      setTab("text");
    } catch (e) {
      setError(`识别失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  /** 懒加载逐字结果：只有真的切到「逐字」tab 才多跑一次推理。 */
  const loadChars = async () => {
    if (!b64Ref.current || detail) return;
    setCharBusy(true);
    setError(null);
    try {
      setDetail(await recognizeChars(b64Ref.current, useCls, engine));
    } catch (e) {
      setError(`逐字定位失败: ${e}`);
    } finally {
      setCharBusy(false);
    }
  };

  const onTabChange = (v: string) => {
    setTab(v);
    if (v === "chars") void loadChars();
  };

  const copyJson = async () => {
    if (!result) return;
    await navigator.clipboard.writeText(JSON.stringify(result.lines, null, 2));
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  };

  const lines: LineResult[] = result?.lines ?? [];

  return (
    <div className="relative flex min-h-0 flex-1 flex-col gap-3">
      {/* 拖拽遮罩 */}
      {dragging && (
        <div className="pointer-events-none absolute inset-0 z-20 flex items-center justify-center rounded-xl bg-blue-50/80 backdrop-blur-[1px]">
          <div className="flex flex-col items-center gap-2 text-blue-700">
            <Upload className="h-7 w-7" />
            <span className="text-[13px] font-medium">松手即可载入这张图</span>
          </div>
        </div>
      )}

      {/* 工具栏 */}
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" onClick={() => inputRef.current?.click()}>
          <FileImage className="h-3.5 w-3.5" />
          {src ? "换一张" : "打开图片"}
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

        <Button size="sm" onClick={() => void onRecognize()} disabled={busy || !src}>
          <ScanText className="h-3.5 w-3.5" />
          {busy ? "识别中…" : "开始识别"}
        </Button>

        {/* 引擎选择：只在确实有第二个可用引擎时出现，避免给小白多一个要理解的概念 */}
        {engines.length > 0 && (
          <div className="flex items-center gap-1">
            <Select
              value={engine || "default"}
              onValueChange={(v) => {
                setEngine(v === "default" ? "" : v);
                // 换引擎后旧结果一定作废，别让用户拿 A 的框对着 B 的图
                setResult(null);
                setDetail(null);
              }}
            >
              <SelectTrigger className="h-8 w-[190px] text-[12.5px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="default">PP-OCR（默认）</SelectItem>
                {engines.map((e) => (
                  <SelectItem key={e.name} value={e.name}>
                    {engineLabel(e)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Hint>
              不同引擎擅长的东西不一样：PP-OCR 适合成段文字（截图、扫描件），
              DDDDOCR 训练在验证码上，短英文数字更快。切换后要重新点「开始识别」。
            </Hint>
          </div>
        )}

        <div className="mx-1 h-5 w-px bg-neutral-200" />

        <div className="flex items-center gap-1.5">
          <Switch id="show-boxes" checked={showBoxes} onCheckedChange={setShowBoxes} />
          <Label
            htmlFor="show-boxes"
            className="flex cursor-pointer items-center gap-1 text-[12.5px] font-normal text-neutral-600"
          >
            {showBoxes ? <Eye className="h-3.5 w-3.5" /> : <EyeOff className="h-3.5 w-3.5" />}
            显示检测框
          </Label>
        </div>

        <div className="flex items-center gap-1">
          <Switch id="use-cls" checked={useCls} onCheckedChange={setUseCls} />
          <Label
            htmlFor="use-cls"
            className="cursor-pointer text-[12.5px] font-normal text-neutral-600"
          >
            方向分类
          </Label>
          <Hint>
            开启后会把倒置 180° 的文字自动翻正再识别。截图、扫描件里常出现倒着的字，
            建议保持开启；纯正方向图片关掉能略快一点。
          </Hint>
        </div>

        <div className="ml-auto flex items-center gap-2">
          {result && (
            <Badge variant="secondary">
              {result.lines.length} 行 · {result.elapsed_ms} ms
            </Badge>
          )}
          <span className="hidden items-center gap-1 text-[11.5px] text-neutral-400 sm:flex">
            <Keyboard className="h-3 w-3" />
            Ctrl+V 粘贴
          </span>
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
            <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium text-neutral-500">
              <FileImage className="h-3.5 w-3.5 text-blue-600" />
              {src ? "原图" : "待识别的图片"}
              {size.width > 0 && (
                <span className="font-normal text-neutral-400">
                  {name && <span className="mr-1.5 text-neutral-500">{name}</span>}
                  {size.width} × {size.height}
                </span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            {src ? (
              <ImageViewer
                src={src}
                width={size.width}
                height={size.height}
                lines={lines}
                showBoxes={showBoxes}
                chars={tab === "chars" ? detail?.lines.flatMap((l) => l.chars) : undefined}
              />
            ) : (
              <EmptyState
                icon={<Hand className="h-5 w-5" />}
                title="先把一张图放进来"
                description="三种方式随你挑，放进来之后点上面的「开始识别」。"
                steps={[
                  {
                    icon: <Upload className="h-3.5 w-3.5" />,
                    text: "拖一张图片到这个区域",
                    detail: "从文件夹直接拖进来就行",
                  },
                  {
                    icon: <Clipboard className="h-3.5 w-3.5" />,
                    text: "或者按 Ctrl+V 粘贴截图",
                    detail: "微信/QQ 截图、Win+Shift+S 都可以",
                  },
                  {
                    icon: <FileImage className="h-3.5 w-3.5" />,
                    text: "也可以点左上角「打开图片」",
                  },
                ]}
                action={
                  <Button variant="outline" size="sm" onClick={() => inputRef.current?.click()}>
                    <FileImage className="h-3.5 w-3.5" />
                    选择文件
                  </Button>
                }
              />
            )}
          </CardContent>
        </Card>

        <Card className="flex min-h-[360px] flex-col overflow-hidden">
          <CardHeader className="pb-2">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-[12.5px] font-medium text-neutral-500">
                <ScanText className="h-3.5 w-3.5 text-blue-600" />
                {result ? "识别结果" : "识别结果会显示在这里"}
              </CardTitle>
              <Button variant="ghost" size="sm" onClick={() => void copyJson()} disabled={!result}>
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
            {result ? (
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
                          <span className="mt-0.5 flex h-4.5 w-4.5 shrink-0 items-center justify-center rounded bg-blue-600 text-[10px] font-semibold text-white" style={{ width: 18, height: 18 }}>
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
                          共 {detail.char_count} 字。图中青色细框就是一个字的范围
                          —— 它是按识别模型的帧对齐推算出来的，字距不均时会有几像素偏差。
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
            ) : (
              <EmptyState
                className="bg-neutral-50/40"
                icon={<Sparkles className="h-5 w-5" />}
                title="还没有识别结果"
                description="载入图片后点「开始识别」，这里会出现三种视图：整段文本、逐行明细、原始 JSON。"
              />
            )}
          </CardContent>
        </Card>
      </div>

      {/* 底部提示：为什么框和字对不上之类的小白困惑 */}
      {result && result.lines.length > 0 && showBoxes && (
        <div className="flex items-center gap-1.5 text-[11.5px] text-neutral-400">
          <GripHorizontal className="h-3 w-3" />
          图上的蓝框就是每一行的位置，编号与「逐行」列表一一对应。
        </div>
      )}
    </div>
  );
}
