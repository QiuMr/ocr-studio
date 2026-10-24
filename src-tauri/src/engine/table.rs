//! 表格：结构识别（SLANet-plus）+ 页面表格定位（PicoDet）。
//!
//! 两个模型是配套的，所以放一个文件
//! -------------------------------------
//! - `detect.onnx`（约 7MB）：在**整页**里找出表格区域。PicoDet 架构，
//!   只有 1 类（`table`），输出 4 个尺度的 score + box。
//! - `structure.onnx`（约 7MB）：把**一张表格图**变成 HTML + 每个单元格的框。
//!   SLANet-plus，字符表直接嵌在 ONNX metadata 的 `character` 里，不用额外字典。
//!
//! 典型用法是两段串起来：整页图 → detect 找到几个表格 → 每个裁出来喂 structure。
//! 也可以跳过检测，直接拿一张裁好的表格图喂 structure。
//!
//! 两条必须照抄的前处理细节（错一个就是满屏乱码）
//! ------------------------------------------------
//! 1. **pad 要填 0，不是 127.5**。PaddleOCR 的 `PaddingTableImage` 是 zeros
//!    padding，而且是在 Normalize **之后**先 CHW 再 pad。paddlex 新写的那套
//!    `Pad(val=127.5)` 套到这个模型上会输出 500 个 `colspan` 垃圾 token。
//! 2. **bbox 两个轴都乘 `max(原图宽, 原图高)`**。因为图被等比缩到长边 488 再
//!    pad 成 488x488 的方形，模型输出的归一化坐标是相对**那个方形**的，
//!    所以 x 和 y 用的是同一个比例。按「x 乘图宽、y 乘图高」会得到压扁的框。
//!
//! 还有一条：PaddleOCR 用 **BGR** 通道序配 **RGB 顺序**的 mean/std（历史错配），
//! 自己换实现时很容易顺手「修正」成 RGB，结果精度掉一截。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, RgbImage};
use ndarray::{Array3, Array4};
use ort::session::Session;

use super::VisionError;
use crate::config::Device;
use crate::ocr::ort_util;

/// 结构识别的长边（等比缩放的目标）。
const STRUCT_LEN: u32 = 488;
/// 结构识别输入的形状（pad 之后的方形）。
const STRUCT_SHAPE: [usize; 4] = [1, 3, STRUCT_LEN as usize, STRUCT_LEN as usize];

/// 页面表格定位的输入尺寸。模型是静态形状，写死。
const DET_H: u32 = 800;
const DET_W: u32 = 608;
const DET_SHAPE: [usize; 4] = [1, 3, DET_H as usize, DET_W as usize];
/// PicoDet 的四个下采样步长，顺序与输出层一致（由大到小）。
const DET_STRIDES: [usize; 4] = [8, 16, 32, 64];
/// DFL 的分箱数（reg_max + 1）。
const REG_BINS: usize = 8;
/// letterbox 填充值，与 PaddleDetection 的 Resize 一致。
const DET_PAD: u8 = 114;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// 模型目录：`models/table/`。
pub fn table_dir(models_root: &Path) -> PathBuf {
    models_root.join("table")
}

pub fn structure_path(models_root: &Path) -> PathBuf {
    table_dir(models_root).join("structure.onnx")
}

pub fn detect_path(models_root: &Path) -> PathBuf {
    table_dir(models_root).join("detect.onnx")
}

/// 结构识别的模型是否就位。
pub fn structure_ready(models_root: &Path) -> bool {
    structure_path(models_root).is_file()
}

/// 页面表格定位的模型是否就位。
pub fn detect_ready(models_root: &Path) -> bool {
    detect_path(models_root).is_file()
}

/// 缺模型时统一的指路文案。
///
/// 分发包里没有源码，任何「去跑 scripts/xxx.py」的提示对终端用户都等于没说。
fn missing_msg() -> String {
    "表格模型还没安装。打开「设置 → 模型库」，下载「表格识别」即可（约 15MB）".to_string()
}

// ---------------------------------------------------------------------------
// 页面级：检测 + 逐张识别
// ---------------------------------------------------------------------------

/// 识别分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 先检测页面里的表格再逐张识别；一张都没检测到就退回整图
    Auto,
    /// 整图当一张表格
    Structure,
    /// 只检测位置
    Detect,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Mode::Auto),
            "structure" | "table" => Some(Mode::Structure),
            "detect" | "find" => Some(Mode::Detect),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Structure => "structure",
            Mode::Detect => "detect",
        }
    }
}

/// 页面上识别到的一处表格。
pub struct PageHit {
    /// 在整页里的位置
    pub area: TableBox,
    /// 结构识别结果。`Mode::Detect` 或该处识别失败时为 None
    pub result: Option<TableResult>,
}

