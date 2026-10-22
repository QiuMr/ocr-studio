/**
 * 接口清单：整个产品的「产品说明」就是这个文件。
 *
 * 首页要按类别给概览、接口页要给完整参数，两处必须同源，
 * 否则改了一边容易忘记另一边（之前 README 和界面就对不上）。
 *
 * 事实来源：`src-tauri/src/api/routes.rs` 的 `build_router()`。
 * 改后端路由时**必须**同步这里。
 */

export type EndpointGroup = "ocr" | "find" | "vision" | "status";

export interface Endpoint {
  method: "GET" | "POST";
  path: string;
  /** 一句话说清干什么，用用户能懂的话 */
  title: string;
  /** 补充说明：什么时候该用它、有什么坑 */
  note?: string;
  /** 关键参数 */
  params?: { name: string; required?: boolean; desc: string }[];
  /** 返回什么 */
  returns?: string;
}

export const GROUP_LABEL: Record<EndpointGroup, string> = {
  ocr: "认字",
  find: "找图",
  vision: "看图",
  status: "状态",
};

export const GROUP_DESC: Record<EndpointGroup, string> = {
  ocr: "把图片里的文字提取成字符串或坐标",
  find: "在一张大图里找东西的位置，返回坐标。模板找图比像素，语义找图靠模型认目标",
  vision: "识别物体类别，或者先框出区域再认字",
  status: "查服务和引擎当前状态，脚本启动时先探一下",
};

