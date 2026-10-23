//! AI 操控接口：`/ai/v1/tools`（工具清单）与 `/ai/v1/tool`（调一个工具）。
//!
//! 为什么是「本机 HTTP」而不是 MCP
//! --------------------------------
//! 用户明确不要 MCP：小白嫌装东西麻烦。而 MCP 的能力边界和本机 HTTP 是一样的
//! （都只能被跑在同一台机器上的 AI 用），却要多装依赖、改配置。
//! 所以直接给两个 HTTP 端点 —— AI 会发 HTTP，就够了。
//!
//! **必须说清的边界**：服务只绑 `127.0.0.1`，只有跑在用户电脑上的 AI（编程助手、
//! 本地脚本、本地模型）能调；**网页版 AI 在云端，访问不到 localhost**。
//!
//! 默认关闭：这是会改变程序行为的对外接口，一律让用户显式打开。
//!
//! 为什么用 `ok/summary/data/error` 而不是全站那套 `code/msg/data`
//! ---------------------------------------------------------------
//! 这两个端点的消费者是**大模型**，不是用户的脚本。`summary` 是给 AI 直接念给
//! 用户听的人话（"在屏幕上找到了 3 个「提交按钮」"），`error` 要告诉 AI 下一步该
//! 怎么做（"缺少 image；要先调 screenshot 截一张图"）—— 这两件事 `{code,msg}` 表达
//! 不了。HTTP 状态码仍然规范使用（401/403/404/409/429）。
//!
//! 参考了用户另一个项目（WPE Lab）的同款设计，那套已经跑了一阵子。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use super::state::AppState;

/// 工具清单。
pub const P_AI_TOOLS: &str = "/ai/v1/tools";
/// 调工具。
pub const P_AI_TOOL: &str = "/ai/v1/tool";

/// 限流：每分钟最多多少次。
///
/// AI 写错提示词可能进死循环地调，120 次/分钟既够正常用，又不会把本机拖垮。
/// （OCR 单次几百毫秒，真跑满一分钟也就 100 来次。）
const RATE_LIMIT: usize = 120;
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// 幂等缓存保留多少条 `request_id`。
const IDEM_KEEP: usize = 200;

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

