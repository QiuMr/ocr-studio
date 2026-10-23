//! AI 工具注册表与实现。
//!
//! 设计原则（照搬用户另一个项目里已经跑顺的那套）：
//! - 工具名用动词开头、参数用 snake_case（大模型对这两个约定最熟）；
//! - 每个工具都要**人话 summary**：AI 念给用户听的就是它，要说清"做了什么、结果如何"；
//! - `risk` 三档：readonly（只读）/ write（改程序状态或读屏幕）/ dangerous（动服务配置）。
//!   写操作全部写审计日志，用户能在运行日志里看到 AI 干了什么；
//! - 参数解析**刻意宽松**：AI 很容易把数字写成字符串、把布尔写成 "true"，
//!   一律容错接收；区间用 clamp 夹住。报错必须告诉 AI 下一步怎么做。
//!
//! **明确不提供**（要么自伤要么动系统，这三件事让人来点界面）：
//! 退出程序、改开机自启注册表、改 AI 开关本身与连接码。
//! 做法是**根本不注册**，不靠运行时黑名单。

use std::sync::Arc;

use image::{DynamicImage, RgbImage};
use serde_json::{json, Value};

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

/// 工具清单（**顺序即展示顺序**：先只读、后写、最后危险，方便 AI 建立风险意识）。
pub const TOOLS: &[Tool] = &[
    // ---------------------------------------------------------------- 只读
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
    // ---------------------------------------------------------------- 写
    tool(
        "screenshot",
        "截一张屏幕并存成文件，返回文件路径和尺寸。后续 ocr_image / find_image 可以直接用这个路径当输入。**会读取用户屏幕内容**。",
        RISK_WRITE,
        &[],
    ),
    tool(
        "ocr_image",
        "认一张图里的字。图可以是本地路径（image_path）、base64（image），不给图就截屏（source=screen）。返回整段文本和每一行的位置。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片绝对路径", false, ""),
            arg("image", "图片 base64（与 image_path 二选一）", false, ""),
            arg("source", "不给图时去哪截：screen（默认）", false, "screen"),
            arg("max_lines", "最多返回几行，1-200（默认 50）", false, "50"),
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
            arg("source", "不给图时去哪截：screen（默认）", false, "screen"),
            arg("conf", "置信度阈值 0-1（默认 0.25），找不到就调低", false, "0.25"),
            arg("max_results", "最多返回几个，1-64（默认 8）", false, "8"),
        ],
    ),
    tool(
        "find_image",
        "模板找图：拿一张小图在大图里找位置（比像素）。目标图不给就截屏。",
        RISK_WRITE,
        &[
            arg("template_path", "小图（模板）的本地路径", false, ""),
            arg("template", "小图 base64（与 template_path 二选一）", false, ""),
            arg("image_path", "大图本地路径", false, ""),
            arg("image", "大图 base64", false, ""),
            arg("source", "不给大图时去哪截：screen（默认）", false, "screen"),
            arg("threshold", "相似度阈值 0-1（默认 0.9）", false, "0.9"),
            arg("max_results", "最多返回几个，1-64（默认 1）", false, "1"),
        ],
    ),
    tool(
        "read_qrcode",
        "读出图里的二维码 / 条形码内容。不给图就截屏。",
        RISK_WRITE,
        &[
            arg("image_path", "本地图片路径", false, ""),
            arg("image", "图片 base64", false, ""),
            arg("source", "不给图时去哪截：screen（默认）", false, "screen"),
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
            arg("box", "参考框 [x1,y1,x2,y2]，参考图左上角为原点", true, ""),
        ],
    ),
    // ---------------------------------------------------------------- 危险
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

/// 提示词里「你可以帮我…」的一句话（人话，不列工具名）。
pub fn highlights() -> Vec<&'static str> {
    vec![
        "看屏幕上有什么、认出上面的字",
        "按中文名找界面元素的位置（按钮、图标、输入框）",
        "教它认新目标（框一次、起个中文名，之后就能找）",
        "读二维码和条形码",
        "看服务状态与运行日志、排查调不通的原因",
        "启停服务、换模型档位和推理设备",
    ]
}

// ---------------------------------------------------------------------------
// 参数解析（宽松）
// ---------------------------------------------------------------------------

