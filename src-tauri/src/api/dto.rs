//! HTTP 接口的请求/响应 DTO 与错误类型。
//!
//! 设计约定（见 `docs/PLAN.md` 第 6 节）：
//! - 全部接口返回统一信封 `{code, msg, data?}`，`code == 0` 表示成功。
//! - 图像入参支持两种形态：`application/json` 的 base64 字段，或
//!   `multipart/form-data` 的文件字段。默认 JSON base64（脚本党习惯）。

use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// 成功码。
pub const CODE_OK: i32 = 0;
/// 请求体/参数错误。
pub const CODE_BAD_REQUEST: i32 = 400;
/// 引擎未加载（模型缺失或加载失败）。
pub const CODE_ENGINE_NOT_READY: i32 = 503;
/// 已规划但尚未实现的功能。
pub const CODE_NOT_IMPLEMENTED: i32 = 501;
/// 路径不存在（打错接口名）。带最近似路径提示。
pub const CODE_NOT_FOUND: i32 = 404;
/// 路径对但方法不对（例如对 POST 接口发了 GET）。
pub const CODE_METHOD_NOT_ALLOWED: i32 = 405;
/// 在途推理请求打满（并发闸门）。服务此刻忙不过来，不是调用方的错。
pub const CODE_TOO_MANY_REQUESTS: i32 = 429;

/// 统一响应信封。
#[derive(Debug, Serialize)]
pub struct ApiResp<T> {
    pub code: i32,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> ApiResp<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: CODE_OK,
            msg: "ok".to_string(),
            data: Some(data),
        }
    }
}

/// 构造错误响应（`data` 为 `None`）。
pub fn err_resp(code: i32, msg: impl Into<String>) -> ApiResp<()> {
    ApiResp {
        code,
        msg: msg.into(),
        data: None,
    }
}

/// 识别请求的公共参数。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OcrOptions {
    /// base64 图像，可带 `data:image/png;base64,` 前缀
    #[serde(default)]
    pub image: String,
    /// 模型档位：tiny / small / medium（缺省用当前配置）
    #[serde(default)]
    pub level: Option<String>,
    /// 设备：directml / cpu（缺省用当前配置）
    #[serde(default)]
    pub device: Option<String>,
    /// 是否启用方向分类（缺省用当前配置）
    #[serde(default)]
    pub use_cls: Option<bool>,
    /// 引擎：`ppocr:small` / `ddddocr:common` / `yolo`。
    ///
    /// 缺省 = 用当前 PP-OCR 配置（行为与历史版本完全一致）。
    /// 写 `ppocr:*` 也归到缺省路径，档位由 `level` 参数控制。
    #[serde(default)]
    pub engine: Option<String>,
    /// 找字关键词：普通子串，或 `/正则/` 形式
    #[serde(default)]
    pub keyword: Option<String>,
    /// 目标检测置信度下限（仅 `/vision/detect`）。缺省 0.25（ultralytics 默认）
    #[serde(default)]
    pub conf: Option<f32>,
    /// NMS IoU 阈值（仅 `/vision/detect`）。缺省 0.7
    #[serde(default)]
    pub iou: Option<f32>,
    /// 只保留这些 class_id（仅 `/vision/detect`）。空/缺省 = 全部
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
    /// `/vision/pipeline` 的检测引擎。缺省 `yolo`
    #[serde(default)]
    pub detect_engine: Option<String>,
    /// 只识别这块区域 `[x, y, w, h]`（原图像素坐标）。**仅 `/ocr/quyuocr` 会读**，
    /// 其它识别接口收到这个字段会忽略。
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 只找这些码制（**仅 `/vision/qrcode` 会读**），如 `["qr","code128"]`。
    /// 支持分组写法 `1d`（全部一维码）/ `2d`（全部二维码）。空/缺省 = 全找。
    #[serde(default)]
    pub barcode_formats: Option<Vec<String>>,
    /// 条码解码器是否多花点时间找（**仅 `/vision/qrcode` 会读**），缺省 true。
    /// 关掉更快，但一维码容易漏。
    #[serde(default)]
    pub try_harder: Option<bool>,
    /// 表格识别分支（**仅 `/ocr/biaoge` 会读**）：
    /// - `auto`（缺省）先检测页面里的表格，再逐张识别；一张都没检测到就退回整图
    /// - `structure` 把整张图当成一张表格（图本来就是裁好的表格时用这个最准）
    /// - `detect` 只找表格位置，不做结构识别（想自己裁图时用）
    #[serde(default)]
    pub mode: Option<String>,
    /// 是否给单元格填文字（**仅 `/ocr/biaoge` 会读**），缺省 true。
    /// 关掉只出结构和坐标，快一到两倍。
    #[serde(default)]
    pub with_text: Option<bool>,
}

