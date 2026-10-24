//! AI 工具注册表与实现。
//!
//! 设计原则（照搬用户另一个项目里已经跑顺的那套）：
//! - 工具名用动词开头、参数用 snake_case（大模型对这两个约定最熟）；
//! - 每个工具都要**人话 summary**：AI 念给用户听的就是它，要说清"做了什么、结果如何"；
//! - `risk` 三档：readonly（只读）/ write（改程序状态或读屏幕）/ dangerous（动服务配置、
//!   或在用户桌面上真的点一下/打字）。写操作全部写审计日志，用户能在运行日志里看到
//!   AI 干了什么；
//! - 参数解析**刻意宽松**：AI 很容易把数字写成字符串、把布尔写成 "true"，一律容错接收；
//!   区间用 clamp 夹住。报错必须告诉 AI 下一步怎么做。
//!
//! **坐标约定（容易出错，务必遵守）**：所有返回的坐标都是**屏幕绝对坐标**，
//! 可以直接喂给 `click_at`。`origin_x/origin_y` 是这次取图的左上角在屏幕上的位置
//! （传图给 AI 时用 0,0；截屏/截窗口时是屏/窗原点，多显示器时可能为负）。
//!
//! **明确不提供**（要么自伤要么动系统，这几件事让人来点界面）：
//! 退出程序、改开机自启注册表、改 AI 开关本身与连接码。
//! 做法是**根本不注册**，不靠运行时黑名单。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use image::{DynamicImage, RgbImage};
use serde_json::{json, Value};

use super::routes::{clamp_region, to_table_dtos};
use super::state::AppState;
use crate::api::dto::{decode_base64_image, decode_image, OcrOptions};
use crate::config::{Device, ModelLevel};
use crate::engine::prompt::PromptInput;
use crate::engine::DetectOptions;

pub const RISK_READONLY: &str = "readonly";
pub const RISK_WRITE: &str = "write";
pub const RISK_DANGEROUS: &str = "dangerous";

/// 一个参数的说明（给 AI 看的 schema，不是校验器）。
pub struct Arg {
    pub name: &'static str,
    pub desc: &'static str,
    pub required: bool,
    pub default: &'static str,
}

const fn arg(name: &'static str, desc: &'static str, required: bool, default: &'static str) -> Arg {
    Arg {
        name,
        desc,
        required,
        default,
    }
}

pub struct Tool {
    pub name: &'static str,
    pub summary: &'static str,
    pub risk: &'static str,
    pub args: &'static [Arg],
}

const fn tool(
    name: &'static str,
    summary: &'static str,
    risk: &'static str,
    args: &'static [Arg],
) -> Tool {
    Tool {
        name,
        summary,
        risk,
        args,
    }
}

// ---------------------------------------------------------------- 取图通用参数说明

/// 截屏 / 截窗口 / 读本地图，这几件事的参数在三类工具里是同一套，单独抽出来复用。
const A_SOURCE: Arg = arg(
    "source",
    "图从哪来：screen（截屏，默认）/ window（截某个窗口，配 title 或 hwnd）",
    false,
    "screen",
);
const A_TITLE: Arg = arg(
    "title",
    "窗口标题（source=window 时用，可以只写一部分，如 \"记事本\"）。先调 list_windows 看有哪些",
    false,
    "",
);
const A_HWND: Arg = arg("hwnd", "窗口句柄（比 title 精确，list_windows 里有）", false, "");
const A_REGION: Arg = arg(
    "region",
    "只处理图上的一块 [x,y,w,h]（原图像素）。截屏时是屏幕坐标，传图时是图内坐标",
    false,
    "",
);

