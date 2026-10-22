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
use ocr_studio_lib::engine::catalog;
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

/// 把 state 跑起来：绑 ephemeral 端口 → 起 axum → 等到能连上。
///
/// 三种「要不要加载引擎」的组合共用这一段（下面三个 `start_server_*`），
/// 免得绑定/等待就绪的逻辑各写一遍、哪天改了只改一处。
fn spawn_server(state: AppState) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口失败");
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();

    let state = std::sync::Arc::new(state);
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

/// 起一个真实服务，引擎用 small + DirectML + 关 cls（与 Python 基线一致）。
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
    spawn_server(state)
}

/// 起一个**不加载 OCR 引擎**的服务。找图类接口是纯 CPU 计算，不需要模型，
/// 这样跑找图测试既快也不依赖 onnxruntime.dll 与模型文件。
fn start_server_without_engine() -> TestServer {
    let root = workspace_root();
    spawn_server(AppState::new(root.join("models"), AppConfig::default()))
}

/// 起一个**只准备了 onnxruntime.dll** 的服务（不加载 PP-OCR）：
/// 语义找图只需要检测模型，加载 det+rec 几百毫秒纯属浪费。
fn start_server_for_detect() -> TestServer {
    let root = workspace_root();
    assert!(
        ensure_ort_dylib_path(&root),
        "找不到 src-tauri/runtime/onnxruntime.dll，请先运行 scripts/copy_runtime.ps1"
    );
    spawn_server(AppState::new(root.join("models"), AppConfig::default()))
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

fn json_bool(body: &str, key: &str) -> Option<bool> {
    let pat = format!("\"{key}\":");
    let i = body.find(&pat)? + pat.len();
    let rest = body[i..].trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

// ---------------------------------------------------------------- 找图

/// 合成一张带纹理的「屏幕」，并在 `(bx, by)` 贴一个高对比度按钮。
fn synth_scene(w: u32, h: u32, bx: u32, by: u32, bw: u32, bh: u32) -> image::RgbImage {
    use image::{Rgb, RgbImage};
    let mut img = RgbImage::new(w, h);
    let mut seed = 0x9e37_79b9u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for y in 0..h {
        for x in 0..w {
            let base = (x * 255 / w.max(1) + y * 128 / h.max(1)) % 256;
            let v = ((base + next() % 40) % 256) as u8;
            img.put_pixel(x, y, Rgb([v, v.wrapping_add(30), v.wrapping_add(70)]));
        }
    }
    for dy in 0..bh {
        for dx in 0..bw {
            let border = dx == 0 || dy == 0 || dx == bw - 1 || dy == bh - 1;
            let c = if border {
                Rgb([20u8, 20, 120])
            } else if (dx + dy) % 7 == 0 {
                Rgb([240, 240, 30])
            } else if dx < bw / 2 {
                Rgb([200, 40, 40])
            } else {
                Rgb([40, 190, 90])
            };
            img.put_pixel(bx + dx, by + dy, c);
        }
    }
    img
}

fn png_b64(img: &image::RgbImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG 编码失败");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

/// 灰度图（掩码）编成 PNG 再 base64。
fn luma_png_b64(img: &image::GrayImage) -> String {
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(img.clone())
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG 编码失败");
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

const SCENE_BTN: (u32, u32, u32, u32) = (150, 120, 64, 48);

#[test]
fn tuzhaotu_finds_template_in_uploaded_image() {
    let srv = start_server_without_engine();
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3);
    let tpl = image::imageops::crop_imm(
        &scene, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3,
    )
    .to_image();

    let payload = format!(
        "{{\"image\":\"{}\",\"template\":\"{}\"}}",
        png_b64(&scene),
        png_b64(&tpl)
    );
    for path in ["/ocr/tuzhaotu1", "/ocr/tuzhaotu2"] {
        let (status, body) = http(srv.addr, "POST", path, "application/json", payload.as_bytes());
        println!("{path} -> {status} {body}");
        assert_eq!(status, 200, "{path} 应返回 200: {body}");
        assert!(body.contains("\"code\":0"), "{path} code 应为 0: {body}");
        assert_eq!(json_bool(&body, "found"), Some(true), "{path} 应命中");
        assert_eq!(json_num(&body, "x"), Some(SCENE_BTN.0 as f64), "{path} x 不对");
        assert_eq!(json_num(&body, "y"), Some(SCENE_BTN.1 as f64), "{path} y 不对");
        assert_eq!(json_num(&body, "width"), Some(SCENE_BTN.2 as f64));
        assert_eq!(json_num(&body, "height"), Some(SCENE_BTN.3 as f64));
        assert!(
            json_num(&body, "score").unwrap_or(0.0) > 0.99,
            "{path} 分数应接近 1"
        );
        assert!(body.contains("\"source\":\"image\""));
    }
}

/// 掩码的核心价值：模板里混着「会变的部分」时，整块比像素会失配，
/// 把变化区域用掩码遮掉就又能命中。这里走 HTTP JSON 全链路验证。
#[test]
fn tuzhaotu_masked_ignores_changing_region() {
    let srv = start_server_without_engine();
    let (tw, th) = (SCENE_BTN.2, SCENE_BTN.3);
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, tw, th);
    let mut tpl =
        image::imageops::crop_imm(&scene, SCENE_BTN.0, SCENE_BTN.1, tw, th).to_image();

    // 模拟「模板中心一块内容变了」（按钮文字换了）：把中间 20×12 涂成纯白
    const CX: u32 = 22;
    const CY: u32 = 18;
    const CW: u32 = 20;
    const CH: u32 = 12;
    for dy in 0..CH {
        for dx in 0..CW {
            tpl.put_pixel(CX + dx, CY + dy, image::Rgb([255u8, 255, 255]));
        }
    }

    // ① 不遮罩 → 因变化块失配
    let payload = format!(
        "{{\"image\":\"{}\",\"template\":\"{}\"}}",
        png_b64(&scene),
        png_b64(&tpl)
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("mask-off -> {status} {body}");
    assert_eq!(status, 200);
    assert_eq!(json_bool(&body, "found"), Some(false), "未遮罩应失配: {body}");

    // ② 遮住变化块 → 命中
    let mut mask = image::GrayImage::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            let changing = x >= CX && x < CX + CW && y >= CY && y < CY + CH;
            mask.put_pixel(x, y, image::Luma([if changing { 0u8 } else { 255 }]));
        }
    }
    let payload = format!(
        "{{\"image\":\"{}\",\"template\":\"{}\",\"mask\":\"{}\"}}",
        png_b64(&scene),
        png_b64(&tpl),
        luma_png_b64(&mask)
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("mask-on  -> {status} {body}");
    assert_eq!(status, 200);
    assert_eq!(json_bool(&body, "found"), Some(true), "遮罩后应命中: {body}");
    assert_eq!(json_num(&body, "x"), Some(SCENE_BTN.0 as f64));
    assert_eq!(json_num(&body, "y"), Some(SCENE_BTN.1 as f64));
    assert!(json_num(&body, "score").unwrap_or(0.0) > 0.99);
}

/// 掩码尺寸与模板不一致必须报 400（而不是自动缩放）。
#[test]
fn tuzhaotu_rejects_mask_size_mismatch() {
    let srv = start_server_without_engine();
    let (tw, th) = (SCENE_BTN.2, SCENE_BTN.3);
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, tw, th);
    let tpl = image::imageops::crop_imm(&scene, SCENE_BTN.0, SCENE_BTN.1, tw, th).to_image();
    // 故意给一个尺寸不对的掩码
    let mask = image::GrayImage::from_pixel(16, 16, image::Luma([255u8]));
    let payload = format!(
        "{{\"image\":\"{}\",\"template\":\"{}\",\"mask\":\"{}\"}}",
        png_b64(&scene),
        png_b64(&tpl),
        luma_png_b64(&mask)
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("mask-bad -> {status} {body}");
    assert_eq!(status, 400, "掩码尺寸不一致应 400: {body}");
    assert!(body.contains("不一致"), "错误信息应说明尺寸不一致: {body}");
}

#[test]
fn vision_qrcode_decodes_generated_qr() {
    use rxing::Writer;
    // 这个端点不碰引擎槽，所以「没有引擎」的服务也能验。
    let srv = start_server_without_engine();
    let mat = rxing::MultiFormatWriter
        .encode(
            "https://example.com/vision-qr",
            &rxing::BarcodeFormat::QR_CODE,
            260,
            260,
        )
        .expect("编码失败");
    let (mw, mh) = (mat.getWidth(), mat.getHeight());
    let mut img = image::RgbImage::new(mw, mh);
    for y in 0..mh {
        for x in 0..mw {
            let v = if mat.get(x, y) { 0u8 } else { 255u8 };
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }

    let payload = format!("{{\"image\":\"{}\"}}", png_b64(&img));
    let (status, body) = http(
        srv.addr,
        "POST",
        "/vision/qrcode",
        "application/json",
        payload.as_bytes(),
    );
    println!("qrcode -> {status} {body}");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"code\":0"), "{body}");
    assert_eq!(json_num(&body, "count"), Some(1.0), "应识别出 1 个码: {body}");
    assert!(body.contains("https://example.com/vision-qr"), "文本应一致: {body}");
    assert!(body.contains("QR_CODE"), "码制应回显: {body}");
    // 注意：`json_num` 取的是**第一个**同名 key，而 codes[] 里也有 width/height
    // （那是命中框的尺寸）。外层的原图尺寸要整串匹配才不会撞上。
    assert!(
        body.contains(&format!("\"width\":{mw},\"height\":{mh}")),
        "外层应回传原图尺寸 {mw}x{mh}: {body}"
    );
}

#[test]
fn vision_qrcode_blank_image_is_not_an_error() {
    let srv = start_server_without_engine();
    let img = image::RgbImage::from_pixel(240, 240, image::Rgb([210u8, 210, 210]));
    let payload = format!("{{\"image\":\"{}\"}}", png_b64(&img));
    let (status, body) = http(
        srv.addr,
        "POST",
        "/vision/qrcode",
        "application/json",
        payload.as_bytes(),
    );
    println!("qrcode(blank) -> {status} {body}");
    // 「图里没有码」是正常业务结果，HTTP 仍 200，只把 count 置 0
    assert_eq!(status, 200, "没有码不该报错: {body}");
    assert_eq!(json_num(&body, "count"), Some(0.0), "{body}");
}

#[test]
fn vision_qrcode_rejects_unknown_format_name() {
    let srv = start_server_without_engine();
    let img = synth_scene(120, 120, 20, 20, 40, 40);
    let payload = format!(
        "{{\"image\":\"{}\",\"barcode_formats\":[\"qrcod3\"]}}",
        png_b64(&img)
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/vision/qrcode",
        "application/json",
        payload.as_bytes(),
    );
    println!("qrcode(bad format) -> {status} {body}");
    assert_eq!(status, 400, "拼错的码制名应 400: {body}");
    assert!(body.contains("不认识的码制"), "{body}");
}

#[test]
fn tuzhaotu_reports_not_found_gracefully() {
    let srv = start_server_without_engine();
    let scene = synth_scene(320, 240, 10, 10, 40, 40);
    // 用与场景无关的反色块当模板
    let mut other = synth_scene(48, 48, 0, 0, 48, 48);
    for p in other.pixels_mut() {
        *p = image::Rgb([255 - p[0], 255 - p[1], 255 - p[2]]);
    }
    let payload = format!(
        "{{\"image\":\"{}\",\"template\":\"{}\"}}",
        png_b64(&scene),
        png_b64(&other)
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("not-found -> {status} {body}");
    // 「没找到」是正常业务结果，HTTP 仍是 200，只把 found 置 false
    assert_eq!(status, 200);
    assert_eq!(json_bool(&body, "found"), Some(false));
    assert!(body.contains("\"matched\":0"));
}

#[test]
fn tuzhaotu_multipart_upload_works() {
    let srv = start_server_without_engine();
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3);
    let tpl = image::imageops::crop_imm(
        &scene, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3,
    )
    .to_image();

    // 多文件 multipart：image + template 各一个文件字段
    let boundary = "----FindBoundary91QX";
    let mut body = Vec::new();
    for (name, data) in [("image", &scene), ("template", &tpl)] {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8((*data).clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&buf.into_inner());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let (status, resp) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        &format!("multipart/form-data; boundary={boundary}"),
        &body,
    );
    println!("multipart find -> {status} {resp}");
    assert_eq!(status, 200, "{resp}");
    assert_eq!(json_bool(&resp, "found"), Some(true));
    assert_eq!(json_num(&resp, "x"), Some(SCENE_BTN.0 as f64));
}

/// multipart 形态下掩码走独立的文件字段（`mask`），验证它被正确读取。
#[test]
fn tuzhaotu_multipart_mask_field_works() {
    let srv = start_server_without_engine();
    let (tw, th) = (SCENE_BTN.2, SCENE_BTN.3);
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, tw, th);
    let mut tpl = image::imageops::crop_imm(&scene, SCENE_BTN.0, SCENE_BTN.1, tw, th).to_image();
    // 中心变化块（与 JSON 用例一致）
    const CX: u32 = 22;
    const CY: u32 = 18;
    const CW: u32 = 20;
    const CH: u32 = 12;
    for dy in 0..CH {
        for dx in 0..CW {
            tpl.put_pixel(CX + dx, CY + dy, image::Rgb([255u8, 255, 255]));
        }
    }
    let mut mask = image::GrayImage::new(tw, th);
    for y in 0..th {
        for x in 0..tw {
            let changing = x >= CX && x < CX + CW && y >= CY && y < CY + CH;
            mask.put_pixel(x, y, image::Luma([if changing { 0u8 } else { 255 }]));
        }
    }

    let boundary = "----FindMaskBoundary7KQ";
    let mut body = Vec::new();
    let rgb_fields: [(&str, &image::RgbImage); 2] = [("image", &scene), ("template", &tpl)];
    for (name, data) in rgb_fields {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8((*data).clone())
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}.png\"\r\nContent-Type: image/png\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&buf.into_inner());
        body.extend_from_slice(b"\r\n");
    }
    {
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageLuma8(mask)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\n",
        );
        body.extend_from_slice(&buf.into_inner());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let (status, resp) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        &format!("multipart/form-data; boundary={boundary}"),
        &body,
    );
    println!("multipart mask -> {status} {resp}");
    assert_eq!(status, 200, "{resp}");
    assert_eq!(json_bool(&resp, "found"), Some(true), "掩码文件字段应生效: {resp}");
    assert_eq!(json_num(&resp, "x"), Some(SCENE_BTN.0 as f64));
}