/// `/vision/qrcode` 出参：一个识别到的码。
#[derive(Debug, Serialize)]
pub struct BarcodeHitDto {
    /// 码里编的内容（原文，不做任何解析）
    pub text: String,
    /// 码制名，如 `QR_CODE` / `CODE_128`
    pub format: String,
    /// 定位点/角点，原图坐标。QR 一般给 3 个定位图案 + 1 个校正点
    pub points: Vec<[f32; 2]>,
    /// 定位点的轴对齐包围盒 `[x, y, w, h]`
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// `/vision/qrcode` 出参。
#[derive(Debug, Serialize)]
pub struct BarcodeData {
    /// 找到几个码
    pub count: usize,
    pub codes: Vec<BarcodeHitDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

// ---------------------------------------------------------------- 表格

/// 一个单元格（`/ocr/biaoge` 出参）。
///
/// 坐标是**原图像素**，不是表格局部坐标 —— 整页识别出多个表格时，
/// 前端/脚本拿它直接画框就能对上位置，不用自己加偏移。
#[derive(Debug, Clone, Serialize)]
pub struct TableCellDto {
    pub row: usize,
    pub col: usize,
    pub rowspan: usize,
    pub colspan: usize,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 格内文字。`with_text=false` 时为空
    pub text: String,
}

/// 一张识别出来的表格。
#[derive(Debug, Clone, Serialize)]
pub struct TableDto {
    /// 在整页里的位置。`mode=structure` 时就是整张图（0,0,width,height）
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 检测置信度。`mode=structure` 时固定 1.0（没走检测）
    pub detect_score: f32,
    /// 结构 token 的平均置信度
    pub structure_score: f32,
    pub rows: usize,
    pub cols: usize,
    /// 单元格太多没识别完（模型输出位置有限）
    pub truncated: bool,
    /// `<table>` 片段，带 colspan/rowspan 和格内文字
    pub html: String,
    /// 纯文本版：列间 `\t`、行间 `\n`，可以直接粘进 Excel
    pub text: String,
    pub cells: Vec<TableCellDto>,
}

/// `/ocr/biaoge` 出参。
#[derive(Debug, Serialize)]
pub struct TableData {
    /// 实际走的分支：`auto` / `structure` / `detect`
    pub mode: String,
    /// `auto` 一个表格都没检测到、退回「整图当一张表格」时为 true。
    /// 显式给出来，否则用户会以为检测模型正常工作只是图里没表
    pub fallback: bool,
    pub count: usize,
    pub tables: Vec<TableDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 四点框（原图坐标，顺序 左上/右上/右下/左下）。
pub type Box4 = [[f64; 2]; 4];

/// 单行识别结果。
#[derive(Debug, Clone, Serialize)]
pub struct LineDto {
    pub text: String,
    /// 字段名必须是 `box`（与原产品一致），而 `box` 是 Rust 保留字，用原始标识符。
    #[serde(rename = "box")]
    pub r#box: Box4,
    pub score: f64,
    /// 检测框得分
    pub det_score: f64,
    /// 是否被 cls 判定为 180° 旋转并已翻转
    pub rotated: bool,
}

/// `/ocr/shibie` 出参。
#[derive(Debug, Serialize)]
pub struct ShibieData {
    /// 全部文本按行用 `\n` 拼接（与原产品行为一致）
    pub text: String,
    pub boxes: Vec<Box4>,
    pub scores: Vec<f64>,
    pub lines: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/quyuocr` 出参：区域 OCR。
///
/// 与 `ShibieData` 的区别只有两点：多一个 `region`（实际生效的区域，已钳到图内），
/// 且 `width`/`height` 是**原图**尺寸——`boxes` 里的坐标已经平移回原图坐标系，
/// 调用方不用再做任何换算。
#[derive(Debug, Serialize)]
pub struct RegionOcrData {
    /// 实际生效的区域 `[x, y, w, h]`（请求值被钳到图内后的结果）
    pub region: [i32; 4],
    /// 区域内全部文本按行用 `\n` 拼接
    pub text: String,
    /// 命中行的框，坐标已在原图坐标系
    pub boxes: Vec<Box4>,
    pub scores: Vec<f64>,
    pub lines: Vec<LineDto>,
    /// 原图宽（不是区域宽）
    pub width: u32,
    /// 原图高（不是区域高）
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/zhaozi` 出参。
#[derive(Debug, Serialize)]
pub struct ZhaoziData {
    pub keyword: String,
    pub matched: usize,
    pub hits: Vec<LineDto>,
    pub elapsed_ms: u128,
}

/// `/ocr/duohangjson` 出参：多行数组。
#[derive(Debug, Serialize)]
pub struct DuohangData {
    pub rows: Vec<LineDto>,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 单字结果。`box` 是该字在原图中的四边形（左上/右上/右下/左下）。
#[derive(Debug, Serialize)]
pub struct CharDto {
    pub ch: String,
    pub r#box: Box4,
    pub score: f64,
    /// 行内序号，0 起
    pub index: usize,
    /// CTC 起止帧，排查对齐问题用
    pub t_start: usize,
    pub t_end: usize,
}

/// 单行结果（含逐字）。
#[derive(Debug, Serialize)]
pub struct DanziLineDto {
    pub text: String,
    pub r#box: Box4,
    pub score: f64,
    pub det_score: f64,
    pub rotated: bool,
    /// rec 总帧数。字框宽度由 `帧区间 / 总帧数` 决定
    pub frames: usize,
    pub chars: Vec<CharDto>,
}

/// `/ocr/danzijson` 出参。
#[derive(Debug, Serialize)]
pub struct DanziData {
    /// 所有行按阅读顺序拼成的纯文本
    pub text: String,
    pub lines: Vec<DanziLineDto>,
    /// 全部字符总数（含所有行），省得调用方自己数
    pub char_count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// 找图请求参数（JSON 形态；multipart 形态用同名 form 字段）。
///
/// 模板找图（`tuzhaotu*` / `jubingzhaotu*` / `pingmuzhaotu*`）三种来源由**端点路径**
/// 决定，这里只做字段容器：
/// - `tuzhaotu*` 用 `image`（调用方自己传搜索图）
/// - `jubingzhaotu*` 用 `hwnd` 或 `title`（内部截窗口）
/// - `pingmuzhaotu*` 用 `region`（内部截屏幕，缺省全屏）
///
/// 语义找图（`yuyizhaotu`）只有一个端点，来源由 `source` 字段给出，
/// 阈值用 `conf` 而不是 `min_score`（前者是检测置信度、后者是模板相似度）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FindOptions {
    /// 搜索图 base64（`tuzhaotu*` 必填）
    #[serde(default)]
    pub image: Option<String>,
    /// 模板图 base64（与 `template_file` 二选一）
    #[serde(default)]
    pub template: Option<String>,
    /// 模板文件绝对路径（与 `template` 二选一，支持 png/jpg/bmp）
    #[serde(default)]
    pub template_file: Option<String>,
    /// 模板掩码 base64（可省略）。**白（≥ 阈值）= 参与匹配**，尺寸必须与模板逐像素一致
    #[serde(default)]
    pub mask: Option<String>,
    /// 掩码文件绝对路径（与 `mask` 二选一）
    #[serde(default)]
    pub mask_file: Option<String>,
    /// 掩码二值化阈值，缺省 128（像素 ≥ 阈值视为参与匹配）
    #[serde(default)]
    pub mask_threshold: Option<u8>,
    /// 窗口句柄（`jubingzhaotu*`）；缺省时回退 `title`
    #[serde(default)]
    pub hwnd: Option<i64>,
    /// 窗口标题，精确匹配优先，找不到再按子串匹配
    #[serde(default)]
    pub title: Option<String>,
    /// 搜索区域 `[x, y, w, h]`（屏幕绝对坐标），仅截屏幕时有效
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 分数下限（NCC），缺省按端点预设：精准 0.92 / 容错 0.75
    #[serde(default)]
    pub min_score: Option<f64>,
    /// 最多返回几个命中，默认 1
    #[serde(default)]
    pub max_results: Option<usize>,