fn s(args: &Value, key: &str) -> Option<String> {
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

/// 取整数数组（框、区域等）。
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
// 图片来源
// ---------------------------------------------------------------------------

/// 取目标图。`image_path` / `image`(base64) 优先，都没有就截屏。
fn load_image(args: &Value) -> Result<DynamicImage, String> {
    if let Some(p) = s(args, "image_path") {
        return image::open(std::path::Path::new(&p))
            .map_err(|e| format!("读不了图片 {p}：{e}（先确认路径存在，注意 Windows 路径用 / 或 \\\\）"));
    }
    if let Some(b64) = s(args, "image") {
        let raw = decode_base64_image(&b64)?;
        return decode_image(&raw).map_err(|e| e.to_string());
    }
    if s(args, "source").as_deref() == Some("window") {
        return Err("source=window 需要 hwnd，AI 接口暂不支持；请改用 screenshot 后传 image_path".into());
    }
    // 没给图 → 截屏
    let rgb = crate::capture::capture_region(None).map_err(|e| format!("截屏失败：{e}"))?;
    Ok(DynamicImage::ImageRgb8(rgb))
}

fn load_rgb(args: &Value) -> Result<RgbImage, String> {
    Ok(load_image(args)?.to_rgb8())
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
// 执行
// ---------------------------------------------------------------------------

pub async fn exec(
    state: &Arc<AppState>,
    name: &str,
    args: &Value,
) -> Result<(String, Value), String> {
    match name {
        // ---------------------------------------------------------- 只读
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
                format!(
                    "服务在跑，地址 http://127.0.0.1:{port}，引擎 {level}（{device}）。"
                )
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

        // ---------------------------------------------------------- 写
        "screenshot" => {
            crate::capture::ensure_dpi_aware();
            let rgb = crate::capture::capture_region(None).map_err(|e| format!("截屏失败：{e}"))?;
            let (w, h) = (rgb.width(), rgb.height());
            let dir = state.data_dir.join("ai_shots");
            let path = save_png(&dir, "shot", &rgb)?;
            Ok((
                format!("截了一张 {w}x{h} 的屏幕，存到 {}。后续工具可以用 image_path 引用它。", path.display()),
                json!({ "image_path": path.display().to_string(), "width": w, "height": h }),
            ))
        }

        "ocr_image" => {
            let img = load_image(args)?;
            let max_lines = clamp_i(f(args, "max_lines").unwrap_or(50.0), 1, 200);
            let lines = state
                .recognize_lines(&OcrOptions::default(), img)
                .await
                .map_err(|e| e.to_string())?;
            let shown: Vec<Value> = lines
                .iter()
                .take(max_lines)
                .map(|l| {
                    let (cx, cy) = line_center(l);
                    json!({
                        "text": l.text,
                        "score": l.score,
                        "center_x": cx,
                        "center_y": cy,
                        "box": l.points.iter().map(|p| [p.0, p.1]).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
            Ok((
                if lines.is_empty() {
                    "图里没认出字（可能真的没字，或者图太糊）。".to_string()
                } else {
                    format!("认出 {} 行字。", lines.len())
                },
                json!({ "text": text, "lines": shown, "count": lines.len() }),
            ))
        }

        "find_target" => {
            // 名字只能来自参考库。旧写法把名字写进 words，这里一并收下
            let raw = s(args, "names")
                .or_else(|| s(args, "words"))
                .ok_or("缺少 names（要找的名字，必须是先用 teach_target 教过的）")?;
            let names: Vec<String> = raw
                .split([',', '，', ';'])
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            if names.is_empty() {
                return Err("names 是空的".into());
            }
            let conf = clamp_f(f(args, "conf").unwrap_or(0.25), 0.0, 1.0) as f32;
            let max_results = clamp_i(f(args, "max_results").unwrap_or(8.0), 1, 64);
            let img = load_image(args)?;
            let opts = DetectOptions {
                conf_thresh: conf,
                iou_thresh: 0.7,
                class_filter: Vec::new(),
            };
            let dets = state
                .prompt_find(PromptInput::Refs { names: names.clone() }, img, opts)
                .await
                .map_err(|e| e.to_string())?;
            let mut hits: Vec<Value> = dets
                .iter()
                .take(max_results)
                .map(|d| {
                    let (cx, cy) = box_center(&d.points);
                    json!({
                        "class_name": d.class_name,
                        "score": d.score,
                        "center_x": cx,
                        "center_y": cy,
                        "box": d.points.iter().map(|p| [p.0, p.1]).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let total = dets.len();
            hits.truncate(max_results);
            Ok((
                if hits.is_empty() {
                    format!(
                        "没找到「{}」。可以调低 conf 再试；如果它外观变了，用 teach_target 重新教一次。",
                        names.join("、")
                    )
                } else {
                    format!("找到 {} 个「{}」。", total, names.join("、"))
                },
                json!({ "found": !hits.is_empty(), "matches": hits, "count": total }),
            ))
        }

        "find_image" => {
            let tpl_args = if s(args, "template_path").is_some() || s(args, "template").is_some() {
                json!({ "image_path": s(args, "template_path"), "image": s(args, "template") })
            } else {
                return Err("缺少模板：给 template_path 或 template（要找的那张小图）".into());
            };
            let needle = load_rgb(&tpl_args)?;
            let hay = load_rgb(args)?;
            let threshold = clamp_f(f(args, "threshold").unwrap_or(0.9), 0.0, 1.0);
            let max_results = clamp_i(f(args, "max_results").unwrap_or(1.0), 1, 64);
            let hits = crate::matcher::find_template(
                &hay,
                &needle,
                &crate::matcher::MatchOptions {
                    min_score: threshold,
                    max_results,
                },
            )?;
            let items: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "x": h.x, "y": h.y, "width": h.width, "height": h.height,
                        "center_x": h.center().0, "center_y": h.center().1,
                        "score": h.score,
                    })
                })
                .collect();
            Ok((
                if items.is_empty() {
                    format!("没找到（阈值 {threshold}）。可以调低 threshold 再试。")
                } else {
                    format!("找到 {} 处，最像的那个在 ({}, {})。", items.len(),
                        items[0]["center_x"], items[0]["center_y"])
                },
                json!({ "found": !items.is_empty(), "matches": items, "count": items.len() }),
            ))
        }

        "read_qrcode" => {
            let img = load_rgb(args)?;
            let hits = crate::barcode::decode_barcodes(
                &img,
                &crate::barcode::BarcodeOptions {
                    formats: Vec::new(),
                    try_harder: b(args, "try_harder").unwrap_or(true),
                },
            )?;
            let items: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "text": h.text,
                        "format": h.format,
                        "bounds": h.bounds,
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
                json!({ "count": items.len(), "codes": items }),
            ))
        }

        "teach_target" => {
            let name = s(args, "name").ok_or("缺少 name（给这个目标起个中文名）")?;
            let img = load_image(args)?;
            let bx = nums(args, "box").ok_or("缺少 box，格式 [x1,y1,x2,y2]（参考图左上角为原点）")?;
            if bx.len() != 4 {
                return Err("box 要是 4 个数：[x1,y1,x2,y2]".into());
            }
            let entry = {
                let mut refs = state.refs.lock().unwrap_or_else(|e| e.into_inner());
                refs.add(&name, &img, [bx[0] as f32, bx[1] as f32, bx[2] as f32, bx[3] as f32])
                    .map_err(|e| e.to_string())?
            };
            Ok((
                format!("记住了「{}」，之后 find_target 直接写这个名字就能找。", entry.name),
                json!({ "name": entry.name, "box": entry.rect, "width": entry.width, "height": entry.height }),
            ))
        }

        // ---------------------------------------------------------- 危险
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
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
            let hint = closest_tool(other)
                .map(|c| format!("。你想说的可能是 {c:?}"))
                .unwrap_or_default();
            Err(format!(
                "未知工具 {other:?}。先调 GET /ai/v1/tools 看看有哪些工具（参数怎么写那里也有）{hint}"
            ))
        }
    }
}

/// 最接近的工具名，给「AI 把工具名写错」时自我修正用。
///
/// 为什么值得做：AI 常见的错法不是随机拼错，而是**把名字写短**
/// （`ocr` → `ocr_image`）或者漏个下划线。光说「未知工具」，
/// AI 只能再去拉一次清单碰运气；给一个候选名，它下一发就对。
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

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn line_center(l: &crate::engine::TextLine) -> (f64, f64) {
    let (sx, sy): (f64, f64) = l.points.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    (sx / 4.0, sy / 4.0)
}

fn box_center(pts: &[(f64, f64); 4]) -> (f64, f64) {
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    (sx / 4.0, sy / 4.0)
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
];

#[cfg(test)]
mod tests {
    use super::*;

    /// 工具清单与 `exec` 的分支必须一一对应：清单里写了但 exec 不认，
    /// AI 会调一个「文档里有、实际没有」的工具。
    #[test]
    fn every_listed_tool_is_executable() {
        for t in TOOLS {
            assert!(
                risk_of(t.name).is_some(),
                "工具 {} 不在 TOOLS 里（重复或漏登记）",
                t.name
            );
            assert!(!t.summary.trim().is_empty(), "{} 缺 summary", t.name);
            assert!(!t.risk.is_empty(), "{} 缺 risk", t.name);
        }
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "工具名重复：{}", t.name);
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
        // 缺参数不该 panic
        assert_eq!(f(&a, "nope"), None);
        assert_eq!(s(&a, "nope"), None);
    }

    #[test]
    fn clamps_keep_values_in_range() {
        assert_eq!(clamp_f(9.0, 0.0, 1.0), 1.0);
        assert_eq!(clamp_f(-5.0, 0.0, 1.0), 0.0);
        assert_eq!(clamp_f(f64::NAN, 0.0, 1.0), 0.0);
        assert_eq!(clamp_i(9999.0, 1, 64), 64);
    }
}
