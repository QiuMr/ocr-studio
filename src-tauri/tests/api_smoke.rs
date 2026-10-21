//! HTTP 接口冒烟测试：起真实 axum 服务，用裸 TCP 发 HTTP/1.1 请求。
//!
//! 为什么不用 reqwest：这是一个只有几个端点的本地服务，手写 HTTP/1.1
//! （带 `Connection: close`，读到 EOF 即完整响应）比引入一个传递依赖巨多的
//! HTTP 客户端更划算，同时能真正验证「服务端能跑起来」这件事。
//!
//! 运行：cargo test --test api_smoke -- --ignored --nocapture

use std::io::{Read, Write};
use std::net::{TcpStream, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use ocr_studio_lib::api::{build_router, AppState};
use ocr_studio_lib::config::AppConfig;
use ocr_studio_lib::ocr::{ensure_ort_dylib_path, Device, ModelLevel};

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

/// 起一个 ephemeral 端口上的真实服务，引擎用 small + DirectML + 关 cls（与 Python 基线一致）。
fn start_server() -> TestServer {
    let root = workspace_root();
    assert!(
        ensure_ort_dylib_path(&root),
        "找不到 src-tauri/runtime/onnxruntime.dll，请先运行 scripts/copy_runtime.ps1"
    );

    let state = AppState::new(root.join("models"), AppConfig::default());
    state
        .load_engine_blocking(Some(ModelLevel::Small), Some(Device::Directml), Some(false))
        .expect("引擎加载失败");

    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口失败");
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let state = std::sync::Arc::new(state);
    let thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let std_listener = listener;
        let tokio_listener = rt.block_on(async {
            tokio::net::TcpListener::from_std(std_listener).expect("from_std")
        });
        rt.block_on(async move {
            axum::serve(tokio_listener, build_router().with_state(state))
                .await
                .expect("serve");
        });
    });

    // 等服务可接受连接
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

/// 发一次请求，返回 (状态码, 响应体字符串)。
fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("连接失败");
    s.set_read_timeout(Some(Duration::from_secs(180)))
        .expect("set timeout");
    s.set_write_timeout(Some(Duration::from_secs(30)))
        .expect("set timeout");

    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccept: */*\r\n\r\n",
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

/// 最小 chunked 解码（正常路径用不到，兜底防止误判）。
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
            Err(_) => return body.to_string(), // 不是 chunked，原样返回
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

/// 读测试图并转 base64。
fn test_image_b64() -> String {
    let bytes = std::fs::read(workspace_root().join("assets").join("test_zh.png"))
        .expect("读取 assets/test_zh.png 失败");
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// 构造 multipart 表单。
fn multipart_body(
    fields: &[(&str, &str)],
    file_field: &str,
    filename: &str,
    file_bytes: &[u8],
) -> (String, Vec<u8>) {
    let boundary = "----OCRStudioSmokeBoundary7MU3";
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{k}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(v.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{file_field}\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn json_num(body: &str, key: &str) -> Option<f64> {
    // 极简 JSON 取值：找 "key": <number>
    let pat = format!("\"{key}\":");
    let i = body.find(&pat)? + pat.len();
    let rest = body[i..].trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn health_returns_engine_status() {
    let srv = start_server();
    let (status, body) = http(srv.addr, "GET", "/ocr/health", "application/json", b"");
    println!("health -> {status} {body}");
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");
    assert!(body.contains("\"status\":\"ok\""), "status 应为 ok: {body}");
    assert!(body.contains("\"model\":\"small\""));
    assert!(body.contains("\"dict_size\":18708"), "字典大小应为 18708");
    assert_eq!(json_num(&body, "dict_size"), Some(18708.0));
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn shibie_json_matches_reference() {
    let srv = start_server();
    let payload = format!(
        "{{\"image\":\"{}\",\"level\":\"small\",\"device\":\"directml\",\"use_cls\":false}}",
        test_image_b64()
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/shibie",
        "application/json",
        payload.as_bytes(),
    );
    println!("shibie(json) -> {status}\n{body}");
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");
    // 参考基线是 7 行
    assert_eq!(
        json_num(&body, "elapsed_ms").is_some(),
        true,
        "缺少 elapsed_ms"
    );
    let lines = body.matches("\"det_score\"").count();
    assert_eq!(lines, 7, "应识别出 7 行（与 Python 基线一致），实际 {lines}");
    assert!(body.contains("OCRStudio测试样本第一行"), "首行文本应正确");
    assert!(body.contains("订单编号SO-2026-1021-8848"));
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn shibie_multipart_matches_json() {
    let srv = start_server();
    let png = std::fs::read(workspace_root().join("assets").join("test_zh.png")).expect("读图");
    let (ct, body_bytes) = multipart_body(
        &[("level", "small"), ("device", "directml"), ("use_cls", "false")],
        "image",
        "test_zh.png",
        &png,
    );
    let (status, body) = http(srv.addr, "POST", "/ocr/shibie", &ct, &body_bytes);
    println!("shibie(multipart) -> {status}\n{}", &body[..body.len().min(600)]);
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");
    let lines = body.matches("\"det_score\"").count();
    assert_eq!(lines, 7, "multipart 路径应同样识别出 7 行，实际 {lines}");
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn zhaozi_filters_by_keyword() {
    let srv = start_server();
    let payload = format!(
        "{{\"image\":\"{}\",\"use_cls\":false,\"keyword\":\"订单编号\"}}",
        test_image_b64()
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/zhaozi",
        "application/json",
        payload.as_bytes(),
    );
    println!("zhaozi -> {status} {body}");
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"));
    assert_eq!(json_num(&body, "matched"), Some(1.0), "应命中 1 行");
    assert!(body.contains("订单编号SO-2026-1021-8848"));
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn regex_keyword_works() {
    let srv = start_server();
    let payload = format!(
        "{{\"image\":\"{}\",\"use_cls\":false,\"keyword\":\"/\\\\d{{11}}/\"}}",
        test_image_b64()
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/zhaozi",
        "application/json",
        payload.as_bytes(),
    );
    println!("zhaozi(regex) -> {status} {body}");
    assert_eq!(status, 200);
    // 13800138000 是 11 位数字
    assert_eq!(json_num(&body, "matched"), Some(1.0), "应命中含 11 位数字的行");
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn m2_endpoints_return_501() {
    let srv = start_server();
    for path in [
        "/ocr/tuzhaotu1",
        "/ocr/tuzhaotu2",
        "/ocr/jubingzhaotu1",
        "/ocr/pingmuzhaotu2",
        "/ocr/danzijson",
    ] {
        let (status, body) = http(srv.addr, "POST", path, "application/json", b"{}");
        println!("{path} -> {status} {body}");
        assert_eq!(status, 501, "{path} 应返回 501");
        assert!(body.contains("\"code\":501"));
    }
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn bad_requests_return_400() {
    let srv = start_server();
    // 缺 image 字段
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/shibie",
        "application/json",
        b"{\"level\":\"small\"}",
    );
    println!("缺 image -> {status} {body}");
    assert_eq!(status, 400);

    // 坏 base64
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/shibie",
        "application/json",
        b"{\"image\":\"!!!not-base64!!!\"}",
    );
    println!("坏 base64 -> {status} {body}");
    assert_eq!(status, 400);

    // 坏 JSON
    let (status, _) = http(srv.addr, "POST", "/ocr/shibie", "application/json", b"{oops");
    assert_eq!(status, 400);

    // 未知路径
    let (status, _) = http(srv.addr, "GET", "/ocr/nope", "application/json", b"");
    assert_eq!(status, 404);
}
