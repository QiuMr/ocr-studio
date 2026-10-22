//! 用户自己放的检测模型能不能真的跑起来——冒烟测试。
//!
//! 与 `yolo_parity.rs`（对固定夹具做逐框对齐）不同，这个测试针对
//! **`models/detect/yolo/` 下用户放进来的任意模型**：只要目录里有 onnx，
//! 就试着加载并检测 `assets/bus.png`，确认链路通（不比对具体框，因为
//! 用户模型是什么、认什么类都不知道）。
//!
//! 目录为空时**自动跳过**（不算失败）——出厂就是这个状态。
//!
//! 运行：
//!   cargo test --test yoloe_smoke -- --ignored --nocapture

use std::path::{Path, PathBuf};

use ocr_studio_lib::config::Device;
use ocr_studio_lib::engine::catalog;
use ocr_studio_lib::engine::yolo::{YoloConfig, YoloEngine};
use ocr_studio_lib::engine::{DetectOptions, VisionEngine};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

#[test]
#[ignore = "重量级：需要 models/detect/yolo/ 下有用户模型 + onnxruntime.dll"]
fn user_models_in_detect_dir_load_and_detect() {
    let root = workspace_root();
    assert!(
        ocr_studio_lib::ocr::ensure_ort_dylib_path(&root),
        "找不到 src-tauri/runtime/onnxruntime.dll"
    );

    let models_root = root.join("models");
    let models = catalog::list_yolo_models(&models_root, &[]);
    if models.is_empty() {
        eprintln!("models/detect/yolo/ 里没有模型，跳过（出厂状态就是这个）");
        return;
    }

    let img = image::open(root.join("assets/bus.png")).expect("读 bus.png 失败");
    for m in &models {
        let mut eng = YoloEngine::load(YoloConfig {
            path: m.path.clone(),
            id: Some(m.id.clone()),
            device: Device::Cpu,
        })
        .unwrap_or_else(|e| panic!("加载 {} 失败: {e}", m.path.display()));

        let layout = eng.layout();
        eprintln!(
            "== {} ==\n   {}\n   类别 {} 个",
            m.id,
            layout.describe(),
            eng.capabilities().classes.len()
        );
        let opts = DetectOptions {
            conf_thresh: 0.25,
            iou_thresh: 0.7,
            class_filter: Vec::new(),
        };
        let dets = eng.detect_image(&img, &opts).expect("检测失败");
        eprintln!("   检出 {} 个目标", dets.len());
        for d in dets.iter().take(5) {
            eprintln!(
                "     {:<16} {:.3}  ({:.0},{:.0})-({:.0},{:.0})",
                d.class_name, d.score, d.points[0].0, d.points[0].1, d.points[2].0, d.points[2].1
            );
        }
        assert!(
            !dets.is_empty(),
            "{} 在 bus.png 上一个目标都没检出——要么模型不认识这张图，要么解码错了",
            m.id
        );
    }
}