export const ENDPOINTS: Endpoint[] = [
  // ------------------------------------------------------------------ 认字
  {
    method: "POST",
    path: "/ocr/shibie",
    title: "整图识别，返回拼好的文本",
    note: "最常用的一条。image 传 base64，返回 text 就是整段文字，按行用 \\n 连起来。",
    params: [
      { name: "image", required: true, desc: "base64 字符串，可带 data URL 前缀" },
      { name: "use_cls", desc: "是否自动翻正倒置文字，默认 true" },
      { name: "engine", desc: "换引擎：ddddocr / yolo，不传用默认 PP-OCR" },
    ],
    returns: "text、boxes、scores、lines、width、height、elapsed_ms",
  },
  {
    method: "POST",
    path: "/ocr/zhaozi",
    title: "找字：只返回包含关键词的行",
    note: "keyword 填普通文字按包含匹配；写成 /正则/ 走正则。找到就省得自己在客户端过滤。",
    params: [
      { name: "image", required: true, desc: "base64 字符串" },
      { name: "keyword", required: true, desc: "要找的文字，或 /正则/" },
    ],
    returns: "keyword、matched（命中数）、hits（命中行的文本+坐标+置信度）",
  },
  {
    method: "POST",
    path: "/ocr/duohangjson",
    title: "多行 JSON：每行的文本与四点坐标",
    note: "适合要把结果喂给别的程序、自己算点击位置的场景。",
    params: [{ name: "image", required: true, desc: "base64 字符串" }],
    returns: "rows[{text, box, score, det_score, rotated}]、width、height",
  },
  {
    method: "POST",
    path: "/ocr/danzijson",
    title: "逐字定位：每个字一个框",
    note: "字框是按识别帧区间推算的，字距不均时有几像素误差，别当像素级真值用。",
    params: [{ name: "image", required: true, desc: "base64 字符串" }],
    returns: "lines[{text, box, chars[{ch, box, score, index}]}]、char_count",
  },
  {
    method: "POST",
    path: "/ocr/quyuocr",
    title: "区域识别：只认指定矩形里的字",
    note: "大图里只关心一小块时用它。返回的框坐标已经平移回原图，不用自己换算；region 超出图片范围会自动截到图内。",
    params: [
      { name: "image", required: true, desc: "base64 字符串" },
      { name: "region", required: true, desc: "要识别的区域 [x, y, w, h]，原图像素坐标" },
      { name: "engine", desc: "换引擎，不传用默认 PP-OCR" },
      { name: "use_cls", desc: "是否自动翻正倒置文字，默认 true" },
    ],
    returns: "region（实际生效的区域）、text、lines、width、height（原图尺寸）",
  },

  // ------------------------------------------------------------------ 找图
  {
    method: "POST",
    path: "/ocr/tuzhaotu1",
    title: "在上传的大图里找小图（精准）",
    note: "精准档要求近乎逐像素一致（相似度 ≥ 0.92）。找不到就往 tuzhaotu2 退。想忽略模板里会变的部分（如按钮上的文字），传 mask 把稳定区域涂白。",
    params: [
      { name: "image", required: true, desc: "大图 base64" },
      { name: "template", required: true, desc: "小图 base64" },
      {
        name: "mask",
        desc: "可选。模板掩码 base64，**白（≥128）= 参与匹配**，只比这块、忽略模板中会变的部分；尺寸须与模板一致（也可用 mask_file 传路径）",
      },
      { name: "mask_threshold", desc: "掩码二值化阈值，默认 128" },
      { name: "min_score", desc: "相似度下限，默认 0.92" },
      { name: "max_results", desc: "最多返回几个位置，默认 1" },
    ],
    returns: "found、x、y、center_x、center_y、score、matched、matches[]",
  },
  {
    method: "POST",
    path: "/ocr/tuzhaotu2",
    title: "在上传的大图里找小图（容错）",
    note: "容错档允许亮度、对比度漂移和轻微压缩噪声（相似度 ≥ 0.72）。",
    params: [
      { name: "image", required: true, desc: "大图 base64" },
      { name: "template", required: true, desc: "小图 base64" },
      { name: "mask", desc: "可选。模板掩码（同 tuzhaotu1）" },
    ],
    returns: "同上",
  },
  {
    method: "POST",
    path: "/ocr/pingmuzhaotu1",
    title: "截整个屏幕找图（精准）",
    note: "不用自己截图：服务内部截屏后直接匹配，返回的是**屏幕绝对坐标**。",
    params: [
      { name: "template", required: true, desc: "小图 base64，或 template_file 传路径" },
      { name: "mask", desc: "可选。模板掩码（同 tuzhaotu1）" },
      { name: "region", desc: "只找某块区域，格式 x,y,w,h" },
    ],
    returns: "同上，坐标已加上区域原点",
  },
  {
    method: "POST",
    path: "/ocr/pingmuzhaotu2",
    title: "截整个屏幕找图（容错）",
    note: "游戏画面有滤镜、亮度变化时用这一档。",
    params: [
      { name: "template", required: true, desc: "小图 base64 或 template_file" },
      { name: "mask", desc: "可选。模板掩码（同 tuzhaotu1）" },
    ],
    returns: "同上",
  },
  {
    method: "POST",
    path: "/ocr/jubingzhaotu1",
    title: "截指定窗口找图（精准）",
    note: "用 hwnd 或窗口标题指定；返回的坐标是该窗口内的相对坐标（配 origin_x/origin_y 可换算）。",
    params: [
      { name: "template", required: true, desc: "小图 base64 或 template_file" },
      { name: "mask", desc: "可选。模板掩码（同 tuzhaotu1）" },
      { name: "hwnd", desc: "窗口句柄，优先于 title" },
      { name: "title", desc: "窗口标题，先精确匹配再子串匹配" },
    ],
    returns: "同上，外加 origin_x / origin_y",
  },
  {
    method: "POST",
    path: "/ocr/jubingzhaotu2",
    title: "截指定窗口找图（容错）",
    note: "窗口被遮挡也没关系：截的是窗口内容，不是屏幕画面。",
    params: [
      { name: "template", required: true, desc: "小图 base64 或 template_file" },
      { name: "mask", desc: "可选。模板掩码（同 tuzhaotu1）" },
    ],
    returns: "同上",
  },
  {
    method: "POST",
    path: "/ocr/yuyizhaotu",
    title: "语义找图：不用模板，让模型直接找目标",
    note: "和上面几条最大的区别是**不用准备模板截图**——换个皮肤、按下态变了照样能找到。代价是只认得模型见过的类别（YOLOE-26 的提示在导出时就烘焙进权重了，换目标要重新导模型）。返回结构和模板找图完全一样，脚本换个路径就能用。",
    params: [
      { name: "source", desc: "去哪找：image（默认）/ screen / window" },
      { name: "image", desc: "source=image 时的搜索图 base64" },
      { name: "engine", desc: "用哪个检测模型，默认 yolo（目录里第一个）" },
      { name: "conf", desc: "置信度阈值 0~1，默认 0.25" },
      { name: "class_filter", desc: "只保留这些类别 id，如 [0,2]；不传 = 全部" },
      { name: "hwnd", desc: "source=window 时的窗口句柄，或改用 title" },
      { name: "max_results", desc: "最多返回几个，默认 1" },
    ],
    returns:
      "与模板找图同构：found、x、y、center_x、center_y、score、matched、matches[]、origin_x/origin_y；额外多 engine 和命中里的 class_id/class_name",
  },

  // ------------------------------------------------------------------ 看图
  {
    method: "POST",
    path: "/vision/detect",
    title: "目标检测：框出物体类别",
    note: "必须显式传 engine: yolo。用通用 COCO 模型时检出的是人/车这类物体，不是文字块。",
    params: [
      { name: "image", required: true, desc: "base64 字符串" },
      { name: "engine", required: true, desc: "固定传 yolo" },
      { name: "conf", desc: "置信度阈值 0~1，默认 0.25" },
      { name: "iou", desc: "NMS 的 IoU 阈值 0~1，默认 0.7" },
    ],
    returns: "count、detections[{box, class_id, class_name, score}]",
  },
  {
    method: "POST",
    path: "/vision/pipeline",
    title: "先框区域，再在框里认字",
    note: "面向自己训练的检测模型：比如先框出「聊天框」，再逐框识别里面的文字。",
    params: [
      { name: "image", required: true, desc: "base64 字符串" },
      { name: "detect_engine", desc: "检测引擎，默认 yolo" },
      { name: "conf", desc: "检测置信度阈值，默认 0.25" },
    ],
    returns: "regions[{box, class_name, score, text, lines[]}]、count",
  },
  {
    method: "POST",
    path: "/vision/qrcode",
    title: "扫码：读出二维码 / 条形码里的内容",
    note: "和 OCR、检测引擎都无关（纯 CPU），引擎没就绪也能用。一张图里有多个码会全部返回。**「一个码都没找到」是正常结果**（count 为 0），不是报错。",
    params: [
      { name: "image", required: true, desc: "base64 字符串" },
      {
        name: "barcode_formats",
        desc: "只找这些码制，如 [\"qr\",\"code128\"]。支持分组写法 1d（全部条形码）/ 2d（全部二维码）。不传 = 全找",
      },
      { name: "try_harder", desc: "是否多花点时间找，默认 true（条形码容易漏，建议保持开）" },
    ],
    returns:
      "count、codes[{text, format, points[[x,y]…], x, y, width, height}]、width、height、elapsed_ms；format 用 ZXing 规范名（QR_CODE / CODE_128 / EAN_13 …）",
  },

  // ------------------------------------------------------------------ 状态
  {
    method: "GET",
    path: "/ocr/health",
    title: "健康检查：服务和引擎是否就绪",
    note: "脚本启动时先调一次，code 为 0 才继续。返回里的 actual_device 才是真正在用的设备。",
    returns: "status、model、device、actual_device、dict_size、port、uptime_sec",
  },
  {
    method: "GET",
    path: "/engine/list",
    title: "列出本机可用的引擎",
    note: "只扫目录、不加载模型，所以很快；能不能真跑起来要看 /ocr/health。",
    returns: "models_root、default_engine、engines[]",
  },
];

/** 按分组取接口，保持 ENDPOINTS 里书写的顺序 */
export function endpointsOf(group: EndpointGroup): Endpoint[] {
  return ENDPOINTS.filter((e) => groupOf(e.path) === group);
}

export function groupOf(path: string): EndpointGroup {
  if (path.startsWith("/vision")) return "vision";
  if (path.includes("zhaotu")) return "find";
  if (path.startsWith("/ocr/") && path !== "/ocr/health") return "ocr";
  return "status";
}
