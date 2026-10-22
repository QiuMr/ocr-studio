//! 运行时提示：YOLOE 开放词汇的「图找图」与「语言找图」。
//!
//! 与 `yolo.rs` 里的普通检测模型不同，这里的目标是**换提示不用换模型**：
//! 提示向量 `pe` 是检测器的一张额外图输入，换一张就换一个要找的目标。
//!
//! 三件套（都在 `models/yoloe/`，由 `scripts/export_yoloe_prompt.py` 产出）
//! ---------------------------------------------------------------------------
//! - `*_prompt.onnx` 检测器：`images[1,3,640,640]` + `pe[1,80,512]` → `[1,300,6+nm]`
//! - `*_vpe.onnx`    视觉提示编码器：`ref[1,3,640,640]` + `vp[1,Q,80,80]` → `vpe[1,Q,512]`
//! - `*_texthead.onnx` 文本提示头：`feat[N,512]` → `pe[1,N,512]`
//!
//! 文本塔（MobileCLIP2-B）**不在这里**：它的 TorchScript 图缺张量秩信息，
//! 导不出 ONNX（详见导出脚本的说明），所以改用纯 Rust 的 `mobileclip2-b-rs`，
//! 在 Rust 侧直接算 `feat`，再喂给上面的 `texthead.onnx`。
//!
//! 为什么 `pe` 固定 80 槽
//! ----------------------
//! 提示头导出的 `nc` 是静态的（80 = yoloe-26 原生类别数）。补零槽位在 head 内部
//! 会走 `F.normalize(0) = 0`，分数恒等于 bias(-10) → `sigmoid ≈ 4.5e-5`，
//! 天然低于任何置信度阈值。好处是**输出形状与烘焙版完全一致**（`[1,300,38]`），
//! `yolo.rs` 的解码器一行都不用改。

use std::path::{Path, PathBuf};

use image::{DynamicImage, GenericImageView};
use ndarray::{Array2, Array3, Array4};

use super::yolo::{decode, letterbox, postprocess_end2end, Candidate, OutputLayout, YoloLayout};
use super::{DetectOptions, Detection, VisionError};
use crate::config::Device;

/// 提示槽位数。与导出时的 `nc` 一致（yoloe-26 原生 80 类）。
pub const NC_SLOTS: usize = 80;
/// 提示向量维度（MobileCLIP2-B 的嵌入维）。
pub const EMBED: usize = 512;
/// 检测器/视觉编码器的输入边长。
pub const PROMPT_IMGSZ: u32 = 640;
/// 提示掩码相对输入边长的缩放，对应 ultralytics `LoadVisualPrompt(scale_factor=1/8)`。
const MASK_SCALE: f64 = 1.0 / 8.0;
/// letterbox 填充值，与 `yolo.rs` 统一。
const PAD_VALUE: u8 = 114;

// ---------------------------------------------------------------------------
// 资产发现
// ---------------------------------------------------------------------------

/// 一套运行时提示所需的文件。
#[derive(Debug, Clone)]
pub struct PromptAssets {
    pub detector: PathBuf,
    /// 视觉提示编码器。缺了就只能用语言找图。
    pub vpe: Option<PathBuf>,
    /// 文本提示头。缺了就只能用图找图。
    pub text_head: Option<PathBuf>,
    /// 文本塔权重（`.safetensors` 优先，其次 ultralytics 的 `.ts`）。
    pub text_weights: Option<PathBuf>,
    /// CLIP BPE 词表。
    pub tokenizer: Option<PathBuf>,
}

