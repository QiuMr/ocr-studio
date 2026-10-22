//! /vision/pipeline 的链路验收：yolo 找区域 -> PP-OCR 框内认字。
//!
//! 运行：cargo test --test pipeline_vision -- --ignored --nocapture

use std::path::{Path, PathBuf};

use base64::Engine as _;
use ocr_studio_lib::api::AppState;
use ocr_studio_lib::config::{AppConfig, Device, ModelLevel};
use ocr_studio_lib::engine::DetectOptions;
use ocr_studio_lib::ocr::ensure_ort_dylib_path;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "重量级：需要 yolo + ppocr 模型 + onnxruntime.dll"]
async fn pipeline_reads_text_inside_detected_regions() {
    let root = workspace_root();
    assert!(
        ensure_ort_dylib_path(&root),
        "找不到 src-tauri/runtime/onnxruntime.dll"
    );

    let state = std::sync::Arc::new(AppState::new(root.join("models"), AppConfig::default()));
    state
        .load_engine_blocking(Some(ModelLevel::Small), Some(Device::Directml), Some(false))
        .expect("PP-OCR 加载失败");

    let png = root.join("assets").join("bus.png");
    let bytes = std::fs::read(&png).expect("读图失败");
    let img = image::load_from_memory(&bytes).expect("解码失败");

    let opts = ocr_studio_lib::api::dto::OcrOptions {
        image: String::new(),
        level: None,
        device: None,
        use_cls: None,
        engine: None,
        keyword: None,
        conf: None,
        iou: None,
        class_filter: None,
        detect_engine: None,
        // 区域 OCR / 扫码是后加的字段，这条 pipeline 测试用不到
        region: None,
        barcode_formats: None,
        try_harder: None,
    };
    let detect = DetectOptions::default();

    let t0 = std::time::Instant::now();
    let regions = state
        .run_pipeline(&opts, img.clone(), detect.clone())
        .await
        .expect("pipeline 失败");
    eprintln!("pipeline 耗时 {:?}，区域 {}", t0.elapsed(), regions.len());

    let mut total_lines = 0usize;
    for (d, lines) in &regions {
        eprintln!(
            "  {:<8} score {:.3} box ({:.0},{:.0})-({:.0},{:.0})  lines {}",
            d.class_name,
            d.score,
            d.points[0].0,
            d.points[0].1,
            d.points[2].0,
            d.points[2].1,
            lines.len()
        );
        for l in lines {
            eprintln!("      {:?} ({:.3})", l.text, l.score);
        }
        total_lines += lines.len();
    }
    eprintln!("总文字行 {total_lines}");

    // 诊断：绕开 run_pipeline，直接用同一把引擎锁对同样裁剪跑 OCR，打印真实错误
    use image::GenericImageView;
    use ocr_studio_lib::engine::VisionEngine as _;
    let dets = {
        let det_opts = ocr_studio_lib::api::dto::OcrOptions {
            image: String::new(),
            level: None,
            device: None,
            use_cls: None,
            engine: Some("yolo".to_string()),
            keyword: None,
            conf: None,
            iou: None,
            class_filter: None,
            detect_engine: None,
            region: None,
            barcode_formats: None,
            try_harder: None,
        };
        state
            .detect_objects(&det_opts, img.clone(), detect)
            .await
            .expect("detect 失败")
    };
    {
        let mut guard = state.engine.lock().unwrap();
        let eng = guard.as_mut().expect("ppocr");
        for d in dets.iter().take(2) {
            let x0 = d.points[0].0.floor().max(0.0) as u32;
            let y0 = d.points[0].1.floor().max(0.0) as u32;
            let x1 = (d.points[2].0.ceil() as u32).min(img.width());
            let y1 = (d.points[2].1.ceil() as u32).min(img.height());
            let crop = img.crop_imm(x0, y0, x1 - x0, y1 - y0);
            eprintln!(
                "诊断 crop {}x{} class={}",
                crop.width(),
                crop.height(),
                d.class_name
            );
            match eng.infer(&crop, &ocr_studio_lib::engine::RecognizeOptions { want_chars: false }) {
                Ok(v) => eprintln!("  ok, {} 行", v.len()),
                Err(e) => eprintln!("  ERR: {e}"),
            }
        }
    }

    // 链路通断断言：bus.png 的巴士区域里肯定有字（对照 /ocr/shibie 有 7 行），
    // 若一个区域都没有文字，说明裁剪或 OCR 调用出了问题。
    assert!(total_lines > 0, "所有区域都没识别出文字，链路有问题");
}
