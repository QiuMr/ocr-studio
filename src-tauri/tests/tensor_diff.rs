//! 像素级差异定位：把 Rust 的 rec 输入张量与 Python dump 出来的逐元素比对。
//!
//! 前置：先运行
//!   python scripts/dump_rec_tensor.py models/small assets/test_zh.png models/small/dump
//!
//! 运行：
//!   cargo test --test tensor_diff -- --ignored --nocapture

use std::path::{Path, PathBuf};

use image::GenericImageView;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有父目录")
        .to_path_buf()
}

#[test]
#[ignore = "需要 dump 目录与 onnxruntime DLL"]
fn rec_tensor_matches_python_dump() {
    let root = workspace_root();
    let dump = root.join("models").join("small").join("dump");
    let meta_path = dump.join("meta.json");
    assert!(meta_path.exists(), "缺少 {}", meta_path.display());

    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    let items = meta.as_array().unwrap();

    let mut worst = 0.0f64;
    let mut worst_idx = 0usize;

    for item in items {
        let i = item["index"].as_u64().unwrap() as usize;
        let crop_path = dump.join(format!("box_{i}_src.png"));
        let tensor_path = dump.join(format!("box_{i}_rec.f32"));
        if !crop_path.exists() || !tensor_path.exists() {
            println!("box {i}: 跳过（缺文件）");
            continue;
        }

        // 读 Python 的裁剪原图，走 Rust 的 rec 预处理
        let bytes = std::fs::read(&crop_path).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();

        let (rust_tensor, rw) =
            ocr_studio_lib::ocr::rec_preprocess(&img);

        // 读 Python 的张量
        let raw = std::fs::read(&tensor_path).unwrap();
        assert_eq!(raw.len() % 4, 0, "f32 长度不对");
        let py: Vec<f32> = raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let rust_flat: Vec<f32> = rust_tensor.iter().copied().collect();

        println!(
            "box {i}: crop={}x{} rust_len={} py_len={} valid_w rust={} py={}",
            img.width(),
            img.height(),
            rust_flat.len(),
            py.len(),
            rw,
            item["valid_width"].as_u64().unwrap()
        );

        if rust_flat.len() != py.len() {
            println!("  !! 长度不一致，跳过比对");
            continue;
        }

        let mut max_diff = 0.0f32;
        let mut max_at = 0usize;
        let mut n_diff = 0usize;
        let mut first_diff: Option<(usize, f32, f32)> = None;
        for (k, (a, b)) in rust_flat.iter().zip(py.iter()).enumerate() {
            let d = (a - b).abs();
            if d > max_diff {
                max_diff = d;
                max_at = k;
            }
            if d > 1e-6 {
                n_diff += 1;
                if first_diff.is_none() {
                    first_diff = Some((k, *a, *b));
                }
            }
        }
        let total = rust_flat.len();
        let pct = n_diff as f64 / total as f64 * 100.0;

        // 把扁平索引解回 (c, y, x)
        let hw = 48 * 320;
        let (mc, my, mx) = (
            max_at / hw,
            (max_at % hw) / 320,
            max_at % 320,
        );
        println!(
            "  不一致元素 {n_diff}/{total} ({pct:.2}%)  最大差 {max_diff:.4} @ (c={mc},y={my},x={mx})"
        );
        println!(
            "  首差 idx={:?}  Rust={:.4} Python={:.4}",
            first_diff.map(|(k, _, _)| k),
            first_diff.map(|(_, a, _)| a).unwrap_or(0.0),
            first_diff.map(|(_, _, b)| b).unwrap_or(0.0)
        );
        // 打印最大偏差处附近 5 个值，看是不是整行/整列错位
        let base = mc * hw + my * 320;
        let lo = base + mx.saturating_sub(2);
        let hi = (base + mx + 3).min(rust_flat.len());
        println!(
            "  该行附近 Rust: {:?}",
            rust_flat[lo..hi].iter().map(|v| (v * 127.5 + 127.5).round() as i32).collect::<Vec<_>>()
        );
        println!(
            "  该行附近 Py  : {:?}",
            py[lo..hi].iter().map(|v| (v * 127.5 + 127.5).round() as i32).collect::<Vec<_>>()
        );

        if max_diff as f64 > worst {
            worst = max_diff as f64;
            worst_idx = i;
        }
    }

    println!("\n最大偏差 {worst:.4}（来自 box {worst_idx}）");
    // 归一化后的值域约 [-1, 1]，1/255 ≈ 0.0039 是一个量化单位
    println!("一个 8bit 量化单位 = 1/255 ≈ 0.0039");
}