/// 页面级识别：按 `mode` 走检测 / 整图 / 只检测。
///
/// 返回 `(是否回退到整图, 各处的识别结果)`。
///
/// 为什么要抽成函数：HTTP 的 `/ocr/biaoge` 和桌面端的 `ocr_biaoge` 命令
/// 必须给出**完全相同**的结果（界面试跑和脚本联调看到的不是一回事，最难查）。
pub fn recognize_page(
    models_root: &Path,
    img: &DynamicImage,
    device: Device,
    mode: Mode,
    conf: f32,
) -> Result<(bool, Vec<PageHit>), VisionError> {
    let (w, h) = img.dimensions();
    let whole = |r: TableResult| {
        vec![PageHit {
            area: TableBox {
                x: 0,
                y: 0,
                width: w,
                height: h,
                score: 1.0,
            },
            result: Some(r),
        }]
    };

    match mode {
        Mode::Detect => {
            let boxes = detect_tables(models_root, img, device, conf)?;
            Ok((
                false,
                boxes
                    .into_iter()
                    .map(|b| PageHit {
                        area: b,
                        result: None,
                    })
                    .collect(),
            ))
        }
        Mode::Structure => {
            let r = recognize(models_root, img, device)?;
            Ok((false, whole(r)))
        }
        Mode::Auto => {
            let boxes = detect_tables(models_root, img, device, conf)?;
            if boxes.is_empty() {
                // 退回整图：用户拍的往往就是一张裁好的表格，检测模型在
                // 「整张图就是一个表格」时确实可能给空。直接报「没找到表格」是最差的
                let r = recognize(models_root, img, device)?;
                return Ok((true, whole(r)));
            }
            let mut hits = Vec::with_capacity(boxes.len());
            for b in boxes {
                let sub = img.crop_imm(b.x, b.y, b.width.max(1), b.height.max(1));
                let result = match recognize(models_root, &sub, device) {
                    Ok(mut r) => {
                        // 识别结果是裁剪图局部坐标，平移回整页
                        r.translate(b.x, b.y);
                        Some(r)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "表格 {}x{}@({},{}) 结构识别失败: {e}",
                            b.width,
                            b.height,
                            b.x,
                            b.y
                        );
                        None
                    }
                };
                hits.push(PageHit { area: b, result });
            }
            Ok((false, hits))
        }
    }
}

// ---------------------------------------------------------------------------
// 会话缓存
// ---------------------------------------------------------------------------

/// 一个缓存下来的模型会话。
///
/// 为什么要缓存：7MB 的模型从磁盘建会话约 60~120ms，比推理本身还慢。
/// 表格识别是「一两秒一次」的调用频率，不缓存就是每次都在重复付这个成本。
struct Cached {
    sess: Mutex<Session>,
    device: Device,
    /// SLANet 的字符表（`sos` + metadata 里的 token + `eos`）。检测模型为空。
    tokens: Vec<String>,
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Cached>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Cached>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key_of(path: &Path, device: Device) -> String {
    format!("{}|{device:?}", path.display())
}

/// 取（或建）会话。同一模型 + 同一设备只建一次。
fn session_for(path: &Path, device: Device, warmup: [usize; 4]) -> Result<Arc<Cached>, VisionError> {
    let key = key_of(path, device);
    if let Some(c) = cache().lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(c);
    }

    let env = ort_util::ort_env().map_err(|e| VisionError::Inference(e.to_string()))?;
    let (sess, actual) = ort_util::build_session_with_fallback(env, path, device, warmup)
        .map_err(|e| VisionError::NotReady(format!("表格模型加载失败（{}）：{e}", path.display())))?;

    let tokens = read_tokens(&sess);
    if !tokens.is_empty() {
        tracing::info!(
            "表格结构模型已加载，字符表 {} 项，实际设备 {actual:?}",
            tokens.len()
        );
    }
    let c = Arc::new(Cached {
        sess: Mutex::new(sess),
        device: actual,
        tokens,
    });
    if let Ok(mut m) = cache().lock() {
        m.insert(key, c.clone());
    }
    Ok(c)
}

/// 从 ONNX metadata 的 `character` 读字符表，前后补 `sos` / `eos`。
///
/// 空表表示这不是结构模型（或者是没写 metadata 的模型），调用方据此报错。
/// 索引约定与 PaddleOCR 一致：0 = `sos`（忽略），末位 = `eos`（中断）。
fn read_tokens(sess: &Session) -> Vec<String> {
    let raw = sess
        .metadata()
        .ok()
        .and_then(|m| m.custom("character"))
        .unwrap_or_default();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut v = Vec::with_capacity(64);
    v.push("sos".to_string());
    v.extend(raw.split('\n').map(str::to_string));
    v.push("eos".to_string());
    v
}

// ---------------------------------------------------------------------------
// 出参
// ---------------------------------------------------------------------------

/// 一个单元格。
#[derive(Debug, Clone, Default)]
pub struct Cell {
    /// 行号（0 起），已考虑 rowspan 占位
    pub row: usize,
    /// 列号（0 起），已考虑 colspan 占位
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
    /// 原图像素坐标
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 格内文字（`with_text` 时填，多行用空格连）
    pub text: String,
}

