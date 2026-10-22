//! YOLO 引擎与本机 ultralytics 的逐框对齐测试。
//!
//! 参考值由 `scripts/ref_yolo.py` 生成：用 ultralytics 跑**同一份导出的 onnx**
//! （而不是 .pt），这样比对差异只可能来自我们的预处理/解码/NMS。
//! 参考文件：`models/_fixtures/detect/yolo/ref_bus.json` / `ref_zidane.json`。
//!
//! 模型与参考都放在 `models/_fixtures/`（**开发夹具**，不进分发包、也不被
//! 产品扫描）：产品的 `models/detect/yolo/` 出厂是空的，检测模型全由用户提供。
//!
//! 测试图用 **PNG**（`assets/bus.png` / `assets/zidane.png`，由官方 jpg 转存）：
//! JPEG 解码各家实现有 ±1 的 IDCT 抖动（cv2 vs image crate），会把 conf 推动约
//! 0.001~0.01，掩盖真实差异；PNG 无损，两边输入逐位一致。
//!
//! 容差依据：我们复刻了 cv2 的 11bit 定点 resize 与 Python 的银行家舍入，
//! 理论上 letterbox 后应当逐位一致；残余差异只可能来自后处理的浮点求和顺序，
//! 所以 IoU ≥ 0.99、conf 差 ≤ 0.01 是**很宽**的线——实测通常 0.999+ / 0.001-。
//!
//! 运行：
//!   cargo test --test yolo_parity -- --ignored --nocapture

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ocr_studio_lib::config::Device;
use ocr_studio_lib::engine::yolo::{YoloConfig, YoloEngine};
use ocr_studio_lib::engine::DetectOptions;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

fn ensure_dylib_path() {
    assert!(
        ocr_studio_lib::ocr::ensure_ort_dylib_path(&workspace_root()),
        "找不到 src-tauri/runtime/onnxruntime.dll，请先运行 scripts/copy_runtime.ps1"
    );
}

fn load_engine(device: Device) -> YoloEngine {
    let root = workspace_root();
    // 夹具根目录：目录结构与产品一致（detect/yolo/…），走真实的目录发现逻辑
    let fixture_root = root.join("models").join("_fixtures");
    let model = ocr_studio_lib::engine::catalog::default_yolo_model(&fixture_root, &[])
        .unwrap_or_else(|| {
            panic!(
                "没有找到检测模型夹具（{}）。这是开发夹具，不应为空；\
                 若被误删，用 scripts/fetch_yolo.ps1 重新导出后放到该目录",
                fixture_root.join("detect/yolo").display()
            )
        });
    YoloEngine::load(YoloConfig {
        path: model.path,
        id: Some(model.id),
        device,
    })
    .expect("加载 yolo 引擎失败")
}

/// 参考 JSON 的结构（scripts/ref_yolo.py 的输出）。
#[derive(serde::Deserialize)]
struct RefPayload {
    width: u32,
    height: u32,
    #[serde(default)]
    conf: f32,
    #[serde(default)]
    iou: f32,
    boxes: Vec<RefBox>,
}

#[derive(serde::Deserialize)]
struct RefBox {
    class_id: usize,
    #[allow(dead_code)]
    class_name: String,
    score: f64,
    xyxy: [f64; 4],
}

fn iou(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let ix1 = a[0].max(b[0]);
    let iy1 = a[1].max(b[1]);
    let ix2 = a[2].min(b[2]);
    let iy2 = a[3].min(b[3]);
    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
    let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn check_reference(file: &str, image: &str) {
    ensure_dylib_path();
    let root = workspace_root();
    let ref_path =
        ocr_studio_lib::engine::catalog::yolo_model_dir(&root.join("models").join("_fixtures"))
            .join(file);
    let payload: RefPayload = serde_json::from_str(
        &std::fs::read_to_string(&ref_path).expect("读参考 JSON 失败"),
    )
    .expect("参考 JSON 解析失败");

    let img_path = root.join("assets").join(image);
    let img = image::open(&img_path).expect("读图失败");
    assert_eq!(
        (img.width(), img.height()),
        (payload.width, payload.height),
        "参考与图片尺寸不一致"
    );

    let mut eng = load_engine(Device::Cpu);
    let opts = DetectOptions {
        conf_thresh: payload.conf,
        iou_thresh: payload.iou,
        class_filter: Vec::new(),
    };
    let dets = eng.detect_image(&img, &opts).expect("检测失败");

    eprintln!(
        "{file}: 参考 {} 框，Rust {} 框",
        payload.boxes.len(),
        dets.len()
    );
    assert_eq!(
        dets.len(),
        payload.boxes.len(),
        "框数不一致：NMS/置信度过滤与 ultralytics 不对齐"
    );

    for (i, (got, want)) in dets.iter().zip(payload.boxes.iter()).enumerate() {
        assert_eq!(got.class_id, want.class_id, "第 {i} 框类别不一致");
        let got_xyxy = [
            got.points[0].0,
            got.points[0].1,
            got.points[2].0,
            got.points[2].1,
        ];
        let v = iou(&got_xyxy, &want.xyxy);
        assert!(
            v >= 0.99,
            "第 {i} 框 IoU={v:.4} 低于 0.99\ngot  {got_xyxy:?}\nwant {:?}",
            want.xyxy
        );
        let dc = (got.score - want.score).abs();
        assert!(
            dc <= 0.01,
            "第 {i} 框置信度差 {dc:.4} 超过 0.01（got {} want {}）",
            got.score,
            want.score
        );
        eprintln!(
            "  [{i}] {:<10} score {:.4} (d{:.4})  IoU {:.4}",
            want.class_name,
            got.score,
            dc,
            v
        );
    }
}

/// bus.png：2 人 + 1 巴士 + 远处 2 人，多类别多目标。
#[test]
#[ignore = "重量级：需要 yolo 模型 + onnxruntime.dll + ultralytics 参考 JSON"]
fn matches_ultralytics_on_bus() {
    check_reference("ref_bus.json", "bus.png");
}

/// zidane.png：另一个人脸/姿态分布，验证不是只对一张图过拟合。
#[test]
#[ignore = "重量级：需要 yolo 模型 + onnxruntime.dll + ultralytics 参考 JSON"]
fn matches_ultralytics_on_zidane() {
    check_reference("ref_zidane.json", "zidane.png");
}