/// 把 Python dump 的 rec 张量直接喂给 Rust 的 rec 会话，打印逐帧 argmax 与置信度。
///
/// 用途：全流程置信度差 0.146 时，确认差异到底在「预处理」还是「推理+解码」。
/// 若这里的结果与 Python 用同一张量跑出来的不一致，则与预处理无关。
#[test]
#[ignore = "需要 dump 目录与 onnxruntime DLL"]
fn rec_probe_matches_python_on_same_tensor() {
    use ndarray::Array3;

    let root = workspace_root();
    let dump = root.join("models").join("small").join("dump");

    assert!(
        ocr_studio_lib::ocr::ensure_ort_dylib_path(&root),
        "找不到 src-tauri/runtime/onnxruntime.dll，请先运行 scripts/copy_runtime.ps1"
    );

    let cfg = ocr_studio_lib::ocr::EngineConfig {
        models_root: root.join("models"),
        level: ocr_studio_lib::ocr::ModelLevel::Small,
        device: ocr_studio_lib::ocr::Device::Directml,
        ..Default::default()
    };
    let mut engine = ocr_studio_lib::ocr::OcrEngine::load(cfg).expect("加载引擎");

    for i in 0..7usize {
        let path = dump.join(format!("box_{i}_rec.f32"));
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read(&path).unwrap();
        let flat: Vec<f32> = raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let t = Array3::from_shape_vec((3, 48, 320), flat).unwrap();
        let p = engine.rec_probe(&t).expect("rec 推理");

        // 逐帧 argmax 与最大概率
        let mut argmax = Vec::new();
        let mut maxp = Vec::new();
        for row in p.rows() {
            let mut best = 0usize;
            let mut bv = f32::NEG_INFINITY;
            for (j, v) in row.iter().enumerate() {
                if *v > bv {
                    bv = *v;
                    best = j;
                }
            }
            argmax.push(best);
            maxp.push(bv);
        }
        println!("box {i}: T={} C={}  [Python dump 张量]", p.nrows(), p.ncols());
        println!("  argmax: {:?}", argmax);
        println!(
            "  maxp  : {:?}",
            maxp.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>()
        );

        // 同一份 crop 走 Rust 的 rec 预处理（= 全流程实际用的张量）
        let crop_path = dump.join(format!("box_{i}_src.png"));
        if crop_path.exists() {
            let img = image::load_from_memory(&std::fs::read(&crop_path).unwrap()).unwrap();
            let (rt, _rw) = ocr_studio_lib::ocr::rec_preprocess(&img);
            // rec_preprocess 返回带 batch 的 [1,3,48,320]，去掉 batch 维
            let rt3 = rt.index_axis(ndarray::Axis(0), 0).to_owned();
            let p2 = engine.rec_probe(&rt3).expect("rec 推理");
            let am: Vec<usize> = p2
                .rows()
                .into_iter()
                .map(|row| {
                    row.iter()
                        .enumerate()
                        .fold((0usize, f32::NEG_INFINITY), |(bi, bv), (j, v)| {
                            if *v > bv { (j, *v) } else { (bi, bv) }
                        })
                        .0
                })
                .collect();
            let mp: Vec<f32> = p2
                .rows()
                .into_iter()
                .map(|row| row.iter().copied().fold(f32::NEG_INFINITY, f32::max))
                .collect();
            println!("box {i}: [Rust rec_preprocess 张量]");
            println!("  argmax: {:?}", am);
            println!(
                "  maxp  : {:?}",
                mp.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>()
            );
            let nb: Vec<(usize, f32)> = am
                .iter()
                .zip(mp.iter())
                .filter(|(a, _)| **a != 0)
                .map(|(a, p)| (*a, *p))
                .collect();
            println!("  非 blank 帧: {:?}", nb);
        }
    }
}

/// 比对 Rust 的 `warp_perspective` 与 Python 的 `crop_by_box`（即 dump 出的裁剪图）。
///
/// 这一步隔离「透视变换」与「缩放」：如果这里就有偏差，说明全流程里
/// Rust 的裁剪结果和 Python 不同，后续再怎么对齐缩放都没用。
#[test]
#[ignore = "需要 dump 目录"]
fn warp_perspective_matches_python_crop() {
    use ocr_studio_lib::ocr::geometry::warp_perspective;

    let root = workspace_root();
    let dump = root.join("models").join("small").join("dump");

    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dump.join("meta.json")).unwrap()).unwrap();

    // 原始测试图
    let img_bytes = std::fs::read(root.join("assets").join("test_zh.png")).unwrap();
    let img = image::load_from_memory(&img_bytes).unwrap();

    for item in meta.as_array().unwrap() {
        let i = item["index"].as_u64().unwrap() as usize;
        let py_png = dump.join(format!("box_{i}_src.png"));
        if !py_png.exists() {
            continue;
        }
        let box_json = &item["box"];
        let pts: [(f64, f64); 4] = std::array::from_fn(|k| {
            (
                box_json[k][0].as_f64().unwrap(),
                box_json[k][1].as_f64().unwrap(),
            )
        });

        let rust_crop = match warp_perspective(&img, pts) {
            Some(c) => c,
            None => {
                println!("box {i}: warp_perspective 返回 None");
                continue;
            }
        };

        let py_crop = image::load_from_memory(&std::fs::read(&py_png).unwrap()).unwrap();
        println!(
            "box {i}: Rust {}x{}  Python {}x{}",
            rust_crop.width(),
            rust_crop.height(),
            py_crop.width(),
            py_crop.height()
        );

        if rust_crop.dimensions() != py_crop.dimensions() {
            println!("  !! 尺寸不一致");
            continue;
        }

        let r = rust_crop.to_rgb8();
        let p = py_crop.to_rgb8();
        let mut max_d = 0i32;
        let mut n_diff = 0usize;
        for (a, b) in r.pixels().zip(p.pixels()) {
            for ch in 0..3 {
                let d = (a.0[ch] as i32 - b.0[ch] as i32).abs();
                max_d = max_d.max(d);
                if d > 0 {
                    n_diff += 1;
                }
            }
        }
        println!(
            "  像素差: 最大 {max_d}  不一致 {n_diff}/{} ({:.2}%)",
            r.pixels().len() * 3,
            100.0 * n_diff as f64 / (r.pixels().len() * 3) as f64
        );
    }
}
