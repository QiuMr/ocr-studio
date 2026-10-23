import { useCallback, useEffect, useRef, useState } from "react";
import {
  Boxes,
  Check,
  ChevronDown,
  Copy,
  Cpu,
  ExternalLink,
  FileText,
  FolderOpen,
  Gauge,
  HardDrive,
  Info,
  KeyRound,
  Languages,
  Loader2,
  Play,
  Plus,
  RotateCcw,
  ScanSearch,
  Server,
  Settings as SettingsIcon,
  Shield,
  Square,
  Trash2,
  Zap,
} from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Badge } from "../components/ui/badge";
import { Callout } from "../components/ui/callout";
import { Hint } from "../components/ui/hint";
import { Label } from "../components/ui/label";
import { Input } from "../components/ui/input";
import { Switch } from "../components/ui/switch";
import { Slider } from "../components/ui/slider";
import {
  aiResetToken,
  aiSetEnabled,
  aiStatus,
  aiTargets,
  detectModels,
  getAutostart,
  getConfig,
  getHealth,
  listEngines,
  modelsRoot,
  pickDetectModel,
  promptAssets,
  removeDetectModel,
  saveConfig,
  serverStart,
  serverStatus,
  serverStop,
  setAutostart,
  setDevice,
  setLevel,
} from "../lib/api";
import { isTauri } from "../lib/tauri";
import { cn } from "../lib/utils";
import {
  DEFAULT_CONFIG,
  type AiStatus,
  type AppConfig,
  type DetectModel,
  type Device,
  type EngineItem,
  type EngineList,
  type Health,
  type ModelLevel,
  type PromptAssets,
} from "../types";

/*
 * 设置页的信息架构（2026-10-23 重做「模型」卡）：
 *
 *   模型   —— 一眼看清「程序靠什么干活」：认字（自带/选快慢）、提示找图（自带/
 *             三条能力各自装没装）、你自己的检测模型（要自己加）。放最前面，
 *             因为小白最先问的就是「我还缺不缺东西」。
 *   常用   —— 剩下两样真的会动的：要不要用显卡、找图要多像
 *   服务   —— 给脚本用的人：开关、地址、端口
 *   AI     —— 给本机 AI 的入口
 *   高级   —— 引擎清单、目录、窗口渲染、关于（默认收起，看的人自己展开）
 *
 * 改动**自动保存**，不再要求用户去右上角找「保存」按钮。
 * 这页里没有一样是「改错了会炸」的，多一个保存步骤只会让人以为没生效。
 */
