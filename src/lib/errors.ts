/**
 * 错误的人话翻译（体验优化的核心一层）。
 *
 * 为什么需要它：后端返回的 `{code, msg}` 里，`msg` 是写给**开发者**看的
 * （"图像解码失败: ..."）。小白看到只会觉得「软件坏了」，不知道下一步干嘛。
 * 而 `code` 里藏着「这事该去哪解决」的信息：
 *   - 503  → 模型没装，去模型库下一个
 *   - 429  → 同时在跑的太多了，退避重试（不是故障）
 *   - 1002 → 这个引擎干不了这事，是选错了引擎
 *   - 400  → 参数/图有问题，细分下去还有六七种
 *
 * 唯一事实源：这张表。页面不许自己拼错误文案，也不许直接把 `${e}` 甩给用户。
 */

/** 页面能执行的「下一步」动作。具体怎么跳由页面决定，这里只给语义。 */
export type ErrorActionKind =
  | "open-models" // 去设置页 → 模型库
  | "open-ai" // 去设置页 → AI 控制
  | "open-service" // 回首页看服务开关
  | "open-settings" // 去设置页
  | "retry"; // 重试当前操作

export interface ErrorHelp {
  /** 一句话说清**发生了什么**（给小白看，不出现术语） */
  title: string;
  /** 为什么会这样。可以稍长，但不要甩堆栈 */
  why: string;
  /** 下一步具体做什么。必须是可执行的动作，不能是「请检查配置」这种废话 */
  next: string;
  /** 语气：error=真出错了，warn=用法问题，info=状态类 */
  tone: "error" | "warn" | "info";
  /** 可点的下一步按钮 */
  action?: { kind: ErrorActionKind; label: string };
}

/** 带错误码的接口错误。`api.ts` 里所有失败都抛这个，别再 `throw new Error(msg)`。 */
export class ApiError extends Error {
  readonly code: number;
  /** 出错的端点，排查时有用 */
  readonly endpoint?: string;
  readonly httpStatus?: number;
  /** 后端原文。界面放在「详情」里，不占主视觉 */
  readonly raw: string;

  constructor(opts: { code: number; msg: string; endpoint?: string; httpStatus?: number }) {
    super(opts.msg);
    this.name = "ApiError";
    this.code = opts.code;
    this.endpoint = opts.endpoint;
    this.httpStatus = opts.httpStatus;
    this.raw = opts.msg;
  }
}

/** 从 `catch (e)` 的 unknown 里认出接口错误。 */
export function asApiError(e: unknown): ApiError | null {
  if (e instanceof ApiError) return e;
  // Tauri invoke 抛的是 string 或被包过的对象，这里做一次宽松识别
  if (e && typeof e === "object" && "code" in e && typeof (e as { code: unknown }).code === "number") {
    const o = e as { code: number; msg?: string; message?: string };
    return new ApiError({ code: o.code, msg: o.msg ?? o.message ?? "" });
  }
  return null;
}

/** 取错误的可读文本（拼进日志/复制给 AI 时用）。 */
export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/**
 * 400 的细分。同一个码底下藏着六七种完全不同的问题，
 * 不细分的话「参数错误」四个字等于没说。
 *
 * 顺序有意义：越具体的越靠前。
 */
const BAD_REQUEST_RULES: Array<{ match: string[]; help: Omit<ErrorHelp, "tone"> }> = [
  {
    match: ["尺寸超限", "像素", "单边"],
    help: {
      title: "这张图太大了",
      why: "超过 8000×8000 或 3200 万像素的图会被拦下——解出来是几百 MB 的位图，服务扛不住。",
      next: "先把图裁小，或者用「区域识别」只圈你关心的那块。",
      action: { kind: "retry", label: "换个尺寸再试" },
    },
  },
  {
    match: ["base64"],
    help: {
      title: "图片数据没传对",
      why: "base64 串坏了，或者带了 `data:image/png;base64,` 之外的多余内容。",
      next: "重新生成一次 base64，别手动截断，也别在里面换行。",
    },
  },
  {
    match: ["解码", "不是", "无法解析", "图像格式"],
    help: {
      title: "这个文件不是能读的图",
      why: "后端按 PNG / JPG / BMP 去解，解不开。可能是文件损坏、后缀名不对，或者传的是别的格式。",
      next: "换成 PNG 或 JPG 再试。能用系统图片查看器打开的图一般都没问题。",
    },
  },
  {
    match: ["正则"],
    help: {
      title: "关键词里的正则写错了",
      why: "用 `/xxx/` 形式时，中间的内容必须是合法正则。",
      next: "检查括号有没有配对、特殊字符有没有转义。不确定就先不用斜杠，当普通文字搜。",
    },
  },
  {
    match: ["hwnd", "title", "窗口"],
    help: {
      title: "没指定要截哪个窗口",
      why: "「窗口找图」得知道截哪个窗口，给窗口标题或窗口句柄都行。",
      next: "填上窗口标题（不用填全，包含就行），或者从下拉里选一个已打开的窗口。",
    },
  },
  {
    match: ["region", "区域"],
    help: {
      title: "圈的区域不对",
      why: "区域要在图片范围内，格式是 [x, y, 宽, 高]。",
      next: "重新框一次，别超出图片边界。",
    },
  },
  {
    match: ["缺少", "需要", "必填", "没给"],
    help: {
      title: "少传了必需的参数",
      why: "这个接口有几个参数是非给不可的，漏了一个就跑不起来。",
      next: "看错误详情里点名的那个字段，补上再试。接口页有每个参数的说明。",
    },
  },
];