    // ---- 以下字段只有语义找图（`/ocr/yuyizhaotu`）会读 ----
    /// 去哪找：`image`（缺省）/ `screen` / `window`
    #[serde(default)]
    pub source: Option<String>,
    /// 用哪个检测模型：`yolo`（缺省，= 目录里第一个）/ `yolo:<模型id>`
    #[serde(default)]
    pub engine: Option<String>,
    /// 检测置信度下限，缺省 0.25。给了 `min_score` 时按它兜底
    #[serde(default)]
    pub conf: Option<f32>,
    /// 只保留这些 class_id；空/缺省 = 全部类别
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
}

/// 运行时提示找图（`/vision/prompt`）的入参。
///
/// 与 `FindOptions` 分开是刻意的：提示找图的「提示」有两条互斥的路
/// （图找图 / 按名字找），塞进 `FindOptions` 会让那个结构变成大杂烩，
/// 而且模板找图的字段在这里一个都用不上。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PromptOptions {
    /// 提示方式：`visual`（图找图，缺省）/ `ref`（按教过的名字找）
    #[serde(default)]
    pub mode: Option<String>,
    /// 目标图 base64。用 `source=screen/window` 时可不传
    #[serde(default)]
    pub image: Option<String>,
    /// 图找图：参考图 base64（必填）
    #[serde(default)]
    pub ref_image: Option<String>,
    /// 图找图：参考框，`x1,y1,x2,y2`，多个用 `;` 分隔（**原图坐标**）
    #[serde(default)]
    pub ref_boxes: Option<String>,
    /// 图找图：每个框的类号，`;` 分隔；缺省全部算 0 类（合成一个提示）
    #[serde(default)]
    pub ref_classes: Option<String>,
    /// 图找图：每个类的显示名，`;` 分隔；缺省回落「参考目标」
    #[serde(default)]
    pub ref_names: Option<String>,
    /// 按名字找：教过的名字，逗号分隔（如 `提交按钮,搜索框`）
    #[serde(default)]
    pub words: Option<String>,
    /// 置信度下限，缺省 0.25
    #[serde(default)]
    pub conf: Option<f32>,
    /// 最多返回几个命中，缺省 8
    #[serde(default)]
    pub max_results: Option<usize>,
    /// 目标图来源：`image`（缺省）/ `screen` / `window`
    #[serde(default)]
    pub source: Option<String>,
    /// `source=window` 时的窗口标题（精确优先，其次子串）
    #[serde(default)]
    pub title: Option<String>,
    /// `source=window` 时的窗口句柄，优先于 `title`
    #[serde(default)]
    pub hwnd: Option<i64>,
    /// `source=screen` 时的截取区域 `[x, y, w, h]`
    #[serde(default)]
    pub region: Option<[i32; 4]>,
    /// 只保留这些提示槽（缺省全部）
    #[serde(default)]
    pub class_filter: Option<Vec<usize>>,
}

/// 单个命中。
///
/// 模板找图与语义找图**共用这一个结构**，脚本切模式不用改解析代码。
#[derive(Debug, Clone, Serialize)]
pub struct FindHitDto {
    /// 左上角（屏幕绝对坐标；`source=image` 为搜索图内坐标）
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub center_x: f64,
    pub center_y: f64,
    /// 模板找图是零均值 NCC（`-1..=1`）；语义找图是检测置信度（`0..=1`）
    pub score: f64,
    /// 语义找图才有：命中的类别 id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<usize>,
    /// 语义找图才有：类别名（模型没写类名时回落 `class_{id}`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
}

/// 找图出参。
///
/// 同时提供扁平字段（`x`/`y`/`score`）与数组（`matches`）：
/// 老脚本习惯直接读 `data.x` / `data.y`，新脚本可用 `matches` 处理多目标。
#[derive(Debug, Serialize)]
pub struct FindData {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub center_y: Option<f64>,
    /// 模板尺寸（未命中时也返回，便于调用方算点击偏移）。
    /// 语义找图没有模板，这里给**首个命中框的尺寸**，未命中为 0。
    pub width: u32,
    pub height: u32,
    /// 命中数量
    pub matched: usize,
    pub matches: Vec<FindHitDto>,
    /// 搜索图来源：`image` / `screen` / `window`
    pub source: String,
    /// 搜索图左上角在屏幕上的坐标。前端要用它把绝对坐标换算回图内坐标
    /// （`source=image` 时恒为 0）。
    pub origin_x: i32,
    pub origin_y: i32,
    /// 语义找图才有：实际使用的检测引擎（`yolo` / `yolo:<id>`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub elapsed_ms: u128,
}

/// `/vision/detect` 的单条检测结果。
#[derive(Debug, Clone, Serialize)]
pub struct DetectHitDto {
    /// 轴对齐框转四点（左上/右上/右下/左下），前端只写一套画框逻辑
    pub r#box: Box4,
    pub class_id: usize,
    /// labels.txt 缺失时回落 `class_{id}`
    pub class_name: String,
    pub score: f64,
}

/// `/vision/detect` 出参。
#[derive(Debug, Serialize)]
pub struct DetectData {
    /// 实际使用的引擎（缺省请求 = 当前 PP-OCR，检测只会失败，这里回请求值）
    pub engine: String,
    pub detections: Vec<DetectHitDto>,
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/vision/pipeline` 的单区域结果：一个检测目标 + 框内识别出的文字。
#[derive(Debug, Clone, Serialize)]
pub struct PipelineRegionDto {
    pub r#box: Box4,
    pub class_id: usize,
    pub class_name: String,
    /// 检测置信度
    pub score: f64,
    /// 框内各行文本按阅读顺序拼接（行间 `\n`）
    pub text: String,
    /// 框内识别出的行（坐标已平移回原图）
    pub lines: Vec<LineDto>,
}

/// `/vision/pipeline` 出参。
#[derive(Debug, Serialize)]
pub struct PipelineData {
    /// 检测引擎标识
    pub detect_engine: String,
    /// 识别引擎标识
    pub ocr_engine: String,
    pub regions: Vec<PipelineRegionDto>,
    /// 检出且成功识别的区域数（识别失败的区域不在其中）
    pub count: usize,
    pub width: u32,
    pub height: u32,
    pub elapsed_ms: u128,
}

/// `/ocr/health` 出参。
#[derive(Debug, Serialize)]
pub struct HealthDto {
    /// 程序版本，如 `"0.1.0"`。与 GitHub Release 的 tag、更新检查比对的是同一个值
    /// （唯一来源 `tauri.conf.json`）。调用方脚本可据此判断自己适配的版本。
    pub version: String,
    pub status: String,
    /// 引擎标识：`ppocr:small` / `ddddocr:common`。前端据此显示「正在用哪个引擎」。
    pub engine: String,
    pub model: String,
    pub device: String,
    /// 实际生效设备（DirectML 不可用时会是 cpu）
    pub actual_device: String,
    pub det_loaded: bool,
    pub rec_loaded: bool,
    pub cls_loaded: bool,
    pub dict_size: usize,
    pub port: u16,
    pub uptime_sec: u64,
}

/// 能力声明。前端据此决定显示哪些面板——**未实现能力的入口直接隐藏**，
/// 不要显示成灰色禁用按钮（小白看到灰按钮只会困惑，不知道为什么点不了）。
#[derive(Debug, Serialize)]
pub struct CapabilitiesDto {
    pub line_boxes: bool,
    pub char_boxes: bool,
    pub rotation_cls: bool,
    pub detect: bool,
    /// 检测类别名。空 = 不做检测，或没给 labels.txt
    pub classes: Vec<String>,
    pub batch: bool,
}

impl From<&crate::engine::Capabilities> for CapabilitiesDto {
    fn from(c: &crate::engine::Capabilities) -> Self {
        Self {
            line_boxes: c.line_boxes,
            char_boxes: c.char_boxes,
            rotation_cls: c.rotation_cls,
            detect: c.detect,
            classes: c.classes.clone(),
            batch: c.batch,
        }
    }
}

/// `/engine/list` 的一条引擎记录。
#[derive(Debug, Serialize)]
pub struct EngineItemDto {
    /// 规范化引擎名，如 `ppocr:small`、`yolo`
    pub name: String,
    /// 引擎族：ppocr / ddddocr / yolo
    pub family: String,
    /// ppocr 的档位或 ddddocr 的版本；YOLO 为 null（版本已含在 name 里）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub capabilities: CapabilitiesDto,
    /// 模型文件齐全、可以加载。**不代表已在内存里**
    pub available: bool,
    /// 实际生效设备。只有当前已加载的那个引擎才有值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_device: Option<String>,
    /// 不可用的原因或补充说明
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<&crate::engine::EngineStatus> for EngineItemDto {
    fn from(s: &crate::engine::EngineStatus) -> Self {
        Self {
            name: s.spec.key(),
            family: s.spec.kind.family().to_string(),
            variant: s.spec.variant.clone(),
            capabilities: CapabilitiesDto::from(&s.capabilities),
            available: s.available,
            actual_device: s
                .actual_device
                .map(|d| format!("{d:?}").to_lowercase()),
            note: s.note.clone(),
        }
    }
}

/// `/engine/list` 出参。
#[derive(Debug, Serialize)]
pub struct EngineListData {
    pub models_root: String,
    /// 当前配置的默认引擎
    pub default_engine: String,
    pub engines: Vec<EngineItemDto>,
}

/// 解析 multipart 表单得到的字段集合。
pub type FormFields = HashMap<String, String>;

/// 从 base64 字符串解码图像字节。
///
/// 容忍三种常见形态：纯 base64、带 `data:...;base64,` 前缀、含换行/空白的 base64。
pub fn decode_base64_image(s: &str) -> Result<Vec<u8>, String> {
    let b64 = match s.find(',') {
        // data URL 前缀只可能出现在开头，取最后一个逗号之后的内容
        Some(i) if s[..i].contains("base64") => &s[i + 1..],
        _ => s,
    };
    use base64::Engine;
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("base64 解码失败: {e}"))
}

/// 解码后允许的最大像素数：3200 万（约 5600×5600，或 11520×2160 的三联 4K 长条）。
///
/// 为什么必须有这道闸：请求体上限是 32MB，而一张 32MB 的 PNG 能声明 8000×8000
/// 以上的尺寸，解出来是 200MB 以上的位图。推理被 `engine` 互斥锁串行化，
/// **排队期间这些位图全部活着** —— 在途请求一多，进程直接被系统 OOM killer
/// 干掉，现象是「程序闪退、零日志」，跟推理出错完全不像，极难查。
/// 且 PP-OCR 的检测本身就会把长边缩到 960，超过这个尺寸对识别没有任何增益。
///
/// 实测依据：48 路并发 + 2400×1600 的图，进程工作集从 204MB 涨到 493MB
/// （在途位图 × 图尺寸，线性）。不设上限时这个乘积没有头。
pub const MAX_DECODE_PIXELS: u64 = 32_000_000;

/// 单边尺寸上限。廉价预过滤，挡「声明 40000×40000 的 PNG 炸弹」——
/// 这种图会在分配第一行之前就被 `image::Limits` 拦下，根本走不到像素复核。
const MAX_DECODE_SIDE: u32 = 8000;

/// 用 `image` crate 解码内存中的图像字节。
///
/// 与 `image::load_from_memory` 的差别只有一个：套了尺寸闸（见 [`MAX_DECODE_PIXELS`）。
pub fn decode_image(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    use image::ImageReader;
    use std::io::Cursor;

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("图像解码失败: {e}"))?;
    // `Limits` 是 #[non_exhaustive] 的，只能改字段不能结构体字面量构造。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    reader.limits(limits);

    let img = reader.decode().map_err(|e| match e {
        // 尺寸超限要给可操作的提示，不能甩一句 "Image size exceeds limit"
        image::ImageError::Limits(_) => format!(
            "图像尺寸超限：单边不得超过 {MAX_DECODE_SIDE}px。大图请先裁剪，或用带 region 参数的接口分区识别。"
        ),
        other => format!("图像解码失败: {other}"),
    })?;

    // 单边闸拦不住「宽而扁」的组合（12000×2000 单边都合规），像素复核才是总闸。
    let pixels = u64::from(img.width()) * u64::from(img.height());
    if pixels > MAX_DECODE_PIXELS {
        return Err(format!(
            "图像尺寸超限：{}×{} = {} 像素，上限 {} 像素（约 5600×5600）。\
             大图请先裁剪，或用带 region 参数的接口分区识别。",
            img.width(),
            img.height(),
            pixels,
            MAX_DECODE_PIXELS
        ));
    }
    Ok(img)
}

/// 接口错误。统一转成 `{code, msg}` JSON，并带上合适的 HTTP 状态码。
#[derive(Debug)]
pub enum ApiError {
    /// 入参问题（缺字段、base64 坏、图像坏、正则坏）
    BadRequest(String),
    /// 引擎未加载：模型文件缺失或加载失败
    EngineNotReady(String),
    /// 引擎不支持该能力（code 1002，见 docs/ENGINES.md 4.3）。
    /// 调用方本应先看 `/engine/list` 的 capabilities，走到这里说明漏判了。
    Unsupported(String),
    /// 未知引擎名（code 1003）
    UnknownEngine(String),
    /// 内部错误（推理失败等）
    Internal(String),
    /// 已规划但尚未实现
    NotImplemented(String),
    /// 在途推理请求打满（并发闸门）。不是调用方的错，是服务此刻忙不过来。
    TooManyRequests(String),
}

impl ApiError {
    pub fn code(&self) -> i32 {
        match self {
            ApiError::BadRequest(_) => CODE_BAD_REQUEST,
            ApiError::EngineNotReady(_) => CODE_ENGINE_NOT_READY,
            ApiError::Unsupported(_) => 1002,
            ApiError::UnknownEngine(_) => 1003,
            ApiError::Internal(_) => 500,
            ApiError::NotImplemented(_) => CODE_NOT_IMPLEMENTED,
            ApiError::TooManyRequests(_) => CODE_TOO_MANY_REQUESTS,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ApiError::BadRequest(m)
            | ApiError::EngineNotReady(m)
            | ApiError::Unsupported(m)
            | ApiError::UnknownEngine(m)
            | ApiError::Internal(m)
            | ApiError::NotImplemented(m)
            | ApiError::TooManyRequests(m) => m.clone(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ApiError {}

impl From<ApiError> for Response {
    fn from(e: ApiError) -> Self {
        use axum::http::StatusCode;
        let status = match &e {
            ApiError::BadRequest(_)
            | ApiError::Unsupported(_)
            | ApiError::UnknownEngine(_) => StatusCode::BAD_REQUEST,
            ApiError::EngineNotReady(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            ApiError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
        };
        let mut resp = (status, axum::Json(err_resp(e.code(), e.message()))).into_response();
        // 429 必须带 Retry-After：否则调用方只会立刻重试，把队列打得更满。
        if matches!(e, ApiError::TooManyRequests(_)) {
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("1"),
            );
        }
        resp
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        self.into()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    // image 0.25 的 `dimensions()` 走 `GenericImageView` trait，得显式导入。
    use image::GenericImageView;

    /// 一张最小的合法 PNG（1×1 白点），用来确认「加了尺寸闸之后正常小图照样解得开」。
    fn tiny_png() -> Vec<u8> {
        // base64: 89 50 4E 47 0D 0A 1A 0A ... 1x1 RGB 白点
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            )
            .unwrap()
    }

    /// 用 image crate 自己造一张指定尺寸的 PNG（测试里不引入 PIL 之外的依赖）。
    fn sized_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    /// 尺寸闸不能误伤正常小图 —— 这条是回归防线：闸门加错了地方，
    /// 所有请求都会变成 400，而单元测试不看 HTTP 层是发现不了的。
    #[test]
    fn decode_image_still_accepts_small_images() {
        let img = decode_image(&tiny_png()).expect("1x1 的 PNG 必须能解开");
        assert_eq!(img.dimensions(), (1, 1));
    }

    /// 单边超限要在**分配位图之前**就被拒（image::Limits 的维度闸）。
    #[test]
    fn decode_image_rejects_oversized_side_before_allocating() {
        let e = decode_image(&sized_png(9000, 100)).unwrap_err();
        assert!(e.contains("单边"), "错误信息要说清是单边超限: {e}");
    }

    /// 单边合规但像素超限（宽而扁的组合）要走像素总闸。
    #[test]
    fn decode_image_rejects_too_many_pixels() {
        let e = decode_image(&sized_png(7000, 5000)).unwrap_err();
        assert!(e.contains("像素"), "错误信息要说清是像素超限: {e}");
        assert!(e.contains("region"), "要给出「分区识别」这条出路: {e}");
    }

    /// 429 必须落在 HTTP 429 且带 Retry-After，否则客户端只会盲目重试。
    #[test]
    fn too_many_requests_maps_to_429_with_retry_after() {
        let resp = ApiError::TooManyRequests("满了".into()).into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp.headers().get(axum::http::header::RETRY_AFTER).unwrap(),
            "1"
        );
    }
}