/// 结构识别的完整结果。
#[derive(Debug, Clone, Default)]
pub struct TableResult {
    /// 规范化的 `<table>` HTML（带 colspan/rowspan）
    pub html: String,
    pub cells: Vec<Cell>,
    /// 实际网格的列数（已把 colspan 展开算）
    pub cols: usize,
    /// 是不是没识别完。模型输出的位置数有限（一张图能表达的 token 数固定），
    /// 单元格太多的表格会被截断 —— 这时语义上是「有表但没全」，
    /// 必须显式告诉调用方，否则拿着半张表当真数据用。
    pub truncated: bool,
    /// 结构 token 的平均置信度
    pub score: f32,
    pub width: u32,
    pub height: u32,
    /// 推理实际用的设备（GPU 降级 CPU 时能看出来）
    pub device: Device,
}

/// 页面里找到的一个表格框。
#[derive(Debug, Clone, Copy)]
pub struct TableBox {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub score: f32,
}

impl TableResult {
    /// 单元格坐标整体平移，并重建 HTML。
    ///
    /// 「整页检测 → 把表格裁出来识别」时，识别结果里的坐标是**裁剪图局部**的，
    /// 平移回原图后调用方才能直接画框。HTML 里没坐标，但一起重建更省心
    /// （改完 cells 忘了重建 HTML 是最容易犯的漏）。
    pub fn translate(&mut self, dx: u32, dy: u32) {
        if dx == 0 && dy == 0 {
            return;
        }
        for c in self.cells.iter_mut() {
            c.x = c.x.saturating_add(dx);
            c.y = c.y.saturating_add(dy);
        }
        self.html = render_html(&self.cells);
    }

    /// 重新生成 HTML。改了 `cells` 里的文字之后必须调一次，
    /// 否则 HTML 和 cells 会对不上（最容易犯的漏）。
    pub fn refresh(&mut self) {
        self.html = render_html(&self.cells);
    }

    /// 纯文本版：列间 `\t`、行间 `\n`，可以直接粘进 Excel。
    ///
    /// 跨列的格子不补空 tab：补了反而会在粘贴时多插一列，
    /// 想按网格取数据请用 `cells` 的 row/col。
    pub fn to_tsv(&self) -> String {
        if self.cells.is_empty() {
            return String::new();
        }
        let rows = self.cells.iter().map(|c| c.row + c.rowspan).max().unwrap_or(0);
        let mut lines = Vec::with_capacity(rows);
        for r in 0..rows {
            let mut row: Vec<&Cell> = self.cells.iter().filter(|c| c.row == r).collect();
            row.sort_by_key(|c| c.col);
            lines.push(
                row.iter()
                    .map(|c| c.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\t"),
            );
        }
        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// 结构识别
// ---------------------------------------------------------------------------

/// 把一张表格图识别成 HTML + 单元格。
pub fn recognize(
    models_root: &Path,
    img: &DynamicImage,
    device: Device,
) -> Result<TableResult, VisionError> {
    let path = structure_path(models_root);
    if !path.is_file() {
        return Err(VisionError::NotReady(missing_msg()));
    }
    let c = session_for(&path, device, STRUCT_SHAPE)?;
    if c.tokens.is_empty() {
        return Err(VisionError::NotReady(format!(
            "{} 里没有 character 元数据，不像是 SLANet 导出的结构识别模型",
            path.display()
        )));
    }

    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(VisionError::BadRequest("图像尺寸为 0".to_string()));
    }

    let tensor = struct_preprocess(img);
    // 见文件头第 2 条：两个轴用同一个比例
    let scale = w.max(h) as f32;

    let mut sess = c
        .sess
        .lock()
        .map_err(|_| VisionError::Inference("表格会话锁被污染".to_string()))?;
    let input = ort_util::input_name(&sess);
    let out = sess
        .run(ort::inputs![input.as_str() => ort::value::Tensor::from_array(tensor)
            .map_err(|e| VisionError::Inference(e.to_string()))?])
        .map_err(|e| VisionError::Inference(format!("表格结构推理失败：{e}")))?;

    let loc: Array3<f32> = out[0]
        .try_extract_array::<f32>()
        .map_err(|e| VisionError::Inference(e.to_string()))?
        .to_owned()
        .into_dimensionality()
        .map_err(|e| VisionError::Inference(format!("loc 输出维度异常：{e}")))?;
    let st: Array3<f32> = out[1]
        .try_extract_array::<f32>()
        .map_err(|e| VisionError::Inference(e.to_string()))?
        .to_owned()
        .into_dimensionality()
        .map_err(|e| VisionError::Inference(format!("struct 输出维度异常：{e}")))?;

    let mut res = decode_structure(&loc, &st, &c.tokens, scale);
    res.width = w;
    res.height = h;
    res.device = c.device;
    Ok(res)
}

/// 等比缩到长边 488，再 pad 到 488x488（先归一化，pad 填 0）。
fn struct_preprocess(img: &DynamicImage) -> Array4<f32> {
    let (w, h) = img.dimensions();
    let ratio = STRUCT_LEN as f32 / w.max(h) as f32;
    let rw = ((w as f32 * ratio).round() as u32).clamp(1, STRUCT_LEN);
    let rh = ((h as f32 * ratio).round() as u32).clamp(1, STRUCT_LEN);
    let small = img.resize_exact(rw, rh, FilterType::Triangle).to_rgb8();

    let mut t = Array4::<f32>::zeros(STRUCT_SHAPE);
    blit_bgr(&mut t, &small, rw as usize, rh as usize, 0, 0);
    t
}

/// 把 `rgb` 归一化后写进 `dst` 的 (off_y, off_x) 起点，BGR 通道序。
fn blit_bgr(dst: &mut Array4<f32>, rgb: &RgbImage, w: usize, h: usize, off_y: usize, off_x: usize) {
    for y in 0..h {
        for x in 0..w {
            let p = rgb.get_pixel(x as u32, y as u32);
            let bgr = [p[2], p[1], p[0]];
            for c in 0..3 {
                let v = bgr[c] as f32 / 255.0;
                dst[[0, c, off_y + y, off_x + x]] = (v - MEAN[c]) / STD[c];
            }
        }
    }
}

/// token 语义分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok<'a> {
    RowStart,
    RowEnd,
    /// 单元格开始（`<td` 或 `<td>`），后面可能跟属性
    CellOpen,
    /// 自闭合单元格（`<td></td>`），不跟属性
    CellEmpty,
    CellClose,
    Colspan(usize),
    Rowspan(usize),
    /// `>` —— `<td` 的收尾，本身没有语义
    Ignore,
    /// 其它标签，忽略（`<thead>` 之类不影响网格）
    Other(&'a str),
}

fn classify(t: &str) -> Tok<'_> {
    match t.trim_end() {
        "<tr>" => Tok::RowStart,
        "</tr>" => Tok::RowEnd,
        "<td" | "<td>" | "<th" => Tok::CellOpen,
        "<td></td>" => Tok::CellEmpty,
        "</td>" | "</th>" => Tok::CellClose,
        ">" => Tok::Ignore,
        s if s.starts_with("colspan=") => Tok::Colspan(attr_num(s).unwrap_or(1)),
        s if s.starts_with("rowspan=") => Tok::Rowspan(attr_num(s).unwrap_or(1)),
        _ => Tok::Other(t),
    }
}

/// 从 `colspan="3"` 里取 3。取不到就当 1（比当 0 安全，0 会让布局循环空转）。
fn attr_num(s: &str) -> Option<usize> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    s[start..end].trim().parse::<usize>().ok().filter(|&n| n >= 1)
}