#[test]
fn tuzhaotu_template_file_path_works() {
    let srv = start_server_without_engine();
    let scene = synth_scene(400, 300, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3);
    let tpl = image::imageops::crop_imm(
        &scene, SCENE_BTN.0, SCENE_BTN.1, SCENE_BTN.2, SCENE_BTN.3,
    )
    .to_image();

    let dir = std::env::temp_dir().join("ocr_studio_find_test");
    std::fs::create_dir_all(&dir).expect("建临时目录失败");
    let tpl_path = dir.join("tpl.png");
    image::DynamicImage::ImageRgb8(tpl)
        .save(&tpl_path)
        .expect("写模板文件失败");

    let payload = format!(
        "{{\"image\":\"{}\",\"template_file\":{:?}}}",
        png_b64(&scene),
        tpl_path.to_string_lossy()
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("template_file -> {status} {body}");
    assert_eq!(status, 200, "{body}");
    assert_eq!(json_bool(&body, "found"), Some(true));
    assert_eq!(json_num(&body, "x"), Some(SCENE_BTN.0 as f64));
}

#[test]
fn find_rejects_bad_extension_for_template_file() {
    let srv = start_server_without_engine();
    let payload = r#"{"image":"AAAA","template_file":"C:\\Windows\\System32\\drivers\\etc\\hosts"}"#;
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("bad ext -> {status} {body}");
    assert_eq!(status, 400, "非图片扩展名必须被拒绝: {body}");
    assert!(body.contains("template_file"), "{body}");
}

#[test]
fn find_requires_template() {
    let srv = start_server_without_engine();
    let scene = synth_scene(100, 100, 10, 10, 20, 20);
    let payload = format!("{{\"image\":\"{}\"}}", png_b64(&scene));
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/tuzhaotu1",
        "application/json",
        payload.as_bytes(),
    );
    println!("no template -> {status} {body}");
    assert_eq!(status, 400);
    assert!(body.contains("模板"), "{body}");
}

