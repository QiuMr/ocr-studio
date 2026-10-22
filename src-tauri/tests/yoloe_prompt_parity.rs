//! 运行时提示（YOLOE 图找图 / 语言找图）与 Python 参考链路的对齐测试。
//!
//! 参考值由 `scripts/ref_yoloe_prompt.py` 生成：用官方 `mobileclip2_b.ts` 文本塔 +
//! ultralytics 的 `reprta` 提示头算文本 `pe`，用 `LoadVisualPrompt` + `savpe` 算视觉
//! `pe`，再用同一份 `pe` 跑出端到端检测框。参考文件：
//! `models/yoloe/reference_prompt.json`。
//!
//! 为什么导出脚本的验收不够
//! ------------------------
//! `scripts/export_yoloe_prompt.py` 只证明了 **ONNX 与 torch 一致**（1e-7）。但运行时
//! 链路上有两段是 Rust 独有的，导出脚本碰不到：
//!
//! 1. **文本塔换了实现**：Python 是 `mobileclip2_b.ts`（TorchScript），Rust 是第三方
//!    crate `mobileclip2-b-rs`（纯 Rust + HF 分词器）。它算得对不对只能拿官方输出比。
//! 2. **提示组装在 Rust 里**：分词、`pad_pe` 补零、掩码栅格化、框从原图搬进 letterbox
//!    空间。任何一处差一点，`pe` 就变，而 `pe` 变了对用户就是「同样的词/框找不到东西」，
//!    **不会有任何报错**。所以这里比的是**中间量**（`pe`），不只比最终框——只看框的话，
//!    一个错的 `pe` 也可能碰巧检出相似的东西，把问题盖住。
//!
//! 容差依据：分词器已逐 id 核对过（`scripts/_probe_tokenizer.py`，10/10 一致），
//! ONNX 与 torch 差 1e-7 量级，所以 `pe` 的差异应当只在 1e-6 上下。这里放宽到 1e-4
//! 是为了给 f32 求和顺序留余量——**错的实现差的是 1e-1 量级**，1e-4 依然能抓住。
//!
//! 运行：
//!   cargo test --test yoloe_prompt_parity -- --ignored --nocapture

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ocr_studio_lib::config::Device;
use ocr_studio_lib::engine::prompt::{PromptEngine, PromptInput};
use ocr_studio_lib::engine::DetectOptions;

/// `pe` / `vpe` 的最大逐值差上限。
const PE_TOL: f32 = 1e-4;

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

fn reference() -> RefPayload {
    let p = workspace_root()
        .join("models")
        .join("yoloe")
        .join("reference_prompt.json");
    let txt = std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "读不到参考基准 {}：{e}\n请先运行 python scripts/ref_yoloe_prompt.py",
            p.display()
        )
    });
    serde_json::from_str(&txt).expect("参考基准 JSON 解析失败")
}

/// 加载引擎。三件套任一缺失都会在这里报错，错误信息里带着缺什么。
fn load_engine() -> PromptEngine {
    ensure_dylib_path();
    let models = workspace_root().join("models");
    PromptEngine::load(&models, Device::Cpu).unwrap_or_else(|e| {
        panic!("提示引擎加载失败：{e}\n（需要 models/yoloe/ 下三件套 + _fixtures/weights/mobileclip2_b.ts）")
    })
}

fn image(name: &str) -> DynamicImage {
    let p = workspace_root().join("assets").join(name);
    image::open(&p).unwrap_or_else(|e| panic!("读图失败 {}：{e}", p.display()))
}