/// 在模型根目录下找运行时提示三件套。
///
/// 搜索顺序：`models/yoloe/`（正式位置）→ `models/_fixtures/weights/`（开发夹具，
/// 省一次 250MB 的重复下载）。找到检测器才算「这套功能可用」；两个编码器各自可缺，
/// 缺哪个就少哪种提示方式（`/engine/list` 会把这个差异写进 note）。
pub fn discover_assets(models_root: &Path) -> Option<PromptAssets> {
    let dirs = [
        models_root.join("yoloe"),
        models_root.join("_fixtures").join("weights"),
    ];
    let find = |cands: &[&str]| -> Option<PathBuf> {
        for d in &dirs {
            for c in cands {
                let p = d.join(c);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    };

    let detector = find(&[
        "yoloe26n_prompt.onnx",
        "yoloe26s_prompt.onnx",
        "yoloe26m_prompt.onnx",
    ])?;
    Some(PromptAssets {
        detector,
        vpe: find(&["yoloe26n_vpe.onnx", "yoloe26s_vpe.onnx", "yoloe26m_vpe.onnx"]),
        text_head: find(&[
            "yoloe26n_texthead.onnx",
            "yoloe26s_texthead.onnx",
            "yoloe26m_texthead.onnx",
        ]),
        text_weights: find(&["mobileclip2_b.safetensors", "mobileclip2_b.ts"]),
        tokenizer: find(&["tokenizer.min.json", "tokenizer.json"]),
    })
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------

/// 文本塔：MobileCLIP2-B（纯 Rust 实现）+ CLIP BPE 分词器。
struct TextTower {
    model: mobileclip2_b_rs::Model,
    tokenizer: mobileclip2_b_rs::Tokenizer,
}

/// 运行时提示引擎。一个实例同时服务「图找图」和「语言找图」——两者最终都只是
/// 往同一个检测器里喂一张 `pe`。
pub struct PromptEngine {
    det: ort::session::Session,
    det_images: String,
    det_pe: String,
    layout: YoloLayout,
    vpe: Option<ort::session::Session>,
    head: Option<ort::session::Session>,
    text: Option<TextTower>,
    actual_device: Device,
    detector_path: PathBuf,
    /// 加载时的备注（缺哪个编码器等），给状态页看
    notes: Vec<String>,
}

impl PromptEngine {
    /// 加载整套。检测器是必需的，两个编码器尽力而为。
    pub fn load(models_root: &Path, device: Device) -> Result<Self, VisionError> {
        let assets = discover_assets(models_root).ok_or_else(|| {
            VisionError::NotReady(format!(
                "没有找到运行时提示模型。请先跑 \
                 `python scripts/export_yoloe_prompt.py all`，\
                 产物会落到 {}/yoloe/",
                models_root.display()
            ))
        })?;

        let env = crate::ocr::ort_util::ort_env()
            .map_err(|e| VisionError::NotReady(format!("ORT 环境初始化失败: {e}")))?;

        // 检测器有两个输入，`ort_util::warmup` 只认单输入，所以这里自己预热
        let mut det = crate::ocr::ort_util::build_session(env, &assets.detector, device)
            .map_err(|e| VisionError::NotReady(format!("提示检测器加载失败: {e}")))?;
        let names: Vec<String> = det.inputs().iter().map(|i| i.name().to_string()).collect();
        let det_images = names.first().cloned().unwrap_or_else(|| "images".into());
        let det_pe = names
            .iter()
            .find(|n| n.as_str() != det_images)
            .cloned()
            .ok_or_else(|| {
                VisionError::NotReady(format!(
                    "{} 只有 1 个输入（{}）。这像是**烘焙过提示**的普通检测模型；\
                     运行时提示需要 `scripts/export_yoloe_prompt.py` 导出的双输入版本",
                    assets.detector.display(),
                    det_images
                ))
            })?;
        if det_pe != "pe" {
            tracing::warn!(
                "{} 的第二个输入叫 `{det_pe}` 而不是 `pe`，按它喂提示向量",
                assets.detector.display()
            );
        }

        let mut notes = Vec::new();
        let size = PROMPT_IMGSZ;
        let dml_ok = device == Device::Directml && crate::ocr::ort_util::directml_available();
        let actual_device = if dml_ok {
            match warmup_two(&mut det, &det_images, &det_pe, size) {
                Ok(()) => Device::Directml,
                Err(e) => {
                    tracing::warn!("提示检测器在 DirectML 上预热失败，降级 CPU: {e}");
                    det = crate::ocr::ort_util::build_session(env, &assets.detector, Device::Cpu)
                        .map_err(|e| VisionError::NotReady(format!("提示检测器加载失败: {e}")))?;
                    warmup_two(&mut det, &det_images, &det_pe, size).map_err(|e| {
                        VisionError::NotReady(format!("提示检测器预热失败: {e}"))
                    })?;
                    notes.push("DirectML 预热失败，已降级 CPU".to_string());
                    Device::Cpu
                }
            }
        } else {
            warmup_two(&mut det, &det_images, &det_pe, size)
                .map_err(|e| VisionError::NotReady(format!("提示检测器预热失败: {e}")))?;
            Device::Cpu
        };

        // 输出布局：运行时提示模型是 end2end 导出，输出 `[1,300,6+nm]`。
        // nm（掩码系数通道）从第二个输出 `proto` 的第 1 维读，读不到就当纯检测。
        let out_dims = probe_dims(&det);
        let nm = probe_dim1(&det, 1).unwrap_or(0);
        let layout = super::yolo::infer_layout(&out_dims, NC_SLOTS, size, nm).map_err(|why| {
            VisionError::NotReady(format!("{} 不是可用的提示检测器：{why}", assets.detector.display()))
        })?;

        let vpe = match &assets.vpe {
            Some(p) => match crate::ocr::ort_util::build_session(env, p, actual_device) {
                Ok(s) => Some(s),
                Err(e) => {
                    notes.push(format!("视觉提示编码器加载失败：{e}"));
                    None
                }
            },
            None => {
                notes.push("缺少视觉提示编码器（*_vpe.onnx），图找图不可用".to_string());
                None
            }
        };

        let head = match &assets.text_head {
            Some(p) => match crate::ocr::ort_util::build_session(env, p, actual_device) {
                Ok(s) => Some(s),
                Err(e) => {
                    notes.push(format!("文本提示头加载失败：{e}"));
                    None
                }
            },
            None => {
                notes.push("缺少文本提示头（*_texthead.onnx），语言找图不可用".to_string());
                None
            }
        };

        // 文本塔：head 与权重/词表三者齐了才有意义
        let text = match (&head, &assets.text_weights, &assets.tokenizer) {
            (Some(_), Some(w), Some(t)) => {
                let cfg = mobileclip2_b_rs::Config::default();
                match mobileclip2_b_rs::Model::from_file(w) {
                    Ok(model) => match mobileclip2_b_rs::Tokenizer::from_file(t, &cfg) {
                        Ok(tokenizer) => Some(TextTower { model, tokenizer }),
                        Err(e) => {
                            notes.push(format!("CLIP 词表加载失败（{}）：{e}", t.display()));
                            None
                        }
                    },
                    Err(e) => {
                        notes.push(format!("MobileCLIP2-B 权重加载失败（{}）：{e}", w.display()));
                        None
                    }
                }
            }
            (Some(_), _, _) => {
                notes.push(format!(
                    "缺 MobileCLIP2-B 权重或 CLIP 词表（找 mobileclip2_b.safetensors + \
                     tokenizer.min.json，放在 {}），语言找图不可用",
                    assets
                        .detector
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ));
                None
            }
            _ => None,
        };

        tracing::info!(
            "运行时提示引擎已加载: {} · {} · 设备 {:?} · 图找图 {} · 语言找图 {}",
            assets.detector.display(),
            layout.describe(),
            actual_device,
            if vpe.is_some() { "可用" } else { "不可用" },
            if text.is_some() { "可用" } else { "不可用" },
        );

        Ok(Self {
            det,
            det_images,
            det_pe,
            layout,
            vpe,
            head,
            text,
            actual_device,
            detector_path: assets.detector,
            notes,
        })
    }

    pub fn actual_device(&self) -> Device {
        self.actual_device
    }

    pub fn detector_path(&self) -> &Path {
        &self.detector_path
    }

    pub fn has_visual(&self) -> bool {
        self.vpe.is_some()
    }

    pub fn has_text(&self) -> bool {
        self.text.is_some() && self.head.is_some()
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// 给 `/engine/list` 用的一句话状态。
    pub fn describe(&self) -> String {
        let mut s = format!(
            "{} · {} · 设备 {:?} · 图找图{} · 语言找图{}",
            self.detector_path.display(),
            self.layout.describe(),
            self.actual_device,
            if self.has_visual() { "可用" } else { "不可用" },
            if self.has_text() { "可用" } else { "不可用" },
        );
        if !self.notes.is_empty() {
            s.push_str(" · ");
            s.push_str(&self.notes.join("；"));
        }
        s
    }

    // -----------------------------------------------------------------------
    // 视觉提示
    // -----------------------------------------------------------------------

    /// 图找图：参考图 + 参考框（**原图坐标**）→ `pe` 与类别名。
    ///
    /// 复刻 ultralytics `YOLOEVPSegPredictor._process_single_image` +
    /// `LoadVisualPrompt`：先把框搬进 letterbox 后的 640 空间，再乘 1/8 栅格化成
    /// 80x80 掩码，按类别做并集；掩码喂视觉编码器得到 `vpe`。
    ///
    /// `classes` 与 `boxes` 等长，同一个类号的多框会合并成一个提示（Q 就是类数）。
    pub fn visual_pe(
        &mut self,
        ref_img: &DynamicImage,
        boxes: &[[f32; 4]],
        classes: &[usize],
        names: &[String],
    ) -> Result<(Array3<f32>, Vec<String>), VisionError> {
        let sess = self.vpe.as_mut().ok_or_else(|| {
            VisionError::Unsupported {
                engine: "yoloe-prompt".into(),
                capability: "visual（缺 *_vpe.onnx）",
            }
        })?;
        if boxes.is_empty() {
            return Err(VisionError::BadRequest(
                "图找图至少要给一个参考框 x1,y1,x2,y2".into(),
            ));
        }
        if classes.len() != boxes.len() {
            return Err(VisionError::BadRequest(format!(
                "classes 与 boxes 数量不一致（{} vs {}）",
                classes.len(),
                boxes.len()
            )));
        }

        let (w0, h0) = ref_img.dimensions();
        if w0 == 0 || h0 == 0 {
            return Err(VisionError::BadRequest("参考图尺寸为 0".into()));
        }
        let (ref_t, _lb) = letterbox(ref_img, PROMPT_IMGSZ, PAD_VALUE);

        // 框从原图坐标搬进 640 空间。**刻意不复用 letterbox 的 pad_x/pad_y**：
        // ultralytics 用的是未取整的 `src*gain`（`round((dst - src*gain)/2 - 0.1)`），
        // 而 LetterBox 里是取整后的 `new_w`，两者在边缘情况下会差 1 像素。
        let gain = (PROMPT_IMGSZ as f64 / w0 as f64).min(PROMPT_IMGSZ as f64 / h0 as f64);
        let ox = ((PROMPT_IMGSZ as f64 - w0 as f64 * gain) / 2.0 - 0.1).round_ties_even();
        let oy = ((PROMPT_IMGSZ as f64 - h0 as f64 * gain) / 2.0 - 0.1).round_ties_even();

        let mh = (PROMPT_IMGSZ as f64 * MASK_SCALE) as usize; // int() 截断，640/8=80
        let mw = mh;
        let q = classes.iter().copied().max().map_or(0, |m| m + 1);
        if q == 0 || q > NC_SLOTS {
            return Err(VisionError::BadRequest(format!(
                "参考框的类号范围 0..{q} 超出可用提示槽位（最多 {NC_SLOTS} 个）"
            )));
        }

        // 每个类一张掩码，同类多框取并集（对齐 get_visuals 的 `visuals[idx] |= mask`）
        let mut vp = Array4::<f32>::zeros((1, q, mh, mw));
        for (b, cls) in boxes.iter().zip(classes.iter()) {
            let x1 = (b[0] as f64 * gain + ox) * MASK_SCALE;
            let y1 = (b[1] as f64 * gain + oy) * MASK_SCALE;
            let x2 = (b[2] as f64 * gain + ox) * MASK_SCALE;
            let y2 = (b[3] as f64 * gain + oy) * MASK_SCALE;
            for c in 0..mh {
                // Python: `(r >= x1) * (r < x2) * (c >= y1) * (c < y2)`
                // 注意 r 遍历的是**宽**（与 x 比），c 遍历的是**高**（与 y 比）
                let y_ok = (c as f64) >= y1 && (c as f64) < y2;
                if !y_ok {
                    continue;
                }
                for r in 0..mw {
                    if (r as f64) >= x1 && (r as f64) < x2 {
                        vp[[0, *cls, c, r]] = 1.0;
                    }
                }
            }
        }

        let out = sess
            .run(ort::inputs![
                "ref" => ort::value::Tensor::from_array(ref_t)
                    .map_err(|e| VisionError::Inference(e.to_string()))?,
                "vp" => ort::value::Tensor::from_array(vp)
                    .map_err(|e| VisionError::Inference(e.to_string()))?,
            ])
            .map_err(|e| VisionError::Inference(format!("视觉提示编码失败: {e}")))?;
        let vpe = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("视觉提示输出读取失败: {e}")))?;

        let pe = pad_pe(&vpe)?;
        // 类别名：调用方给了就用，否则按类号回落到「参考目标 N」
        let labels = (0..q)
            .map(|i| {
                names
                    .get(i)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| {
                        if q == 1 {
                            "参考目标".to_string()
                        } else {
                            format!("参考目标{}", i + 1)
                        }
                    })
            })
            .collect();
        Ok((pe, labels))
    }

    // -----------------------------------------------------------------------
    // 文本提示
    // -----------------------------------------------------------------------

    /// 语言找图：词 → `pe` 与类别名（类名就是词本身）。
    ///
    /// 链路：CLIP BPE 分词 → MobileCLIP2-B 文本塔（纯 Rust）→ 提示头 `reprta` + 归一化。
    /// 与 ultralytics `YOLOEModel.get_text_pe` 同构。
    pub fn text_pe(&mut self, words: &[String]) -> Result<(Array3<f32>, Vec<String>), VisionError> {
        let tower = self.text.as_ref().ok_or_else(|| VisionError::Unsupported {
            engine: "yoloe-prompt".into(),
            capability: "text（缺 MobileCLIP2-B 权重 / 词表 / 提示头）",
        })?;
        let head = self.head.as_mut().ok_or_else(|| VisionError::Unsupported {
            engine: "yoloe-prompt".into(),
            capability: "text（缺 *_texthead.onnx）",
        })?;

        let words: Vec<String> = words
            .iter()
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            return Err(VisionError::BadRequest(
                "语言找图至少要给一个词，例如 \"person,bus\"".into(),
            ));
        }
        if words.len() > NC_SLOTS {
            return Err(VisionError::BadRequest(format!(
                "提示词最多 {NC_SLOTS} 个，收到 {}",
                words.len()
            )));
        }

        let refs: Vec<&str> = words.iter().map(|s| s.as_str()).collect();
        let ids = tower
            .tokenizer
            .encode_batch(&refs)
            .map_err(|e| VisionError::Inference(format!("分词失败: {e}")))?;
        let feat = tower
            .model
            .forward(&ids)
            .map_err(|e| VisionError::Inference(format!("MobileCLIP2-B 编码失败: {e}")))?;
        let feat: Vec<Vec<f32>> = feat
            .to_vec2::<f32>()
            .map_err(|e| VisionError::Inference(format!("文本特征读取失败: {e}")))?;
        let n = feat.len();
        let mut arr = Array2::<f32>::zeros((n, EMBED));
        for (i, row) in feat.iter().enumerate() {
            if row.len() != EMBED {
                return Err(VisionError::Inference(format!(
                    "MobileCLIP2-B 输出维度是 {}，期望 {EMBED}",
                    row.len()
                )));
            }
            for (j, v) in row.iter().enumerate() {
                arr[[i, j]] = *v;
            }
        }

        let out = head
            .run(ort::inputs![
                "feat" => ort::value::Tensor::from_array(arr)
                    .map_err(|e| VisionError::Inference(e.to_string()))?,
            ])
            .map_err(|e| VisionError::Inference(format!("文本提示编码失败: {e}")))?;
        let tpe = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("文本提示输出读取失败: {e}")))?;
        let pe = pad_pe(&tpe)?;
        Ok((pe, words))
    }

    // -----------------------------------------------------------------------
    // 检测
    // -----------------------------------------------------------------------

    /// 用给定的 `pe` 在目标图里找目标。`names` 是类别名，下标即 `class_id`。
    pub fn detect(
        &mut self,
        img: &DynamicImage,
        pe: &Array3<f32>,
        names: &[String],
        opts: &DetectOptions,
    ) -> Result<Vec<Detection>, VisionError> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        let size = self.layout.input_size;
        let (tensor, lb) = letterbox(img, size, PAD_VALUE);
        let layout = self.layout;

        let out = self
            .det
            .run(ort::inputs![
                self.det_images.as_str() => ort::value::Tensor::from_array(tensor)
                    .map_err(|e| VisionError::Inference(e.to_string()))?,
                self.det_pe.as_str() => ort::value::Tensor::from_array(pe.clone())
                    .map_err(|e| VisionError::Inference(e.to_string()))?,
            ])
            .map_err(|e| VisionError::Inference(format!("提示检测推理失败: {e}")))?;
        let p = out[0]
            .try_extract_array::<f32>()
            .map_err(|e| VisionError::Inference(format!("提示检测输出读取失败: {e}")))?;

        let cands = decode(&p, layout).ok_or_else(|| {
            VisionError::Inference(format!("提示检测输出维度变了: {:?}", p.shape()))
        })?;

        let kept: Vec<Candidate> = if layout.layout == OutputLayout::EndToEnd {
            // end2end 头已内置 NMS，只做阈值/类别过滤与上限
            postprocess_end2end(cands, opts)
        } else {
            super::yolo::nms(cands, opts)
        };

        // letterbox 坐标 → 原图坐标（同 yolo.rs 的换算）
        let sx = 1.0 / lb.scale;
        let mut dets = Vec::with_capacity(kept.len());
        for c in kept {
            let x1 = ((c.x1 as f64 - lb.pad_x) * sx).clamp(0.0, w as f64);
            let y1 = ((c.y1 as f64 - lb.pad_y) * sx).clamp(0.0, h as f64);
            let x2 = ((c.x2 as f64 - lb.pad_x) * sx).clamp(0.0, w as f64);
            let y2 = ((c.y2 as f64 - lb.pad_y) * sx).clamp(0.0, h as f64);
            let name = names
                .get(c.class_id)
                .cloned()
                .unwrap_or_else(|| format!("提示{}", c.class_id + 1));
            dets.push(Detection {
                points: [(x1, y1), (x2, y1), (x2, y2), (x1, y2)],
                class_id: c.class_id,
                class_name: name,
                score: c.score as f64,
            });
        }
        Ok(dets)
    }
}

