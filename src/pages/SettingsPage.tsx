import { useEffect, useState } from "react";
import {
  Boxes,
  Check,
  FolderOpen,
  Info,
  Plus,
  RotateCcw,
  Save,
  ScanSearch,
  Server,
  Settings as SettingsIcon,
  Shield,
  Trash2,
} from "lucide-react";
import { PageHeader } from "../components/ui/page-header";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Callout } from "../components/ui/callout";
import { Hint } from "../components/ui/hint";
import { Label } from "../components/ui/label";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import {
  detectModels,
  getConfig,
  listEngines,
  modelsRoot,
  pickDetectModel,
  removeDetectModel,
  saveConfig,
} from "../lib/api";
import { isTauri } from "../lib/tauri";
import { cn } from "../lib/utils";
import {
  DEFAULT_CONFIG,
  type AppConfig,
  type DetectModel,
  type Device,
  type EngineItem,
  type EngineList,
  type ModelLevel,
} from "../types";

export function SettingsPage() {
  const [cfg, setCfg] = useState<AppConfig>(DEFAULT_CONFIG);
  /** 磁盘上的值，用来判断「有没有改动」 */
  const [initial, setInitial] = useState<AppConfig | null>(null);
  const [root, setRoot] = useState("-");
  const [engines, setEngines] = useState<EngineList | null>(null);
  const [models, setModels] = useState<DetectModel[]>([]);
  const [picking, setPicking] = useState(false);
  const [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      try {
        const c = await getConfig();
        setCfg(c);
        setInitial(c);
        setRoot(await modelsRoot());
        setEngines(await listEngines());
        setModels(await detectModels());
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

  /** 弹系统文件选择框；取消时后端原样返回列表，不用特殊处理 */
  const onAddModel = async () => {
    setError(null);
    setPicking(true);
    try {
      setModels(await pickDetectModel());
      // 引擎清单里的 yolo 条目也要跟着变（新模型的类名/可用性）
      setEngines(await listEngines());
    } catch (e) {
      setError(`添加模型失败：${e}`);
    } finally {
      setPicking(false);
    }
  };

  const onRemoveModel = async (path: string) => {
    setError(null);
    try {
      setModels(await removeDetectModel(path));
      setEngines(await listEngines());
    } catch (e) {
      setError(`移除失败：${e}`);
    }
  };

  /** 有模型就打开它所在的目录并选中文件；没有就打开约定的模型目录 */
  const onOpenFolder = async () => {
    setError(null);
    try {
      const { openPath, revealItemInDir } = await import("@tauri-apps/plugin-opener");
      if (models.length > 0) {
        await revealItemInDir(models[0].path);
      } else {
        await openPath(`${root}\\detect\\yolo`);
      }
    } catch (e) {
      setError(
        models.length > 0
          ? `打不开目录：${e}`
          : `模型目录还不存在，先放一个 onnx 进去，或用「添加模型」选任意位置的文件（${e}）`,
      );
    }
  };

  // 只看用户能改的四项；webview_compat 由后端维护，不该让它把「已保存」一直点亮
  const fingerprint = (c: AppConfig) =>
    `${c.level}|${c.device}|${c.port}|${c.template_threshold}`;
  const dirty = initial !== null && fingerprint(cfg) !== fingerprint(initial);

  const onSave = async () => {
    setError(null);
    setSaved(false);
    setBusy(true);
    try {
      setCfg(await saveConfig(cfg));
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2200);
    } catch (e) {
      setError(`保存失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const onReset = () => {
    if (!confirmReset) {
      setConfirmReset(true);
      window.setTimeout(() => setConfirmReset(false), 4000);
      return;
    }
    setCfg({ ...DEFAULT_CONFIG, webview_compat: cfg.webview_compat });
    setConfirmReset(false);
  };

  const portValid = Number.isInteger(cfg.port) && cfg.port >= 1024 && cfg.port <= 65535;

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        icon={<SettingsIcon className="h-[18px] w-[18px]" />}
        title="设置"
        description="这些是全局默认值，改完要点右上角「保存」。日常用得上的其实只有端口和默认设备。"
        actions={
          <>
            <Button
              variant="outline"
              size="sm"
              onClick={onReset}
              className={confirmReset ? "text-red-600" : undefined}
            >
              <RotateCcw className="h-3.5 w-3.5" />
              {confirmReset ? "再点一次确认恢复" : "恢复默认"}
            </Button>
            <Button
              size="sm"
              variant={dirty ? "default" : "outline"}
              onClick={() => void onSave()}
              disabled={busy || !isTauri()}
            >
              {saved ? (
                <Check className="h-3.5 w-3.5" />
              ) : (
                <Save className="h-3.5 w-3.5" />
              )}
              {busy ? "保存中…" : saved ? "已保存" : dirty ? "保存修改" : "保存"}
            </Button>
          </>
        }
      />

      {error && (
        <Callout variant="warn" icon={<Info className="h-4 w-4" />}>
          {error}
        </Callout>
      )}
      {!isTauri() && (
        <Callout variant="info" icon={<Info className="h-4 w-4" />}>
          浏览器预览模式下设置不可用，请在桌面 App 内修改。
        </Callout>
      )}

      <Card>
        <CardHeader>
          {/* h-8：卡片头第一行统一高度，标题不会因为右侧有没有控件而错位 */}
          <div className="flex h-8 items-center">
            <CardTitle className="flex items-center gap-2">
              <Server className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              推理与服务
            </CardTitle>
          </div>
          <CardDescription>
            换档位或换设备会立刻重载引擎（几秒）；改端口会重启 HTTP 服务。
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-1 gap-x-5 gap-y-4 sm:grid-cols-2">
            <div className="flex flex-col gap-1.5">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                默认模型档位
                <Hint>
                  每次启动用哪套模型。Tiny 最快、Small 均衡（推荐）、Medium 最准但慢。
                </Hint>
              </Label>
              <Select
                value={cfg.level}
                onValueChange={(v) => setCfg({ ...cfg, level: v as ModelLevel })}
              >
                <SelectTrigger>
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
                默认推理设备
                <Hint>
                  有独显选 DirectML；识别报显存/驱动错误，或换机器后异常，改成 CPU 最稳。
                </Hint>
              </Label>
              <Select
                value={cfg.device}
                onValueChange={(v) => setCfg({ ...cfg, device: v as Device })}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="directml">DirectML（GPU，推荐）</SelectItem>
                  <SelectItem value="cpu">CPU（兼容性最好）</SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                HTTP 服务端口
                <Hint>
                  外部脚本调接口用这个端口，范围 1024~65535。被占用时启动会失败，
                  换一个（比如 8850）再存。
                </Hint>
              </Label>
              <Input
                type="number"
                min={1024}
                max={65535}
                value={cfg.port}
                onChange={(e) => setCfg({ ...cfg, port: Number(e.target.value) })}
                className={portValid ? undefined : "border-red-300 focus-visible:ring-red-400"}
              />
              {!portValid && (
                <span className="text-[11.5px] text-red-600">端口需在 1024~65535 之间</span>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                找图默认阈值
                <Hint>
                  模板匹配的相似度下限，0~1。0.9 表示「几乎要一模一样」。
                  易语言/按键精灵脚本不传阈值时就用这个值。
                </Hint>
              </Label>
              <Input
                type="number"
                step={0.01}
                min={0}
                max={1}
                value={cfg.template_threshold}
                onChange={(e) => setCfg({ ...cfg, template_threshold: Number(e.target.value) })}
              />
            </div>
          </div>
        </CardContent>
      </Card>

      {/* 引擎清单：只读，告诉用户「本机能用哪些、缺什么」 */}
      {engines && engines.engines.length > 0 && (
        <Card>
          <CardHeader>
            <div className="flex h-8 items-center">
              <CardTitle className="flex items-center gap-2">
                <Boxes className="h-3.5 w-3.5 shrink-0 text-blue-600" />
                识别引擎
              </CardTitle>
            </div>
            <CardDescription>
              这是 App 扫描模型目录得到的结果，只看文件齐不齐、没有加载模型。
              「未安装」的补上对应文件就能用，不用改任何设置。
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-1.5">
            {engines.engines.map((e) => (
              <EngineRow key={e.name} e={e} isDefault={e.name === engines.default_engine} />
            ))}
          </CardContent>
        </Card>
      )}

      {/* 目标检测模型：用户自己的模型，放目录或指向任意位置 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <ScanSearch className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              目标检测模型
            </CardTitle>
            <div className="flex items-center gap-2">
              <Badge variant={models.length > 0 ? "success" : "outline"}>
                {models.length > 0 ? `${models.length} 个` : "未添加"}
              </Badge>
            </div>
          </div>
          <CardDescription>
            检测模型是你自己的：丢一个 ultralytics 导出的 onnx 到
            <code className="mx-1 rounded bg-neutral-100 px-1 py-0.5 font-mono text-[11px]">
              models\detect\yolo\
            </code>
            就能用，也可以点「添加模型」直接指向任意位置的 onnx（不会复制文件）。
            类别名会<span className="font-medium text-neutral-700">自动从模型里读</span>
            ，不用手写 labels.txt；类别数要跑过一次识别才显示（读类名必须加载模型）。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {models.length === 0 ? (
            <div className="rounded-lg bg-neutral-50/70 px-3 py-4 text-center text-[12px] leading-5 text-neutral-500">
              还没有检测模型。加一个之后，测试台里就能选「目标检测」并对图出框。
            </div>
          ) : (
            models.map((m, i) => {
              // 类名藏在 ONNX metadata 里，只有加载过才知道；没加载过就如实说
              const nClasses =
                engines?.engines.find((e) => e.name === m.engine)?.capabilities.classes.length ?? 0;
              return (
                <div
                  key={m.path}
                  className="flex items-start gap-3 rounded-lg bg-neutral-50/70 px-3 py-2.5"
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <span className="truncate text-[12.5px] font-medium text-neutral-800">
                        {m.id}
                      </span>
                      {i === 0 && <Badge variant="default">默认</Badge>}
                      <Badge variant={m.external ? "secondary" : "outline"}>
                        {m.external ? "外部路径" : "模型目录"}
                      </Badge>
                      <Badge variant={nClasses > 0 ? "secondary" : "outline"}>
                        {nClasses > 0 ? `${nClasses} 类` : "类数未知"}
                      </Badge>
                      <code className="font-mono text-[11px] text-neutral-400">{m.file_name}</code>
                    </div>
                    <p className="mt-1 break-all font-mono text-[11px] leading-4 text-neutral-500">
                      {m.path}
                    </p>
                  </div>
                  {m.external && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => void onRemoveModel(m.path)}
                      title="从列表移除（不删除磁盘上的文件）"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                      移除
                    </Button>
                  )}
                </div>
              );
            })
          )}

          <div className="mt-1 flex flex-wrap items-center gap-2">
            <Button size="sm" onClick={() => void onAddModel()} disabled={picking || !isTauri()}>
              <Plus className="h-3.5 w-3.5" />
              {picking ? "选择中…" : "添加模型"}
            </Button>
            <Button size="sm" variant="outline" onClick={() => void onOpenFolder()} disabled={!isTauri()}>
              <FolderOpen className="h-3.5 w-3.5" />
              打开模型目录
            </Button>
            <span className="text-[11.5px] leading-4 text-neutral-400">
              列表第一项就是默认模型（测试台里显示为「目标检测」）
            </span>
          </div>
        </CardContent>
      </Card>

      {/* 渲染兼容：只读，解释给用户听 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <Shield className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              窗口渲染
            </CardTitle>
            <Badge variant={cfg.webview_compat ? "warning" : "success"}>
              {cfg.webview_compat ? "兼容模式" : "硬件加速"}
            </Badge>
          </div>
          <CardDescription>
            部分机器的显卡驱动会让 WebView 加载失败（表现为窗口一片白），App 会自动切到兼容模式。
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="flex items-start gap-2 rounded-lg bg-neutral-50/80 px-3 py-2.5 text-[12px] leading-5 text-neutral-600">
            <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-neutral-400" />
            <span>
              {cfg.webview_compat
                ? "已启用软件合成（--disable-gpu-compositing）。界面会略微不那么顺滑，但能正常显示。想强制回到硬件加速，删掉配置文件里的 webview_compat 字段再启动。"
                : "当前使用硬件加速。如果双击后窗口一直是白的，说明触发了自动兜底，重启一次就会进兼容模式。"}
            </span>
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <div className="flex h-8 items-center">
            <CardTitle className="flex items-center gap-2">
              <Info className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              关于
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="flex flex-col gap-2.5 text-[12px] text-neutral-600">
          <Row label="模型根目录">
            <code className="selectable break-all font-mono text-[11px] text-neutral-700">
              {root}
            </code>
          </Row>
          <Row label="配置文件">
            <code className="selectable font-mono text-[11px] text-neutral-700">
              {"<应用数据目录>\\config.json"}
            </code>
          </Row>
          <Row label="日志文件">
            <code className="selectable font-mono text-[11px] text-neutral-700">
              {"<应用数据目录>\\ocr-studio.log"}
            </code>
          </Row>
          <Row label="字体">
            <span>MiSans（小米）</span>
          </Row>
          <Row label="版本">
            <span className="font-mono">0.1.0</span>
          </Row>
        </CardContent>
      </Card>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <span className="shrink-0 text-neutral-500">{label}</span>
      <span className="min-w-0 text-right">{children}</span>
    </div>
  );
}

/** 一行引擎：名字 + 状态 + 能力标签 + 说明。 */
function EngineRow({ e, isDefault }: { e: EngineItem; isDefault: boolean }) {
  const caps = [
    e.capabilities.line_boxes && "逐行",
    e.capabilities.char_boxes && "逐字",
    e.capabilities.rotation_cls && "方向矫正",
    e.capabilities.detect && "目标检测",
  ].filter(Boolean) as string[];

  return (
    <div
      className={cn(
        "rounded-lg px-3 py-2.5 transition-colors",
        e.available ? "bg-neutral-50/70" : "bg-neutral-50/40",
      )}
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <code
          className={cn(
            "font-mono text-[12px] font-medium",
            e.available ? "text-neutral-800" : "text-neutral-400",
          )}
        >
          {e.name}
        </code>
        {isDefault && <Badge variant="default">默认</Badge>}
        <Badge variant={e.available ? "success" : "outline"}>
          {e.available ? "可用" : "未安装"}
        </Badge>
        {e.actual_device && (
          <Badge variant="secondary">
            {e.actual_device === "directml" ? "GPU" : "CPU"}
          </Badge>
        )}
        {caps.length > 0 && (
          <span className="ml-auto flex flex-wrap justify-end gap-1">
            {caps.map((c) => (
              <span
                key={c}
                className="rounded bg-white px-1.5 py-0.5 text-[10.5px] text-neutral-500 ring-1 ring-neutral-200/70"
              >
                {c}
              </span>
            ))}
          </span>
        )}
      </div>
      {e.note && (
        <p className="mt-1 break-all font-mono text-[11px] leading-4 text-neutral-500">
          {e.note}
        </p>
      )}
    </div>
  );
}