/// 工具清单（**顺序即展示顺序**：先只读、后写、最后危险，方便 AI 建立风险意识）。
pub const TOOLS: &[Tool] = &[
    // ================================================================ 只读
    tool(
        "get_status",
        "看服务在不在跑、端口、引擎、推理设备、模型目录。排查「调不通」时先调它。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "list_endpoints",
        "列出所有 HTTP 端点（路径 + 方法）。要写调用代码时先调它，别凭记忆写路径。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "list_engines",
        "列出可用引擎与各自的能力（认字 / 检测 / 提示找图），以及模型根目录。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "get_config",
        "看当前配置（端口、模型档、设备、模板阈值、是否允许浏览器调用、AI 接口开关）。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "get_logs",
        "看最近的运行日志（新的在后）。用户说「它刚才报错了」时用它看现场。",
        RISK_READONLY,
        &[arg("limit", "最多几条，1-500（默认 50）", false, "50")],
    ),
    tool(
        "list_targets",
        "列出中文参考库里已经教过的目标名。找界面元素前先看一眼有没有现成的。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "list_windows",
        "列出当前打开的窗口（标题、句柄、位置、尺寸、哪个在前台）。要操作某个软件之前先调它，拿到 title/hwnd。",
        RISK_READONLY,
        &[arg("title", "只看标题里包含这几个字的窗口（可空 = 全部）", false, "")],
    ),
    tool(
        "list_models",
        "看模型库里每个能力装没装、多大、装上能用哪些接口，以及正在下载的进度。缺能力时用它确认。",
        RISK_READONLY,
        &[arg("refresh", "true = 强制联网刷新清单（默认 false 用缓存）", false, "false")],
    ),
    tool(
        "check_update",
        "查这个工具本身有没有新版本。只查不装，装不装由用户决定。",
        RISK_READONLY,
        &[],
    ),
    tool(
        "wait",
        "等一会儿再继续（毫秒，最多 10000）。点完按钮等界面加载、等窗口出来时用，别连续猛调。",
        RISK_READONLY,
        &[arg("ms", "等多久，1-10000（默认 500）", false, "500")],
    ),

    // ================================================================ 写
    tool(
        "screenshot",
        "截一张图存成文件，返回文件路径、尺寸和原点坐标。可以截全屏、截某个窗口、或只截一块。后续 ocr_image / read_table 等可以直接用这个路径当输入。**会读取用户屏幕内容**。",
        RISK_WRITE,
        &[
            A_SOURCE,
            A_TITLE,
            A_HWND,
            A_REGION,
        ],
    ),
    tool(
        "ocr_image",
        "认出图里所有文字。图可以是本地路径（image_path）、base64（image），也可以截屏/截窗口。返回整段文本 + 每一行的屏幕坐标。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片绝对路径", false, ""),
            arg("image", "图片 base64（与 image_path 二选一）", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            A_REGION,
            arg("max_lines", "最多返回几行，1-200（默认 50）", false, "50"),
        ],
    ),
    tool(
        "find_text",
        "在屏幕上找到某段文字在哪（先认字再筛），返回每处命中的坐标。想知道「确定按钮在哪」「有没有报错提示」就用它——比 find_target 省事，不需要先教。",
        RISK_WRITE,
        &[
            arg(
                "keyword",
                "要找的文字。默认是「包含即命中」；用 /正则/ 两边加斜杠可以写正则，如 /\\d+个结果/",
                true,
                "",
            ),
            arg("image_path", "在本地这张图里找", false, ""),
            arg("image", "图片 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            A_REGION,
            arg("max_results", "最多返回几处，1-200（默认 20）", false, "20"),
        ],
    ),
    tool(
        "find_target",
        "按**教过的名字**找东西的位置。名字必须是先用 teach_target 教过的（list_targets 能看有哪些）；没教过会明确报错并列出可用名字。不给图就截屏。",
        RISK_WRITE,
        &[
            arg("names", "教过的名字，逗号分隔，如 \"提交按钮,搜索框\"", true, ""),
            arg("image_path", "在本地这张图里找", false, ""),
            arg("image", "图片 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            arg("conf", "置信度阈值 0-1（默认 0.25），找不到就调低", false, "0.25"),
            arg("max_results", "最多返回几个，1-64（默认 8）", false, "8"),
        ],
    ),
    tool(
        "find_image",
        "模板找图：拿一张小图在大图/屏幕里找位置（比像素，要求长得几乎一样）。目标图不给就截屏。",
        RISK_WRITE,
        &[
            arg("template_path", "小图（模板）的本地路径", false, ""),
            arg("template", "小图 base64（与 template_path 二选一）", false, ""),
            arg("image_path", "大图本地路径", false, ""),
            arg("image", "大图 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            A_REGION,
            arg("threshold", "相似度阈值 0-1（默认 0.9）", false, "0.9"),
            arg("max_results", "最多返回几个，1-64（默认 1）", false, "1"),
        ],
    ),
    tool(
        "detect_objects",
        "通用目标检测：找图里的物体（人、车、表格、按钮……取决于装了哪个检测模型），返回类别名 + 框 + 分数。想按类别找东西、又没现成模板时用它。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片路径", false, ""),
            arg("image", "图片 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            arg("classes", "只看这些类别（逗号分隔，按名字包含匹配，如 \"人,车\"）；空 = 全部", false, ""),
            arg("conf", "置信度阈值 0-1（默认 0.3）", false, "0.3"),
            arg("max_results", "最多返回几个，1-200（默认 30）", false, "30"),
        ],
    ),
    tool(
        "read_table",
        "读表格：整页里自动找表格 → 认出结构 → 填上文字。返回 HTML 表格、TSV 文本（可直接粘 Excel）和每个单元格坐标。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片路径", false, ""),
            arg("image", "图片 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            arg(
                "mode",
                "auto（默认，先找表格再识别）/ structure（整图当一张表）/ detect（只给表格位置）",
                false,
                "auto",
            ),
            arg("with_text", "要不要把文字填进单元格（默认 true）", false, "true"),
        ],
    ),
    tool(
        "read_qrcode",
        "读出图里的二维码 / 条形码内容。不给图就截屏。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片路径", false, ""),
            arg("image", "图片 base64", false, ""),
            A_SOURCE,
            A_TITLE,
            A_HWND,
            arg("try_harder", "图糊或扫不出来时设 true，会多试几遍（慢一点）", false, "true"),
        ],
    ),
    tool(
        "teach_target",
        "教它认一个新目标：给一张参考图 + 图上的一个框 + 一个中文名。教完之后 find_target 就能直接用这个中文名找。**遇到没见过的目标就用这个先教一次**。",
        RISK_WRITE,
        &[
            arg("name", "中文名，如 \"提交按钮\"", true, ""),
            arg("image_path", "参考图的本地路径（**整张图**，通常是 screenshot 返回的）", false, ""),
            arg("image", "参考图 base64", false, ""),
            arg("box", "参考框 [x1,y1,x2,y2]，参考图左上角为原点（截屏得到的图就是屏幕坐标）", true, ""),
        ],
    ),
    tool(
        "forget_target",
        "忘掉一个教过的目标（按名字）。教错了、或者目标已经不用了就用它清掉。",
        RISK_WRITE,
        &[arg("name", "要忘掉的名字", true, "")],
    ),
    tool(
        "focus_window",
        "把某个窗口切到最前面（最小化的先还原）。要往某个软件里打字/点击之前，必须先调它——输入只会进前台窗口。",
        RISK_WRITE,
        &[A_TITLE, A_HWND],
    ),

    // ================================================================ 危险
    tool(
        "move_mouse",
        "把鼠标移到某个坐标（只移动不点击）。悬停展开菜单时用。**会在用户桌面上真的动鼠标**。",
        RISK_DANGEROUS,
        &[arg("x", "屏幕 X 坐标", true, ""), arg("y", "屏幕 Y 坐标", true, "")],
    ),
    tool(
        "click_at",
        "在屏幕某个坐标点一下（支持左右中键、双击）。**会在用户电脑上真的点下去**，先跟用户确认再动手。",
        RISK_DANGEROUS,
        &[
            arg("x", "屏幕 X 坐标", true, ""),
            arg("y", "屏幕 Y 坐标", true, ""),
            arg("button", "left（默认）/ right / middle", false, "left"),
            arg("count", "点几下，1-3（2 = 双击，默认 1）", false, "1"),
        ],
    ),
    tool(
        "drag_mouse",
        "从一个点按住拖到另一个点（拖文件、拖滚动条、框选）。**会真的操作鼠标**。",
        RISK_DANGEROUS,
        &[
            arg("x", "起点 X", true, ""),
            arg("y", "起点 Y", true, ""),
            arg("to_x", "终点 X", true, ""),
            arg("to_y", "终点 Y", true, ""),
            arg("button", "left（默认）/ right / middle", false, "left"),
        ],
    ),
    tool(
        "scroll",
        "在某个位置滚轮（不给坐标就在鼠标当前位置滚）。delta 正数向上、负数向下，一格约 120。",
        RISK_DANGEROUS,
        &[
            arg("x", "屏幕 X（可空 = 当前鼠标位置）", false, ""),
            arg("y", "屏幕 Y（可空 = 当前鼠标位置）", false, ""),
            arg("delta", "滚动量，正数向上 / 负数向下（默认 -120，即向下滚一格）", false, "-120"),
        ],
    ),
    tool(
        "type_text",
        "往当前前台窗口里打一段文字（支持中文）。**会真的往用户正在用的软件里输入**，先确认焦点窗口对不对。",
        RISK_DANGEROUS,
        &[arg("text", "要输入的文字", true, "")],
    ),
    tool(
        "press_key",
        "按一个键或组合键，如 \"enter\"、\"ctrl+s\"、\"alt+f4\"、\"ctrl+shift+esc\"。**会真的按键**。",
        RISK_DANGEROUS,
        &[arg("keys", "按键，多个键用 + 连接", true, "")],
    ),
    tool(
        "install_model",
        "装一个模型（按 id，list_models 里有）。大模型有一两百 MB，会**在后台下载**，立刻返回，用 list_models 看进度。",
        RISK_DANGEROUS,
        &[arg("id", "模型 id，如 table / ddddocr / medium", true, "")],
    ),
    tool(
        "remove_model",
        "卸载一个模型（删掉 models 下对应的目录，腾空间）。正在用它的话相关接口会改报缺模型。",
        RISK_DANGEROUS,
        &[arg("id", "模型 id", true, "")],
    ),
    tool(
        "server_start",
        "启动 HTTP 服务。本来就开着的话直接返回端口。",
        RISK_DANGEROUS,
        &[],
    ),
    tool(
        "server_stop",
        "停止 HTTP 服务。**会让正在调用接口的所有脚本全部断掉**，动之前先跟用户确认。",
        RISK_DANGEROUS,
        &[],
    ),
    tool(
        "set_port",
        "改服务端口。会**重启服务**，改完地址就变了，要提醒用户。",
        RISK_DANGEROUS,
        &[arg("port", "新端口，1024-65535", true, "")],
    ),
    tool(
        "set_level",
        "换模型档位（tiny / small / medium）。会重新加载模型，头一次慢。",
        RISK_DANGEROUS,
        &[arg("level", "tiny / small / medium", true, "")],
    ),
    tool(
        "set_device",
        "换推理设备（directml 用显卡 / cpu）。会重新加载模型。",
        RISK_DANGEROUS,
        &[arg("device", "directml（或 gpu） / cpu", true, "")],
    ),
    tool(
        "set_threshold",
        "改模板找图的默认相似度阈值（0-1，默认 0.9）。",
        RISK_DANGEROUS,
        &[arg("threshold", "0-1", true, "")],
    ),
];

/// 给 `/ai/v1/tools` 用的 JSON。
pub fn catalog() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|t| {
            let args: Vec<Value> = t
                .args
                .iter()
                .map(|a| {
                    json!({
                        "name": a.name,
                        "desc": a.desc,
                        "required": a.required,
                        "default": a.default,
                    })
                })
                .collect();
            json!({
                "name": t.name,
                "summary": t.summary,
                "risk": t.risk,
                "args": args,
            })
        })
        .collect()
}

/// 某个工具的风险档（给审计日志用）。
pub fn risk_of(name: &str) -> Option<&'static str> {
    TOOLS.iter().find(|t| t.name == name).map(|t| t.risk)
}

/// 按风险档分组计数，界面与提示词里用（避免两边各写一份数字）。
pub fn risk_counts() -> (usize, usize, usize) {
    let mut r = (0, 0, 0);
    for t in TOOLS {
        match t.risk {
            RISK_READONLY => r.0 += 1,
            RISK_WRITE => r.1 += 1,
            _ => r.2 += 1,
        }
    }
    r
}

/// 提示词里「你可以帮我…」的一句话（人话，不列工具名）。
pub fn highlights() -> Vec<&'static str> {
    vec![
        "看屏幕上有什么、认出上面的字、按文字定位（「确定」按钮在哪）",
        "按中文名找界面元素的位置（按钮、图标、输入框）",
        "教它认新目标（框一次、起个中文名，之后就能找）",
        "读表格成 Excel 可粘的文本 / 读二维码和条形码",
        "真的动手：点按钮、拖拽、滚动、打字、按快捷键",
        "装/卸模型（表格识别、更大的识别模型……按需下载）",
        "看服务状态与运行日志、查有没有新版本、启停服务和换设备",
    ]
}