// ---------------------------------------------------------------- 语义找图

/// 路由存在 + 参数校验。这三条都在碰引擎**之前**返回，所以不需要模型和 DLL。
#[test]
fn yuyizhaotu_route_exists_and_validates_params() {
    let srv = start_server_without_engine();
    let scene = synth_scene(120, 90, 20, 20, 30, 24);
    let img = png_b64(&scene);

    // 缺搜索图 → 400（**不是 404**，证明路由已注册）
    let (status, body) = http(srv.addr, "POST", "/ocr/yuyizhaotu", "application/json", b"{}");
    println!("yuyizhaotu(no image) -> {status} {body}");
    assert_eq!(status, 400, "缺 image 应 400 而不是 404: {body}");
    assert!(body.contains("搜索图"), "应提示缺搜索图: {body}");

    // source 写错 → 400，且把可选值列出来
    let payload = format!("{{\"image\":\"{img}\",\"source\":\"desktop\"}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/yuyizhaotu",
        "application/json",
        payload.as_bytes(),
    );
    println!("yuyizhaotu(bad source) -> {status} {body}");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("source") && body.contains("image"), "{body}");

    // conf 越界 → 400（3 不是概率）
    let payload = format!("{{\"image\":\"{img}\",\"conf\":3}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/yuyizhaotu",
        "application/json",
        payload.as_bytes(),
    );
    println!("yuyizhaotu(bad conf) -> {status} {body}");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("conf"), "{body}");
}

