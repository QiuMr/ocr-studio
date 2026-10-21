//! 只做「找文本行」的检测器：det.onnx + DB 后处理，不带识别。
//!
//! 存在的理由：DDDDOCR 的 common/old 模型是**单行分类器**（一行图进、一串字出），
//! 自带的 `common_det.onnx` 是 YOLOX 目标检测（416×416、strides 8/16/32），
//! 训练目标是验证码里的字符块，拿它去切文档截图效果很差。所以 DDDDOCR 引擎
//! 的「找行」这一步复用 PP-OCR 的 det 模型，只用 ddddocr 做识别。
//!
//! 为什么不直接复用 `OcrEngine`：它的 det/rec/cls 是一起加载的（`load` 里缺一不可），
//! 为了拿一个 det 而加载整个 PP-OCR 明显浪费；而把 `OcrEngine` 拆成三个可选会话
//! 会动到精度基线的代码路径。这里复刻的只是「预处理 + 后处理 + 还原坐标」这三步，
//! 用的都是 `ocr::preprocess` / `ocr::det` 里的同一份函数，不存在两套算法。

use std::path::{Path, PathBuf};

use image::DynamicImage;
use ort::session::Session;
use ort::value::Tensor;

use super::det::{db_postprocess, DetBox, DetConfig};
use super::ort_util;
use super::preprocess::{det_preprocess, DetLimitType};
use crate::config::Device;

/// 检测长边上限，与 PP-OCR 引擎保持一致（`engine.rs` 里同样是 960）。
pub const DET_LIMIT_SIDE: u32 = 960;

pub struct LineDetector {
    sess: Session,
    input: String,
    cfg: DetConfig,
    path: PathBuf,
    device: Device,
}

impl LineDetector {
    /// 加载 det 模型。`device` 不可用时会自动降级到 CPU。
    pub fn load(det_path: &Path, device: Device) -> ort::error::Result<Self> {
        if !det_path.exists() {
            return Err(ort::Error::new(format!(
                "检测模型不存在: {}",
                det_path.display()
            )));
        }
        let env = ort_util::ort_env()?;
        // det 输入是动态尺寸 [N,3,H,W]，预热用 32×32 就够（与 PP-OCR 引擎一致）
        let (sess, actual) =
            ort_util::build_session_with_fallback(env, det_path, device, [1, 3, 32, 32])?;
        let input = ort_util::input_name(&sess);
        Ok(Self {
            sess,
            input,
            cfg: DetConfig::default(),
            path: det_path.to_path_buf(),
            device: actual,
        })
    }

    pub fn actual_device(&self) -> Device {
        self.device
    }

    pub fn config(&self) -> &DetConfig {
        &self.cfg
    }

    pub fn config_mut(&mut self) -> &mut DetConfig {
        &mut self.cfg
    }

    /// 找出全部文本行框（原图坐标，已按阅读顺序排序）。
    pub fn detect(&mut self, img: &DynamicImage) -> ort::error::Result<Vec<DetBox>> {
        if img.width() == 0 || img.height() == 0 {
            return Ok(Vec::new());
        }
        let (tensor, ratio) = det_preprocess(img, DET_LIMIT_SIDE, DetLimitType::Max);
        let out = self.sess.run(ort::inputs![
            self.input.as_str() => Tensor::from_array(tensor)?
        ])?;
        let prob = out[0].try_extract_array::<f32>()?;
        let prob4 = prob
            .view()
            .into_dimensionality::<ndarray::Ix4>()
            .map_err(|e| ort::Error::new(format!("det 输出维度异常: {e}")))?;
        Ok(db_postprocess(&prob4, ratio, &self.cfg))
    }

    /// 模型路径（错误信息里要能说清是哪个文件出的问题）。
    pub fn path(&self) -> &Path {
        &self.path
    }
}