// ---------------------------------------------------------------------------
// 提示来源
// ---------------------------------------------------------------------------

/// 一次「提示找图」的提示来源。
#[derive(Debug)]
pub enum PromptInput {
    /// 图找图：参考图 + 参考框（**原图坐标**）+ 每个框的类号 + 可选类名。
    ///
    /// `classes` 与 `boxes` 等长；同一个类号的多框会合并成一个提示。
    Visual {
        reference: DynamicImage,
        boxes: Vec<[f32; 4]>,
        classes: Vec<usize>,
        names: Vec<String>,
    },
    /// 语言找图：提示词，一个词一个提示槽。
    Text { words: Vec<String> },
}

impl PromptInput {
    /// 给日志/出参看的模式名。
    pub fn mode(&self) -> &'static str {
        match self {
            PromptInput::Visual { .. } => "visual",
            PromptInput::Text { .. } => "text",
        }
    }
}

impl PromptEngine {
    /// 一步到位：按提示来源算出 `pe`，再在目标图里找。
    pub fn run(
        &mut self,
        target: &DynamicImage,
        input: &PromptInput,
        opts: &DetectOptions,
    ) -> Result<Vec<Detection>, VisionError> {
        let (pe, names) = match input {
            PromptInput::Visual {
                reference,
                boxes,
                classes,
                names,
            } => self.visual_pe(reference, boxes, classes, names)?,
            PromptInput::Text { words } => self.text_pe(words)?,
        };
        self.detect(target, &pe, &names, opts)
    }
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

/// 把 `[1,Q,512]`（Q ≤ 80）补零成 `[1,80,512]`。
///
/// 补零槽在 head 内部走 `F.normalize(0)=0`，分数恒为 bias(-10)，不会冒出检测。
fn pad_pe(raw: &ndarray::ArrayViewD<f32>) -> Result<Array3<f32>, VisionError> {
    let s = raw.shape();
    if s.len() != 3 || s[0] != 1 {
        return Err(VisionError::Inference(format!(
            "提示向量形状 {s:?} 不是 `[1,Q,512]`"
        )));
    }
    let (q, d) = (s[1], s[2]);
    if d != EMBED {
        return Err(VisionError::Inference(format!(
            "提示向量维度是 {d}，期望 {EMBED}"
        )));
    }
    if q > NC_SLOTS {
        return Err(VisionError::BadRequest(format!(
            "提示有 {q} 个，超过可用槽位 {NC_SLOTS}"
        )));
    }
    let flat = raw
        .as_slice()
        .ok_or_else(|| VisionError::Inference("提示向量不是连续内存".into()))?;
    let mut pe = Array3::<f32>::zeros((1, NC_SLOTS, EMBED));
    for i in 0..q {
        for j in 0..EMBED {
            pe[[0, i, j]] = flat[i * EMBED + j];
        }
    }
    Ok(pe)
}

/// 双输入模型的预热：图像喂全 0，提示喂全 0。
///
/// 为什么必须预热：`ort_util` 里记过的那条 —— `SessionBuilder::with_execution_providers`
/// 对单个 EP 注册失败只打日志然后跳过，整体仍返回 `Ok`。只有真跑一次才能确认
/// 这份 DLL 真的用了 DirectML（否则会「显示 GPU、实际 CPU」）。
fn warmup_two(
    sess: &mut ort::session::Session,
    images: &str,
    pe: &str,
    size: u32,
) -> Result<(), String> {
    let x = Array4::<f32>::zeros((1, 3, size as usize, size as usize));
    let p = Array3::<f32>::zeros((1, NC_SLOTS, EMBED));
    sess.run(ort::inputs![
        images => ort::value::Tensor::from_array(x).map_err(|e| e.to_string())?,
        pe => ort::value::Tensor::from_array(p).map_err(|e| e.to_string())?,
    ])
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn probe_dims(sess: &ort::session::Session) -> Vec<i64> {
    sess.outputs()
        .first()
        .map(|o| match o.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => shape.iter().copied().collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

fn probe_dim1(sess: &ort::session::Session, idx: usize) -> Option<usize> {
    let o = sess.outputs().get(idx)?;
    match o.dtype() {
        ort::value::ValueType::Tensor { shape, .. } => shape.get(1).copied().filter(|v| *v > 0),
        _ => None,
    }
    .map(|v| v as usize)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    /// 掩码栅格化必须复刻 Python 的 `(r>=x1)*(r<x2)*(c>=y1)*(c<y2)`：
    /// 半开区间 [x1, x2)，且 r 比 x、c 比 y。
    #[test]
    fn mask_rasterisation_is_half_open() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(640, 640));
        let eng = DummyVpe;
        let boxes = [[64.0, 64.0, 128.0, 128.0]]; // 640 空间
        let m = eng.rasterize(&img, &boxes, &[0]);
        // 1/8 后 = [8,8,16,16)
        assert_eq!(m[[0, 0, 8, 8]], 1.0, "左上角应置位");
        assert_eq!(m[[0, 0, 15, 15]], 1.0, "右下角内一格应置位（半开区间）");
        assert_eq!(m[[0, 0, 16, 16]], 0.0, "x2/y2 本身不含");
        assert_eq!(m[[0, 0, 7, 7]], 0.0, "x1/y1 之前不含");
        let total: f32 = m.iter().sum();
        assert_eq!(total, 8.0 * 8.0, "8x8 = 64 个像素");
    }

    /// 同类多框取并集，不同类各占一个提示槽。
    #[test]
    fn mask_groups_by_class() {
        let img = DynamicImage::ImageRgb8(RgbImage::new(640, 640));
        let eng = DummyVpe;
        let m = eng.rasterize(
            &img,
            &[[0.0, 0.0, 80.0, 80.0], [160.0, 0.0, 240.0, 80.0], [320.0, 0.0, 400.0, 80.0]],
            &[0, 0, 1],
        );
        assert_eq!(m.shape(), &[1, 2, 80, 80], "两个类 → Q=2");
        assert_eq!(sum_class(&m, 0), 200.0, "类 0 = 两个框并集 10x10x2");
        assert_eq!(sum_class(&m, 1), 100.0, "类 1 = 一个框 10x10");
    }

    /// 补零：Q < 80 时后面必须全 0，且值原样搬过去。
    #[test]
    fn pad_pe_zero_fills_tail() {
        let mut raw = Array3::<f32>::zeros((1, 2, EMBED));
        raw[[0, 0, 0]] = 1.5;
        raw[[0, 1, EMBED - 1]] = -2.5;
        let v = raw.view().into_dyn();
        let pe = pad_pe(&v).expect("补零应成功");
        assert_eq!(pe.shape(), &[1, NC_SLOTS, EMBED]);
        assert_eq!(pe[[0, 0, 0]], 1.5);
        assert_eq!(pe[[0, 1, EMBED - 1]], -2.5);
        assert_eq!(sum_tail(&pe, 2), 0.0, "第 2 槽往后必须全 0");
    }

    #[test]
    fn pad_pe_rejects_oversized() {
        let raw = Array3::<f32>::zeros((1, NC_SLOTS + 1, EMBED));
        assert!(pad_pe(&raw.view().into_dyn()).is_err());
    }

    /// 某个提示槽里置位的像素数。
    fn sum_class(m: &Array4<f32>, cls: usize) -> f32 {
        let (h, w) = (m.shape()[2], m.shape()[3]);
        let mut s = 0.0;
        for c in 0..h {
            for r in 0..w {
                s += m[[0, cls, c, r]];
            }
        }
        s
    }

    fn sum_tail(pe: &Array3<f32>, from: usize) -> f32 {
        let (n, d) = (pe.shape()[1], pe.shape()[2]);
        let mut s = 0.0;
        for i in from..n {
            for j in 0..d {
                s += pe[[0, i, j]];
            }
        }
        s
    }

    /// 只用来复用栅格化逻辑的测试替身。
    struct DummyVpe;    impl DummyVpe {
        fn rasterize(
            &self,
            img: &DynamicImage,
            boxes: &[[f32; 4]],
            classes: &[usize],
        ) -> Array4<f32> {
            let (w0, h0) = img.dimensions();
            let gain = (PROMPT_IMGSZ as f64 / w0 as f64).min(PROMPT_IMGSZ as f64 / h0 as f64);
            let ox = ((PROMPT_IMGSZ as f64 - w0 as f64 * gain) / 2.0 - 0.1).round_ties_even();
            let oy = ((PROMPT_IMGSZ as f64 - h0 as f64 * gain) / 2.0 - 0.1).round_ties_even();
            let mh = (PROMPT_IMGSZ as f64 * MASK_SCALE) as usize;
            let mw = mh;
            let q = classes.iter().copied().max().map_or(0, |m| m + 1);
            let mut vp = Array4::<f32>::zeros((1, q, mh, mw));
            for (b, cls) in boxes.iter().zip(classes.iter()) {
                let x1 = (b[0] as f64 * gain + ox) * MASK_SCALE;
                let y1 = (b[1] as f64 * gain + oy) * MASK_SCALE;
                let x2 = (b[2] as f64 * gain + ox) * MASK_SCALE;
                let y2 = (b[3] as f64 * gain + oy) * MASK_SCALE;
                for c in 0..mh {
                    if !((c as f64) >= y1 && (c as f64) < y2) {
                        continue;
                    }
                    for r in 0..mw {
                        if (r as f64) >= x1 && (r as f64) < x2 {
                            vp[[0, *cls, c, r]] = 1.0;
                        }
                    }
                }
            }
            vp
        }
    }
}