/// 语义找图真跑一遍：需要 `models/detect/yolo/` 下有**认得 assets/bus.png** 的模型。
///
/// 目录为空时自动跳过（出厂状态就是这个，和 `yoloe_smoke` 一个口径）。
/// 跑：`cargo test --test api_smoke -- --ignored --nocapture`
#[test]
#[ignore = "重量级：需要 models/detect/yolo/ 下有认得 assets/bus.png 的检测模型 + onnxruntime.dll"]
fn yuyizhaotu_finds_target_with_user_model() {
    let root = workspace_root();
    let models = catalog::list_yolo_models(&root.join("models"), &[]);
    if models.is_empty() {
        eprintln!("models/detect/yolo/ 里没有模型，跳过（出厂状态就是这个）");
        return;
    }
    let srv = start_server_for_detect();

    let b64 = base64::engine::general_purpose::STANDARD
        .encode(std::fs::read(root.join("assets").join("bus.png")).expect("读 bus.png 失败"));

    // 用文件名当模型 id，避开「哪个是默认模型」这个与测试无关的问题
    let engine = format!("yolo:{}", models[0].id);
    let payload = format!("{{\"image\":\"{b64}\",\"engine\":\"{engine}\",\"conf\":0.25,\"max_results\":5}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/yuyizhaotu",
        "application/json",
        payload.as_bytes(),
    );
    println!("yuyizhaotu -> {status}\n{}", &body[..body.len().min(800)]);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"code\":0"), "{body}");

    // 出参必须与模板找图**同构**：这些字段一个都不能少，否则脚本换路径就崩
    for key in [
        "\"found\":",
        "\"x\":",
        "\"y\":",
        "\"score\":",
        "\"center_x\":",
        "\"center_y\":",
        "\"width\":",
        "\"height\":",
        "\"matched\":",
        "\"matches\":",
        "\"origin_x\":",
        "\"origin_y\":",
        "\"elapsed_ms\":",
    ] {
        assert!(body.contains(key), "语义找图出参缺少 {key}: {body}");
    }
    // 语义找图独有的字段
    assert!(body.contains("\"engine\":"), "应回显实际引擎: {body}");
    assert!(body.contains("\"source\":\"image\""), "{body}");

    assert_eq!(json_bool(&body, "found"), Some(true), "应检出目标: {body}");
    assert!(
        body.contains("\"class_name\":") && body.contains("\"class_id\":"),
        "命中里应带类别: {body}"
    );
    // 图内坐标：source=image 时偏移恒为 0
    assert_eq!(json_num(&body, "origin_x"), Some(0.0));
    assert_eq!(json_num(&body, "origin_y"), Some(0.0));
    // 首框中心必须落在 bus.png 尺寸内（1280x720 之类，给足余量）
    let cx = json_num(&body, "center_x").expect("缺 center_x");
    let cy = json_num(&body, "center_y").expect("缺 center_y");
    assert!(
        (0.0..5000.0).contains(&cx) && (0.0..5000.0).contains(&cy),
        "中心坐标越界: ({cx}, {cy})"
    );
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn danzijson_returns_char_boxes() {
    let srv = start_server();

    // 缺 image 必须报 400，而不是 200 + 空结果
    let (status, body) = http(srv.addr, "POST", "/ocr/danzijson", "application/json", b"{}");
    println!("/ocr/danzijson (no image) -> {status} {body}");
    assert_eq!(status, 400, "缺 image 应返回 400: {body}");

    // 有图：应返回逐字数据，且字数大于 0（test_zh.png 是中文样本）
    let payload = format!("{{\"image\":\"{}\"}}", test_image_b64());
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/danzijson",
        "application/json",
        payload.as_bytes(),
    );
    println!("/ocr/danzijson -> {status} {}", &body[..body.len().min(300)]);
    assert_eq!(status, 200, "danzijson 应返回 200: {body}");
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");
    assert!(body.contains("\"lines\""), "应含 lines: {body}");
    assert!(body.contains("\"frames\""), "应含 frames: {body}");
    let chars = json_num(&body, "char_count").unwrap_or(0.0);
    assert!(chars > 0.0, "应识别出至少一个字符，实际 {chars}: {body}");
}