/// AI 接口的响应信封。`summary` 是给 AI 转述给用户的人话。
#[derive(Debug, Clone, Serialize)]
pub struct AiResp {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AiResp {
    pub fn ok(summary: impl Into<String>, data: Value) -> Self {
        Self {
            ok: true,
            summary: Some(summary.into()),
            data: Some(data),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            summary: None,
            data: None,
            error: Some(msg.into()),
        }
    }
}

fn json_resp(status: StatusCode, body: &AiResp) -> Response {
    (status, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// 鉴权
// ---------------------------------------------------------------------------

/// 生成一个连接码，形如 `A1B2-C3D4-E5F6-7A8B`。
///
/// 没引 `rand`：本机回环接口不需要密码学强度，但也不能是纯时间戳（能被猜到）。
/// 这里把时间、进程号、一个自增序号和一次堆分配的地址混在一起做 FNV 哈希 ——
/// 地址每次运行都不同（ASLR），足够挡住顺手猜码。
pub fn new_ai_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let addr = Box::into_raw(Box::new(0u8)) as u64;

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in [nanos, pid, seq, addr, nanos.rotate_left(37)] {
        h ^= v;
        h = h.wrapping_mul(0x1_0000_01b3);
        h ^= h >> 29;
    }
    let raw = format!("{:016X}", h);
    format!(
        "{}-{}-{}-{}",
        &raw[0..4],
        &raw[4..8],
        &raw[8..12],
        &raw[12..16]
    )
}

/// 常数时间比较：不按内容提前返回，避免靠响应时间逐字节猜连接码。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 取出请求头里的连接码（`Bearer <码>` 或裸码都认）。
fn bearer_of(req: &Request) -> String {
    let raw = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_string();
    let s = raw.strip_prefix("Bearer ").unwrap_or(&raw);
    s.trim().to_string()
}

// ---------------------------------------------------------------------------
// 限流与幂等（状态挂在 AppState 上）
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct AiIdem {
    /// `request_id` → 上次的响应体（已序列化，直接回放）
    pub cache: HashMap<String, String>,
    /// FIFO 淘汰顺序
    pub order: VecDeque<String>,
    /// 正在执行中的 `request_id`（防并发重复执行）
    pub inflight: HashSet<String>,
}

/// 幂等键。`request_id` 为空表示调用方不要求幂等。
///
/// 分隔符用 `\x00` 而不是下划线：`request_id` 里本来就可能有下划线，
/// 用下划线拼会让 ("a_b","c") 和 ("a","b_c") 撞成同一个键。
fn idem_key(tool: &str, request_id: &str) -> Option<String> {
    let id = request_id.trim();
    if id.is_empty() {
        return None;
    }
    Some(format!("{tool}\x00{id}"))
}

/// 滑动窗口限流：清掉窗口外的旧戳，再看窗口内有多少次。
fn rate_allow(state: &AppState) -> bool {
    let mut q = state.ai_rate.lock().unwrap_or_else(|e| e.into_inner());
    let cut = Instant::now() - RATE_WINDOW;
    while let Some(front) = q.front() {
        if *front < cut {
            q.pop_front();
        } else {
            break;
        }
    }
    q.push_back(Instant::now());
    q.len() <= RATE_LIMIT
}

// ---------------------------------------------------------------------------
// 端点
// ---------------------------------------------------------------------------

/// `GET /ai/v1/tools`：列出能调什么。
pub async fn ai_tools(State(state): State<Arc<AppState>>, req: Request) -> Response {
    if let Some(r) = gate(&state, &req) {
        return r;
    }
    if !rate_allow(&state) {
        return json_resp(
            StatusCode::TOO_MANY_REQUESTS,
            &AiResp::err("调用过于频繁（每分钟 120 次），请稍后再试"),
        );
    }

    let tools = super::ai_tools::catalog();
    json_resp(
        StatusCode::OK,
        &AiResp::ok(
            format!(
                "共 {} 个工具。readonly = 只是看；write = 会改变程序状态或读屏幕；\
                 dangerous = 会动服务配置，可能影响正在调用接口的其他脚本。",
                tools.len()
            ),
            json!({ "tools": tools }),
        ),
    )
}

/// `POST /ai/v1/tool`：调一个工具。
pub async fn ai_tool(State(state): State<Arc<AppState>>, req: Request) -> Response {
    if let Some(r) = gate(&state, &req) {
        return r;
    }
    if !rate_allow(&state) {
        return json_resp(
            StatusCode::TOO_MANY_REQUESTS,
            &AiResp::err("调用过于频繁（每分钟 120 次），请稍后再试"),
        );
    }

    // --- 读请求体 ---
    let bytes = match axum::body::to_bytes(req.into_body(), super::routes::MAX_BODY).await {
        Ok(v) => v,
        Err(e) => {
            return json_resp(
                StatusCode::BAD_REQUEST,
                &AiResp::err(format!("请求体读不进来：{e}")),
            )
        }
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            return json_resp(
                StatusCode::BAD_REQUEST,
                &AiResp::err(format!(
                    "JSON 解析失败：{e}。请求体形如 \
                     {{\"tool\":\"ocr_image\",\"args\":{{\"image_path\":\"...\"}},\"request_id\":\"...\"}}"
                )),
            )
        }
    };

    let tool = body.get("tool").and_then(|v| v.as_str()).unwrap_or("").trim();
    if tool.is_empty() {
        return json_resp(
            StatusCode::BAD_REQUEST,
            &AiResp::err("缺少 tool 字段（工具名）。先调 GET /ai/v1/tools 看有哪些工具"),
        );
    }
    let args = body.get("args").cloned().unwrap_or_else(|| json!({}));
    let request_id = body
        .get("request_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // --- 幂等：同一次意图重试不该执行两遍 ---
    let key = idem_key(tool, &request_id);
    if let Some(k) = &key {
        let mut idem = state.ai_idem.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = idem.cache.get(k) {
            return (
                StatusCode::OK,
                [(&axum::http::header::CONTENT_TYPE, "application/json")],
                cached.clone(),
            )
                .into_response();
        }
        if idem.inflight.contains(k) {
            return json_resp(
                StatusCode::CONFLICT,
                &AiResp::err("同一个 request_id 的请求正在处理中，请稍后重试"),
            );
        }
        idem.inflight.insert(k.clone());
    }

    // --- 执行 ---
    let t0 = Instant::now();
    let out = super::ai_tools::exec(&state, tool, &args).await;
    let ms = t0.elapsed().as_millis();

    let (status, resp) = match out {
        Ok((summary, data)) => (
            StatusCode::OK,
            AiResp::ok(summary.clone(), data),
        ),
        Err(msg) => (StatusCode::BAD_REQUEST, AiResp::err(msg.clone())),
    };

    // 审计：写操作记 warn，只读记 info。用户能在运行日志里看到 AI 干了什么。
    let risk = super::ai_tools::risk_of(tool).unwrap_or("?");
    if risk == "readonly" {
        tracing::info!("AI 调用 {tool}（{risk}），耗时 {ms}ms");
    } else {
        let brief = {
            let s = serde_json::to_string(&args).unwrap_or_default();
            s.chars().take(200).collect::<String>()
        };
        tracing::warn!("AI 调用 {tool}（{risk}），耗时 {ms}ms，参数：{brief}");
    }

    let body_text = serde_json::to_string(&resp).unwrap_or_else(|e| {
        format!("{{\"ok\":false,\"error\":\"响应序列化失败: {e}\"}}")
    });

    // 释放占位 + 写缓存（无论成功失败都要释放，否则这个 id 永远卡住）
    if let Some(k) = &key {
        let mut idem = state.ai_idem.lock().unwrap_or_else(|e| e.into_inner());
        idem.inflight.remove(k);
        if resp.ok {
            idem.cache.insert(k.clone(), body_text.clone());
            idem.order.push_back(k.clone());
            while idem.order.len() > IDEM_KEEP {
                if let Some(old) = idem.order.pop_front() {
                    idem.cache.remove(&old);
                }
            }
        }
    }

    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )],
        body_text,
    )
        .into_response()
}