/// 逐值比对并返回最大差。`got` 是 `[1,80,512]`，只比前 `rows` 行（其余是补零槽）。
fn max_diff(got: &[Vec<f32>], want: &[Vec<f32>], rows: usize) -> f32 {
    let mut worst = 0.0f32;
    for i in 0..rows {
        assert_eq!(got[i].len(), want[i].len(), "第 {i} 行长度不一致");
        for (a, b) in got[i].iter().zip(want[i].iter()) {
            worst = worst.max((a - b).abs());
        }
    }
    worst
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

// --------------------------------------------------------------------------- 结构

#[derive(serde::Deserialize)]
struct RefPayload {
    nc_slots: usize,
    embed: usize,
    text: RefText,
    visual: RefVisual,
    end2end: RefEnd2End,
}

#[derive(serde::Deserialize)]
struct RefText {
    words: Vec<String>,
    #[allow(dead_code)]
    feat: Vec<Vec<f32>>,
    pe: Vec<Vec<f32>>,
}

#[derive(serde::Deserialize)]
struct RefVisual {
    image: String,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
    boxes: Vec<[f32; 4]>,
    classes: Vec<usize>,
    names: Vec<String>,
    q: usize,
    /// 掩码里置位的像素数——掩码栅格化错了它就会变（是 pe 之外的第二道保险）
    mask_set_pixels: usize,
    vpe: Vec<Vec<f32>>,
}

#[derive(serde::Deserialize)]
struct RefEnd2End {
    image: String,
    words: Vec<String>,
    conf: f32,
    detections: Vec<RefDet>,
}

#[derive(serde::Deserialize)]
struct RefDet {
    class_id: usize,
    class_name: String,
    score: f64,
    xyxy: [f64; 4],
}

// --------------------------------------------------------------------------- 测试

/// 语言找图：Rust 的分词 + MobileCLIP2-B + 提示头算出的 `pe` 必须与官方一致。
///
/// 这是整个功能里**唯一**换了实现的环节，也是最容易悄悄错的一环。
#[test]
#[ignore = "重量级：需要 models/yoloe 三件套 + mobileclip2_b.ts + onnxruntime.dll"]
fn text_pe_matches_official_tower() {
    let r = reference();
    let mut eng = load_engine();
    assert!(eng.has_text(), "语言找图不可用：{}", eng.describe());

    let (pe, names) = eng.text_pe(&r.text.words).expect("算文本提示失败");
    assert_eq!(names, r.text.words, "回显的类名应当就是提示词本身");
    assert_eq!(pe.shape(), [1, r.nc_slots, r.embed], "pe 形状不对");

    let rows: Vec<Vec<f32>> = (0..r.text.words.len())
        .map(|i| pe.slice(ndarray::s![0, i, ..]).to_vec())
        .collect();
    let d = max_diff(&rows, &r.text.pe, r.text.words.len());
    eprintln!(
        "文本 pe：{} 词 × {} 维，与官方 TorchScript 文本塔最大差 {d:.3e}",
        r.text.words.len(),
        r.embed
    );
    assert!(
        d <= PE_TOL,
        "文本 pe 最大差 {d:.3e} 超过 {PE_TOL:.0e}：分词或文本塔与官方不一致"
    );

    // 补零槽必须是干净的 0：head 里 F.normalize(0)=0 → 分数恒为 bias(-10)，
    // 这是「80 槽里只用前 Q 槽」这个设计能成立的前提。
    let tail = pe.slice(ndarray::s![0, r.text.words.len().., ..]);
    let tmax = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(tmax == 0.0, "补零槽不是 0（最大绝对值 {tmax:.3e}）");
    eprintln!("补零槽 {} 行全为 0 ✅", r.nc_slots - r.text.words.len());
}

/// 图找图：参考图 + 框 → 掩码 → 视觉编码器 → `pe`，必须与 ultralytics 一致。
///
/// 这条覆盖了掩码栅格化（1/8 缩放、r 比 x / c 比 y 的轴序、同类并集）和框的
/// letterbox 搬运（`round((dst - src*gain)/2 - 0.1)` 的银行家舍入）。
#[test]
#[ignore = "重量级：需要 models/yoloe 三件套 + onnxruntime.dll"]
fn visual_pe_matches_ultralytics_mask() {
    let r = reference();
    let mut eng = load_engine();
    assert!(eng.has_visual(), "图找图不可用：{}", eng.describe());

    let (pe, names) = eng
        .visual_pe(
            &image(&r.visual.image),
            &r.visual.boxes,
            &r.visual.classes,
            &r.visual.names,
        )
        .expect("算视觉提示失败");

    assert_eq!(names, r.visual.names, "类名回显不对");
    assert_eq!(pe.shape(), [1, r.nc_slots, r.embed], "pe 形状不对");

    let rows: Vec<Vec<f32>> = (0..r.visual.q)
        .map(|i| pe.slice(ndarray::s![0, i, ..]).to_vec())
        .collect();
    let d = max_diff(&rows, &r.visual.vpe, r.visual.q);
    eprintln!(
        "视觉 pe：{} 槽（掩码 {} 个像素置位）最大差 {d:.3e}",
        r.visual.q, r.visual.mask_set_pixels
    );
    assert!(
        d <= PE_TOL,
        "视觉 pe 最大差 {d:.3e} 超过 {PE_TOL:.0e}：掩码栅格化或 letterbox 搬运与 ultralytics 不一致"
    );
}

/// 端到端：语言提示 → `pe` → 检测器 → 框。整条链路跑通且框对得上。
///
/// 比的是「置信度过滤 + IoU 匹配」而不是逐值：端到端头是 top-300，尾部全是近并列
/// 的低分框，排序在 torch 与 onnxruntime 之间不稳定（项目记忆里那条「误检框置信度
/// 是混沌的」）。有语义的框（conf 明显高于阈值）才是有意义的比对对象。
#[test]
#[ignore = "重量级：需要 models/yoloe 三件套 + mobileclip2_b.ts + onnxruntime.dll"]
fn end2end_text_prompt_finds_the_same_boxes() {
    let r = reference();
    let mut eng = load_engine();
    let target = image(&r.end2end.image);

    let dets = eng
        .run(
            &target,
            &PromptInput::Text {
                words: r.end2end.words.clone(),
            },
            &DetectOptions {
                conf_thresh: r.end2end.conf,
                iou_thresh: 0.7,
                class_filter: Vec::new(),
            },
        )
        .expect("端到端提示找图失败");

    eprintln!(
        "端到端：参考 {} 框（conf>{}），Rust {} 框",
        r.end2end.detections.len(),
        r.end2end.conf,
        dets.len()
    );
    assert_eq!(
        dets.len(),
        r.end2end.detections.len(),
        "框数不一致：置信度过滤或解码与 ultralytics 不对齐"
    );

    // 先把所有对比打出来再断言：只报第一条失败的话，看不出是「整体偏了」还是
    // 「只有个别框错位」，而这两种情况指向的原因完全不同。
    let mut worst_iou = f64::MAX;
    let mut worst_conf = 0.0f64;
    for (i, (got, want)) in dets.iter().zip(r.end2end.detections.iter()).enumerate() {
        let got_xyxy = [
            got.points[0].0,
            got.points[0].1,
            got.points[2].0,
            got.points[2].1,
        ];
        let v = iou(&got_xyxy, &want.xyxy);
        let dc = (got.score - want.score).abs();
        worst_iou = worst_iou.min(v);
        worst_conf = worst_conf.max(dc);
        eprintln!(
            "  [{i}] {:<10} cls {}/{}  score {:.4} (d{:.4})  IoU {v:.4}\n      got  {got_xyxy:?}\n      want {:?}",
            want.class_name, got.class_id, want.class_id, got.score, dc, want.xyxy
        );
    }
    assert!(worst_iou >= 0.9, "最小 IoU {worst_iou:.4} 低于 0.9");
    assert!(
        worst_conf <= 0.02,
        "最大置信度差 {worst_conf:.4} 超过 0.02"
    );
    for (i, (got, want)) in dets.iter().zip(r.end2end.detections.iter()).enumerate() {
        assert_eq!(got.class_id, want.class_id, "第 {i} 框类别不一致");
        assert_eq!(
            got.class_name, want.class_name,
            "第 {i} 框类名不一致（类名就是提示词）"
        );
    }
}