#[test]
fn engine_list_reports_capabilities() {
    // 只扫目录，不需要模型与 DLL，所以不标 ignore
    let srv = start_server_without_engine();
    let (status, body) = http(srv.addr, "GET", "/engine/list", "application/json", b"");
    println!("/engine/list -> {status} {}", &body[..body.len().min(400)]);
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");

    // 引擎数**按族**断言，不写死总数：检测模型是「用户自己的」，
    // models/detect/yolo/ 里放几个就有几个，写死总数只会随用户放模型而变红。
    assert_eq!(
        body.matches("\"family\":\"ppocr\"").count(),
        3,
        "PP-OCR 三档: {body}"
    );
    assert_eq!(
        body.matches("\"family\":\"ddddocr\"").count(),
        2,
        "DDDDOCR 两版: {body}"
    );

    // 本仓库只有 models/small 是真模型，它必须是「可用」的
    assert!(body.contains("\"name\":\"ppocr:small\""), "应含 ppocr:small");
    assert!(body.contains("\"default_engine\":\"ppocr:small\""));
    // 空目录的档位必须明确标成不可用，并说清缺什么
    assert!(body.contains("\"name\":\"ppocr:tiny\""));
    assert!(
        body.contains("缺少"),
        "不可用的引擎要给出缺失文件提示: {body}"
    );
    // 只有 ppocr 支持逐字；YOLO 条目不能声称支持文本行
    assert!(body.contains("\"line_boxes\":true"));
    assert!(body.contains("\"detect\":false"));
    // 出厂状态（detect/yolo/ 为空）下**不应该**有 yolo 条目——
    // 程序不预置检测模型。有模型时这条自然跳过。
    let has_user_model = !catalog::list_yolo_models(&workspace_root().join("models"), &[]).is_empty();
    assert_eq!(
        body.contains("\"family\":\"yolo\""),
        has_user_model,
        "yolo 条目应当且只应当在目录里有模型时出现: {body}"
    );
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
fn quyuocr_validates_region_before_engine() {
    // 轻量：region 校验发生在调引擎之前，所以没有模型也能验。
    let srv = start_server_without_engine();
    let img = synth_scene(120, 90, 20, 20, 40, 30);
    let b64 = png_b64(&img);

    // 缺 region → 400
    let payload = format!("{{\"image\":\"{b64}\"}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/quyuocr",
        "application/json",
        payload.as_bytes(),
    );
    println!("quyuocr(no region) -> {status} {body}");
    assert_eq!(status, 400, "缺 region 应 400: {body}");
    assert!(body.contains("region"), "错误应提到 region: {body}");

    // 宽高非正 → 400
    let payload = format!("{{\"image\":\"{b64}\",\"region\":[0,0,0,10]}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/quyuocr",
        "application/json",
        payload.as_bytes(),
    );
    println!("quyuocr(zero w) -> {status} {body}");
    assert_eq!(status, 400, "宽为 0 应 400: {body}");

    // 与图片没有有效交集（全在界外）→ 400
    let payload = format!("{{\"image\":\"{b64}\",\"region\":[500,500,40,40]}}");
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/quyuocr",
        "application/json",
        payload.as_bytes(),
    );
    println!("quyuocr(out of bounds) -> {status} {body}");
    assert_eq!(status, 400, "越界应 400: {body}");
}

