//! 精度对齐测试：Rust 实现 vs Python 参考实现（scripts/ref_pipeline.py）的黄金基线。
//!
//! 基线文件：`models/small/reference_output.json`
//! 测试图：`assets/test_zh.png`
//!
//! 运行前提：
//!   1. 已执行 `scripts/fetch_models.ps1` 下载 small 档模型
//!   2. 已执行 `scripts/copy_runtime.ps1` 拷贝 onnxruntime DLL
//!   3. 设置 `ORT_DYLIB_PATH` 指向 `src-tauri/runtime/onnxruntime.dll`
//!      （未设置时本测试会自动补上）
//!
//! 运行：
//!   cargo test --test pipeline_match -- --ignored --nocapture
//!
//! 判定标准：
//!   - 文本必须完全一致（这是 OCR 的核心产出）
//!   - 框角点允许 ≤2px 偏差（轮廓点集与 OpenCV CHAIN_APPROX 的已知差异）
//!   - 置信度允许 0.05 偏差

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ocr_studio_lib::ocr::{EngineConfig, ModelLevel, OcrEngine, Device};

/// 从模型目录读取 Python 参考输出。
///
/// JSON 里的字段名是 `box`，但 `box` 是 Rust 保留字，用 serde rename 映射。
#[derive(Debug, serde::Deserialize)]
struct RefItem {
    text: String,
    score: f64,
    #[serde(rename = "box")]
    points: Vec<[i64; 2]>,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

/// 确保 ORT_DYLIB_PATH 已设置，否则指向项目内的 runtime DLL。
///
/// 不设置的后果：ort 会按 Windows 标准顺序找到 `C:\WINDOWS\system32\onnxruntime.dll`，
/// 本机那份是 1.17.1，低于 ort 要求（>=1.24.x），进程直接起不来。
fn ensure_dylib_path() {
    assert!(
        ocr_studio_lib::ocr::ensure_ort_dylib_path(&workspace_root()),
        "找不到 src-tauri/runtime/onnxruntime.dll，请先运行 scripts/copy_runtime.ps1"
    );
}

/// 读图（绕开 Windows 非 ASCII 路径问题）。
fn read_image(path: &Path) -> DynamicImage {
    let bytes = std::fs::read(path).expect("读图失败");
    image::load_from_memory(&bytes).expect("解码失败")
}

fn dist(a: [f64; 2], b: [i64; 2]) -> f64 {
    ((a[0] - b[0] as f64).powi(2) + (a[1] - b[1] as f64).powi(2)).sqrt()
}

#[test]
#[ignore = "需要模型文件与 onnxruntime DLL，属于重量级集成测试"]
fn rust_matches_python_reference() {
    ensure_dylib_path();
    let root = workspace_root();

    let img_path = root.join("assets").join("test_zh.png");
    assert!(img_path.exists(), "缺少测试图 {}", img_path.display());
    let img = read_image(&img_path);

    // 设备选择必须与 Python 基线一致：scripts/ref_pipeline.py 用的是
    // DmlExecutionProvider（见其输出 "det provider: DmlExecutionProvider"）。
    // DirectML 与 CPU 的浮点累加顺序不同，对低置信度的误检框影响可达 0.15，
    // 所以这里必须对齐 provider，否则会误判成实现 bug。
    let mut engine = OcrEngine::load(EngineConfig {
        models_root: root.join("models"),
        level: ModelLevel::Small,
        device: Device::Directml,
        use_cls: false,
        ..Default::default()
    })
    .expect("引擎加载失败");

    let health = engine.health();
    println!(
        "请求设备={:?} 实际设备={:?} 字典大小={} cls={}",
        health.requested_device,
        health.actual_device,
        health.dict_size,
        health.cls_loaded
    );
    assert_eq!(
        health.actual_device,
        Device::Directml,
        "DirectML 不可用时会自动降级 CPU，但那样无法与 DirectML 基线对齐"
    );

    let results = engine.recognize(&img).expect("识别失败");

    let ref_path = root.join("models").join("small").join("reference_output.json");
    let ref_json = std::fs::read_to_string(&ref_path).expect("读取参考输出失败");
    let reference: Vec<RefItem> =
        serde_json::from_str(&ref_json).expect("解析参考输出失败");

    println!("\n=== Rust 输出 ({} 行) ===", results.len());
    for r in &results {
        println!(
            "  [{:.3}] {} | box={:?}",
            r.score,
            r.text,
            r.points
                .iter()
                .map(|p| [p.0.round() as i64, p.1.round() as i64])
                .collect::<Vec<_>>()
        );
    }

    println!("\n=== Python 参考 ({} 行) ===", reference.len());
    for r in &reference {
        println!("  [{:.3}] {} | box={:?}", r.score, r.text, r.points);
    }

    // ---- 逐项比对 ----
    assert_eq!(
        results.len(),
        reference.len(),
        "行数不一致：Rust {} vs Python {}",
        results.len(),
        reference.len()
    );

    let mut text_mismatch = Vec::new();
    let mut box_mismatch = Vec::new();
    let mut score_mismatch = Vec::new();

    for (i, (r, rf)) in results.iter().zip(reference.iter()).enumerate() {
        if r.text != rf.text {
            text_mismatch.push(format!(
                "  #{i}: Rust={:?}  Python={:?}",
                r.text, rf.text
            ));
        }
        // 框：四点按顺序比对，允许 2px
        let max_box_err = r
            .points
            .iter()
            .zip(rf.points.iter())
            .map(|(p, b)| dist([p.0, p.1], *b))
            .fold(0.0f64, f64::max);
        if max_box_err > 2.0 {
            box_mismatch.push(format!(
                "  #{i}: 最大偏差 {max_box_err:.2}px  Rust={:?}  Python={:?}",
                r.points
                    .iter()
                    .map(|p| [p.0.round() as i64, p.1.round() as i64])
                    .collect::<Vec<_>>(),
                rf.points
            ));
        }
        if (r.score - rf.score).abs() > 0.05 {
            score_mismatch.push(format!(
                "  #{i}: Rust={:.4}  Python={:.4}",
                r.score, rf.score
            ));
        }
    }

    println!("\n=== 比对结果 ===");
    println!("文本不一致: {}", text_mismatch.len());
    for m in &text_mismatch {
        println!("{m}");
    }
    println!("框偏差 >2px: {}", box_mismatch.len());
    for m in &box_mismatch {
        println!("{m}");
    }
    println!("置信度偏差 >0.05: {}", score_mismatch.len());
    for m in &score_mismatch {
        println!("{m}");
    }

    assert!(
        text_mismatch.is_empty(),
        "{} 行文本不一致",
        text_mismatch.len()
    );
    assert!(
        box_mismatch.is_empty(),
        "{} 个框偏差超过 2px",
        box_mismatch.len()
    );
    assert!(
        score_mismatch.is_empty(),
        "{} 个置信度偏差超过 0.05",
        score_mismatch.len()
    );

    println!("\n全部通过：Rust 实现与 Python 参考基线一致。");
}
