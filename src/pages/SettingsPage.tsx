import { useCallback, useEffect, useRef, useState } from "react";
import {
  Boxes,
  Check,
  ChevronDown,
  Copy,
  Cpu,
  ExternalLink,
  FolderOpen,
  Gauge,
  Info,
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
  detectModels,
  getAutostart,
  getConfig,
  getHealth,
  listEngines,
  modelsRoot,
  pickDetectModel,
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
  type AppConfig,
  type DetectModel,
  type Device,
  type EngineItem,
  type EngineList,
  type Health,
  type ModelLevel,
} from "../types";

/*
 * 设置页的信息架构（2026-10-22 重做）：
 *
 *   常用   —— 小白真的会动的三样：识别快慢、要不要用显卡、找图像不像
 *   服务   —— 给脚本用的人：开关、地址、端口
 *   模型   —— 自己的检测模型
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

  const saveTimer = useRef<number | undefined>(undefined);

  const loadAll = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const [c, h, s, r, e, m] = await Promise.all([
        getConfig(),
        getHealth(),
        serverStatus(),
        modelsRoot(),
        listEngines(),
        detectModels(),
      ]);
      setCfg(c);
      setHealth(h);
      setRunning(s);
      setRoot(r);
      setEngines(e);
      setModels(m);
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

      {/* ------------------------------------------------------------ 常用 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center">
            <CardTitle className="flex items-center gap-2">
              <Gauge className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              常用
            </CardTitle>
          </div>
          <CardDescription>这三样决定了「快不快、准不准」，换完立刻生效。</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          {/* 识别精度：三段按钮比下拉直观，选中的一眼看得出来 */}
          <div className="flex flex-col gap-2">
            <Label className="flex items-center gap-1.5 text-[12px] text-neutral-600">
              识别精度
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

      {/* ------------------------------------------------------ 目标检测模型 */}
      <Card>
        <CardHeader>
          <div className="flex h-8 items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2">
              <ScanSearch className="h-3.5 w-3.5 shrink-0 text-blue-600" />
              我自己的检测模型
            </CardTitle>
            <Badge variant={models.length > 0 ? "success" : "outline"}>
              {models.length > 0 ? `${models.length} 个` : "未添加"}
            </Badge>
          </div>
          <CardDescription>
            程序**不预置**检测模型——这是你自己的模型工作台。把导出的 onnx 加进来，
            识别页就能选它找物体（v5~v12、v26，以及 YOLOE 开放词汇模型都行）。
            没有模型也不影响认字。
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-2">
          {models.length === 0 ? (
            <div className="rounded-lg bg-neutral-50/70 px-3 py-4 text-[12px] leading-5 text-neutral-500">
              还没有检测模型。两种加法：把 .onnx 丢进下面的「模型目录」，
              或点「添加模型」指向任意位置的文件（只记路径、不复制）。
              加好之后，识别页的「识别方式」里会多出「找物体」这一项。
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
            })
          )}

          <div className="mt-1 flex flex-wrap items-center gap-2">
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
