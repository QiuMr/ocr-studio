import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  Cpu,
  FolderTree,
  Gauge,
  HardDrive,
  Play,
  RefreshCw,
  Square,
  Zap,
} from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Label } from "../components/ui/label";
import { Separator } from "../components/ui/separator";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { getHealth, modelsRoot, serverStart, serverStatus, serverStop, setDevice, setLevel } from "../lib/api";
import { isTauri } from "../lib/tauri";
import type { Device, Health, ModelLevel } from "../types";

const LEVEL_LABEL: Record<ModelLevel, string> = {
  tiny: "Tiny（最快）",
  small: "Small（均衡）",
  medium: "Medium（最准）",
};

function fmtUptime(sec: number): string {
  if (sec < 60) return `${sec} 秒`;
  if (sec < 3600) return `${Math.floor(sec / 60)} 分 ${sec % 60} 秒`;
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return `${h} 小时 ${m} 分`;
}

export function Dashboard() {
  const [health, setHealth] = useState<Health | null>(null);
  const [running, setRunning] = useState<boolean | null>(null);
  const [root, setRoot] = useState<string>("-");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauri()) {
      setError("浏览器预览模式：仅 Playground 可用（走 HTTP 降级）");
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
    const t = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(t);
  }, [refresh]);

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

  const onOpenDocs = async () => {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(`http://127.0.0.1:${health?.port ?? 8848}/ocr/health`);
  };

  const ready = health?.status === "ok";

  return (
    <div className="flex flex-col gap-5">
      {error && (
        <div className="rounded-md border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
          {error}
        </div>
      )}

      {/* 引擎状态 */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Activity className="h-4 w-4 text-blue-600" />
                引擎状态
              </CardTitle>
              <CardDescription>模型加载、推理设备与字典规模</CardDescription>
            </div>
            <Badge variant={ready ? "success" : "warning"}>
              {ready ? "运行中" : "未加载"}
            </Badge>
          </div>
        </CardHeader>
        <CardContent className="grid grid-cols-2 gap-4 lg:grid-cols-4">
          <Stat
            icon={<Gauge className="h-4 w-4 text-blue-600" />}
            label="模型档位"
            value={health ? LEVEL_LABEL[health.model].split("（")[0] : "-"}
            hint={health?.model ?? ""}
          />
          <Stat
            icon={<Cpu className="h-4 w-4 text-blue-600" />}
            label="实际设备"
            value={health?.actual_device === "directml" ? "DirectML (GPU)" : (health?.actual_device ?? "-")}
            hint={health ? `请求: ${health.device}` : ""}
          />
          <Stat
            icon={<HardDrive className="h-4 w-4 text-blue-600" />}
            label="字典大小"
            value={health ? String(health.dict_size) : "-"}
            hint={health?.cls_loaded ? "含方向分类" : "无方向分类"}
          />
          <Stat
            icon={<Zap className="h-4 w-4 text-blue-600" />}
            label="运行时长"
            value={health ? fmtUptime(health.uptime_sec) : "-"}
            hint={`端口 ${health?.port ?? "-"}`}
          />
        </CardContent>
      </Card>

      {/* 模型与设备切换 */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <RefreshCw className="h-4 w-4 text-blue-600" />
            模型与设备
          </CardTitle>
          <CardDescription>切换会重载引擎，约需数秒</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap items-end gap-6">
          <div className="flex flex-col gap-2">
            <Label>模型档位</Label>
            <Select
              value={health?.model ?? "small"}
              onValueChange={(v) => void onLevel(v as ModelLevel)}
              disabled={busy}
            >
              <SelectTrigger className="w-52">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="tiny">Tiny（最快）</SelectItem>
                <SelectItem value="small">Small（均衡）</SelectItem>
                <SelectItem value="medium">Medium（最准）</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="flex flex-col gap-2">
            <Label>推理设备</Label>
            <Select
              value={health?.device ?? "directml"}
              onValueChange={(v) => void onDevice(v as Device)}
              disabled={busy}
            >
              <SelectTrigger className="w-52">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="directml">DirectML（GPU）</SelectItem>
                <SelectItem value="cpu">CPU</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <Button variant="outline" onClick={() => void refresh()} disabled={busy}>
            <RefreshCw className={`h-4 w-4 ${busy ? "animate-spin" : ""}`} />
            刷新状态
          </Button>
        </CardContent>
      </Card>

      {/* HTTP 服务 */}
      <Card>
        <CardHeader>
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Play className="h-4 w-4 text-blue-600" />
                本地 HTTP 服务
              </CardTitle>
              <CardDescription>
                外部脚本走 /ocr/* 接口，地址 http://127.0.0.1:{health?.port ?? 8848}
              </CardDescription>
            </div>
            <Badge variant={running ? "success" : "secondary"}>{running ? "已启动" : "已停止"}</Badge>
          </div>
        </CardHeader>
        <CardContent className="flex flex-wrap items-center gap-3">
          <Button onClick={() => void onToggleServer()} disabled={busy}>
            {running ? <Square className="h-4 w-4" /> : <Play className="h-4 w-4" />}
            {running ? "停止服务" : "启动服务"}
          </Button>
          <Button variant="outline" onClick={() => void onOpenDocs()} disabled={!running}>
            <Activity className="h-4 w-4" />
            打开 /ocr/health
          </Button>
        </CardContent>
      </Card>

      {/* 模型目录 */}
      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <FolderTree className="h-4 w-4 text-blue-600" />
            模型目录
          </CardTitle>
          <CardDescription>缺失模型时请运行 scripts/fetch_models.ps1</CardDescription>
        </CardHeader>
        <CardContent>
          <code className="selectable block rounded-md bg-neutral-50 px-3 py-2 font-mono text-xs text-neutral-700">
            {root}
          </code>
          <Separator className="my-4" />
          <p className="text-xs text-neutral-500">
            环境变量 <code className="font-mono">OCR_MODELS_ROOT</code> 可覆盖模型根目录。
          </p>
        </CardContent>
      </Card>
    </div>
  );
}

function Stat({
  icon,
  label,
  value,
  hint,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  hint?: string;
}) {
  return (
    <div className="rounded-lg border border-neutral-200 bg-white p-4">
      <div className="flex items-center gap-2 text-xs text-neutral-500">
        {icon}
        {label}
      </div>
      <div className="mt-2 text-lg font-semibold text-neutral-900">{value}</div>
      {hint && <div className="mt-0.5 text-xs text-neutral-400">{hint}</div>}
    </div>
  );
}
