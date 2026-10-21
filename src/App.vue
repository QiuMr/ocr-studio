<script setup lang="ts">
import { ref, onMounted, computed } from "vue";
import { invoke } from "@tauri-apps/api/core";

type Level = "tiny" | "small" | "medium";

const levels: { key: Level; name: string; desc: string }[] = [
  { key: "tiny", name: "Tiny", desc: "1.5M · 极速，适合端侧/低配" },
  { key: "small", name: "Small", desc: "7.7M · 速度与精度平衡（推荐）" },
  { key: "medium", name: "Medium", desc: "34.5M · 最高精度，适合服务器" },
];

const level = ref<Level>("small");
const device = ref<"directml" | "cpu">("directml");
const port = ref(8848);
const serviceRunning = ref(false);
const busy = ref(false);
const engineInfo = ref<string>("未加载");
const greetMsg = ref("");

const tabs = ["总览", "测试台", "日志", "设置"] as const;
const tab = ref<(typeof tabs)[number]>("总览");

const currentLevel = computed(() => levels.find((l) => l.key === level.value)!);

async function ping() {
  greetMsg.value = await invoke<string>("greet", { name: "OCR" });
}

async function toggleService() {
  busy.value = true;
  // 占位：后续接入 Rust 端 start_http_service / stop_http_service
  await new Promise((r) => setTimeout(r, 600));
  serviceRunning.value = !serviceRunning.value;
  busy.value = false;
}

onMounted(() => {
  ping();
});
</script>

<template>
  <div class="h-full flex flex-col bg-slate-50 text-slate-800">
    <!-- 顶栏 -->
    <header class="h-14 flex items-center gap-3 px-5 border-b border-slate-200 bg-white shrink-0">
      <div class="w-8 h-8 rounded-lg bg-gradient-to-br from-indigo-500 to-cyan-500 flex items-center justify-center text-white font-bold">O</div>
      <div>
        <div class="text-sm font-semibold leading-tight">OCR Studio</div>
        <div class="text-[11px] text-slate-400 leading-tight">开箱即用 · Rust + ONNX Runtime</div>
      </div>
      <nav class="ml-6 flex gap-1">
        <button
          v-for="t in tabs"
          :key="t"
          class="px-3 py-1.5 text-sm rounded-md transition"
          :class="tab === t ? 'bg-indigo-50 text-indigo-600 font-medium' : 'text-slate-500 hover:bg-slate-100'"
          @click="tab = t"
        >{{ t }}</button>
      </nav>
      <div class="ml-auto flex items-center gap-2 text-xs">
        <span class="flex items-center gap-1.5">
          <span class="w-2 h-2 rounded-full" :class="serviceRunning ? 'bg-emerald-500' : 'bg-slate-300'"></span>
          {{ serviceRunning ? '服务运行中' : '服务已停止' }}
        </span>
      </div>
    </header>

    <!-- 内容区 -->
    <main class="flex-1 overflow-auto p-5">
      <!-- 总览 -->
      <section v-if="tab === '总览'" class="grid grid-cols-3 gap-4 max-w-5xl">
        <div class="col-span-2 rounded-xl bg-white p-5 shadow-sm border border-slate-100">
          <h2 class="text-sm font-semibold mb-4">模型档位</h2>
          <div class="grid grid-cols-3 gap-3">
            <button
              v-for="l in levels"
              :key="l.key"
              class="text-left rounded-lg border p-3 transition"
              :class="level === l.key ? 'border-indigo-500 ring-2 ring-indigo-100 bg-indigo-50/40' : 'border-slate-200 hover:border-slate-300'"
              @click="level = l.key"
            >
              <div class="font-medium text-sm">{{ l.name }}</div>
              <div class="text-[11px] text-slate-400 mt-1">{{ l.desc }}</div>
            </button>
          </div>

          <h2 class="text-sm font-semibold mt-6 mb-3">推理设备</h2>
          <div class="flex gap-2">
            <button
              class="px-4 py-2 rounded-lg border text-sm transition"
              :class="device === 'directml' ? 'border-indigo-500 bg-indigo-50 text-indigo-600' : 'border-slate-200'"
              @click="device = 'directml'"
            >DirectML (GPU)</button>
            <button
              class="px-4 py-2 rounded-lg border text-sm transition"
              :class="device === 'cpu' ? 'border-indigo-500 bg-indigo-50 text-indigo-600' : 'border-slate-200'"
              @click="device = 'cpu'"
            >CPU</button>
          </div>
          <p class="text-[11px] text-slate-400 mt-2">当前设备：AMD Radeon Graphics · DirectML 可用</p>
        </div>

        <div class="rounded-xl bg-white p-5 shadow-sm border border-slate-100 flex flex-col">
          <h2 class="text-sm font-semibold mb-3">服务</h2>
          <div class="text-xs text-slate-500 space-y-2">
            <div class="flex justify-between"><span>监听端口</span><input v-model.number="port" type="number" class="w-20 text-right border rounded px-2 py-1" /></div>
            <div class="flex justify-between"><span>引擎</span><span class="text-slate-700">{{ engineInfo }}</span></div>
            <div class="flex justify-between"><span>健康检查</span><span class="text-slate-700">/ocr/health</span></div>
          </div>
          <button
            class="mt-auto w-full py-2 rounded-lg text-sm font-medium text-white transition"
            :class="serviceRunning ? 'bg-rose-500 hover:bg-rose-600' : 'bg-indigo-600 hover:bg-indigo-700'"
            :disabled="busy"
            @click="toggleService"
          >{{ busy ? '处理中…' : serviceRunning ? '停止服务' : '启动服务' }}</button>
        </div>
      </section>

      <!-- 测试台 -->
      <section v-else-if="tab === '测试台'" class="max-w-5xl">
        <div class="rounded-xl border-2 border-dashed border-slate-300 bg-white h-72 flex flex-col items-center justify-center text-slate-400">
          <div class="text-3xl mb-2">🖼️</div>
          <p class="text-sm">拖入 / 粘贴图片开始识别</p>
          <p class="text-[11px] mt-1">（M1 里程碑接通后可用）</p>
        </div>
        <div class="mt-3 text-xs text-slate-400">{{ greetMsg }}</div>
      </section>

      <section v-else-if="tab === '日志'" class="max-w-5xl">
        <div class="rounded-xl bg-slate-900 text-slate-200 p-4 font-mono text-xs h-80 overflow-auto">
          <div>[info] OCR Studio 启动</div>
          <div>[info] 模型档位: {{ currentLevel.name }}</div>
          <div>[info] 推理设备: {{ device }}</div>
          <div class="text-slate-500">// 实时日志将在 M2 接通 tracing → 前端事件</div>
        </div>
      </section>

      <section v-else class="max-w-2xl space-y-4">
        <div class="rounded-xl bg-white p-5 shadow-sm border border-slate-100">
          <h2 class="text-sm font-semibold mb-3">通用</h2>
          <label class="flex items-center justify-between text-sm py-2">
            <span>开机自启</span><input type="checkbox" class="accent-indigo-600" />
          </label>
          <label class="flex items-center justify-between text-sm py-2">
            <span>模板匹配阈值</span><input type="range" min="0.5" max="0.99" step="0.01" value="0.9" class="w-40" />
          </label>
        </div>
      </section>
    </main>
  </div>
</template>
