import { useCallback, useEffect, useRef, useState } from "react";
import {
  Braces,
  Clipboard,
  Copy,
  Crosshair,
  Eye,
  EyeOff,
  FileImage,
  List,
  ScanText,
  Type,
} from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Label } from "../components/ui/label";
import { Separator } from "../components/ui/separator";
import { Switch } from "../components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../components/ui/tabs";
import { ScrollArea } from "../components/ui/scroll-area";
import { ImageViewer } from "../components/ImageViewer";
import { FindPanel } from "../components/FindPanel";
import { fileToBase64, imageSize, recognize } from "../lib/api";
import type { LineResult, RecognizeResult } from "../types";

export function Playground() {
  const [b64, setB64] = useState<string | null>(null);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <Tabs defaultValue="ocr" className="flex h-full min-h-0 flex-col gap-4">
        <TabsList className="self-start">
          <TabsTrigger value="ocr">
            <ScanText />
            文字识别
          </TabsTrigger>
          <TabsTrigger value="find">
            <Crosshair />
            找图
          </TabsTrigger>
        </TabsList>
        <TabsContent value="ocr" className="min-h-0 flex-1">
          <OcrPanel onImageLoaded={setB64} />
        </TabsContent>
        <TabsContent value="find" className="min-h-0 flex-1">
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
  const [size, setSize] = useState({ width: 0, height: 0 });
  const [result, setResult] = useState<RecognizeResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showBoxes, setShowBoxes] = useState(true);
  const [useCls, setUseCls] = useState(true);
  const b64Ref = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadFile = useCallback(
    async (file: File) => {
      try {
        const b64 = await fileToBase64(file);
        const url = URL.createObjectURL(file);
        const dim = await imageSize(url);
        b64Ref.current = b64;
        setSrc(url);
        setSize(dim);
        setResult(null);
        setError(null);
        onImageLoaded(b64);
      } catch (e) {
        setError(`加载图片失败: ${e}`);
      }
    },
    [onImageLoaded],
  );

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
      setResult(await recognize(b64Ref.current, useCls));
    } catch (e) {
      setError(`识别失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const copyJson = async () => {
    if (!result) return;
    await navigator.clipboard.writeText(JSON.stringify(result.lines, null, 2));
  };

  const lines: LineResult[] = result?.lines ?? [];

  return (
    <div className="flex h-full flex-col gap-4">
      {/* 工具栏 */}
      <div className="flex flex-wrap items-center gap-3">
        <Button onClick={() => inputRef.current?.click()}>
          <FileImage className="h-4 w-4" />
          打开图片
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
        <Button onClick={() => void onRecognize()} disabled={busy || !src}>
          <ScanText className="h-4 w-4" />
          {busy ? "识别中…" : "开始识别"}
        </Button>

        <Separator orientation="vertical" className="mx-1 h-8" />

        <div className="flex items-center gap-2">
          <Switch id="show-boxes" checked={showBoxes} onCheckedChange={setShowBoxes} />
          <Label htmlFor="show-boxes" className="flex cursor-pointer items-center gap-1.5">
            {showBoxes ? <Eye className="h-3.5 w-3.5" /> : <EyeOff className="h-3.5 w-3.5" />}
            显示检测框
          </Label>
        </div>
        <div className="flex items-center gap-2">
          <Switch id="use-cls" checked={useCls} onCheckedChange={setUseCls} />
          <Label htmlFor="use-cls" className="cursor-pointer">
            方向分类（cls）
          </Label>
        </div>

        <div className="ml-auto flex items-center gap-2">
          {result && (
            <Badge variant="secondary">
              {result.lines.length} 行 · {result.elapsed_ms} ms
            </Badge>
          )}
          <Badge variant="outline">
            <Clipboard className="h-3 w-3" />
            支持 Ctrl+V
          </Badge>
        </div>
      </div>

      {error && (
        <div className="rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      )}

      {/* 主体：左图右结果 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-4 lg:grid-cols-[3fr_2fr]">
        <Card className="flex min-h-[420px] flex-col overflow-hidden">
          <CardHeader className="pb-3">
            <CardTitle className="flex items-center gap-2 text-sm">
              <FileImage className="h-4 w-4 text-blue-600" />
              原图
              {size.width > 0 && (
                <span className="font-normal text-neutral-400">
                  {size.width} × {size.height}
                </span>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            <ImageViewer
              src={src}
              width={size.width}
              height={size.height}
              lines={lines}
              showBoxes={showBoxes}
            />
          </CardContent>
        </Card>

        <Card className="flex min-h-[420px] flex-col overflow-hidden">
          <CardHeader className="pb-3">
            <div className="flex items-center justify-between">
              <CardTitle className="flex items-center gap-2 text-sm">
                <ScanText className="h-4 w-4 text-blue-600" />
                识别结果
              </CardTitle>
              <Button variant="ghost" size="sm" onClick={() => void copyJson()} disabled={!result}>
                <Copy className="h-3.5 w-3.5" />
                复制 JSON
              </Button>
            </div>
          </CardHeader>
          <CardContent className="min-h-0 flex-1">
            <Tabs defaultValue="text" className="flex h-full flex-col">
              <TabsList className="self-start">
                <TabsTrigger value="text">
                  <Type />
                  文本
                </TabsTrigger>
                <TabsTrigger value="lines">
                  <List />
                  逐行
                </TabsTrigger>
                <TabsTrigger value="json">
                  <Braces />
                  JSON
                </TabsTrigger>
              </TabsList>

              <TabsContent value="text" className="min-h-0 flex-1">
                <ScrollArea className="h-full rounded-md border border-neutral-200 bg-neutral-50/60 p-3">
                  <pre className="selectable whitespace-pre-wrap font-sans text-sm leading-7 text-neutral-800">
                    {result?.text || "（尚未识别）"}
                  </pre>
                </ScrollArea>
              </TabsContent>

              <TabsContent value="lines" className="min-h-0 flex-1">
                <ScrollArea className="h-full rounded-md border border-neutral-200">
                  <ul className="divide-y divide-neutral-100">
                    {lines.map((l, i) => (
                      <li key={i} className="flex items-start gap-3 px-3 py-2.5">
                        <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded bg-blue-600 text-[11px] font-semibold text-white">
                          {i + 1}
                        </span>
                        <div className="min-w-0 flex-1">
                          <div className="truncate text-sm text-neutral-900">{l.text}</div>
                          <div className="mt-0.5 font-mono text-[11px] text-neutral-400">
                            score {l.score.toFixed(3)} · det {l.det_score.toFixed(3)}
                            {l.rotated ? " · 已翻转" : ""}
                          </div>
                        </div>
                      </li>
                    ))}
                    {lines.length === 0 && (
                      <li className="px-3 py-8 text-center text-sm text-neutral-400">
                        （尚未识别）
                      </li>
                    )}
                  </ul>
                </ScrollArea>
              </TabsContent>

              <TabsContent value="json" className="min-h-0 flex-1">
                <ScrollArea className="h-full rounded-md border border-neutral-200 bg-neutral-50/60 p-3">
                  <pre className="selectable font-mono text-[11px] leading-5 text-neutral-700">
                    {result ? JSON.stringify(result.lines, null, 2) : "（尚未识别）"}
                  </pre>
                </ScrollArea>
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
