//! AI 操控接口（`/ai/v1/tools` + `/ai/v1/tool`）端到端测试。
//!
//! 这些行为的消费者是**大模型**，出错时 AI 看不到堆栈、只会把 error 文案
//! 转述给用户 —— 所以「拒绝得是否明确、错误是否可读」必须当成功能来测，
//! 不能等发布后让用户的 AI 撞上一次难看的报错才发现。
//!
//! 钉住的行为：
//! - 默认关闭 → 403 + 告诉用户去哪开（不是 404，也不是静默放行）
//! - 连接码缺失/错误 → 401 + 告诉 AI 下一步怎么做
//! - 工具清单带 risk 档位（AI 要能自己判断这个调用危不危险）
//! - 同一个 request_id 重试 → 原样回放，不执行第二遍
//! - 工具名/参数写错 → 可读错误（AI 靠这个自我修正）
//!
//! 运行：cargo test --test ai_service -- --nocapture

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ocr_studio_lib::api::{build_router, AppState};
use ocr_studio_lib::config::AppConfig;
use serde_json::Value;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

struct TestServer {
    addr: std::net::SocketAddr,
    _thread: std::thread::JoinHandle<()>,
}

fn spawn_server(state: AppState) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口失败");
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let state = Arc::new(state);
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let tokio_listener = rt.block_on(async {
            tokio::net::TcpListener::from_std(listener).expect("from_std")
        });
        rt.block_on(async move {
            axum::serve(tokio_listener, build_router().with_state(state))
                .await
                .expect("serve");
        });
    });

    for _ in 0..100 {
        if TcpStream::connect(addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    TestServer {
        addr,
        _thread: thread,
    }
}

/// 起一个 AI 接口开着、连接码固定的服务（不加载引擎：
/// 这些用例只碰门禁/清单/幂等/错误文案，不需要真推理）。
fn start_ai_server(token: &str) -> TestServer {
    let root = workspace_root();
    let mut cfg = AppConfig::default();
    cfg.ai_enabled = true;
    cfg.ai_token = token.to_string();
    spawn_server(AppState::new(root.join("models"), cfg))
}

/// 起一个**默认配置**的服务（AI 接口关着）。
fn start_default_server() -> TestServer {
    let root = workspace_root();
    spawn_server(AppState::new(root.join("models"), AppConfig::default()))
}

/// 发一次请求，可带额外请求头（Authorization 就靠它）。返回 (状态码, 响应体)。
fn http_hdr(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("连接失败");
    s.set_read_timeout(Some(Duration::from_secs(60)))
        .expect("set timeout");
    s.set_write_timeout(Some(Duration::from_secs(30)))
        .expect("set timeout");

    let extra = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}\r\n"))
        .collect::<String>();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccept: */*\r\n{extra}\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).expect("写请求头失败");
    if !body.is_empty() {
        s.write_all(body).expect("写请求体失败");
    }

    let mut buf = Vec::new();
    s.read_to_end(&mut buf).expect("读响应失败");
    let text = String::from_utf8_lossy(&buf).to_string();

    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.split_once("\r\n\r\n") {
        Some((_, b)) => dechunk(b),
        None => String::new(),
    };
    (status, body)
}

/// 最小 chunked 解码（axum 对小响应走 content-length，这里是兜底防误判）。
fn dechunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let line_end = match rest.find("\r\n") {
            Some(i) => i,
            None => break,
        };
        let size = match usize::from_str_radix(rest[..line_end].trim(), 16) {
            Ok(n) => n,
            Err(_) => return body.to_string(),
        };
        if size == 0 {
            break;
        }
        let start = line_end + 2;
        if start + size > rest.len() {
            break;
        }
        out.push_str(&rest[start..start + size]);
        rest = &rest[start + size..];
        if rest.starts_with("\r\n") {
            rest = &rest[2..];
        }
    }
    if out.is_empty() {
        body.to_string()
    } else {
        out
    }
}

const TOKEN: &str = "TEST-1234-ABCD-5678";

// ---------------------------------------------------------------------------
// 门禁
// ---------------------------------------------------------------------------

/// 默认关闭：必须明确拒绝并指路，不能 404（那样 AI 会以为地址写错了），
/// 更不能放行。
#[test]
fn ai_off_by_default_refuses_with_hint() {
    let srv = start_default_server();
    let (st, body) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        br#"{"tool":"get_status","args":{}}"#,
        &[],
    );
    assert_eq!(st, 403, "默认应该是 403，实际 {st}: {body}");
    let j: Value = serde_json::from_str(&body).expect("响应应是 JSON");
    assert_eq!(j["ok"], false);
    let err = j["error"].as_str().unwrap_or("");
    assert!(
        err.contains("没开") && err.contains("设置"),
        "错误文案要告诉用户去哪开，实际：{err}"
    );
}

/// 连接码三道关：没带 → 401；带错 → 401；带对 → 200。
/// 另外验证**裸码**（不带 Bearer 前缀）也认 —— AI 经常忘了写前缀。
#[test]
fn ai_token_gate_accepts_only_right_code() {
    let srv = start_ai_server(TOKEN);

    let (st, body) = http_hdr(
        srv.addr,
        "GET",
        "/ai/v1/tools",
        "application/json",
        b"",
        &[],
    );
    assert_eq!(st, 401, "没带连接码应 401，实际 {st}: {body}");
    assert!(body.contains("Authorization"), "错误要提请求头名：{body}");

    let (st, body) = http_hdr(
        srv.addr,
        "GET",
        "/ai/v1/tools",
        "application/json",
        b"",
        &[("Authorization", "Bearer WRONG-CODE")],
    );
    assert_eq!(st, 401, "错码应 401，实际 {st}: {body}");

    // 裸码也认
    let (st, body) = http_hdr(
        srv.addr,
        "GET",
        "/ai/v1/tools",
        "application/json",
        b"",
        &[("Authorization", TOKEN)],
    );
    assert_eq!(st, 200, "对的裸码应放行，实际 {st}: {body}");

    // Bearer 前缀也认
    let (st, _) = http_hdr(
        srv.addr,
        "GET",
        "/ai/v1/tools",
        "application/json",
        b"",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st, 200, "Bearer 前缀应放行");
}

// ---------------------------------------------------------------------------
// 工具清单
// ---------------------------------------------------------------------------

/// 清单是 AI 的全部信息来源：每个工具都得有 name / summary / risk / args，
/// risk 只能是三档之一，且**危险工具必须存在并被标出来**（启停服务那类）。
#[test]
fn ai_tools_catalog_has_risk_and_args() {
    let srv = start_ai_server(TOKEN);
    let (st, body) = http_hdr(
        srv.addr,
        "GET",
        "/ai/v1/tools",
        "application/json",
        b"",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st, 200, "{body}");
    let j: Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(j["ok"], true);

    let tools = j["data"]["tools"].as_array().expect("tools 应是数组");
    assert!(tools.len() >= 15, "工具不该这么少：{}", tools.len());

    let mut names = Vec::new();
    let mut dangerous = 0usize;
    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        assert!(!name.is_empty(), "每个工具都要有名字：{t}");
        assert!(
            t["summary"].as_str().map(|s| !s.is_empty()).unwrap_or(false),
            "{name} 缺 summary（AI 靠它决定调不调）"
        );
        let risk = t["risk"].as_str().unwrap_or("");
        assert!(
            matches!(risk, "readonly" | "write" | "dangerous"),
            "{name} 的 risk 必须是三档之一，实际 {risk}"
        );
        assert!(t["args"].is_array(), "{name} 要有 args 数组（空也行）");
        if risk == "dangerous" {
            dangerous += 1;
        }
        names.push(name.to_string());
    }

    for must in ["get_status", "list_endpoints", "screenshot", "teach_target", "server_stop"] {
        assert!(names.iter().any(|n| n == must), "清单里该有 {must}：{names:?}");
    }
    assert!(dangerous >= 3, "启停服务/改端口这类必须标 dangerous，实际 {dangerous}");

    // 提示词里让 AI「先 GET /ai/v1/tools 看有没有 teach_target」，
    // 这个工具必须真的在（否则中文参考库教不了新目标）。
    let teach = tools
        .iter()
        .find(|t| t["name"] == "teach_target")
        .expect("teach_target 必须在清单里");
    let arg_names: Vec<&str> = teach["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap_or(""))
        .collect();
    for must in ["name", "image", "box"] {
        assert!(arg_names.contains(&must), "teach_target 的参数里该有 {must}：{arg_names:?}");
    }
}

// ---------------------------------------------------------------------------
// 错误可读性（AI 的自我修正全靠它）
// ---------------------------------------------------------------------------

/// 工具名写错：错误里要带「去看清单」的下一步，不能只说「不存在」。
#[test]
fn ai_unknown_tool_error_points_to_catalog() {
    let srv = start_ai_server(TOKEN);
    let (st, body) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        br#"{"tool":"ocr","args":{}}"#,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st, 400, "{body}");
    let j: Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(j["ok"], false);
    let err = j["error"].as_str().unwrap_or("");
    assert!(
        err.contains("/ai/v1/tools"),
        "错误要指向工具清单，实际：{err}"
    );
    // 顺手验一下拼写建议：ocr 该被提示成 ocr_image
    assert!(
        err.contains("ocr_image"),
        "该给最接近的候选名，实际：{err}"
    );
}

/// 缺必填参数：错误里要点名缺哪个（AI 才能补），不能只说「参数不对」。
#[test]
fn ai_missing_arg_error_names_the_arg() {
    let srv = start_ai_server(TOKEN);
    let (st, body) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        br#"{"tool":"teach_target","args":{}}"#,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st, 400, "{body}");
    let j: Value = serde_json::from_str(&body).expect("JSON");
    let err = j["error"].as_str().unwrap_or("");
    assert!(err.contains("name"), "错误要点名缺的参数，实际：{err}");
}

/// 请求体不是 JSON：错误里要给请求体长什么样的示例。
#[test]
fn ai_bad_json_error_shapes_the_body() {
    let srv = start_ai_server(TOKEN);
    let (st, body) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        b"not json at all",
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st, 400, "{body}");
    // 注意：error 文案是 JSON 字符串，示例里的引号是转义形式，断言不能带引号
    assert!(
        body.contains("request_id") && body.contains("tool"),
        "错误里该有请求体示例，实际：{body}"
    );
}

// ---------------------------------------------------------------------------
// 幂等
// ---------------------------------------------------------------------------

/// 同一个 request_id 重试：原样回放上次的响应体，**不执行第二遍**。
/// （写操作重复执行是真的会出事的，比如重复教同一个目标、重复停服务。）
#[test]
fn ai_idempotent_replay_returns_same_body() {
    let srv = start_ai_server(TOKEN);
    let body_text = br#"{"tool":"list_endpoints","args":{},"request_id":"idem-1"}"#;

    let (st1, b1) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        body_text,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st1, 200, "{b1}");

    let (st2, b2) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        body_text,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st2, 200, "{b2}");
    assert_eq!(b1, b2, "同一 request_id 必须原样回放，不能执行两遍");

    // 换个 request_id 就是新意图，该正常执行
    let (st3, b3) = http_hdr(
        srv.addr,
        "POST",
        "/ai/v1/tool",
        "application/json",
        br#"{"tool":"list_endpoints","args":{},"request_id":"idem-2"}"#,
        &[("Authorization", &format!("Bearer {TOKEN}"))],
    );
    assert_eq!(st3, 200, "{b3}");
    assert_eq!(b1, b3, "list_endpoints 是确定性的，两次结果该一致");
}