/// 统一的门禁：开关 → 连接码。返回 `Some` 表示要直接回给调用方。
///
/// **刻意是同步的**：它借用了 `&Request`，而 `Request<Body>` 不是 `Sync` ——
/// 一旦跨 `.await` 持有，整个 handler 的 future 就不 `Send`，axum 的 `Handler`
/// 直接不认（报的是匪夷所思的 "Handler<_, _> is not satisfied"）。
fn gate(state: &Arc<AppState>, req: &Request) -> Option<Response> {
    let (enabled, token) = {
        let cfg = state.cfg.read().unwrap_or_else(|e| e.into_inner());
        (cfg.ai_enabled, cfg.ai_token.clone())
    };

    if !enabled {
        return Some(json_resp(
            StatusCode::FORBIDDEN,
            &AiResp::err(
                "AI 接口没开。请让用户在程序里打开「设置 → AI 控制」，\
                 然后用那里的连接码调用。",
            ),
        ));
    }

    let want = token.trim();
    if want.is_empty() || !constant_time_eq(bearer_of(req).as_bytes(), want.as_bytes()) {
        return Some(json_resp(
            StatusCode::UNAUTHORIZED,
            &AiResp::err(
                "连接码不正确。每次请求都要带请求头 \
                 `Authorization: Bearer <连接码>`（连接码在「设置 → AI 控制」里，可以复制或重置）。",
            ),
        ));
    }
    None
}

// ---------------------------------------------------------------------------
// 提示词
// ---------------------------------------------------------------------------

/// 生成「复制给 AI 的提示词」。
///
/// 里面带好地址、连接码、用法和一个能直接抄的示例；**工具清单不写进去** ——
/// 让它自己 `GET /ai/v1/tools` 拿，这样提示词不会随工具变多而膨胀，也永远是最新的。
pub fn prompt(base_url: &str, token: &str) -> String {
    format!(
        "请帮我操控本机的 OCR Studio（一个 OCR / 找图服务），\
         你直接用 HTTP 调用下面的接口，**不需要安装任何东西**。\n\n\
         接口地址：{base}\n\
         我的连接码：{token}\n\
         （每次请求都要带上请求头：Authorization: Bearer {token}）\n\n\
         用法：\n\
         1. 先调 GET {base}/ai/v1/tools 看看有哪些工具、参数怎么写\n\
         2. 调工具：POST {base}/ai/v1/tool，请求体形如\n   \
            {{\"tool\":\"ocr_image\",\"args\":{{\"image_path\":\"C:/x.png\"}}}}\n\
         3. 返回里 summary 是转述给我听的人话，data 是结构化数据\n\
         4. 写操作请带上 request_id：同一次意图重试要带同一个值，避免重复执行\n\n\
         你可以帮我：{highlights}。\n\n\
         注意事项：\n\
         - 这个接口只在**本机**能访问（127.0.0.1），跑在云端的 AI 连不上；\n\
         - 找我界面上的东西时，**中文词可以直接用**（如「提交按钮」「人」「公交车」）；\n\
         - 每次直接告诉我结果（用中文、说人话），别把原始 JSON 贴给我；\n\
         - 如果提示词里有你没见过的中文目标，先调 GET /ai/v1/tools 看看有没有 \
           teach_target 这类工具，或者让我到界面上框一次教给它；\n\
         - 若接口没打开，让我到「设置 → AI 控制」里打开再试。\n",
        base = base_url,
        token = token,
        highlights = super::ai_tools::highlights().join("、"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_looks_like_a_token() {
        let t = new_ai_token();
        assert_eq!(t.len(), 19, "形如 A1B2-C3D4-E5F6-7A8B：{t}");
        assert_eq!(t.matches('-').count(), 3);
        assert_ne!(t, new_ai_token(), "连着生成两个不该一样");
    }

    #[test]
    fn constant_time_eq_behaves() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"", b"a"));
    }

    #[test]
    fn idem_key_separates_tool_and_id() {
        // 换成分隔符就能撞车的经典例子：("a_b","c") vs ("a","b_c")
        assert_ne!(
            idem_key("a_b", "c").unwrap(),
            idem_key("a", "b_c").unwrap()
        );
        assert_eq!(idem_key("x", ""), None, "空 request_id 表示不要求幂等");
    }

    #[test]
    fn prompt_carries_everything_ai_needs() {
        let p = prompt("http://127.0.0.1:8848", "AAAA-BBBB-CCCC-DDDD");
        assert!(p.contains("http://127.0.0.1:8848"));
        assert!(p.contains("AAAA-BBBB-CCCC-DDDD"));
        assert!(p.contains("Authorization: Bearer"));
        assert!(p.contains("/ai/v1/tools"));
        assert!(p.contains("request_id"));
        assert!(p.contains("本机"), "必须写明云端 AI 连不上");
    }
}
