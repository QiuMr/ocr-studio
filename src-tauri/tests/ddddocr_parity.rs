//! ddddocr 引擎的跨语言对齐测试。
//!
//! 为什么需要它：ddddocr 的预处理与 PP-OCR **完全不同**（灰度、高度 64、LANCZOS、
//! 只除 255 不归一化），解码约定也不同（**blank 就在字典第 0 位**，输出是 logits）。
//! 这些差异一旦写错，表现是「识别出来的字整体错位一位」或「分数离谱但不是 0」，
//! 光看文本像不像根本发现不了。所以这里直接对官方 Python 实现：
//!
//! 期望值由 `scripts/ref_ddddocr.py` 生成（脚本逐行复刻 ddddocr 1.6.1
//! `core/ocr_engine.py` 的 `_preprocess_image` + `_ctc_decode_indices`），
//! 输入是 `models/small/dump/box_N_src.png` —— PP-OCR 检测框透视矫正后的行图。
//!
//! 运行：
//!   cargo test --test ddddocr_parity -- --ignored --nocapture

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ocr_studio_lib::config::Device;
use ocr_studio_lib::engine::ddddocr::{DdddConfig, DdddOcrEngine};

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

fn read_image(path: &Path) -> DynamicImage {
    let bytes = std::fs::read(path).expect("读图失败");
    image::load_from_memory(&bytes).expect("解码失败")
}

fn load_engine(device: Device) -> DdddOcrEngine {
    let root = workspace_root();
    let dir = root.join("models").join("ocr").join("ddddocr").join("common");
    if !dir.join("rec.onnx").exists() {
        panic!(
            "缺少 ddddocr 模型：{}\n（从 pip sdist 取 common.onnx 重命名为 rec.onnx，\
             并把 CHARSET_BETA 写成 charset.txt）",
            dir.display()
        );
    }
    DdddOcrEngine::load(DdddConfig {
        dir,
        det_dir: root.join("models").join("small"),
        device,
    })
    .expect("加载 ddddocr 引擎失败")
}

/// 单行识别与官方 Python 实现逐字一致。
#[test]
#[ignore = "重量级：需要 ddddocr 模型 + onnxruntime.dll"]
fn rec_matches_python_reference_on_dumped_crops() {
    ensure_dylib_path();
    let mut eng = load_engine(Device::Cpu);
    assert_eq!(eng.charset_len(), 8210, "common 版字典应为 8210 项（含 blank）");

    // 期望值：scripts/ref_ddddocr.py 对同一批裁剪图的输出
    let cases: [(usize, &str); 7] = [
        (0, "OCRStudio测试样本第一行"),
        (1, "混合Engish123与中文混排"),
        (2, "订单编号SO202610218848"),
        (3, "手机号3800138000耶箱abCOm"),
        (4, "小五号子出集合集测试行内谷"),
        (5, "怀点付号"),
        (6, "ll"),
    ];

    let root = workspace_root();
    for (i, want) in cases {
        let p = root
            .join("models")
            .join("small")
            .join("dump")
            .join(format!("box_{i}_src.png"));
        let img = read_image(&p);
        let (text, score, spans, frames) = eng.infer_line(&img).expect("推理失败");
        assert_eq!(
            text, want,
            "box_{i} 的识别结果与 Python 参考不一致（检查预处理顺序/字典 blank 位置）"
        );
        // 每个字必须有帧区间，且落在总帧数内
        assert_eq!(spans.len(), want.chars().count(), "逐字数量应等于字符数");
        assert!(
            spans.iter().all(|s| s.t_end < frames),
            "帧区间越界：frames={frames}"
        );
        assert!(score > 0.5, "box_{i} 置信度异常低: {score}");
    }
}

/// 整图（det + rec）能跑通，且结果落在图像范围内。
#[test]
#[ignore = "重量级：需要 ddddocr 模型 + PP-OCR det + onnxruntime.dll"]
fn full_image_pipeline_with_shared_det() {
    ensure_dylib_path();
    let mut eng = load_engine(Device::Cpu);
    let root = workspace_root();
    let img = read_image(&root.join("assets").join("test_zh.png"));
    let (w, h) = (img.width() as f64, img.height() as f64);

    let lines = ocr_studio_lib::engine::VisionEngine::infer(
        &mut eng,
        &img,
        &ocr_studio_lib::engine::RecognizeOptions { want_chars: true },
    )
    .expect("整图识别失败");

    assert!(!lines.is_empty(), "应当至少识别出一行");
    let mut joined = String::new();
    for l in &lines {
        assert!(!l.text.is_empty(), "不应出现空行文本");
        for p in &l.points {
            assert!(
                p.0 >= -2.0 && p.0 <= w + 2.0 && p.1 >= -2.0 && p.1 <= h + 2.0,
                "框点超出图像范围: {:?}",
                p
            );
        }
        // 逐字框数应等于文本字数
        assert_eq!(l.chars.len(), l.text.chars().count());
        joined.push_str(&l.text);
        joined.push('\n');
    }
    eprintln!("ddddocr 整图结果：\n{joined}");

    // 数字串是这张图上最稳的部分（不依赖中文分词），用它确认链路真的通了
    assert!(
        joined.contains("8848") || joined.contains("13800"),
        "数字串都没认出来，说明 det/rec 链路有问题：\n{joined}"
    );
}