// ---------------------------------------------------------------------------
// 参数解析（宽松）
// ---------------------------------------------------------------------------

pub(crate) fn s(args: &Value, key: &str) -> Option<String> {
    match args.get(key)? {
        Value::String(v) => {
            let t = v.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 取数字：字符串、数字、布尔都收（AI 写 `conf: "0.3"` 很常见）。
fn f(args: &Value, key: &str) -> Option<f64> {
    match args.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(v) => v.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// 必填数字。缺失时报错要写清「该填什么」，AI 才能自我修正。
pub(crate) fn req_i(args: &Value, key: &str, hint: &str) -> Result<i32, String> {
    match f(args, key) {
        Some(v) if v.is_finite() => Ok(v.round() as i32),
        Some(_) => Err(format!("{key} 不是有效数字")),
        None => Err(format!("缺少 {key}（{hint}）")),
    }
}

fn b(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(v) => Some(*v),
        Value::Number(n) => Some(n.as_f64().unwrap_or(0.0) != 0.0),
        Value::String(v) => match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// 取数字数组（框、区域等）。
fn nums(args: &Value, key: &str) -> Option<Vec<f64>> {
    let v = args.get(key)?;
    match v {
        Value::Array(a) => {
            let out: Option<Vec<f64>> = a
                .iter()
                .map(|x| match x {
                    Value::Number(n) => n.as_f64(),
                    Value::String(t) => t.trim().parse::<f64>().ok(),
                    _ => None,
                })
                .collect();
            out.filter(|v| !v.is_empty())
        }
        // "10,20,100,40" 这种也收
        Value::String(t) => {
            let out: Option<Vec<f64>> = t
                .split([',', '，', ' '])
                .filter(|p| !p.trim().is_empty())
                .map(|p| p.trim().parse::<f64>().ok())
                .collect();
            out.filter(|v| !v.is_empty())
        }
        _ => None,
    }
}

/// `[x, y, w, h]`，四个数、宽高为正。给的是「空数组/缺省」时返回 None。
fn num4(args: &Value, key: &str) -> Option<[i32; 4]> {
    let v = nums(args, key)?;
    if v.len() != 4 {
        return None;
    }
    Some([v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32])
}

/// 逗号分隔的字符串列表（中英文逗号、分号、竖线都认）。
fn list_of(args: &Value, key: &str) -> Vec<String> {
    s(args, key)
        .map(|raw| {
            raw.split([',', '，', ';', '；', '|'])
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn clamp_f(v: f64, lo: f64, hi: f64) -> f64 {
    if !v.is_finite() {
        return lo;
    }
    v.clamp(lo, hi)
}

fn clamp_i(v: f64, lo: usize, hi: usize) -> usize {
    clamp_f(v, lo as f64, hi as f64).round() as usize
}

// ---------------------------------------------------------------------------
// 图片来源（带屏幕原点）
// ---------------------------------------------------------------------------

/// 取到的图 + 它在屏幕上的原点。
///
/// 原点不是 0 的时候，图内坐标 + 原点 = 屏幕绝对坐标。**只有带上它，
/// 找出来的位置才能直接拿去点** —— 这是 AI 闭环的最后一环。
struct Loaded {
    img: DynamicImage,
    ox: i32,
    oy: i32,
}

impl Loaded {
    fn from_file(img: DynamicImage) -> Self {
        Loaded { img, ox: 0, oy: 0 }
    }

    fn rect(args: &Value) -> Option<[i32; 4]> {
        num4(args, "region")
    }

    /// 按 region 裁一块，并把原点平移到裁剪处。
    fn crop(self, region: Option<[i32; 4]>) -> Result<Self, String> {
        let Some([x, y, w, h]) = region else {
            return Ok(self);
        };
        let (iw, ih) = (self.img.width(), self.img.height());
        let Some([cx, cy, cw, ch]) = clamp_region(x, y, w, h, iw, ih) else {
            return Err(format!(
                "region [{x},{y},{w},{h}] 与这张图（{iw}x{ih}）没有有效交集"
            ));
        };
        let img = self
            .img
            .crop_imm(cx as u32, cy as u32, cw as u32, ch as u32);
        Ok(Loaded {
            img,
            ox: self.ox + cx,
            oy: self.oy + cy,
        })
    }
}

/// 取图。优先级：image_path / image（本地图）> window（截窗口）> screen（截屏）。
fn load_image(args: &Value) -> Result<Loaded, String> {
    let region = Loaded::rect(args);

    if let Some(p) = s(args, "image_path") {
        let img = image::open(std::path::Path::new(&p)).map_err(|e| {
            format!("读不了图片 {p}：{e}（确认路径存在，Windows 路径用 / 或 \\\\）")
        })?;
        return Loaded::from_file(img).crop(region);
    }
    if let Some(b64) = s(args, "image") {
        let raw = decode_base64_image(&b64)?;
        let img = decode_image(&raw).map_err(|e| e.to_string())?;
        return Loaded::from_file(img).crop(region);
    }

    let source = s(args, "source").unwrap_or_else(|| "screen".to_string());
    match source.trim().to_ascii_lowercase().as_str() {
        "window" | "jubing" => {
            let hwnd = resolve_hwnd(args)?;
            let win = crate::capture::window_region(hwnd).map_err(|e| format!("取窗口位置失败：{e}"))?;
            let img = crate::capture::capture_window(hwnd).map_err(|e| format!("截窗口失败：{e}"))?;
            Loaded {
                img: DynamicImage::ImageRgb8(img),
                ox: win.x,
                oy: win.y,
            }
            .crop(region)
        }
        _ => {
            let vs = crate::capture::virtual_screen();
            let img = crate::capture::capture_region(region.map(|[x, y, w, h]| {
                crate::capture::ScreenRegion {
                    x,
                    y,
                    width: w,
                    height: h,
                }
            }))
            .map_err(|e| format!("截屏失败：{e}"))?;
            // 截了区域的话，原点就是区域左上角；否则是整个虚拟屏的左上角
            let (ox, oy) = match region {
                Some([x, y, _, _]) => (x, y),
                None => (vs.x, vs.y),
            };
            Ok(Loaded {
                img: DynamicImage::ImageRgb8(img),
                ox,
                oy,
            })
        }
    }
}

/// 句柄优先，其次标题（精确 → 子串）。与 HTTP 层同一套规则，避免两处解析不一致。
fn resolve_hwnd(args: &Value) -> Result<isize, String> {
    if let Some(h) = f(args, "hwnd") {
        if h as isize != 0 {
            return Ok(h as isize);
        }
    }
    if let Some(title) = s(args, "title") {
        if let Some(h) = crate::capture::find_window(&title) {
            return Ok(h);
        }
        if let Some(h) = crate::capture::find_window_contains(&title) {
            return Ok(h);
        }
        let titles: Vec<String> = crate::capture::list_windows()
            .into_iter()
            .map(|w| w.title)
            .take(15)
            .collect();
        return Err(format!(
            "找不到标题含 {title:?} 的可见窗口。当前开着的有：{}。\
             也可以先调 list_windows 拿 hwnd。",
            if titles.is_empty() {
                "（一个都没有）".to_string()
            } else {
                titles.join("、")
            }
        ));
    }
    Err("source=window 需要 title 或 hwnd（先调 list_windows 看有哪些窗口）".to_string())
}

fn load_rgb(args: &Value) -> Result<Loaded, String> {
    let l = load_image(args)?;
    Ok(Loaded {
        img: DynamicImage::ImageRgb8(l.img.to_rgb8()),
        ox: l.ox,
        oy: l.oy,
    })
}

fn save_png(dir: &std::path::Path, prefix: &str, img: &RgbImage) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("建不了目录 {}：{e}", dir.display()))?;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let p = dir.join(format!("{prefix}_{ms}.png"));
    img.save(&p).map_err(|e| format!("图存不下来（{}）：{e}", p.display()))?;
    Ok(p)
}

// ---------------------------------------------------------------------------
// 模型安装进度（AI 后台下载用）
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct InstallState {
    done: u64,
    total: u64,
    phase: String,
}

fn installs() -> &'static Mutex<HashMap<String, InstallState>> {
    static M: OnceLock<Mutex<HashMap<String, InstallState>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_install(id: &str, done: u64, total: u64, phase: &str) {
    let mut g = installs().lock().unwrap_or_else(|e| e.into_inner());
    g.insert(
        id.to_string(),
        InstallState {
            done,
            total,
            phase: phase.to_string(),
        },
    );
}

fn clear_install(id: &str) {
    let mut g = installs().lock().unwrap_or_else(|e| e.into_inner());
    g.remove(id);
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

pub async fn exec(
    state: &Arc<AppState>,
    name: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    match name {
        // ============================================================ 只读
        "get_status" => {
            let (running, port, level, device, allow_browser, ai_enabled) = {
                let cfg = state.cfg.read().unwrap_or_else(|e| e.into_inner());
                (
                    state.is_running(),
                    cfg.port,
                    format!("{:?}", cfg.level).to_lowercase(),
                    format!("{:?}", cfg.device).to_lowercase(),
                    cfg.allow_browser,
                    cfg.ai_enabled,
                )
            };
            let health = state.health();
            let data = json!({
                "running": running,
                "port": port,
                "base_url": format!("http://127.0.0.1:{port}"),
                "level": level,
                "device": device,
                "actual_device": health.as_ref().map(|h| format!("{:?}", h.actual_device).to_lowercase()),
                "engine_loaded": health.is_some(),
                "dict_size": health.as_ref().map(|h| h.dict_size),
                "models_root": state.models_root.display().to_string(),
                "allow_browser": allow_browser,
                "ai_enabled": ai_enabled,
            });
            let human = if running {
                format!("服务在跑，地址 http://127.0.0.1:{port}，引擎 {level}（{device}）。")
            } else {
                format!("服务没启动（端口 {port}）。需要的话调 server_start。")
            };
            Ok((human, data))
        }

        "list_endpoints" => {
            let items: Vec<Value> = ENDPOINT_METHODS
                .iter()
                .map(|(p, m)| json!({ "path": p, "method": m }))
                .collect();
            Ok((
                format!("共 {} 个 HTTP 端点，请求体上限 32MB，信封是 {{code,msg,data}}。", items.len()),
                json!({ "endpoints": items, "count": items.len() }),
            ))
        }

        "list_engines" => {
            let engines = state.engine_status_list();
            let items: Vec<Value> = engines
                .iter()
                .map(|e| serde_json::to_value(crate::api::dto::EngineItemDto::from(e)).unwrap_or_default())
                .collect();
            Ok((
                format!("共 {} 个引擎，模型在 {}。", items.len(), state.models_root.display()),
                json!({
                    "engines": items,
                    "models_root": state.models_root.display().to_string(),
                    "default": crate::engine::EngineSpec::ppocr(
                        state.cfg.read().unwrap_or_else(|e| e.into_inner()).level
                    ).key(),
                    "note": state.prompt_loaded_note(),
                }),
            ))
        }

        "get_config" => {
            let cfg = state.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
            let data = serde_json::to_value(&cfg).unwrap_or_default();
            Ok((
                format!(
                    "端口 {}、模型档 {:?}、设备 {:?}、模板阈值 {}。",
                    cfg.port, cfg.level, cfg.device, cfg.template_threshold
                ),
                data,
            ))
        }

        "get_logs" => {
            let limit = clamp_i(f(args, "limit").unwrap_or(50.0), 1, 500);
            let all = crate::logbuf::snapshot();
            let start = all.len().saturating_sub(limit);
            let items: Vec<Value> = all[start..]
                .iter()
                .map(|l| json!({ "ts_ms": l.ts_ms, "level": l.level, "message": l.message }))
                .collect();
            Ok((
                format!("最近 {} 条日志（共缓存 {} 条）。", items.len(), all.len()),
                json!({ "logs": items, "count": items.len() }),
            ))
        }

        "list_targets" => {
            let refs = state.refs.lock().unwrap_or_else(|e| e.into_inner());
            let items: Vec<Value> = refs
                .list()
                .iter()
                .map(|e| json!({ "name": e.name, "box": e.rect, "width": e.width, "height": e.height }))
                .collect();
            Ok((
                if items.is_empty() {
                    "参考库还是空的。遇到没见过的中文目标，用 teach_target 教一次。".to_string()
                } else {
                    format!(
                        "已经教过 {} 个：{}。这些名字可以直接给 find_target 用。",
                        items.len(),
                        refs.list().iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join("、")
                    )
                },
                json!({ "targets": items, "count": items.len() }),
            ))
        }

        "list_windows" => {
            crate::capture::ensure_dpi_aware();
            let filter = s(args, "title").unwrap_or_default();
            let fg = crate::input::foreground_window();
            let wins: Vec<Value> = crate::capture::list_windows()
                .into_iter()
                .filter(|w| filter.is_empty() || w.title.contains(&filter))
                .map(|w| {
                    json!({
                        "title": w.title,
                        "hwnd": w.hwnd,
                        "x": w.rect.x,
                        "y": w.rect.y,
                        "width": w.rect.width,
                        "height": w.rect.height,
                        "foreground": w.hwnd == fg,
                    })
                })
                .collect();
            Ok((
                if wins.is_empty() {
                    if filter.is_empty() {
                        "现在没有可见窗口。".to_string()
                    } else {
                        format!("没有标题含 {filter:?} 的窗口。去掉 title 参数可看全部。")
                    }
                } else {
                    format!("当前开着 {} 个窗口，最前面的是「{}」。", wins.len(),
                        wins.iter().find(|w| w["foreground"] == json!(true))
                            .and_then(|w| w["title"].as_str()).unwrap_or("（未知）"))
                },
                json!({ "windows": wins, "count": wins.len() }),
            ))
        }

        "list_models" => {
            let refresh = b(args, "refresh").unwrap_or(false);
            let data_dir = state.data_dir.clone();
            let (cat, online) = tokio::task::spawn_blocking(move || {
                crate::modellib::load_catalog(&data_dir, refresh)
            })
            .await
            .map_err(|e| format!("读清单任务异常：{e}"))?;

            let statuses = crate::modellib::statuses(&state.models_root, &cat, online);
            let mut missing = 0usize;
            let mut installed = 0usize;
            let items: Vec<Value> = statuses
                .iter()
                .map(|m| {
                    if m.installed { installed += 1 } else { missing += 1 }
                    let prog = {
                        let g = installs().lock().unwrap_or_else(|e| e.into_inner());
                        g.get(&m.id).cloned()
                    };
                    let pct = prog.as_ref().map(|p| {
                        if p.total == 0 { 0.0 } else { (p.done as f64 / p.total as f64 * 100.0).round() }
                    });
                    json!({
                        "id": m.id,
                        "name": m.name,
                        "group": m.group,
                        "note": m.note,
                        "size_mb": (m.size as f64 / 1048576.0 * 10.0).round() / 10.0,
                        "installed": m.installed,
                        "recommended": m.recommended,
                        "endpoints": m.endpoints,
                        "installing": prog.as_ref().map(|p| json!({
                            "phase": p.phase,
                            "percent": pct,
                        })),
                    })
                })
                .collect();
            Ok((
                format!(
                    "模型库共 {} 项，已装 {installed} 项，还缺 {missing} 项。{}",
                    items.len(),
                    if online { "" } else { "（没连上网络，这是本地缓存的清单）" }
                ),
                json!({
                    "models": items,
                    "count": items.len(),
                    "installed": installed,
                    "missing": missing,
                    "online": online,
                    "models_root": state.models_root.display().to_string(),
                }),
            ))
        }

        "check_update" => {
            let r = tokio::task::spawn_blocking(crate::updater::check)
                .await
                .map_err(|e| format!("查更新任务异常：{e}"))?;
            if let Some(err) = &r.error {
                return Ok((
                    format!("查不到更新：{err}。这可能只是网络问题，不影响使用。"),
                    json!({ "checked": false, "error": err, "current": r.current }),
                ));
            }
            Ok((
                if r.has_update {
                    format!("有新版本 {}（当前 {}）。装不装由你决定，装了程序会自动重启。", r.latest, r.current)
                } else {
                    format!("已经是最新的 {}。", r.current)
                },
                json!({
                    "checked": true,
                    "current": r.current,
                    "latest": r.latest,
                    "has_update": r.has_update,
                    "notes": r.notes,
                    "size_mb": (r.size as f64 / 1048576.0 * 10.0).round() / 10.0,
                }),
            ))
        }

        "wait" => {
            let ms = clamp_i(f(args, "ms").unwrap_or(500.0), 1, 10_000) as u64;
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok((format!("等了 {ms} 毫秒。"), json!({ "waited_ms": ms })))
        }

        // ============================================================ 写
        "screenshot" => {
            crate::capture::ensure_dpi_aware();
            let loaded = load_rgb(args)?;
            let (w, h) = (loaded.img.width(), loaded.img.height());
            let dir = state.data_dir.join("ai_shots");
            let path = save_png(&dir, "shot", &loaded.img.to_rgb8())?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let where_ = if ox == 0 && oy == 0 {
                "（这是图内坐标，坐标系与屏幕一致时可当屏幕坐标用）".to_string()
            } else {
                format!("原点在屏幕 ({ox}, {oy})")
            };
            Ok((
                format!(
                    "截了一张 {w}x{h} 的图，存到 {}，{where_}。后续工具可以用 image_path 引用它。",
                    path.display()
                ),
                json!({
                    "image_path": path.display().to_string(),
                    "width": w,
                    "height": h,
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "ocr_image" => {
            let loaded = load_rgb(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let max_lines = clamp_i(f(args, "max_lines").unwrap_or(50.0), 1, 200);
            let lines = state
                .recognize_lines(&OcrOptions::default(), loaded.img)
                .await
                .map_err(|e| e.to_string())?;
            let shown: Vec<Value> = lines.iter().take(max_lines).map(|l| line_json(l, ox, oy)).collect();
            let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
            Ok((
                if lines.is_empty() {
                    "图里没认出字（可能真的没字，或者图太糊）。".to_string()
                } else {
                    format!("认出 {} 行字。", lines.len())
                },
                json!({
                    "text": text,
                    "lines": shown,
                    "count": lines.len(),
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "find_text" => {
            let keyword = s(args, "keyword").ok_or("缺少 keyword（要找的文字，正则写成 /.../）")?;
            let max_results = clamp_i(f(args, "max_results").unwrap_or(20.0), 1, 200);
            let loaded = load_rgb(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let matcher = super::routes::build_matcher(&keyword).map_err(|e| e.message())?;

            let lines = state
                .recognize_lines(&OcrOptions::default(), loaded.img)
                .await
                .map_err(|e| e.to_string())?;
            let hits: Vec<Value> = lines
                .iter()
                .filter(|l| matcher.is_match(&l.text))
                .take(max_results)
                .map(|l| line_json(l, ox, oy))
                .collect();
            Ok((
                if hits.is_empty() {
                    format!("没找到「{keyword}」。屏幕上有 {} 行字，可以先用 ocr_image 看全部内容，或者放宽关键词。", lines.len())
                } else if hits.len() == 1 {
                    format!("找到「{keyword}」，中心点在 ({}, {})，可以直接 click_at。",
                        hits[0]["center_x"], hits[0]["center_y"])
                } else {
                    format!("找到 {} 处「{keyword}」，第一处在 ({}, {})。",
                        hits.len(), hits[0]["center_x"], hits[0]["center_y"])
                },
                json!({
                    "keyword": keyword,
                    "found": !hits.is_empty(),
                    "count": hits.len(),
                    "matches": hits,
                    "scanned_lines": lines.len(),
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "find_target" => {
            // 名字只能来自参考库。旧写法把名字写进 words，这里一并收下
            let names = {
                let mut v = list_of(args, "names");
                if v.is_empty() {
                    v = list_of(args, "words");
                }
                v
            };
            if names.is_empty() {
                return Err("缺少 names（要找的名字，必须是先用 teach_target 教过的）".into());
            }
            let conf = clamp_f(f(args, "conf").unwrap_or(0.25), 0.0, 1.0) as f32;
            let max_results = clamp_i(f(args, "max_results").unwrap_or(8.0), 1, 64);
            let loaded = load_image(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let opts = DetectOptions {
                conf_thresh: conf,
                iou_thresh: 0.7,
                class_filter: Vec::new(),
            };
            let dets = state
                .prompt_find(PromptInput::Refs { names: names.clone() }, loaded.img, opts)
                .await
                .map_err(|e| e.to_string())?;
            let total = dets.len();
            let hits: Vec<Value> = dets
                .iter()
                .take(max_results)
                .map(|d| det_json(d, ox, oy))
                .collect();
            Ok((
                if hits.is_empty() {
                    format!(
                        "没找到「{}」。可以调低 conf 再试；如果它外观变了，用 teach_target 重新教一次。",
                        names.join("、")
                    )
                } else {
                    format!(
                        "找到 {} 个「{}」，最像的在 ({}, {})。",
                        total, names.join("、"), hits[0]["center_x"], hits[0]["center_y"]
                    )
                },
                json!({
                    "found": !hits.is_empty(),
                    "matches": hits,
                    "count": total,
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "find_image" => {
            let tpl_args = if s(args, "template_path").is_some() || s(args, "template").is_some() {
                json!({ "image_path": s(args, "template_path"), "image": s(args, "template") })
            } else {
                return Err("缺少模板：给 template_path 或 template（要找的那张小图）".into());
            };
            let needle = load_rgb(&tpl_args)?.img.to_rgb8();
            let hay = load_rgb(args)?;
            let (ox, oy) = (hay.ox, hay.oy);
            let threshold = clamp_f(f(args, "threshold").unwrap_or(0.9), 0.0, 1.0);
            let max_results = clamp_i(f(args, "max_results").unwrap_or(1.0), 1, 64);
            let hits = tokio::task::spawn_blocking(move || {
                crate::matcher::find_template(
                    &hay.img.to_rgb8(),
                    &needle,
                    &crate::matcher::MatchOptions {
                        min_score: threshold,
                        max_results,
                    },
                )
            })
            .await
            .map_err(|e| format!("匹配任务异常：{e}"))??;

            let items: Vec<Value> = hits
                .iter()
                .map(|h| {
                    let (cx, cy) = h.center();
                    json!({
                        "x": h.x as i32 + ox,
                        "y": h.y as i32 + oy,
                        "width": h.width,
                        "height": h.height,
                        "center_x": cx + ox as f64,
                        "center_y": cy + oy as f64,
                        "score": h.score,
                    })
                })
                .collect();
            Ok((
                if items.is_empty() {
                    format!("没找到（阈值 {threshold}）。可以调低 threshold 再试。")
                } else {
                    format!("找到 {} 处，最像的那个中心在 ({}, {})。", items.len(),
                        items[0]["center_x"], items[0]["center_y"])
                },
                json!({
                    "found": !items.is_empty(),
                    "matches": items,
                    "count": items.len(),
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "detect_objects" => {
            let loaded = load_rgb(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let conf = clamp_f(f(args, "conf").unwrap_or(0.3), 0.0, 1.0) as f32;
            let max_results = clamp_i(f(args, "max_results").unwrap_or(30.0), 1, 200);
            let model = s(args, "model").or_else(|| s(args, "engine"));
            let opts = OcrOptions {
                engine: Some(model.unwrap_or_else(|| "yolo".to_string())),
                ..Default::default()
            };
            let detect = DetectOptions {
                conf_thresh: conf,
                iou_thresh: 0.7,
                class_filter: Vec::new(),
            };
            let dets = state
                .detect_objects(&opts, loaded.img, detect)
                .await
                .map_err(|e| e.message())?;

            // 类别按名字过滤而不是 id：AI 和用户都只知道「人/车」，不知道模型里 0 是哪个
            let want = list_of(args, "classes");
            let items: Vec<Value> = dets
                .iter()
                .filter(|d| {
                    want.is_empty()
                        || want.iter().any(|w| {
                            let name = d.class_name.to_lowercase();
                            let w = w.to_lowercase();
                            name.contains(&w) || w.contains(&name) || d.class_id.to_string() == w
                        })
                })
                .take(max_results)
                .map(|d| det_json(d, ox, oy))
                .collect();
            let names: Vec<String> = items
                .iter()
                .filter_map(|d| d["class_name"].as_str().map(str::to_string))
                .collect();
            let uniq: Vec<String> = {
                let mut u: Vec<String> = Vec::new();
                for n in names {
                    if !u.contains(&n) {
                        u.push(n);
                    }
                }
                u
            };
            Ok((
                if items.is_empty() {
                    format!("没检出目标（共 {} 个候选）。可以调低 conf，或确认装了检测模型（list_models）。", dets.len())
                } else {
                    format!("检出 {} 个目标：{}。", items.len(),
                        if uniq.is_empty() { "（无类别名）".to_string() } else { uniq.join("、") })
                },
                json!({
                    "count": items.len(),
                    "detections": items,
                    "class_names": uniq,
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "read_table" => {
            let loaded = load_rgb(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let mode_raw = s(args, "mode").unwrap_or_else(|| "auto".to_string());
            let mode = crate::engine::table::Mode::parse(&mode_raw).ok_or_else(|| {
                format!("mode 只能是 auto / structure / detect，收到「{mode_raw}」")
            })?;
            let with_text = b(args, "with_text").unwrap_or(true);
            let device = match s(args, "device").map(|d| d.to_ascii_lowercase()) {
                Some(d) if d == "cpu" => Device::Cpu,
                _ => Device::Directml,
            };

            let root = state.models_root.clone();
            let page = loaded.img.clone();
            let (fallback, mut hits) = tokio::task::spawn_blocking(move || {
                crate::engine::table::recognize_page(&root, &page, device, mode, 0.5)
            })
            .await
            .map_err(|e| format!("表格识别任务异常：{e}"))?
            .map_err(|e| e.to_string())?;

            if with_text && hits.iter().any(|h| h.result.is_some()) {
                match state.recognize_lines(&OcrOptions::default(), loaded.img).await {
                    Ok(lines) => super::routes::fill_table_text(&mut hits, &lines),
                    Err(e) => tracing::warn!("表格文字识别失败，结构与坐标照常返回：{e:?}"),
                }
            }

            let tables: Vec<Value> = to_table_dtos(hits)
                .into_iter()
                .map(|t| {
                    let mut v = serde_json::to_value(&t).unwrap_or_default();
                    // 表格框的坐标也要平移成屏幕绝对坐标，否则跟别的工具对不上
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("x".into(), json!(t.x as i64 + ox as i64));
                        obj.insert("y".into(), json!(t.y as i64 + oy as i64));
                    }
                    v
                })
                .collect();

            let truncated = tables.iter().any(|t| t["truncated"] == json!(true));
            Ok((
                if tables.is_empty() {
                    "这张图里没找到表格。如果图本来就是一张表格，试 mode=structure。".to_string()
                } else {
                    format!(
                        "找到 {} 张表格{}{}。",
                        tables.len(),
                        if fallback { "（没检出表格框，按整图识别）" } else { "" },
                        if truncated { "，注意：其中有的表太大，单元格被截断了" } else { "" }
                    )
                },
                json!({
                    "mode": if fallback { "structure" } else { mode.as_str() },
                    "fallback": fallback,
                    "count": tables.len(),
                    "tables": tables,
                    "origin_x": ox,
                    "origin_y": oy,
                }),
            ))
        }

        "read_qrcode" => {
            let loaded = load_rgb(args)?;
            let (ox, oy) = (loaded.ox, loaded.oy);
            let img = loaded.img.to_rgb8();
            let try_harder = b(args, "try_harder").unwrap_or(true);
            let hits = tokio::task::spawn_blocking(move || {
                crate::barcode::decode_barcodes(
                    &img,
                    &crate::barcode::BarcodeOptions {
                        formats: Vec::new(),
                        try_harder,
                    },
                )
            })
            .await
            .map_err(|e| format!("条码识别任务异常：{e}"))??;

            let items: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "text": h.text,
                        "format": h.format,
                        "x": h.bounds[0] + ox,
                        "y": h.bounds[1] + oy,
                        "width": h.bounds[2],
                        "height": h.bounds[3],
                        "center_x": h.bounds[0] + ox + h.bounds[2] / 2,
                        "center_y": h.bounds[1] + oy + h.bounds[3] / 2,
                    })
                })
                .collect();
            Ok((
                if items.is_empty() {
                    "图里没有二维码或条码（这是正常结果，不是报错）。".to_string()
                } else {
                    format!("读到 {} 个码：{}。", items.len(),
                        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>().join(" / "))
                },
                json!({ "count": items.len(), "codes": items, "origin_x": ox, "origin_y": oy }),
            ))
        }

        "teach_target" => {
            let name = s(args, "name").ok_or("缺少 name（给这个目标起个中文名）")?;
            let loaded = load_image(args)?;
            let bx = nums(args, "box").ok_or("缺少 box，格式 [x1,y1,x2,y2]（参考图左上角为原点）")?;
            if bx.len() != 4 {
                return Err("box 要是 4 个数：[x1,y1,x2,y2]".into());
            }
            let entry = {
                let mut refs = state.refs.lock().unwrap_or_else(|e| e.into_inner());
                refs.add(&name, &loaded.img, [bx[0] as f32, bx[1] as f32, bx[2] as f32, bx[3] as f32])
                    .map_err(|e| e.to_string())?
            };
            Ok((
                format!("记住了「{}」，之后 find_target 直接写这个名字就能找。", entry.name),
                json!({ "name": entry.name, "box": entry.rect, "width": entry.width, "height": entry.height }),
            ))
        }

        "forget_target" => {
            let name = s(args, "name").ok_or("缺少 name（要忘掉哪个目标）")?;
            let removed = {
                let mut refs = state.refs.lock().unwrap_or_else(|e| e.into_inner());
                refs.remove(&name).map_err(|e| e.to_string())?
            };
            Ok((
                if removed {
                    format!("忘掉「{name}」了。")
                } else {
                    format!("没有叫「{name}」的目标，可能已经删过了。list_targets 能看到现有的。")
                },
                json!({ "name": name, "removed": removed }),
            ))
        }

        "focus_window" => {
            let hwnd = resolve_hwnd(args)?;
            let ok = crate::input::focus_window(hwnd)?;
            let title = crate::capture::list_windows()
                .into_iter()
                .find(|w| w.hwnd == hwnd)
                .map(|w| w.title)
                .unwrap_or_default();
            Ok((
                if ok {
                    format!("已把「{title}」切到前台，现在可以往里点击/打字了。")
                } else {
                    format!(
                        "没能把「{title}」切到前台（Windows 前台锁拦截，后台进程不能随便抢焦点）。\
                         可以让我先把窗口最小化再还原，或者由你手动点一下任务栏。"
                    )
                },
                json!({ "hwnd": hwnd, "title": title, "foreground": ok }),
            ))
        }

        // ============================================================ 桌面操控
        "move_mouse" => {
            let x = req_i(args, "x", "屏幕 X 坐标")?;
            let y = req_i(args, "y", "屏幕 Y 坐标")?;
            crate::input::move_to(x, y)?;
            Ok((
                format!("鼠标移到 ({x}, {y})。"),
                json!({ "x": x, "y": y }),
            ))
        }

        "click_at" => {
            let x = req_i(args, "x", "屏幕 X 坐标（find_text/find_target 返回的 center_x）")?;
            let y = req_i(args, "y", "屏幕 Y 坐标")?;
            let button = match crate::input::Button::parse(&s(args, "button").unwrap_or_default()) {
                Some(b) => b,
                None => return Err("button 只能是 left / right / middle".into()),
            };
            let count = clamp_i(f(args, "count").unwrap_or(1.0), 1, 3) as u32;
            crate::input::click(x, y, button, count)?;
            Ok((
                format!(
                    "在 ({x}, {y}) {}{}。",
                    match button {
                        crate::input::Button::Left => "左键",
                        crate::input::Button::Right => "右键",
                        crate::input::Button::Middle => "中键",
                    },
                    if count > 1 { format!("点了 {count} 下") } else { "点了一下".to_string() }
                ),
                json!({ "x": x, "y": y, "count": count }),
            ))
        }

        "drag_mouse" => {
            let x = req_i(args, "x", "起点 X")?;
            let y = req_i(args, "y", "起点 Y")?;
            let to_x = req_i(args, "to_x", "终点 X")?;
            let to_y = req_i(args, "to_y", "终点 Y")?;
            let button = match crate::input::Button::parse(&s(args, "button").unwrap_or_default()) {
                Some(b) => b,
                None => return Err("button 只能是 left / right / middle".into()),
            };
            crate::input::drag((x, y), (to_x, to_y), button)?;
            Ok((
                format!("从 ({x}, {y}) 拖到 ({to_x}, {to_y})。"),
                json!({ "from": [x, y], "to": [to_x, to_y] }),
            ))
        }

        "scroll" => {
            let (cx, cy) = crate::input::cursor_pos();
            let x = f(args, "x").map(|v| v.round() as i32).unwrap_or(cx);
            let y = f(args, "y").map(|v| v.round() as i32).unwrap_or(cy);
            let delta = clamp_f(f(args, "delta").unwrap_or(-120.0), -12000.0, 12000.0) as i32;
            crate::input::scroll(x, y, delta)?;
            Ok((
                format!(
                    "在 ({x}, {y}) 滚了 {} 格。",
                    (delta as f64 / 120.0).round()
                ),
                json!({ "x": x, "y": y, "delta": delta }),
            ))
        }

        "type_text" => {
            let text = s(args, "text").ok_or("缺少 text（要输入的文字）")?;
            crate::input::type_text(&text)?;
            Ok((
                format!("输入了 {} 个字：{}", text.chars().count(),
                    text.chars().take(40).collect::<String>()),
                json!({ "typed_chars": text.chars().count() }),
            ))
        }

        "press_key" => {
            let keys = s(args, "keys").ok_or("缺少 keys（如 \"enter\" 或 \"ctrl+s\"）")?;
            crate::input::press_keys(&keys)?;
            Ok((
                format!("按了 {keys}。"),
                json!({ "keys": keys }),
            ))
        }

        // ============================================================ 模型 / 服务
        "install_model" => {
            let id = s(args, "id").ok_or("缺少 id（模型标识，list_models 里有）")?;
            let data_dir = state.data_dir.clone();
            let (cat, _online) = tokio::task::spawn_blocking(move || {
                crate::modellib::load_catalog(&data_dir, false)
            })
            .await
            .map_err(|e| format!("读清单任务异常：{e}"))?;
            let entry = crate::modellib::find(&cat, &id)
                .ok_or_else(|| {
                    let ids: Vec<&str> = cat.models.iter().map(|m| m.id.as_str()).collect();
                    format!("没有 id 为 {id:?} 的模型。可用：{}", ids.join(" / "))
                })?
                .clone();

            if crate::modellib::is_installed(&state.models_root, &entry) {
                return Ok((
                    format!("「{}」本来就装好了，直接用就行。", entry.name),
                    json!({ "id": id, "installed": true }),
                ));
            }
            {
                let g = installs().lock().unwrap_or_else(|e| e.into_inner());
                if g.contains_key(&id) {
                    return Ok((
                        format!("「{}」已经在下载了，用 list_models 看进度。", entry.name),
                        json!({ "id": id, "installing": true }),
                    ));
                }
            }

            // 后台跑：一两百 MB 的下载不能让 AI 的 HTTP 请求干等着超时
            let root = state.models_root.clone();
            let dd = state.data_dir.clone();
            let e = entry.clone();
            set_install(&id, 0, e.size, "start");
            tokio::task::spawn_blocking(move || {
                let id2 = e.id.clone();
                let r = crate::modellib::install(&root, &dd, &e, |done, total, phase| {
                    set_install(&id2, done, total, phase);
                });
                if let Err(err) = &r {
                    tracing::warn!("AI 装模型 {} 失败：{err}", e.id);
                }
                clear_install(&id2);
            });

            Ok((
                format!(
                    "开始下载「{}」（约 {:.1} MB），在后台跑。用 list_models 看进度，装完对应接口就能用。",
                    entry.name,
                    entry.size as f64 / 1048576.0
                ),
                json!({ "id": id, "started": true, "size_mb": (entry.size as f64 / 1048576.0 * 10.0).round() / 10.0 }),
            ))
        }

        "remove_model" => {
            let id = s(args, "id").ok_or("缺少 id（模型标识，list_models 里有）")?;
            let data_dir = state.data_dir.clone();
            let (cat, _) = tokio::task::spawn_blocking(move || {
                crate::modellib::load_catalog(&data_dir, false)
            })
            .await
            .map_err(|e| format!("读清单任务异常：{e}"))?;
            let entry = crate::modellib::find(&cat, &id)
                .ok_or_else(|| format!("没有 id 为 {id:?} 的模型"))?
                .clone();
            let root = state.models_root.clone();
            let name = entry.name.clone();
            tokio::task::spawn_blocking(move || crate::modellib::remove(&root, &entry))
                .await
                .map_err(|e| format!("卸载任务异常：{e}"))??;
            Ok((
                format!("卸载了「{name}」。再要用得重新装（install_model）。"),
                json!({ "id": id, "removed": true }),
            ))
        }

        "server_start" => {
            if state.is_running() {
                return Ok((
                    format!("服务本来就在跑，端口 {}。", state.port()),
                    json!({ "running": true, "port": state.port() }),
                ));
            }
            crate::commands::start_server_background(state);
            // 等它真的 listen
            for _ in 0..100 {
                if state.is_running() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if !state.is_running() {
                return Err(format!("服务没起来：127.0.0.1:{} 可能被占用", state.port()));
            }
            Ok((
                format!("服务已启动，地址 http://127.0.0.1:{}。", state.port()),
                json!({ "running": true, "port": state.port() }),
            ))
        }

        "server_stop" => {
            if !state.is_running() {
                return Ok(("服务本来就没在跑。".into(), json!({ "running": false })));
            }
            let port = state.port();
            {
                let mut guard = state.shutdown.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(tx) = guard.take() {
                    let _ = tx.send(());
                }
            }
            Ok((
                format!("服务已停止（原端口 {port}），正在调用接口的脚本会全部断掉。"),
                json!({ "running": false, "port": port }),
            ))
        }

        "set_port" => {
            let port = clamp_i(f(args, "port").unwrap_or(0.0), 1024, 65535) as u16;
            {
                let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
                cfg.port = port;
                cfg.save(&state.data_dir).map_err(|e| format!("配置存不下来：{e}"))?;
            }
            Ok((
                format!("端口改成 {port} 了，需要重启服务才生效（调 server_stop 再 server_start）。"),
                json!({ "port": port, "restart_needed": true }),
            ))
        }

        "set_level" => {
            let raw = s(args, "level").ok_or("缺少 level（tiny / small / medium）")?;
            let level = ModelLevel::from_str(&raw)
                .ok_or_else(|| format!("未知档位 {raw}，只能是 tiny / small / medium"))?;
            state.reload_engine(Some(level), None, None).await?;
            {
                let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
                cfg.level = level;
                cfg.save(&state.data_dir).map_err(|e| format!("配置存不下来：{e}"))?;
            }
            Ok((
                format!("模型档位换成 {raw} 了，模型已重新加载。"),
                json!({ "level": format!("{level:?}").to_lowercase() }),
            ))
        }

        "set_device" => {
            let raw = s(args, "device").ok_or("缺少 device（directml / cpu）")?;
            let device = Device::from_str(&raw)
                .ok_or_else(|| format!("未知设备 {raw}，只能是 directml（或 gpu）/ cpu"))?;
            state.reload_engine(None, Some(device), None).await?;
            {
                let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
                cfg.device = device;
                cfg.save(&state.data_dir).map_err(|e| format!("配置存不下来：{e}"))?;
            }
            Ok((
                format!("推理设备换成 {raw} 了，模型已重新加载。"),
                json!({ "device": format!("{device:?}").to_lowercase() }),
            ))
        }

        "set_threshold" => {
            let v = clamp_f(f(args, "threshold").unwrap_or(-1.0), -1.0, 1.0);
            if !(0.0..=1.0).contains(&v) {
                return Err("threshold 要在 0..1 之间".into());
            }
            {
                let mut cfg = state.cfg.write().unwrap_or_else(|e| e.into_inner());
                cfg.template_threshold = v as f32;
                cfg.save(&state.data_dir).map_err(|e| format!("配置存不下来：{e}"))?;
            }
            Ok((
                format!("模板找图阈值改成 {v} 了。"),
                json!({ "threshold": v }),
            ))
        }

        other => {
            // 清单里有、这里没实现 = 程序 bug，明确说出来，别让 AI 以为是自己参数写错了
            if risk_of(other).is_some() {
                return Err(format!(
                    "工具 {other:?} 在清单里但还没实现（这是程序 bug，不是你的参数问题）。\
                     先用 list_endpoints 做别的，并把这句话告诉用户。"
                ));
            }
            let hint = closest_tool(other)
                .map(|c| format!("。你想说的可能是 {c:?}"))
                .unwrap_or_default();
            Err(format!(
                "未知工具 {other:?}。先调 GET /ai/v1/tools 看看有哪些工具（参数怎么写那里也有）{hint}"
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// 出参小工具（坐标一律平移到屏幕绝对坐标）
// ---------------------------------------------------------------------------

/// 一行文字 → JSON。`ox/oy` 是这张图在屏幕上的原点。
fn line_json(l: &crate::engine::TextLine, ox: i32, oy: i32) -> Value {
    let pts: Vec<[f64; 2]> = l.points.iter().map(|p| [p.0 + ox as f64, p.1 + oy as f64]).collect();
    let xs: Vec<f64> = pts.iter().map(|p| p[0]).collect();
    let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
    let x0 = xs.iter().cloned().fold(f64::MAX, f64::min);
    let x1 = xs.iter().cloned().fold(f64::MIN, f64::max);
    let y0 = ys.iter().cloned().fold(f64::MAX, f64::min);
    let y1 = ys.iter().cloned().fold(f64::MIN, f64::max);
    json!({
        "text": l.text,
        "score": l.score,
        "x": x0.round() as i32,
        "y": y0.round() as i32,
        "width": (x1 - x0).round().max(0.0) as i32,
        "height": (y1 - y0).round().max(0.0) as i32,
        "center_x": ((x0 + x1) / 2.0).round() as i32,
        "center_y": ((y0 + y1) / 2.0).round() as i32,
        "box": pts,
    })
}

/// 一个检测框 → JSON（同样平移到屏幕绝对坐标）。
fn det_json(d: &crate::engine::Detection, ox: i32, oy: i32) -> Value {
    let (x0, y0) = (d.points[0].0 + ox as f64, d.points[0].1 + oy as f64);
    let (x1, y1) = (d.points[2].0 + ox as f64, d.points[2].1 + oy as f64);
    json!({
        "class_id": d.class_id,
        "class_name": d.class_name,
        "score": d.score,
        "x": x0.round() as i32,
        "y": y0.round() as i32,
        "width": (x1 - x0).round().max(0.0) as i32,
        "height": (y1 - y0).round().max(0.0) as i32,
        "center_x": ((x0 + x1) / 2.0).round() as i32,
        "center_y": ((y0 + y1) / 2.0).round() as i32,
    })
}

fn closest_tool(name: &str) -> Option<&'static str> {
    let n = name.trim().to_ascii_lowercase();
    if n.is_empty() {
        return None;
    }
    // 1) 前缀/被前缀：写短或写长了一截（取最短的命中，最贴近原意）
    if let Some(hit) = TOOLS
        .iter()
        .map(|t| t.name)
        .filter(|t| t.starts_with(&n) || n.starts_with(t))
        .min_by_key(|t| t.len())
    {
        return Some(hit);
    }
    // 2) 编辑距离：拼错一两个字母
    TOOLS
        .iter()
        .map(|t| (edit_distance(&n, t.name), t.name))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, name)| name)
}

/// Levenshtein 编辑距离（工具名都很短，O(n*m) 足够，没必要引库）。
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// 端点 → 方法。必须与 `routes::PATHS` 一一对应（有单测钉住）。
const ENDPOINT_METHODS: &[(&str, &str)] = &[
    ("/ocr/shibie", "POST"),
    ("/ocr/zhaozi", "POST"),
    ("/ocr/duohangjson", "POST"),
    ("/ocr/danzijson", "POST"),
    ("/ocr/quyuocr", "POST"),
    ("/ocr/tuzhaotu1", "POST"),
    ("/ocr/tuzhaotu2", "POST"),
    ("/ocr/jubingzhaotu1", "POST"),
    ("/ocr/jubingzhaotu2", "POST"),
    ("/ocr/pingmuzhaotu1", "POST"),
    ("/ocr/pingmuzhaotu2", "POST"),
    ("/ocr/yuyizhaotu", "POST"),
    ("/ocr/health", "GET"),
    ("/engine/list", "GET"),
    ("/vision/detect", "POST"),
    ("/vision/pipeline", "POST"),
    ("/vision/qrcode", "POST"),
    ("/vision/prompt", "POST"),
    ("/vision/promptref", "GET/POST/DELETE"),
    ("/ocr/biaoge", "POST"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 工具清单自身的一致性：名字唯一、都有 summary 与合法风险档。
    #[test]
    fn every_listed_tool_is_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "工具名重复：{}", t.name);
            assert!(!t.summary.trim().is_empty(), "{} 缺 summary", t.name);
            assert!(
                matches!(t.risk, RISK_READONLY | RISK_WRITE | RISK_DANGEROUS),
                "{} 的风险档非法：{}",
                t.name,
                t.risk
            );
            assert!(
                t.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{} 不是 snake_case（AI 最容易照抄错的就是名字）",
                t.name
            );
        }
    }

    /// 风险分档要「先只读、后写、最后危险」——清单顺序就是展示顺序，
    /// 打乱了 AI 会先看见一堆危险工具，容易被诱导直接上手动操作。
    #[test]
    fn tools_are_grouped_by_risk() {
        let rank = |r: &str| match r {
            RISK_READONLY => 0,
            RISK_WRITE => 1,
            _ => 2,
        };
        let mut last = 0;
        for t in TOOLS {
            let now = rank(t.risk);
            assert!(now >= last, "{} 的风险档排到前面去了（顺序：只读→写→危险）", t.name);
            last = now;
        }
    }

    /// 写错工具名时的候选建议：前缀优先（写短是最常见的错法），
    /// 拼错一两个字母靠编辑距离。
    #[test]
    fn closest_tool_suggests_sensible_candidate() {
        assert_eq!(closest_tool("ocr"), Some("ocr_image"));
        assert_eq!(closest_tool("screenshot_tool"), Some("screenshot"));
        assert_eq!(closest_tool("server_stp"), Some("server_stop"));
        assert_eq!(closest_tool("get_stat"), Some("get_status"));
        assert_eq!(closest_tool("teach"), Some("teach_target"));
        assert_eq!(closest_tool("完全不挨边"), None);
        assert_eq!(closest_tool(""), None);
    }

    #[test]
    fn edit_distance_counts_substitutions() {
        assert_eq!(edit_distance("", ""), 0);
        assert_eq!(edit_distance("abc", "abc"), 0);
        assert_eq!(edit_distance("abc", "abd"), 1);
        assert_eq!(edit_distance("abc", "ab"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn endpoint_methods_match_routes_table() {
        // 这是第二真相源，靠测试钉住：routes::PATHS 变了这里必须跟着改。
        let paths: Vec<&str> = ENDPOINT_METHODS.iter().map(|(p, _)| *p).collect();
        assert_eq!(
            paths.len(),
            super::super::routes::PATHS.len(),
            "端点数量对不上：ai_tools 的 ENDPOINT_METHODS 要跟 routes::PATHS 同步"
        );
        for p in super::super::routes::PATHS {
            assert!(paths.contains(p), "routes::PATHS 里的 {p} 没登记方法");
        }
    }

    #[test]
    fn loose_arg_parsing_tolerates_ai_writing_strings() {
        let a = json!({"conf": "0.3", "max_results": 5, "flag": "yes", "box": "1,2,3,4"});
        assert_eq!(f(&a, "conf"), Some(0.3));
        assert_eq!(f(&a, "max_results"), Some(5.0));
        assert_eq!(b(&a, "flag"), Some(true));
        assert_eq!(nums(&a, "box"), Some(vec![1.0, 2.0, 3.0, 4.0]));
        assert_eq!(num4(&a, "box"), Some([1, 2, 3, 4]));
        // 缺参数不该 panic
        assert_eq!(f(&a, "nope"), None);
        assert_eq!(s(&a, "nope"), None);
    }

    #[test]
    fn list_of_accepts_chinese_separators() {
        let a = json!({ "names": "提交按钮，搜索框; 关 闭" });
        assert_eq!(list_of(&a, "names"), vec!["提交按钮", "搜索框", "关 闭"]);
    }

    #[test]
    fn clamps_keep_values_in_range() {
        assert_eq!(clamp_f(9.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp_f(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_f(f64::NAN, 0.0, 1.0), 0.0);
        assert_eq!(clamp_i(9999.0, 1, 64), 64);
    }

    /// AI 最常见的错法是「只写了 x 忘了 y」——报错必须点名缺哪个参数。
    #[test]
    fn missing_required_number_names_the_field() {
        let a = json!({ "x": 10 });
        let e = req_i(&a, "y", "屏幕 Y 坐标").unwrap_err();
        assert!(e.contains("y"), "报错里要点名缺的是 y：{e}");
        assert!(e.contains("屏幕 Y 坐标"), "还要说清该填什么：{e}");
    }
}