/** 按 code 分的主体映射。 */
const CODE_HELP: Record<number, Omit<ErrorHelp, "tone"> & { tone: ErrorHelp["tone"] }> = {
  400: {
    title: "请求参数有问题",
    why: "传给接口的字段不对、少了、或者格式不对。",
    next: "看下面的错误详情，里面点名了具体是哪个字段。",
    tone: "warn",
  },
  401: {
    title: "AI 连接码不对",
    why: "AI 接口的每个请求都要带连接码，这个码不对或者已经作废了。",
    next: "去「设置 → AI 控制」复制一个新的连接码。",
    tone: "warn",
    action: { kind: "open-ai", label: "去复制连接码" },
  },
  403: {
    title: "AI 接口没开",
    why: "AI 控制默认是关的（毕竟它能操控你的鼠标键盘），没开就调不了。",
    next: "去「设置 → AI 控制」把开关打开。",
    tone: "info",
    action: { kind: "open-ai", label: "去打开 AI 控制" },
  },
  404: {
    title: "接口名打错了",
    why: "这个地址不存在。错误信息里一般带最相近的正确路径。",
    next: "照着错误详情里的提示改。接口页有完整的清单。",
    tone: "warn",
  },
  405: {
    title: "请求方法用错了",
    why: "接口存在，但它是 POST 你发了 GET（或者反过来）。",
    next: "看接口页上这个接口标的是哪种方法。",
    tone: "warn",
  },
  429: {
    title: "同时在跑的请求太多了",
    why: "同时处理的上限是 32 个，超出的会被立刻挡回来。这是保护，不是故障——排队只会让所有人一起超时。",
    next: "等 1 秒再重试（响应里带了 Retry-After）。批量跑的话把并发降到 32 以内。",
    tone: "info",
    action: { kind: "retry", label: "重试一次" },
  },
  500: {
    title: "识别过程内部出错",
    why: "引擎在正常跑的时候炸了。通常是模型文件损坏、显存不足，或者遇到了极端输入。",
    next: "先去日志页看具体报错。频繁出现的话，去模型库把当前模型重新装一遍。",
    tone: "error",
    action: { kind: "open-models", label: "去模型库" },
  },
  501: {
    title: "这功能还没实现",
    why: "接口位置留好了，但代码还没写。",
    next: "换别的方式达成目的，或者等后续版本。",
    tone: "info",
  },
  503: {
    title: "模型还没装好",
    why: "识别要用的模型文件不在，或者加载失败了。服务本身是好的。",
    next: "去「设置 → 模型库」把需要的模型装上，装完不用重启。",
    tone: "warn",
    action: { kind: "open-models", label: "去模型库安装" },
  },
  1002: {
    title: "这个引擎干不了这件事",
    why: "不同引擎能力不同：OCR 引擎只会认字，让它去「找目标」就做不到。",
    next: "换一个支持这个能力的引擎（装对应模型后可选）。",
    tone: "warn",
    action: { kind: "open-models", label: "看看有哪些引擎" },
  },
  1003: {
    title: "引擎名写错了",
    why: "填的引擎名不存在。引擎名形如 `ppocr:small`、`yolo`。",
    next: "调 `/engine/list` 看实际有哪些引擎，或者去设置页看下拉框。",
    tone: "warn",
  },
};

/** 连不上服务这类「根本没到后端」的错误。 */
const NETWORK_HELP: ErrorHelp = {
  title: "连不上服务",
  why: "请求没到后端。通常是服务没启动，或者端口被别的程序占了。",
  next: "回首页看服务是不是开着，没开就点启动。",
  tone: "warn",
  action: { kind: "open-service", label: "回首页看服务" },
};

/**
 * 从错误文案反推业务码。
 *
 * 为什么需要反推：桌面端走 Tauri `invoke`，而命令层返回的是
 * `Result<T, String>`——**错误码在 Rust 侧就被丢了**，前端只拿到一句文案。
 * 把 44 个命令的签名全改成结构化错误代价太大（还得全量重编），
 * 而 `VisionError` 的文案是有规律的：
 *   "引擎 x 不支持 y" → 1002 / "未知引擎" → 1003 / "引擎未就绪" → 503
 * 所以这里按文案猜码，猜中就走正常映射表，猜不中当 400 处理。
 */