#[test]
#[ignore = "重量级集成测试：需要模型与 DLL"]
fn quyuocr_region_matches_full_image_when_full() {
    // 用整张图当区域：结果应与 /ocr/shibie 一致（坐标偏移为 0，可逐行比对）。
    let srv = start_server();
    let png = std::fs::read(workspace_root().join("assets").join("test_zh.png")).expect("读图");
    let (w, h) = image::load_from_memory(&png)
        .expect("解码")
        .to_rgb8()
        .dimensions();

    let payload = format!(
        "{{\"image\":\"{}\",\"use_cls\":false,\"region\":[0,0,{w},{h}]}}",
        test_image_b64()
    );
    let (status, body) = http(
        srv.addr,
        "POST",
        "/ocr/quyuocr",
        "application/json",
        payload.as_bytes(),
    );
    println!("quyuocr(full) -> {status}\n{body}");
    assert_eq!(status, 200);
    assert!(body.contains("\"code\":0"), "code 应为 0: {body}");
    let lines = body.matches("\"det_score\"").count();
    assert_eq!(lines, 7, "整图区域应识别出 7 行，实际 {lines}");
    assert!(body.contains("订单编号SO-2026-1021-8848"));
    // 出参里应带上原图尺寸与生效区域
    assert_eq!(json_num(&body, "width"), Some(w as f64));
    assert_eq!(json_num(&body, "height"), Some(h as f64));
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
