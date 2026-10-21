//! OCR 模块：引擎、预处理、检测后处理、识别解码。
//!
//! 模块划分与 `docs/PLAN.md` 第 3 节一致：
//! - `engine`      会话构建、EP 探测与降级、det->cls->rec 编排
//! - `preprocess`  resize / 归一化 / NCHW
//! - `det`         DB 后处理（二值化、轮廓、unclip、打分、排序）
//! - `rec`         CTC 解码
//! - `geometry`    最小外接矩形、unclip 偏移、多边形填充、透视变换

pub mod det;
pub mod engine;
pub mod geometry;
pub mod preprocess;
pub mod rec;

pub use det::{db_postprocess, DetBox, DetConfig};
pub use engine::{
    engine_from_dir, engine_from_dir_with, ensure_ort_dylib_path, EngineConfig, HealthInfo,
    OcrEngine, TextBox,
};
// Device / ModelLevel 统一定义在 crate::config，经此重新导出以保持 `ocr::Device` 路径可用
pub use crate::config::{Device, ModelLevel};
pub use preprocess::{det_preprocess, rec_preprocess, DetLimitType};
pub use rec::{ctc_decode, RecResult};
