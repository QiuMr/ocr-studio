//! 运行时提示（YOLOE 图找图）与 Python 参考链路的对齐测试。
//!
//! 参考值由 `scripts/ref_yoloe_prompt.py` 生成：用 ultralytics 的
//! `LoadVisualPrompt` + `savpe` 算视觉 `pe`。参考文件：
//! `models/yoloe/reference_prompt.json`。
//!
//! 为什么导出脚本的验收不够
//! ------------------------
//! `scripts/export_yoloe_prompt.py` 只证明了 **ONNX 与 torch 一致**（1e-7）。但运行时
//! 链路上还有一段是 Rust 独有的，导出脚本碰不到：
//!
//! **提示组装在 Rust 里**：`pad_pe` 补零、掩码栅格化、框从原图搬进 letterbox 空间。
//! 任何一处差一点，`pe` 就变，而 `pe` 变了对用户就是「同样的框找不到东西」，
//! **不会有任何报错**。所以这里比的是**中间量**（`pe`），不只比最终框——只看框的话，
//! 一个错的 `pe` 也可能碰巧检出相似的东西，把问题盖住。
//!
//! 曾经的「文本提示」也有一条同样的对齐用例，2026-10-23 随功能一起移除
//! （实测在界面控件上分数只有 0.1~0.2，等于噪声）。参考 JSON 里仍留有
//! `text` / `end2end` 字段，供历史追溯，不再被读取。
//!
//! 容差依据：ONNX 与 torch 差 1e-7 量级，所以 `pe` 的差异应当只在 1e-6 上下。这里
//! 放宽到 1e-4 是为了给 f32 求和顺序留余量——**错的实现差的是 1e-1 量级**。
//!
//! 运行：
//!   cargo test --test yoloe_prompt_parity -- --ignored --nocapture

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ocr_studio_lib::config::Device;
use ocr_studio_lib::engine::prompt::PromptEngine;

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

// --------------------------------------------------------------------------- 结构

#[derive(serde::Deserialize)]
struct RefPayload {
    nc_slots: usize,
    embed: usize,
    visual: RefVisual,
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

// --------------------------------------------------------------------------- 测试

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