function inferCodeFromText(text: string): number | null {
  const rules: Array<[string[], number]> = [
    [["引擎未就绪", "引擎未加载", "还没有模型", "模型缺失", "没有模型", "未加载"], 503],
    [["未知引擎"], 1003],
    [["不支持"], 1002],
    [["尺寸超限", "像素", "单边"], 400],
    [["base64", "解码", "文件格式", "不支持的文件类型"], 400],
    [["正则"], 400],
    [["hwnd", "title", "窗口没"], 400],
    [["正在运行", "已经在跑", "已达上限", "在途"], 429],
  ];
  for (const [keys, code] of rules) {
    if (keys.some((k) => text.includes(k))) return code;
  }
  return null;
}

/**
 * 判断这条文案是不是**已经写好的白话引导**。
 *
 * 界面上有两类东西共用一个错误位：真错误（`扫码失败`）和前置校验提示
 * （`还没选模板图。模板 = 你要找的那个小图标/小按钮的截图。`）。
 * 后者本身就说清了「缺什么 + 怎么做」，再翻译一遍只会变成一句没用的
 * 「出错了」。所以这类原样展示，只按句号拆成「标题 + 补充」。
 */
function splitGuidance(text: string): ErrorHelp | null {
  const t = text.trim();
  if (!/^(还没|还没选|请先|请选|请先载入|需要)/.test(t)) return null;
  const parts = t.split("。").filter((s) => s.trim().length > 0);
  return {
    title: parts[0] ?? t,
    why: parts.slice(1).join("。"),
    // 这类提示自带做法，不再叠一句「下一步」
    next: "",
    tone: "info",
  };
}

/**
 * 把任意 `catch` 到的东西翻译成人话。
 *
 * 判断顺序：白话引导 → 接口错误码（HTTP 通道）→ 文案反推码（桌面通道）
 * → 网络类 → 兜底。
 */
export function describeError(e: unknown): ErrorHelp {
  // 前置校验提示：本来就是人话，别翻译
  const guidance = splitGuidance(errorText(e));
  if (guidance) return guidance;

  let api = asApiError(e);

  // 桌面通道：命令层把码丢了，用文案猜一个回来
  if (!api) {
    const text = errorText(e);
    const guessed = inferCodeFromText(text);
    if (guessed !== null) {
      api = new ApiError({ code: guessed, msg: text });
    }
  }

  if (api) {
    const base = CODE_HELP[api.code];
    if (api.code === 400) {
      const raw = api.raw;
      const hit = BAD_REQUEST_RULES.find((r) => r.match.some((k) => raw.includes(k)));
      if (hit) return { ...hit.help, tone: "warn" };
    }
    if (base) return base;
    // 有码但表里没有（后端新加的码）：至少别装作什么都不知道
    return {
      title: "出错了",
      why: `后端返回了错误码 ${api.code}。`,
      next: "下面是后端给的原文，看不懂可以直接复制给 AI 问。",
      tone: "error",
    };
  }

  const text = errorText(e);
  const low = text.toLowerCase();
  if (
    low.includes("failed to fetch") ||
    low.includes("networkerror") ||
    low.includes("connection refused") ||
    low.includes("10061") ||
    low.includes("timed out") ||
    low.includes("network")
  ) {
    return NETWORK_HELP;
  }
  if (low.includes("请先载入图片") || low.includes("请先选择")) {
    return {
      title: "还没选图",
      why: "得先有一张图才能干活。",
      next: "点左边上传，或者直接把图拖进窗口。",
      tone: "info",
    };
  }

  return {
    title: "出错了",
    why: text || "发生了没归类的问题。",
    next: "下面是原始信息，看不懂可以直接复制给 AI 问。",
    tone: "error",
  };
}

/**
 * 把错误打包成一段可以直接粘给任意 AI 的提示词。
 *
 * 为什么要自包含：用户可能把这段粘给一个全新的对话，AI 看不到我们的界面，
 * 所以背景、环境、已试过的操作都得写在里面。
 */
export function buildAiHelpPrompt(e: unknown, ctx?: { endpoint?: string; hint?: string }): string {
  const api = asApiError(e);
  const help = describeError(e);
  const lines: string[] = [
    "我在用 OCR Studio（一个跑在本机 127.0.0.1:8848 的 OCR/找图接口服务）时遇到一个错误，帮我看看是什么原因、我该怎么做。",
    "",
    "【错误】",
    `- 人话描述：${help.title}`,
    `- 我理解的原因：${help.why}`,
    `- 错误码：${api ? api.code : "（不是接口错误，可能是前端/网络问题）"}`,
    `- 接口：${api?.endpoint ?? ctx?.endpoint ?? "（未知）"}`,
    `- 后端原文：${api?.raw ?? errorText(e)}`,
  ];
  if (ctx?.hint) {
    lines.push("", "【我在做什么】", ctx.hint);
  }
  lines.push(
    "",
    "【请回答】",
    "1. 用大白话解释最可能的原因；",
    "2. 给出**具体**的解决步骤（点哪里、传什么参数），不要说「检查配置」这种空话；",
    "3. 如果需要代码，给出可以直接运行的 curl 或 Python。",
  );
  return lines.join("\n");
}
