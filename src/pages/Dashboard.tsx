import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  Check,
  Copy,
  Cpu,
  ExternalLink,
  FolderOpen,
  Gauge,
  HardDrive,
  LayoutDashboard,
  Library,
  Play,
  RefreshCw,
  ScanText,
  Server,
  Square,
  Zap,
} from "lucide-react";
import type { PageKey } from "../App";
import { PageHeader } from "../components/ui/page-header";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Hint } from "../components/ui/hint";
import { Callout } from "../components/ui/callout";
import { Label } from "../components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { getHealth, modelsRoot, serverStart, serverStatus, serverStop, setDevice, setLevel } from "../lib/api";
import { isTauri } from "../lib/tauri";
import { cn } from "../lib/utils";
import type { Device, Health, ModelLevel } from "../types";

const LEVEL_LABEL: Record<ModelLevel, string> = {
  tiny: "Tiny · 最快",
  small: "Small · 均衡",
  medium: "Medium · 最准",
};

function fmtUptime(sec: number): string {
  if (sec < 60) return `${sec} 秒`;
  if (sec < 3600) return `${Math.floor(sec / 60)} 分 ${sec % 60} 秒`;
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return `${h} 小时 ${m} 分`;
}

interface Props {
  onNavigate: (page: PageKey) => void;
}

