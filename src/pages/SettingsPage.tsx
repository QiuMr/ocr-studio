import { useEffect, useState } from "react";
import { Info, RotateCcw, Save, Server } from "lucide-react";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../components/ui/card";
import { Label } from "../components/ui/label";
import { Input } from "../components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../components/ui/select";
import { Separator } from "../components/ui/separator";
import { getConfig, modelsRoot, saveConfig } from "../lib/api";
import { isTauri } from "../lib/tauri";
import { DEFAULT_CONFIG, type AppConfig, type Device, type ModelLevel } from "../types";

export function SettingsPage() {
  const [cfg, setCfg] = useState<AppConfig>(DEFAULT_CONFIG);
  const [root, setRoot] = useState("-");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    void (async () => {
      try {
        setCfg(await getConfig());
        setRoot(await modelsRoot());
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

  const onSave = async () => {
    setError(null);
    setSaved(false);
    try {
      await saveConfig(cfg);
      setSaved(true);
    } catch (e) {
      setError(`保存失败: ${e}`);
    }
  };

  return (
    <div className="flex max-w-3xl flex-col gap-5">
      {error && (
        <div className="rounded-md border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-800">
          {error}
        </div>
      )}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Server className="h-4 w-4 text-blue-600" />
            服务与推理
          </CardTitle>
          <CardDescription>保存后立即生效；端口变更会自动重启 HTTP 服务</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          <div className="grid grid-cols-1 gap-5 sm:grid-cols-2">
            <div className="flex flex-col gap-2">
              <Label>默认模型档位</Label>
              <Select
                value={cfg.level}
                onValueChange={(v) => setCfg({ ...cfg, level: v as ModelLevel })}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="tiny">Tiny</SelectItem>
                  <SelectItem value="small">Small</SelectItem>
                  <SelectItem value="medium">Medium</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-2">
              <Label>默认推理设备</Label>
              <Select
                value={cfg.device}
                onValueChange={(v) => setCfg({ ...cfg, device: v as Device })}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="directml">DirectML（GPU）</SelectItem>
                  <SelectItem value="cpu">CPU</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-2">
              <Label>HTTP 服务端口</Label>
              <Input
                type="number"
                min={1024}
                max={65535}
                value={cfg.port}
                onChange={(e) => setCfg({ ...cfg, port: Number(e.target.value) })}
              />
            </div>
            <div className="flex flex-col gap-2">
              <Label>模板匹配阈值（找图，M2 生效）</Label>
              <Input
                type="number"
                step={0.01}
                min={0}
                max={1}
                value={cfg.template_threshold}
                onChange={(e) =>
                  setCfg({ ...cfg, template_threshold: Number(e.target.value) })
                }
              />
            </div>
          </div>

          <div className="flex items-center gap-3">
            <Button onClick={() => void onSave()}>
              <Save className="h-4 w-4" />
              保存配置
            </Button>
            <Button variant="outline" onClick={() => setCfg(DEFAULT_CONFIG)}>
              <RotateCcw className="h-4 w-4" />
              恢复默认
            </Button>
            {saved && <span className="text-sm text-emerald-600">已保存并生效</span>}
          </div>
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <Info className="h-4 w-4 text-blue-600" />
            关于
          </CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-3 text-sm text-neutral-600">
          <div className="flex justify-between">
            <span>模型根目录</span>
            <code className="selectable font-mono text-xs text-neutral-700">{root}</code>
          </div>
          <Separator />
          <div className="flex justify-between">
            <span>配置文件</span>
            <code className="selectable font-mono text-xs text-neutral-700">
              {"<应用数据目录>/config.json"}
            </code>
          </div>
          <Separator />
          <div className="flex justify-between">
            <span>字体</span>
            <span>MiSans（小米，落地于 src/assets/fonts）</span>
          </div>
          <Separator />
          <div>
            <p className="mb-2 text-xs text-neutral-500">运行测试：</p>
            <code className="selectable block rounded-md bg-neutral-50 px-3 py-2 font-mono text-xs text-neutral-700">
              cargo test --lib &amp;&amp; cargo test --test pipeline_match --test tensor_diff --test
              api_smoke -- --ignored
            </code>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
