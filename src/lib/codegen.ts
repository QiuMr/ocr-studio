/**
 * 按场景生成调用代码。
 *
 * 为什么需要它：这个软件的本质是**一台本机接口服务**，界面上的「试跑」
 * 只是让用户确认「这条路走得通」。走得通之后他真正要的是「怎么在我的
 * 脚本里跑」——以前界面只给「复制 JSON」，复制走的是**结果数据**，
 * 用户拿到手还是不知道该怎么调，等于把最后一步留给他自己琢磨。
 *
 * 设计：一份场景定义 → 两种语言（Python / curl）。场景与端点清单
 * （`lib/endpoints.ts`）保持一致，新增能力时在这里补一条。
 */

export type Scene =
  | "recognize" // 整图认字
  | "region" // 区域认字
  | "chars" // 逐字定位
  | "table" // 表格
  | "barcode" // 扫码
  | "find-image" // 模板找图（传图）
  | "find-screen" // 模板找图（截屏）
  | "find-window" // 模板找图（截窗口）
  | "find-semantic" // 语义找图
  | "find-prompt" // 提示找图
  | "detect"; // 目标检测

export interface SceneOpts {
  /** 服务地址，形如 `127.0.0.1:8848` */
  endpoint: string;
  /** 区域 [x,y,w,h]（region 场景） */
  region?: [number, number, number, number];
  /** 引擎名（可选） */
  engine?: string;
  /** 置信度下限（检测/找图类） */
  conf?: number;
  /** 匹配阈值（模板找图） */
  minScore?: number;
  /** 关键词（找字） */
  keyword?: string;
  /** 表格模式 */
  mode?: "auto" | "structure" | "detect";
}

/** 每个场景：端点 + 请求体 + 一句「这段代码干什么」 */
interface Def {
  path: string;
  /** 请求体（不含 image 字段，那个单独拼） */
  body: (o: SceneOpts) => Record<string, unknown>;
  /** 结果里该取什么字段 */
  pick: string;
  /** 给小白看的一句说明 */
  note: string;
}

const DEFS: Record<Scene, Def> = {
  recognize: {
    path: "/ocr/shibie",
    body: (o) => (o.engine ? { engine: o.engine } : {}),
    pick: `r["data"]["text"]`,
    note: "把图里的字全认出来，返回整段文本和每一行的位置。",
  },
  region: {
    path: "/ocr/quyuocr",
    body: (o) => ({ region: o.region ?? [0, 0, 400, 200] }),
    pick: `r["data"]["lines"]`,
    note: "只认你圈出来的那一块，比整图识别快，也不会混进框外的字。",
  },
  chars: {
    path: "/ocr/danzijson",
    body: (o) => (o.engine ? { engine: o.engine } : {}),
    pick: `r["data"]["lines"]`,
    note: "除整行外，额外给出每个字的位置（字距不均时会有几像素偏差）。",
  },
  table: {
    path: "/ocr/biaoge",
    body: (o) => ({ mode: o.mode ?? "auto" }),
    pick: `r["data"]["tables"]`,
    note: "把表格图变成行列结构，可以直接拼成 CSV / 写进 Excel。",
  },
  barcode: {
    path: "/vision/qrcode",
    body: () => ({}),
    pick: `r["data"]["codes"]`,
    note: "读出图里的二维码和条形码。一个都没扫到是正常的，不算报错。",
  },
  "find-image": {
    path: "/ocr/tuzhaotu1",
    body: (o) => ({ template: "<模板图base64>", ...(o.minScore ? { min_score: o.minScore } : {}) }),
    pick: `r["data"]["matches"]`,
    note: "在大图里找小图（模板匹配），返回命中位置的中心坐标。",
  },
  "find-screen": {
    path: "/ocr/pingmuzhaotu1",
    body: (o) => ({ template: "<模板图base64>", ...(o.minScore ? { min_score: o.minScore } : {}) }),
    pick: `r["data"]["matches"]`,
    note: "截当前屏幕，再在上面找小图。不用自己截图传进来。",
  },
  "find-window": {
    path: "/ocr/jubingzhaotu1",
    body: (o) => ({ template: "<模板图base64>", title: "<窗口标题>", ...(o.minScore ? { min_score: o.minScore } : {}) }),
    pick: `r["data"]["matches"]`,
    note: "截指定窗口再找小图。窗口标题不用填全，包含即可；也可以传 hwnd。",
  },
  "find-semantic": {
    path: "/ocr/yuyizhaotu",
    body: (o) => ({ source: "screen", ...(o.engine ? { engine: o.engine } : {}), ...(o.conf ? { conf: o.conf } : {}) }),
    pick: `r["data"]["matches"]`,
    note: "用检测模型直接找目标，不用模板图。模型只认得它学过的类别。",
  },
  "find-prompt": {
    path: "/vision/prompt",
    body: (o) => ({
      mode: "visual",
      source: "screen",
      ref_image: "<参考图base64>",
      ref_boxes: [[0, 0, 100, 60]],
      ...(o.conf ? { conf: o.conf } : {}),
    }),
    pick: `r["data"]["matches"]`,
    note: "给一张参考图 + 圈出目标，按这个提示去找。换目标不用换模型。",
  },
  detect: {
    path: "/vision/detect",
    body: (o) => ({ ...(o.engine ? { engine: o.engine } : {}), ...(o.conf ? { conf: o.conf } : {}) }),
    pick: `r["data"]["detections"]`,
    note: "框出图里的物体并给出类别，需要先在设置里加检测模型。",
  },
};

export function sceneNote(s: Scene): string {
  return DEFS[s].note;
}

/** 生成 Python 示例。刻意用 `requests` 而不是裸 urllib——脚本党手边基本都有。 */
function toPython(s: Scene, o: SceneOpts): string {
  const d = DEFS[s];
  const extra = d.body(o);
  const lines = Object.entries(extra).map(([k, v]) => `        "${k}": ${JSON.stringify(v)},`);
  const body = lines.length ? `\n${lines.join("\n")}` : "";

  return `import base64, requests

b64 = base64.b64encode(open("shot.png", "rb").read()).decode()
r = requests.post(
    "http://${o.endpoint}${d.path}",
    json={
        "image": b64,${body}
    },
).json()

# ${d.note}
print(${d.pick})`;
}

/** 生成 curl 示例。base64 太长，用一个占位符让用户自己替换。 */
function toCurl(s: Scene, o: SceneOpts): string {
  const d = DEFS[s];
  const extra = d.body(o);
  const obj: Record<string, unknown> = { image: "<图片base64>", ...extra };
  const json = JSON.stringify(obj, null, 2)
    .split("\n")
    .map((l, i) => (i === 0 ? l : `  ${l}`))
    .join("\\\n");

  return `curl -X POST http://${o.endpoint}${d.path} \\
  -H "Content-Type: application/json" \\
  -d '${json}'

# ${d.note}
# 结果在返回的 data 字段里`;
}

export function buildSample(s: Scene, o: SceneOpts): { python: string; curl: string } {
  return { python: toPython(s, o), curl: toCurl(s, o) };
}