export function Dashboard({ onNavigate }: Props) {
  const [health, setHealth] = useState<Health | null>(null);
  const [running, setRunning] = useState<boolean | null>(null);
  const [root, setRoot] = useState<string>("-");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError("浏览器预览模式连不到后端引擎，请在桌面 App 里操作。");
      return;
    }
    try {
      const [h, s, r] = await Promise.all([getHealth(), serverStatus(), modelsRoot()]);
      setHealth(h);
      setRunning(s);
      setRoot(r);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // 5 秒轮询：运行时长、服务状态都要自己往上走，不能只靠按钮
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

  const copy = async (text: string, tag: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(tag);
    window.setTimeout(() => setCopied(null), 1400);
  };

  const onLevel = async (level: ModelLevel) => {
    setBusy(true);
    try {
      setHealth(await setLevel(level));
      setError(null);
    } catch (e) {
      setError(`切换档位失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const onDevice = async (device: Device) => {
    setBusy(true);
    try {
      setHealth(await setDevice(device));
      setError(null);
    } catch (e) {
      setError(`切换设备失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const onToggleServer = async () => {
    setBusy(true);
    try {
      if (running) {
        await serverStop();
        setRunning(false);
      } else {
        await serverStart();
        setRunning(true);
      }
      setError(null);
    } catch (e) {
      setError(`服务操作失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const onOpenHealth = async () => {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(`http://127.0.0.1:${health?.port ?? 8848}/ocr/health`);
  };

  const ready = health?.status === "ok";
  const port = health?.port ?? 8848;
  const endpoint = `http://127.0.0.1:${port}`;

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        icon={<LayoutDashboard className="h-[18px] w-[18px]" />}
        title="仪表盘"
        description="一眼看清引擎有没有就绪、本地服务开没开。日常只需要看这一页的状态和下面两个开关。"
        actions={
          <Button variant="outline" size="sm" onClick={() => onNavigate("playground")}>
            <ScanText className="h-3.5 w-3.5" />
            去测试台试一下
          </Button>
        }
      />

      {error && (
        <Callout variant="warn" icon={<Activity className="h-4 w-4" />}>
          {error}
        </Callout>
      )}

      {/* 概览：状态在左，关键数字在右 */}
      <Card>
        <CardContent className="flex flex-col gap-5 p-5 pt-5 lg:flex-row lg:items-center">
          <div className="flex items-center gap-3 lg:w-[220px] lg:shrink-0">
            <StatusOrb ok={!!ready} />
            <div>
              <div className="flex items-center gap-1.5 text-[14px] font-semibold text-neutral-900">
                {ready ? "引擎已就绪" : "引擎未加载"}
                <Hint>
                  引擎 = 检测模型 + 识别模型 + 字典。未就绪时识别接口会返回错误，
                  通常是模型文件没下全，或档位切换失败。
                </Hint>
              </div>
              <div className="mt-0.5 text-[12px] text-neutral-500">
                {ready ? `已稳定运行 ${health ? fmtUptime(health.uptime_sec) : "-"}` : "在设置里切换一次档位即可重新加载"}
              </div>
            </div>
          </div>

          <div className="grid flex-1 grid-cols-2 gap-2.5 lg:grid-cols-4">
            <Stat
              icon={<Gauge className="h-3.5 w-3.5" />}
              label="模型档位"
              value={health ? LEVEL_LABEL[health.model].split(" · ")[0] : "-"}
              hint={health ? LEVEL_LABEL[health.model].split(" · ")[1] : ""}
            />
            <Stat
              icon={<Cpu className="h-3.5 w-3.5" />}
              label="推理设备"
              // 值只放「GPU / CPU」，全长写在 hint 里：卡片窄，DirectML 会被截断
              value={health?.actual_device === "directml" ? "GPU" : (health?.actual_device ?? "-")}
              hint={
                health
                  ? health.actual_device === "directml"
                    ? "DirectML"
                    : `请求 ${health.device}`
                  : ""
              }
              tip={
                <>
                  DirectML 走显卡，通常比 CPU 快数倍。若实际设备显示为 CPU 而你选了 GPU，
                  多半是缺 ONNX Runtime 的 DirectML 动态库（runtime/*.dll）。
                </>
              }
            />
            <Stat
              icon={<HardDrive className="h-3.5 w-3.5" />}
              label="识别字典"
              value={health ? String(health.dict_size) : "-"}
              hint={health?.cls_loaded ? "含方向分类" : "无方向分类"}
              tip="字典里是模型能认出的全部字符。中文模型约 6600+，含方向分类时会更大。"
            />
            <Stat
              icon={<Zap className="h-3.5 w-3.5" />}
              label="服务端口"
              value={String(port)}
              hint={running ? "服务运行中" : "服务已停止"}
            />
          </div>
        </CardContent>
      </Card>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        {/* 模型与设备 */}
        <Card>
          <CardHeader>
            {/* h-8：卡片头第一行统一高度，标题才不会因为右侧有没有按钮而上下错位 */}
            <div className="flex h-8 items-center">
              <CardTitle className="flex items-center gap-2">
                <RefreshCw className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                模型与设备
              </CardTitle>
            </div>
            <CardDescription>
              换档位或换设备都会重新加载模型，需要几秒钟，期间识别请求会等待。
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                模型档位
                <Hint>
                  Tiny 快但准头一般，适合简单界面；Small 是默认的均衡选择；
                  Medium 最准也最慢。换完不放心就回测试台跑一张图看看。
                </Hint>
              </Label>
              <Select
                value={health?.model ?? "small"}
                onValueChange={(v) => void onLevel(v as ModelLevel)}
                disabled={busy || !isTauri()}
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="tiny">Tiny（最快）</SelectItem>
                  <SelectItem value="small">Small（均衡，推荐）</SelectItem>
                  <SelectItem value="medium">Medium（最准）</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                推理设备
                <Hint>
                  有独立显卡就选 DirectML；没有独显、或识别时报显存/驱动错误，选 CPU。
                </Hint>
              </Label>
              <Select
                value={health?.device ?? "directml"}
                onValueChange={(v) => void onDevice(v as Device)}
                disabled={busy || !isTauri()}
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="directml">DirectML（GPU，推荐）</SelectItem>
                  <SelectItem value="cpu">CPU（兼容性最好）</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex items-center gap-2 pt-0.5">
              <Button variant="outline" size="sm" onClick={() => void refresh()} disabled={busy}>
                <RefreshCw className={cn("h-3.5 w-3.5", busy && "animate-spin")} />
                刷新状态
              </Button>
              {busy && <span className="text-[12px] text-neutral-400">正在应用…</span>}
            </div>
          </CardContent>
        </Card>

        {/* 本地 HTTP 服务 */}
        <Card>
          <CardHeader>
            {/* 徽章与标题同处一条 h-8 的行：原来用 items-start，徽章会比标题低 7px */}
            <div className="flex h-8 items-center justify-between gap-2">
              <CardTitle className="flex items-center gap-2">
                <Server className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                本地 HTTP 服务
              </CardTitle>
              <Badge variant={running ? "success" : "secondary"} className="shrink-0 whitespace-nowrap">
                {running ? "运行中" : "已停止"}
              </Badge>
            </div>
            <CardDescription>
              外部脚本（易语言、按键精灵、Python）通过下面这个地址调识别能力。
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="flex items-center gap-2 rounded-lg bg-neutral-50 px-3 py-2">
              <code className="selectable min-w-0 flex-1 truncate font-mono text-[12px] text-neutral-700">
                {endpoint}
              </code>
              <button
                type="button"
                onClick={() => void copy(endpoint, "url")}
                title="复制地址"
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-neutral-400 hover:bg-white hover:text-neutral-700"
              >
                {copied === "url" ? (
                  <Check className="h-3.5 w-3.5 text-emerald-600" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
              </button>
            </div>

            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={() => void onToggleServer()} disabled={busy || !isTauri()}>
                {running ? <Square className="h-4 w-4" /> : <Play className="h-4 w-4" />}
                {running ? "停止服务" : "启动服务"}
              </Button>
              <Button variant="outline" onClick={() => void onOpenHealth()} disabled={!running}>
                <ExternalLink className="h-3.5 w-3.5" />
                打开健康检查
              </Button>
            </div>

            <p className="text-[11.5px] leading-5 text-neutral-400">
              服务只监听本机 127.0.0.1，不对外网开放。改端口去「设置」。
            </p>
          </CardContent>
        </Card>
      </div>

      {/* 模型目录 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <Library className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              模型目录
            </CardTitle>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void copy(root, "root")}
              disabled={root === "-"}
            >
              {copied === "root" ? (
                <Check className="h-3.5 w-3.5 text-emerald-600" />
              ) : (
                <FolderOpen className="h-3.5 w-3.5" />
              )}
              {copied === "root" ? "已复制" : "复制路径"}
            </Button>
          </div>
          <CardDescription>
            模型文件放这里。缺模型时执行 <code className="font-mono">scripts/fetch_models.ps1</code> 自动下载。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          <code className="selectable block break-all rounded-lg bg-neutral-50 px-3 py-2 font-mono text-[11.5px] text-neutral-600">
            {root}
          </code>
          <p className="text-[11.5px] text-neutral-400">
            环境变量 <code className="font-mono">OCR_MODELS_ROOT</code> 可覆盖，适合把模型放到别的盘。
          </p>
        </CardContent>
      </Card>
    </div>
  );
}

function StatusOrb({ ok }: { ok: boolean }) {
  return (
    <span className="relative flex h-9 w-9 shrink-0 items-center justify-center">
      <span
        className={cn(
          "absolute h-9 w-9 rounded-full",
          ok ? "bg-emerald-100" : "bg-neutral-100",
        )}
      />
      <span
        className={cn(
          "relative h-3 w-3 rounded-full",
          ok ? "bg-emerald-500" : "bg-neutral-300",
        )}
      />
      {ok && (
        <span className="absolute h-3 w-3 animate-ping rounded-full bg-emerald-400 opacity-60" />
      )}
    </span>
  );
}

function Stat({
  icon,
  label,
  value,
  hint,
  tip,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  hint?: string;
  tip?: React.ReactNode;
}) {
  return (
    <div className="rounded-lg bg-neutral-50/80 px-3 py-2.5">
      <div className="flex items-center gap-1.5 text-[11.5px] text-neutral-500">
        <span className="text-neutral-400">{icon}</span>
        {label}
        {tip && <Hint>{tip}</Hint>}
      </div>
      <div className="mt-1 truncate text-[15px] font-semibold text-neutral-900" title={value}>
        {value}
      </div>
      {hint && <div className="mt-0.5 truncate text-[11px] text-neutral-400">{hint}</div>}
    </div>
  );
}