export function SettingsPage() {
  const [cfg, setCfg] = useState<AppConfig>(DEFAULT_CONFIG);
  const [health, setHealth] = useState<Health | null>(null);
  const [root, setRoot] = useState("-");
  const [engines, setEngines] = useState<EngineList | null>(null);
  const [models, setModels] = useState<DetectModel[]>([]);
  const [assets, setAssets] = useState<PromptAssets | null>(null);
  const [picking, setPicking] = useState(false);
  const [running, setRunning] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [savedAt, setSavedAt] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  /** 开机自启：状态存在系统注册表里，不来自 config.json，所以单独拉 */
  const [autostart, setAutostartState] = useState<boolean | null>(null);
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [aiTargetsList, setAiTargetsList] = useState<string[]>([]);
  const [aiBusy, setAiBusy] = useState(false);
  const [aiCopied, setAiCopied] = useState(false);

  const saveTimer = useRef<number | undefined>(undefined);

  const loadAll = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [c, h, s, r, e, m, pa] = await Promise.all([
        getConfig(),
        getHealth(),
        serverStatus(),
        modelsRoot(),
        listEngines(),
        detectModels(),
        promptAssets(),
      ]);
      setCfg(c);
      setHealth(h);
      setRunning(s);
      setRoot(r);
      setEngines(e);
      setModels(m);
      setAssets(pa);
      const [aiState, targets] = await Promise.all([aiStatus(), aiTargets()]);
      setAi(aiState);
      setAiTargetsList(targets);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  useEffect(() => {
    if (!isTauri()) return;
    void getAutostart()
      .then(setAutostartState)
      .catch(() => setAutostartState(null));
  }, []);

  // AI 在本机改了服务/配置时，HTTP 层没有 AppHandle 发不了 Tauri 事件；
  // 用轻量轮询代替（3s 一次，只在设置页打开时跑）。AI 启停服务、改端口后
  // 「当前监听」徽标和状态会自己变，不用用户手动刷新。
  useEffect(() => {
    if (!isTauri()) return;
    const t = window.setInterval(() => {
      void aiStatus().then(setAi).catch(() => {});
    }, 3000);
    return () => window.clearInterval(t);
  }, []);

  const onToggleAutostart = async (on: boolean) => {
    try {
      setAutostartState(await setAutostart(on));
      setError(null);
    } catch (e) {
      setError(String(e));
      // 失败就回读真实状态，别让开关停在用户以为成功的位置
      void getAutostart().then(setAutostartState).catch(() => {});
    }
  };

  /** 写盘（延迟 600ms 合并连点；改坏了也不会写一半） */
  const persist = useCallback((next: AppConfig) => {
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      void (async () => {
        try {
          await saveConfig(next);
          setSavedAt(Date.now());
          setError(null);
        } catch (e) {
          setError(`保存失败：${e}`);
        }
      })();
    }, 600);
  }, []);

  const patch = (p: Partial<AppConfig>) => {
    const next = { ...cfg, ...p };
    setCfg(next);
    persist(next);
  };

  const onLevel = async (level: ModelLevel) => {
    const next = { ...cfg, level };
    setCfg(next);
    persist(next);
    if (!isTauri()) return;
    try {
      setHealth(await setLevel(level));
    } catch (e) {
      setError(`切换识别精度失败：${e}`);
    }
  };

  const onDevice = async (useGpu: boolean) => {
    const device: Device = useGpu ? "directml" : "cpu";
    const next = { ...cfg, device };
    setCfg(next);
    persist(next);
    if (!isTauri()) return;
    try {
      setHealth(await setDevice(device));
    } catch (e) {
      setError(`切换显卡加速失败：${e}`);
    }
  };

  const onToggleAi = async (enabled: boolean) => {
    setAiBusy(true);
    try {
      setAi(await aiSetEnabled(enabled));
      setError(null);
    } catch (e) {
      setError(`AI 接口开关失败：${e}`);
    } finally {
      setAiBusy(false);
    }
  };

  const onResetAiToken = async () => {
    setAiBusy(true);
    try {
      setAi(await aiResetToken());
      setError(null);
    } catch (e) {
      setError(`重置 AI 连接码失败：${e}`);
    } finally {
      setAiBusy(false);
    }
  };

  const copyAiPrompt = async () => {
    if (!ai) return;
    await copy(ai.prompt, "ai");
    setAiCopied(true);
    window.setTimeout(() => setAiCopied(false), 1400);
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
      setError(`服务操作失败：${e}`);
    } finally {
      setBusy(false);
    }
  };

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

  const onOpenHealth = async () => {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(`http://127.0.0.1:${cfg.port}/ocr/health`);
  };

  const copy = async (text: string, tag = "") => {
    await navigator.clipboard.writeText(text);
    setCopied(!!tag);
    window.setTimeout(() => setCopied(false), 1400);
  };

  const onReset = () => {
    if (!confirmReset) {
      setConfirmReset(true);
      window.setTimeout(() => setConfirmReset(false), 4000);
      return;
    }
    const next = { ...DEFAULT_CONFIG, webview_compat: cfg.webview_compat };
    setCfg(next);
    persist(next);
    setConfirmReset(false);
  };

  const portValid = Number.isInteger(cfg.port) && cfg.port >= 1024 && cfg.port <= 65535;
  const gpu = cfg.device === "directml";
  const endpoint = `http://127.0.0.1:${cfg.port}`;
  const justSaved = savedAt > 0 && Date.now() - savedAt < 2200;

  return (
    <div className="flex flex-col gap-4">
      <div className="mb-1 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-[26px] font-bold leading-none tracking-tight text-neutral-900">
            设置
          </h1>
          <p className="mt-2 flex items-center gap-1.5 text-[13px] font-medium leading-5 text-neutral-500">
            改完自动保存，不用点按钮。
            {justSaved && (
              <span className="flex items-center gap-1 font-medium text-emerald-600">
                <Check className="h-3 w-3" strokeWidth={3} />
                已保存
              </span>
            )}
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={onReset}
          className={confirmReset ? "text-red-600" : undefined}
        >
          <RotateCcw className="h-3.5 w-3.5" />
          {confirmReset ? "再点一次确认恢复" : "恢复默认"}
        </Button>
      </div>

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

      {/* ------------------------------------------------------------ 模型 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <Boxes className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              模型
            </CardTitle>
            <Badge variant={assets === null || assets.detector ? "success" : "warning"}>
              {assets === null
                ? "读取中…"
                : assets.detector
                  ? "认字与找图已就绪"
                  : "提示找图未安装"}
            </Badge>
          </div>
          <CardDescription>
            程序靠模型干活。认字和找图的模型已随程序带好，通常不用管；
            只有「检测你自己的物体」需要加模型。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          {/* 组 1：文字识别——自带，这里只选快慢 */}
          <ModelGroup
            icon={<FileText className="h-3.5 w-3.5" />}
            title="文字识别"
            badge="已就绪"
            desc="认字的主力：检测、识别、方向矫正三件套加一本字典（约 31MB）。随程序带好，装在哪个盘都不用动。"
          >
            <div className="flex flex-col gap-2">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                识别快慢
                <Hint>
                  「快」用的模型小，几乎秒出，适合界面文字清晰的截图；「均衡」是默认推荐；
                  「准」最准也最慢，适合字小、模糊、背景花的图。换完引擎会重载几秒。
                </Hint>
              </Label>
              <div className="grid grid-cols-3 gap-2">
                {(
                  [
                    { v: "tiny" as ModelLevel, title: "快", desc: "秒出，够用" },
                    { v: "small" as ModelLevel, title: "均衡", desc: "推荐" },
                    { v: "medium" as ModelLevel, title: "准", desc: "慢一点，最准" },
                  ]
                ).map((it) => (
                  <button
                    key={it.v}
                    type="button"
                    onClick={() => void onLevel(it.v)}
                    disabled={!isTauri()}
                    className={cn(
                      "flex flex-col items-start rounded-md border-2 px-3 py-2 text-left transition-colors",
                      cfg.level === it.v
                        ? "border-[#2563EB] bg-[#EFF4FF]"
                        : "border-neutral-200 bg-white hover:border-neutral-300",
                    )}
                  >
                    <span
                      className={cn(
                        "flex items-center gap-1.5 text-[13px] font-bold",
                        cfg.level === it.v ? "text-[#2563EB]" : "text-neutral-800",
                      )}
                    >
                      {cfg.level === it.v && <Check className="h-3.5 w-3.5" strokeWidth={3} />}
                      {it.title}
                    </span>
                    <span className="mt-0.5 text-[12px] text-neutral-500">{it.desc}</span>
                  </button>
                ))}
              </div>
              {health && (
                <p className="flex items-center gap-1.5 text-[12px] text-neutral-400">
                  <Cpu className="h-3 w-3" />
                  当前实际使用：{health.model} · {health.actual_device === "directml" ? "显卡" : "处理器"}
                </p>
              )}
            </div>
          </ModelGroup>

          {/* 组 2：提示找图——自带，两种用法共用同一套文件 */}
          <ModelGroup
            icon={<ScanSearch className="h-3.5 w-3.5" />}
            title="提示找图"
            badge={assets?.detector ? "已就绪" : "未安装"}
            badgeVariant={assets?.detector ? "success" : "warning"}
            desc="在屏幕上找目标，只认「长得像」。两套文件缺哪个补哪个，不用改任何设置。"
          >
            <div className="flex flex-col gap-1.5">
              <AssetRow
                icon={<ScanSearch className="h-3.5 w-3.5" />}
                name="图找图"
                desc="截一块目标当参考图，找屏幕上长得一样的——换皮肤、按下态也认"
                ok={!!assets?.detector && !!assets?.vpe}
              />
              <AssetRow
                icon={<Languages className="h-3.5 w-3.5" />}
                name="按中文名找"
                desc="框一次、起个中文名（如「提交按钮」），以后直接用这个名字找"
                ok={!!assets?.detector && !!assets?.vpe}
              />
            </div>
            {assets && !assets.detector && (
              <p className="text-[12px] leading-4 text-amber-600">
                缺检测器，整套用不了。把导出好的 onnx 放进下面的模型目录即可。
              </p>
            )}
          </ModelGroup>

          {/* 组 3：你自己的检测模型——唯一要用户动手的一组 */}
          <ModelGroup
            icon={<Boxes className="h-3.5 w-3.5" />}
            title="你自己的检测模型"
            badge={models.length > 0 ? `${models.length} 个` : "未添加"}
            badgeVariant={models.length > 0 ? "success" : "outline"}
            desc="检测「你自己的物体」：把训练导出的 onnx 加进来，识别页就能选它。v5~v12、v26 和 YOLOE 开放词汇模型都认。不加也能认字、找图。"
          >
            {models.length === 0 ? (
              <div className="rounded-md bg-white px-3 py-2.5 text-[12px] leading-5 text-neutral-500">
                还没有检测模型。两种加法：把 .onnx 丢进下面的「模型目录」，
                或点「添加模型」指向任意位置的文件（只记路径、不复制）。
                加好之后，识别页的「识别方式」里会多出「找物体」这一项。
              </div>
            ) : (
              <div className="flex flex-col gap-1.5">
                {models.map((m, i) => {
                  // 类名藏在 ONNX metadata 里，只有加载过才知道；没加载过就如实说
                  const nClasses =
                    engines?.engines.find((e) => e.name === m.engine)?.capabilities.classes.length ?? 0;
                  return (
                    <div
                      key={m.path}
                      className="flex items-start gap-3 rounded-md bg-white px-3 py-2.5"
                    >
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          <span className="truncate text-[13px] font-medium text-neutral-800">
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
                })}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" onClick={() => void onAddModel()} disabled={picking || !isTauri()}>
                <Plus className="h-3.5 w-3.5" />
                {picking ? "选择中…" : "添加模型"}
              </Button>
              <Button
                size="sm"
                variant="outline"
                onClick={() => void onOpenFolder()}
                disabled={!isTauri()}
              >
                <FolderOpen className="h-3.5 w-3.5" />
                打开模型目录
              </Button>
              <span className="text-[12px] leading-4 text-neutral-400">
                列表第一项就是默认模型；类别名会自动从模型里读，不用手写 labels.txt
              </span>
            </div>
          </ModelGroup>

          {/* 模型目录：前面三组都指着它，单独给一行 */}
          <div className="flex items-center justify-between gap-3 rounded-lg bg-neutral-50/70 px-3.5 py-2.5">
            <div className="min-w-0">
              <div className="flex items-center gap-1.5 text-[13px] font-medium text-neutral-800">
                <HardDrive className="h-3.5 w-3.5 text-neutral-400" />
                模型目录
              </div>
              <code className="selectable break-all font-mono text-[11px] leading-4 text-neutral-500">
                {root}
              </code>
            </div>
            <Button
              size="sm"
              variant="outline"
              onClick={() => void onOpenFolder()}
              disabled={!isTauri()}
            >
              <FolderOpen className="h-3.5 w-3.5" />
              打开
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* ------------------------------------------------------------ 常用 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center">
            <CardTitle className="flex items-center gap-2">
              <Gauge className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              常用
            </CardTitle>
          </div>
          <CardDescription>这两样决定了跑得多快、找图多严，换完立刻生效。</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          {/* 显卡加速 */}
          <div className="flex items-start justify-between gap-4 rounded-lg bg-neutral-50 px-3.5 py-3">
            <div className="min-w-0">
              <Label
                htmlFor="gpu"
                className="flex cursor-pointer items-center gap-1.5 text-[13px] font-medium text-neutral-800"
              >
                <Zap className="h-3.5 w-3.5 text-neutral-400" />
                用显卡加速
                <Hint>
                  有独立显卡就开着，通常比处理器快好几倍。如果识别时报显存/驱动错误，
                  关掉它退回处理器最稳（会慢，但一定能跑）。
                </Hint>
              </Label>
              <p className="mt-1 text-[12px] leading-4 text-neutral-500">
                {gpu ? "正在用显卡跑识别" : "正在用处理器跑识别，兼容性最好"}
                {health && health.actual_device !== cfg.device && (
                  <span className="ml-1 text-amber-600">
                    （当前实际是{health.actual_device === "directml" ? "显卡" : "处理器"}，可能缺运行库）
                  </span>
                )}
              </p>
            </div>
            <Switch id="gpu" checked={gpu} onCheckedChange={(v) => void onDevice(v)} disabled={!isTauri()} />
          </div>

          {/* 找图相似度 */}
          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
                找图要多像才算找到
                <Hint>
                  0.9 表示「几乎一模一样」才算命中。找不到就调低，误报多就调高。
                  脚本调接口时没传阈值就用这个值。
                </Hint>
              </Label>
              <span className="font-mono text-[13px] font-semibold text-neutral-700">
                {cfg.template_threshold.toFixed(2)}
              </span>
            </div>
            <Slider
              value={[cfg.template_threshold]}
              min={0.5}
              max={1}
              step={0.01}
              onValueChange={([v]) => setCfg({ ...cfg, template_threshold: v })}
              onValueCommit={([v]) => patch({ template_threshold: v })}
              disabled={!isTauri()}
            />
            <div className="flex justify-between text-[11px] text-neutral-400">
              <span>宽松（容易误报）</span>
              <span>严格（容易找不到）</span>
            </div>
          </div>
        </CardContent>
      </Card>

      {/* -------------------------------------------------------- 本地服务 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <Server className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              本地服务
            </CardTitle>
            <Badge variant={running ? "success" : "secondary"} className="shrink-0 whitespace-nowrap">
              {running ? "运行中" : "已停止"}
            </Badge>
          </div>
          <CardDescription>
            易语言、按键精灵、Python 脚本通过下面这个地址调识别能力。自己用 App 界面的话，这里可以一直关着。
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
              {copied ? (
                <Check className="h-3.5 w-3.5 text-emerald-600" />
              ) : (
                <Copy className="h-3.5 w-3.5" />
              )}
            </button>
          </div>

          <div className="flex flex-wrap items-end gap-3">
            <Button onClick={() => void onToggleServer()} disabled={busy || !isTauri()}>
              {busy ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : running ? (
                <Square className="h-4 w-4" />
              ) : (
                <Play className="h-4 w-4" />
              )}
              {running ? "停止服务" : "启动服务"}
            </Button>
            <Button variant="outline" onClick={() => void onOpenHealth()} disabled={!running}>
              <ExternalLink className="h-3.5 w-3.5" />
              打开健康检查
            </Button>

            <div className="flex flex-col gap-1">
              <Label htmlFor="port" className="text-[12px] text-neutral-500">
                端口
              </Label>
              <Input
                id="port"
                type="number"
                min={1024}
                max={65535}
                value={cfg.port}
                onChange={(e) => setCfg({ ...cfg, port: Number(e.target.value) })}
                onBlur={() => portValid && patch({ port: cfg.port })}
                className={cn("w-[120px]", !portValid && "border-red-300 focus-visible:ring-red-400")}
              />
            </div>
            {!portValid && (
              <span className="pb-2 text-[12px] text-red-600">端口需在 1024~65535 之间</span>
            )}
          </div>

          <p className="text-[12px] leading-5 text-neutral-400">
            服务只监听本机 127.0.0.1，不对外网开放，别人连不进来。
          </p>

          {/* CORS：默认关。开之前必须让用户知道代价——截屏类接口会被网页驱动 */}
          <div className="flex items-start justify-between gap-3 border-t border-neutral-100 pt-3">
            <div className="min-w-0">
              <Label htmlFor="allow-browser" className="flex items-center gap-1.5 text-[12px] text-neutral-700">
                允许浏览器页面调用
                <Hint>
                  本地 HTML 页面、Electron 应用里用 fetch 调这些接口时，浏览器会先发 OPTIONS 预检。不开这个开关，预检过不去，页面里只会看到一个 CORS 报错。
                </Hint>
              </Label>
              <p className="mt-1 text-[12px] leading-5 text-neutral-500">
                {cfg.allow_browser
                  ? "已允许任意网页跨域调用。注意：截屏找图类接口（pingmuzhaotu / jubingzhaotu）会读你的屏幕，任何打开着的网页都能驱动它，用完建议关掉。"
                  : "默认关闭。只有在你自己的网页里调接口时才需要打开。"}
              </p>
            </div>
            <Switch
              id="allow-browser"
              checked={!!cfg.allow_browser}
              onCheckedChange={(v) => patch({ allow_browser: v })}
              disabled={!isTauri()}
            />
          </div>
          {cfg.allow_browser && (
            <p className="text-[12px] leading-5 text-amber-700">
              改动会在服务重启后生效（改端口或点一次「停止 → 启动」即可）。
            </p>
          )}

          {/* 开机自启：常驻服务的标配。开机后只进托盘，不弹窗 */}
          <div className="flex items-start justify-between gap-3 border-t border-neutral-100 pt-3">
            <div className="min-w-0">
              <Label htmlFor="autostart" className="flex items-center gap-1.5 text-[12px] text-neutral-700">
                开机自动启动
                <Hint>
                  开机后程序自动在后台跑起来，界面直接缩在托盘里，不会弹窗打扰你。脚本一开机就能调接口，不用先点一下图标。
                </Hint>
              </Label>
              <p className="mt-1 text-[12px] leading-5 text-neutral-500">
                {autostart === null
                  ? "当前状态读取失败，可稍后重试。"
                  : autostart
                    ? "已开启。开机后自动进托盘，右键托盘图标可打开界面或退出。"
                    : "关闭。每次开机需要自己双击一次程序。"}
              </p>
            </div>
            <Switch
              id="autostart"
              checked={!!autostart}
              onCheckedChange={(v) => void onToggleAutostart(v)}
              disabled={!isTauri() || autostart === null}
            />
          </div>
        </CardContent>
      </Card>

      {/* ---------------------------------------------------------- AI 控制 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center">
            <CardTitle className="flex items-center gap-2">
              <KeyRound className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              AI 控制
              <Badge variant={ai?.enabled ? "success" : "outline"}>
                {ai?.enabled ? "已开启" : "默认关闭"}
              </Badge>
            </CardTitle>
          </div>
          <CardDescription>
            给本机 AI 一个简单的 HTTP 地址，让它直接调用 OCR、找图、截图和服务控制，不需要安装 MCP。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          <div className="flex items-start justify-between gap-3 rounded-lg bg-neutral-50 px-3.5 py-3">
            <div className="min-w-0">
              <Label htmlFor="ai-enabled" className="flex items-center gap-1.5 text-[13px] font-medium text-neutral-800">
                开放 AI 操控接口
                <Hint>只监听 127.0.0.1，云端网页 AI 无法访问；开启后仍需连接码。</Hint>
              </Label>
              <p className="mt-1 text-[12px] leading-4 text-neutral-500">
                {ai?.enabled ? "已开放，AI 可以按工具清单调用本机服务。" : "关闭状态，AI 请求会被拒绝。"}
              </p>
            </div>
            <Switch
              id="ai-enabled"
              checked={!!ai?.enabled}
              onCheckedChange={(v) => void onToggleAi(v)}
              disabled={!isTauri() || !ai || aiBusy}
            />
          </div>

          {ai && (
            <div className="flex flex-col gap-2">
              <Row label="接口地址">
                <code className="selectable font-mono text-[12px] text-neutral-700">{ai.base_url}/ai/v1</code>
              </Row>
              <Row label="连接码">
                <div className="flex max-w-[330px] items-center gap-2">
                  <code className="selectable break-all font-mono text-[12px] text-neutral-700">{ai.token}</code>
                  <Button variant="ghost" size="icon-xs" onClick={() => void copy(ai.token, "token")} title="复制连接码">
                    {copied ? <Check className="h-3.5 w-3.5 text-emerald-600" /> : <Copy className="h-3.5 w-3.5" />}
                  </Button>
                  <Button variant="outline" size="xs" onClick={() => void onResetAiToken()} disabled={aiBusy}>
                    <RotateCcw className="h-3.5 w-3.5" />
                    重置
                  </Button>
                </div>
              </Row>
              <Row label="当前监听">
                <Badge variant={ai.running ? "success" : "warning"}>{ai.running ? "服务运行中" : "服务未启动"}</Badge>
              </Row>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-2 border-t border-neutral-100 pt-3">
            <Button variant="outline" size="sm" onClick={() => void copyAiPrompt()} disabled={!ai || !isTauri()}>
              {aiCopied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
              {aiCopied ? "已复制提示词" : "一键复制给 AI 的提示词"}
            </Button>
            <span className="text-[12px] text-neutral-400">
              已教目标：{aiTargetsList.length ? aiTargetsList.join("、") : "暂无，可用 teach_target 教一次"}
            </span>
          </div>
        </CardContent>
      </Card>

      {/* ---------------------------------------------------------- 高级 */}
      <Card>
        <button
          type="button"
          onClick={() => setShowAdvanced((v) => !v)}
          className="flex w-full items-center justify-between gap-2 px-5 py-3.5 text-left"
          aria-expanded={showAdvanced}
        >
          <span className="flex items-center gap-2 text-[13px] font-semibold text-neutral-900">
            <SettingsIcon className="h-3.5 w-3.5 shrink-0 text-neutral-400" />
            高级信息
            <span className="text-[12px] font-normal text-neutral-400">
              引擎清单、文件位置、渲染方式
            </span>
          </span>
          <ChevronDown
            className={cn(
              "h-4 w-4 shrink-0 text-neutral-400 transition-transform",
              showAdvanced && "rotate-180",
            )}
          />
        </button>

        {showAdvanced && (
          <CardContent className="flex flex-col gap-4 pt-0">
            {/* 引擎清单：只读，告诉用户「本机能用哪些、缺什么」 */}
            {engines && engines.engines.length > 0 && (
              <section className="flex flex-col gap-1.5">
                <h3 className="flex items-center gap-1.5 text-[12px] font-semibold text-neutral-700">
                  <Boxes className="h-3.5 w-3.5 text-neutral-400" />
                  本机可用的识别引擎
                </h3>
                <p className="text-[12px] leading-4 text-neutral-400">
                  扫描模型目录得到的结果，只看文件齐不齐、没有加载模型。「未安装」的补上对应文件就能用，不用改任何设置。
                </p>
                {engines.engines.map((e) => (
                  <EngineRow key={e.name} e={e} isDefault={e.name === engines.default_engine} />
                ))}
              </section>
            )}

            {/* 窗口渲染 */}
            <section className="flex flex-col gap-1.5">
              <h3 className="flex items-center gap-1.5 text-[12px] font-semibold text-neutral-700">
                <Shield className="h-3.5 w-3.5 text-neutral-400" />
                窗口渲染
                <Badge variant={cfg.webview_compat ? "warning" : "success"}>
                  {cfg.webview_compat ? "兼容模式" : "硬件加速"}
                </Badge>
              </h3>
              <div className="flex items-start gap-2 rounded-lg bg-neutral-50/80 px-3 py-2.5 text-[12px] leading-5 text-neutral-600">
                <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-neutral-400" />
                <span>
                  {cfg.webview_compat
                    ? "已启用软件合成（--disable-gpu-compositing）。界面会略微不那么顺滑，但能正常显示。想强制回到硬件加速，删掉配置文件里的 webview_compat 字段再启动。"
                    : "当前使用硬件加速。如果双击后窗口一直是白的，说明显卡驱动有兼容问题，重启一次会自动进兼容模式。"}
                </span>
              </div>
            </section>

            {/* 位置与版本 */}
            <section className="flex flex-col gap-2">
              <h3 className="flex items-center gap-1.5 text-[12px] font-semibold text-neutral-700">
                <Info className="h-3.5 w-3.5 text-neutral-400" />
                文件位置
              </h3>
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
                <span className="text-[12px] text-neutral-600">MiSans（小米）</span>
              </Row>
              <Row label="版本">
                <span className="font-mono text-[12px] text-neutral-600">0.1.0</span>
              </Row>
              <p className="text-[12px] text-neutral-400">
                环境变量 <code className="font-mono">OCR_MODELS_ROOT</code> 可覆盖模型目录，适合把模型放到别的盘。
              </p>
            </section>
          </CardContent>
        )}
      </Card>
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4">
      <span className="shrink-0 text-[12px] text-neutral-500">{label}</span>
      <span className="min-w-0 text-right">{children}</span>
    </div>
  );
}

/**
 * 「模型」卡里的一组：图标 + 标题 + 状态徽标 + 一句人话 + 子内容。
 *
 * 组容器是 lg(10) 圆角，里面的行/空态用 md(8)——子比父小一档。
 */
function ModelGroup({
  icon,
  title,
  badge,
  badgeVariant = "success",
  desc,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  badge?: string;
  badgeVariant?: "default" | "secondary" | "success" | "warning" | "outline";
  desc: string;
  children?: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2.5 rounded-lg bg-neutral-50/70 px-3.5 py-3">
      <div className="flex h-8 items-center gap-2">
        <span className="flex items-center gap-2 text-[13px] font-semibold text-neutral-900">
          <span className="text-neutral-400">{icon}</span>
          {title}
        </span>
        {badge && <Badge variant={badgeVariant}>{badge}</Badge>}
      </div>
      <p className="text-[12px] leading-4 text-neutral-500">{desc}</p>
      {children}
    </section>
  );
}

/** 一行「装没装」：能力名 + 一句话 + 右侧徽标。白底，与父组的灰底区分。 */
function AssetRow({
  icon,
  name,
  desc,
  ok,
}: {
  icon: React.ReactNode;
  name: string;
  desc: string;
  ok: boolean;
}) {
  return (
    <div className="flex items-center gap-2.5 rounded-md bg-white px-3 py-2">
      <span className="shrink-0 text-neutral-400">{icon}</span>
      <div className="min-w-0 flex-1">
        <div className="text-[12px] font-medium text-neutral-800">{name}</div>
        <div className="text-[11px] leading-4 text-neutral-500">{desc}</div>
      </div>
      <Badge variant={ok ? "success" : "outline"}>{ok ? "已装" : "未装"}</Badge>
    </div>
  );
}

/** 一行引擎：名字 + 状态 + 能力标签 + 说明。 */
function EngineRow({ e, isDefault }: { e: EngineItem; isDefault: boolean }) {
  const caps = [
    e.capabilities.line_boxes && "逐行",
    e.capabilities.char_boxes && "逐字",
    e.capabilities.rotation_cls && "方向矫正",
    e.capabilities.detect && "找物体",
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
          <Badge variant="secondary">{e.actual_device === "directml" ? "GPU" : "CPU"}</Badge>
        )}
        {caps.length > 0 && (
          <span className="ml-auto flex flex-wrap justify-end gap-1">
            {caps.map((c) => (
              <span
                key={c}
                className="rounded bg-white px-1.5 py-0.5 text-[11px] text-neutral-500 ring-1 ring-neutral-200/70"
              >
                {c}
              </span>
            ))}
          </span>
        )}
      </div>
      {e.note && (
        <p className="mt-1 break-all font-mono text-[11px] leading-4 text-neutral-500">{e.note}</p>
      )}
    </div>
  );
}