/// 待布局的原始单元格。
struct RawCell {
    colspan: usize,
    rowspan: usize,
    bbox: [f32; 4],
}

/// 解码结构 + 位置。
///
/// 三条与 PaddleOCR 一致的规矩：
/// - 逐位置 argmax，**不去重**（这不是 CTC；去重会把 `>` `</td>` 吃掉）
/// - 第 0 位是 `sos` 要跳过，最后一个是 `eos` 要中断
/// - 只有单元格开头的 token 才吃掉一个 bbox
fn decode_structure(
    loc: &Array3<f32>,
    st: &Array3<f32>,
    tokens: &[String],
    scale: f32,
) -> TableResult {
    let seq = st.shape()[1];
    let classes = st.shape()[2];
    let end_idx = tokens.len().saturating_sub(1);

    let mut rows: Vec<Vec<RawCell>> = Vec::new();
    let mut cur: Option<RawCell> = None;
    let mut score_sum = 0f32;
    let mut score_n = 0usize;
    let mut last_tok = "";

    let flush = |cur: &mut Option<RawCell>, rows: &mut Vec<Vec<RawCell>>| {
        if let Some(c) = cur.take() {
            if rows.is_empty() {
                rows.push(Vec::new());
            }
            rows.last_mut().unwrap().push(c);
        }
    };

    for i in 0..seq {
        // argmax
        let mut best = 0usize;
        let mut best_v = f32::NEG_INFINITY;
        for c in 0..classes {
            let v = st[[0, i, c]];
            if v > best_v {
                best_v = v;
                best = c;
            }
        }
        if i > 0 && best == end_idx {
            break;
        }
        if best == 0 {
            continue; // sos
        }
        let Some(tok) = tokens.get(best).map(String::as_str) else {
            continue;
        };
        score_sum += best_v;
        score_n += 1;
        last_tok = tok;

        match classify(tok) {
            Tok::RowStart => {
                flush(&mut cur, &mut rows);
                rows.push(Vec::new());
            }
            Tok::CellOpen => {
                flush(&mut cur, &mut rows);
                cur = Some(RawCell {
                    colspan: 1,
                    rowspan: 1,
                    bbox: bbox_at(loc, i, scale),
                });
            }
            Tok::CellEmpty => {
                // 自闭合单元格：立即落到当前行，不占用「当前格」状态
                flush(&mut cur, &mut rows);
                if rows.is_empty() {
                    rows.push(Vec::new());
                }
                rows.last_mut().unwrap().push(RawCell {
                    colspan: 1,
                    rowspan: 1,
                    bbox: bbox_at(loc, i, scale),
                });
            }
            Tok::Colspan(n) => {
                if let Some(c) = cur.as_mut() {
                    c.colspan = n;
                }
            }
            Tok::Rowspan(n) => {
                if let Some(c) = cur.as_mut() {
                    c.rowspan = n;
                }
            }
            Tok::CellClose | Tok::RowEnd => flush(&mut cur, &mut rows),
            Tok::Ignore | Tok::Other(_) => {}
        }
    }
    flush(&mut cur, &mut rows);
    // 空行去掉（模型偶尔会给连续 </tr>）
    rows.retain(|r| !r.is_empty());

    // 网格布局：按 colspan/rowspan 占位，算出每格真实行列
    let mut cells: Vec<Cell> = Vec::new();
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    let mut max_col = 0usize;

    for (r, row) in rows.iter().enumerate() {
        while occupied.len() <= r {
            occupied.push(Vec::new());
        }
        let mut c = 0usize;
        for raw in row {
            while occupied[r].get(c).copied().unwrap_or(false) {
                c += 1;
            }
            for dr in 0..raw.rowspan {
                let rr = r + dr;
                while occupied.len() <= rr {
                    occupied.push(Vec::new());
                }
                for dc in 0..raw.colspan {
                    let cc = c + dc;
                    if occupied[rr].len() <= cc {
                        occupied[rr].resize(cc + 1, false);
                    }
                    occupied[rr][cc] = true;
                }
            }
            cells.push(Cell {
                row: r,
                col: c,
                rowspan: raw.rowspan,
                colspan: raw.colspan,
                x: raw.bbox[0].max(0.0) as u32,
                y: raw.bbox[1].max(0.0) as u32,
                width: raw.bbox[2].max(0.0) as u32,
                height: raw.bbox[3].max(0.0) as u32,
                text: String::new(),
            });
            c += raw.colspan;
            max_col = max_col.max(c);
        }
    }
    let cols = max_col.max(occupied.iter().map(Vec::len).max().unwrap_or(0));

    // 截断判定：序列用完还没收尾，说明单元格太多没表达完
    let lt = last_tok.trim_end();
    let truncated = !lt.is_empty() && lt != "</tr>" && lt != "</tbody>" && lt != "</table>";

    TableResult {
        html: render_html(&cells),
        cells,
        cols,
        truncated,
        score: if score_n > 0 {
            score_sum / score_n as f32
        } else {
            0.0
        },
        width: 0,
        height: 0,
        device: Device::Cpu,
    }
}

/// 取第 i 个位置的四点 bbox，换算成 `[x, y, w, h]` 原图像素。
fn bbox_at(loc: &Array3<f32>, i: usize, scale: f32) -> [f32; 4] {
    if i >= loc.shape()[1] {
        return [0.0; 4];
    }
    let mut x1 = f32::INFINITY;
    let mut x2 = f32::NEG_INFINITY;
    let mut y1 = f32::INFINITY;
    let mut y2 = f32::NEG_INFINITY;
    for k in 0..4 {
        let x = loc[[0, i, k * 2]] * scale;
        let y = loc[[0, i, k * 2 + 1]] * scale;
        x1 = x1.min(x);
        x2 = x2.max(x);
        y1 = y1.min(y);
        y2 = y2.max(y);
    }
    if !x1.is_finite() || !y1.is_finite() {
        return [0.0; 4];
    }
    [x1, y1, (x2 - x1).max(0.0), (y2 - y1).max(0.0)]
}

/// 生成规范 HTML。
///
/// 为什么不直接把 token 连起来：token 流里 `<td` 和 `>` 是两个 token，
/// 一旦模型漏掉 `>` 就会拼出 `<td</td>` 这种坏标签。自己拼能保证闭合正确，
/// 顺便把 colspan/rowspan 写在正确位置。
fn render_html(cells: &[Cell]) -> String {
    if cells.is_empty() {
        return String::new();
    }
    let rows = cells.iter().map(|c| c.row + c.rowspan).max().unwrap_or(0);
    let mut by_row: Vec<Vec<&Cell>> = vec![Vec::new(); rows];
    for c in cells {
        if c.row < rows {
            by_row[c.row].push(c);
        }
    }
    for r in by_row.iter_mut() {
        r.sort_by_key(|c| c.col);
    }

    let mut out = String::from("<table>");
    for r in by_row.iter() {
        out.push_str("<tr>");
        for c in r.iter() {
            out.push_str("<td");
            if c.colspan > 1 {
                out.push_str(&format!(" colspan=\"{}\"", c.colspan));
            }
            if c.rowspan > 1 {
                out.push_str(&format!(" rowspan=\"{}\"", c.rowspan));
            }
            out.push('>');
            out.push_str(&escape_html(&c.text));
            out.push_str("</td>");
        }
        out.push_str("</tr>");
    }
    out.push_str("</table>");
    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// 页面表格定位（PicoDet）
// ---------------------------------------------------------------------------

/// 在整页图里找表格。按面积从大到小返回。
pub fn detect_tables(
    models_root: &Path,
    img: &DynamicImage,
    device: Device,
    conf: f32,
) -> Result<Vec<TableBox>, VisionError> {
    let path = detect_path(models_root);
    if !path.is_file() {
        return Err(VisionError::NotReady(missing_msg()));
    }
    let c = session_for(&path, device, DET_SHAPE)?;
    let (w, h) = img.dimensions();
    let (tensor, scale) = det_preprocess(img);

    let mut sess = c
        .sess
        .lock()
        .map_err(|_| VisionError::Inference("表格会话锁被污染".to_string()))?;
    let input = ort_util::input_name(&sess);
    let out = sess
        .run(ort::inputs![input.as_str() => ort::value::Tensor::from_array(tensor)
            .map_err(|e| VisionError::Inference(e.to_string()))?])
        .map_err(|e| VisionError::Inference(format!("表格定位推理失败：{e}")))?;

    // 8 个输出是 4 个候选分（最后一维 1）+ 4 个 DFL 框（最后一维 32）交叉排列，
    // 按维度区分比按名字稳（名字是 transpose_0/2/4/6 这种导出产物）
    let mut scores: Vec<(usize, Array3<f32>)> = Vec::new();
    let mut boxs: Vec<(usize, Array3<f32>)> = Vec::new();
    for (_, v) in out.iter() {
        let Ok(a) = v.try_extract_array::<f32>() else {
            continue;
        };
        let Ok(a3) = a.to_owned().into_dimensionality::<ndarray::Ix3>() else {
            continue;
        };
        match a3.shape()[2] {
            1 => scores.push((a3.shape()[1], a3)),
            32 => boxs.push((a3.shape()[1], a3)),
            _ => {}
        }
    }
    if scores.len() != 4 || boxs.len() != 4 {
        return Err(VisionError::Inference(format!(
            "表格定位模型输出形状不认识：{} 个候选分 + {} 个框（期望各 4 个）",
            scores.len(),
            boxs.len()
        )));
    }
    scores.sort_by(|a, b| b.0.cmp(&a.0));
    boxs.sort_by(|a, b| b.0.cmp(&a.0));

    let mut cands: Vec<(f32, [f32; 4])> = Vec::new();
    for lv in 0..4 {
        let st = DET_STRIDES[lv] as f32;
        let gh = (DET_H as f32 / st).ceil() as usize;
        let gw = (DET_W as f32 / st).ceil() as usize;
        let sc = &scores[lv].1;
        let bx = &boxs[lv].1;
        if gh * gw != sc.shape()[1] {
            tracing::warn!(
                "表格定位 stride {st} 的网格 {gh}x{gw} 与候选数 {} 不符，跳过这一层",
                sc.shape()[1]
            );
            continue;
        }
        for i in 0..gh * gw {
            // 模型**已经输出概率**，不要再 sigmoid。再套一层会把背景的 0.0
            // 变成 0.50，于是几千个背景位置全部过阈值 —— 实测就是这样：
            // 一次 detect 返回 4093 个框，auto 模式挨个去识别，直接把进程跑满。
            let p = sc[[0, i, 0]];
            if p < conf {
                continue;
            }
            let (dl, dt, dr, db) = dfl(bx, i, st);
            let cx = ((i % gw) as f32 + 0.5) * st;
            let cy = ((i / gw) as f32 + 0.5) * st;
            let x1 = (cx - dl) / scale;
            let y1 = (cy - dt) / scale;
            let x2 = (cx + dr) / scale;
            let y2 = (cy + db) / scale;
            // 太小的框基本是格线噪声，别当成表格
            if x2 - x1 < 8.0 || y2 - y1 < 8.0 {
                continue;
            }
            cands.push((p, [x1, y1, x2, y2]));
        }
    }

    let mut out_boxes: Vec<TableBox> = nms(&cands, 0.6)
        .into_iter()
        .map(|i| {
            let (s, b) = cands[i];
            let x1 = b[0].max(0.0).min(w as f32);
            let y1 = b[1].max(0.0).min(h as f32);
            let x2 = b[2].max(0.0).min(w as f32);
            let y2 = b[3].max(0.0).min(h as f32);
            TableBox {
                x: x1 as u32,
                y: y1 as u32,
                width: (x2 - x1) as u32,
                height: (y2 - y1) as u32,
                score: s,
            }
        })
        .collect();
    out_boxes.sort_by(|a, b| (b.width * b.height).cmp(&(a.width * a.height)));
    // 一页文档里的表格不会超过几十个；上百个只可能是模型抽风。而 auto 模式要对
    // 每个框跑一次结构识别，兜住上限才不会被异常输出拖死（实测踩过：
    // 分数算错时返回 4093 个框，auto 直接把六核跑满 3500 CPU 秒）。
    if out_boxes.len() > 20 {
        tracing::warn!(
            "表格检测返回 {} 个框，只取分数最高的 20 个（阈值 {} 可能偏低）",
            out_boxes.len(),
            conf
        );
        out_boxes.sort_by(|a, b| b.score.total_cmp(&a.score));
        out_boxes.truncate(20);
    }
    Ok(out_boxes)
}

/// 一个位置的 4 个方向距离（已乘 stride，换算成输入图上的像素）。
fn dfl(bx: &Array3<f32>, i: usize, stride: f32) -> (f32, f32, f32, f32) {
    let mut d = [0f32; 4];
    for side in 0..4 {
        let base = side * REG_BINS;
        let mut mx = f32::NEG_INFINITY;
        for b in 0..REG_BINS {
            mx = mx.max(bx[[0, i, base + b]]);
        }
        let mut sum = 0f32;
        let mut acc = 0f32;
        for b in 0..REG_BINS {
            let e = (bx[[0, i, base + b]] - mx).exp();
            sum += e;
            acc += e * b as f32;
        }
        d[side] = if sum > 0.0 { acc / sum * stride } else { 0.0 };
    }
    (d[0], d[1], d[2], d[3])
}

/// letterbox 到 800x608，pad 114，BGR 归一化。返回 (张量, 缩放比)。
fn det_preprocess(img: &DynamicImage) -> (Array4<f32>, f32) {
    let (w, h) = img.dimensions();
    let scale = (DET_H as f32 / h as f32).min(DET_W as f32 / w as f32);
    let rw = ((w as f32 * scale).round() as u32).clamp(1, DET_W);
    let rh = ((h as f32 * scale).round() as u32).clamp(1, DET_H);
    let small = img.resize_exact(rw, rh, FilterType::Triangle).to_rgb8();

    // pad 填 114 的**归一化后**值：PaddleDetection 是先 pad 再归一化，
    // 等价于 (114/255 - mean) / std
    let mut t = Array4::<f32>::zeros(DET_SHAPE);
    for c in 0..3 {
        let v = (DET_PAD as f32 / 255.0 - MEAN[c]) / STD[c];
        t.slice_mut(ndarray::s![0, c, .., ..]).fill(v);
    }
    blit_bgr(&mut t, &small, rw as usize, rh as usize, 0, 0);
    (t, scale)
}

/// 单类别 NMS，返回保留下来的下标。
///
/// 先按分数截到前 `MAX_NMS` 个再两两比：候选动辄几千个，纯 O(n²) 会把一个
/// 请求拖到几十秒；截断不影响结果，分数低的本来也留不下来。
fn nms(cands: &[(f32, [f32; 4])], iou_thr: f32) -> Vec<usize> {
    const MAX_NMS: usize = 1000;
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_by(|&a, &b| cands[b].0.total_cmp(&cands[a].0));
    order.truncate(MAX_NMS);
    let mut keep = Vec::new();
    let mut dead = vec![false; cands.len()];
    for &i in &order {
        if dead[i] {
            continue;
        }
        keep.push(i);
        let bi = cands[i].1;
        for &j in &order {
            if j == i || dead[j] {
                continue;
            }
            let bj = cands[j].1;
            let x1 = bi[0].max(bj[0]);
            let y1 = bi[1].max(bj[1]);
            let x2 = bi[2].min(bj[2]);
            let y2 = bi[3].min(bj[3]);
            let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
            let ai = (bi[2] - bi[0]) * (bi[3] - bi[1]);
            let aj = (bj[2] - bj[0]) * (bj[3] - bj[1]);
            let union = ai + aj - inter;
            if union > 0.0 && inter / union > iou_thr {
                dead[j] = true;
            }
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_the_real_token_alphabet() {
        assert_eq!(classify("<tr>"), Tok::RowStart);
        assert_eq!(classify("</tr>"), Tok::RowEnd);
        assert_eq!(classify("<td"), Tok::CellOpen);
        assert_eq!(classify("<td></td>"), Tok::CellEmpty);
        assert_eq!(classify("</td>"), Tok::CellClose);
        assert_eq!(classify(">"), Tok::Ignore);
        assert_eq!(classify(" colspan=\"3\""), Tok::Colspan(3));
        assert_eq!(classify(" rowspan=\"2\""), Tok::Rowspan(2));
        // 数值坏掉时兜 1，不能兜 0 —— 0 会让占位循环空转
        assert_eq!(classify(" colspan=\"x\""), Tok::Colspan(1));
    }

    #[test]
    fn attr_num_reads_quoted_number() {
        assert_eq!(attr_num("colspan=\"20\""), Some(20));
        assert_eq!(attr_num("rowspan=\"2\""), Some(2));
        assert_eq!(attr_num("colspan="), None);
    }

    #[test]
    fn render_html_emits_span_attributes() {
        let cells = vec![
            Cell {
                row: 0,
                col: 0,
                colspan: 2,
                rowspan: 1,
                text: "合并".to_string(),
                ..Default::default()
            },
            Cell {
                row: 0,
                col: 2,
                colspan: 1,
                rowspan: 1,
                text: "普通".to_string(),
                ..Default::default()
            },
        ];
        assert_eq!(
            render_html(&cells),
            "<table><tr><td colspan=\"2\">合并</td><td>普通</td></tr></table>"
        );
    }

    #[test]
    fn render_html_escapes_content() {
        let cells = vec![Cell {
            text: "a<b>&c".to_string(),
            ..Default::default()
        }];
        let html = render_html(&cells);
        assert!(html.contains("a&lt;b&gt;&amp;c"), "{html}");
    }

    #[test]
    fn bbox_at_takes_outer_rect_of_four_points() {
        let mut loc = Array3::<f32>::zeros((1, 1, 8));
        loc[[0, 0, 0]] = 0.4; // x1
        loc[[0, 0, 1]] = 0.2; // y1
        loc[[0, 0, 2]] = 0.9; // x2
        loc[[0, 0, 3]] = 0.1; // y2
        loc[[0, 0, 4]] = 0.2; // x3
        loc[[0, 0, 5]] = 0.8; // y3
        loc[[0, 0, 6]] = 0.6; // x4
        loc[[0, 0, 7]] = 0.5; // y4
        let b = bbox_at(&loc, 0, 100.0);
        assert!((b[0] - 20.0).abs() < 0.01, "{b:?}");
        assert!((b[1] - 10.0).abs() < 0.01, "{b:?}");
        assert!((b[2] - 70.0).abs() < 0.01, "{b:?}");
        assert!((b[3] - 70.0).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn dfl_is_weighted_expectation() {
        // 每个方向都只往 bin 0 压 -> 距离 0
        let mut bx = Array3::<f32>::from_elem((1, 1, 32), -20.0);
        for side in 0..4 {
            bx[[0, 0, side * REG_BINS]] = 20.0;
        }
        let (l, t, r, b) = dfl(&bx, 0, 16.0);
        assert!(l < 0.01 && t < 0.01 && r < 0.01 && b < 0.01, "{l} {t} {r} {b}");

        // 只往 bin 3 压 -> 3 * stride
        let mut bx = Array3::<f32>::from_elem((1, 1, 32), -20.0);
        for side in 0..4 {
            bx[[0, 0, side * REG_BINS + 3]] = 20.0;
        }
        let (l, _, _, _) = dfl(&bx, 0, 16.0);
        assert!((l - 48.0).abs() < 0.1, "{l}");
    }

    #[test]
    fn nms_keeps_highest_and_drops_overlap() {
        let cands = vec![
            (0.9, [0.0, 0.0, 100.0, 100.0]),
            (0.8, [5.0, 5.0, 100.0, 100.0]),     // 与第一个几乎重合
            (0.7, [200.0, 200.0, 300.0, 300.0]), // 不重叠
        ];
        assert_eq!(nms(&cands, 0.6), vec![0, 2]);
    }

    #[test]
    fn decode_handles_a_two_by_two_table() {
        // 手工构造：字符表 [sos, <tr>, <td, >, </td>, </tr>, eos]
        let tokens: Vec<String> = ["sos", "<tr>", "<td", ">", "</td>", "</tr>", "eos"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let seq = 8;
        let mut st = Array3::<f32>::zeros((1, seq, tokens.len()));
        let idx = [1usize, 2, 3, 4, 2, 3, 4, 5]; // <tr> <td > </td> <td > </td> </tr>
        for (i, &t) in idx.iter().enumerate() {
            st[[0, i, t]] = 1.0;
        }
        // 第二个位置（<td）的框：整张图左上四分之一
        let mut loc = Array3::<f32>::zeros((1, seq, 8));
        for k in 0..4 {
            loc[[0, 1, k * 2]] = if k == 1 || k == 2 { 0.5 } else { 0.0 };
            loc[[0, 1, k * 2 + 1]] = if k >= 2 { 0.5 } else { 0.0 };
        }
        let r = decode_structure(&loc, &st, &tokens, 100.0);
        assert_eq!(r.cells.len(), 2, "{:?}", r.cells);
        assert_eq!(r.cells[0].col, 0);
        assert_eq!(r.cells[1].col, 1);
        assert_eq!(r.html, "<table><tr><td></td><td></td></tr></table>");
        assert!(!r.truncated);
    }
}
