//! OCR 引擎：加载三档 ONNX 模型、编排 det -> (cls) -> rec 全流程。
//!
//! 关键设计：
//! - **EP 探测靠真实推理，不靠注册结果。** DirectML 在 AMD 核显上注册可能成功但推理失败，
//!   因此加载后会跑一次最小张量的预热推理，失败则整体降级到 CPU 并记录。
//! - **输入名动态获取**，不硬编码 "x"。虽然当前三个模型的输入都叫 x，
//!   但换模型时不该因此炸掉。
//! - 模型目录布局见 `docs/PLAN.md` 5.3 节：`models/<level>/{det,rec,cls}.onnx + keys.txt`。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::DynamicImage;
use ndarray::Array4;
use ort::environment::Environment;
use ort::ep::{CPU, DirectML};
use ort::error::Result as OrtResult;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;

use super::det::{db_postprocess, DetBox, DetConfig};
use super::geometry::{order_points, warp_perspective, Pt};
use super::preprocess::{det_preprocess, rec_preprocess, DetLimitType, CLS_IMG_H, CLS_IMG_W};
use super::rec::{ctc_decode, RecResult};

// 注意：`Device` / `ModelLevel` 统一定义在 `crate::config`，此处仅引用，
// 避免出现两套同名类型互相不兼容的问题（曾因此编译失败）。
use crate::config::{Device, ModelLevel};

impl ModelLevel {
    /// 模型子目录名。
    pub fn dir_name(&self) -> &'static str {
        match self {
            ModelLevel::Tiny => "tiny",
            ModelLevel::Small => "small",
            ModelLevel::Medium => "medium",
        }
    }
}

/// ort 的 `Environment` 是**进程级单例**：`ort::init().build()` 调用第二次会直接报
/// "only one environment is allowed per process"。
///
/// 这不是测试专属问题——运行时切换模型档位/设备就要重载引擎，同样会踩到。
/// 因此这里缓存一份，整个进程复用。
static ORT_ENV: OnceLock<Environment> = OnceLock::new();
/// 串行化首次环境创建，避免两个线程同时 build 时其中一个白跑。
static ORT_ENV_INIT: Mutex<()> = Mutex::new(());

fn ort_env() -> OrtResult<&'static Environment> {
    if let Some(env) = ORT_ENV.get() {
        return Ok(env);
    }
    let _guard = ORT_ENV_INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = ORT_ENV.get() {
        return Ok(env);
    }
    let env = ort::init().with_name("ocr-studio").build()?;
    let _ = ORT_ENV.set(env);
    Ok(ORT_ENV.get().expect("ORT_ENV 刚被设置"))
}

/// 引擎加载配置。
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// 模型根目录（其下应有 tiny/small/medium 子目录）
    pub models_root: PathBuf,
    pub level: ModelLevel,
    pub device: Device,
    /// 是否启用方向分类（需要 cls.onnx 存在）
    pub use_cls: bool,
    /// cls 判定阈值：旋转概率超过该值才执行 180° 翻转
    pub cls_thresh: f32,
    pub det_cfg: DetConfig,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            models_root: PathBuf::from("models"),
            level: ModelLevel::Small,
            device: Device::default(),
            use_cls: true,
            cls_thresh: 0.9,
            det_cfg: DetConfig::default(),
        }
    }
}

/// 单个文本行的识别结果。
#[derive(Debug, Clone)]
pub struct TextBox {
    /// 四点，原图坐标，顺序 `[左上, 右上, 右下, 左下]`
    pub points: [Pt; 4],
    pub text: String,
    /// 识别置信度（被采纳字符的逐帧概率均值）
    pub score: f64,
    /// 检测框得分
    pub det_score: f64,
    /// 是否被 cls 判定为 180° 旋转并已翻转
    pub rotated: bool,
}

/// 引擎健康状态。
#[derive(Debug, Clone)]
pub struct HealthInfo {
    pub level: ModelLevel,
    pub requested_device: Device,
    /// 实际生效的设备（可能因 DirectML 不可用而降级为 CPU）
    pub actual_device: Device,
    pub det_loaded: bool,
    pub rec_loaded: bool,
    pub cls_loaded: bool,
    pub dict_size: usize,
}

/// OCR 引擎。`Session` 本身可跨线程共享（内部有锁），因此引擎是 `Send + Sync`。
pub struct OcrEngine {
    det: Session,
    rec: Session,
    cls: Option<Session>,
    chars: Vec<String>,
    det_input: String,
    rec_input: String,
    cls_input: String,
    cfg: EngineConfig,
    actual_device: Device,
}

impl OcrEngine {
    /// 按配置加载引擎。DirectML 不可用时会自动降级到 CPU。
    pub fn load(cfg: EngineConfig) -> OrtResult<Self> {
        let dir = level_dir(&cfg.models_root, cfg.level);
        let det_path = dir.join("det.onnx");
        let rec_path = dir.join("rec.onnx");
        let cls_path = dir.join("cls.onnx");
        let keys_path = dir.join("keys.txt");

        if !det_path.exists() {
            return Err(ort::Error::new(format!(
                "检测模型不存在: {}（请先运行 scripts/fetch_models.ps1）",
                det_path.display()
            )));
        }
        if !rec_path.exists() {
            return Err(ort::Error::new(format!(
                "识别模型不存在: {}",
                rec_path.display()
            )));
        }

        let chars = load_dict(&keys_path)?;

        let env = ort_env()?;

        // 先尝试请求的设备；若预热推理失败则降级 CPU
        let (det, rec, actual_device) = match cfg.device {
            Device::Directml => match build_pair(&env, &det_path, &rec_path, Device::Directml) {
                Ok((mut d, mut r)) => {
                    // 用一次真实推理验证 DirectML 真的能用
                    match warmup_directml(&mut d, &mut r) {
                        Ok(()) => (d, r, Device::Directml),
                        Err(e) => {
                            tracing::warn!("DirectML 预热推理失败，降级到 CPU: {e}");
                            let cpu = build_pair(&env, &det_path, &rec_path, Device::Cpu)?;
                            (cpu.0, cpu.1, Device::Cpu)
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("DirectML 会话构建失败，降级到 CPU: {e}");
                    let cpu = build_pair(&env, &det_path, &rec_path, Device::Cpu)?;
                    (cpu.0, cpu.1, Device::Cpu)
                }
            },
            Device::Cpu => {
                let cpu = build_pair(&env, &det_path, &rec_path, Device::Cpu)?;
                (cpu.0, cpu.1, Device::Cpu)
            }
        };

        // cls 为可选项：配置开启且文件存在才加载
        let cls = if cfg.use_cls && cls_path.exists() {
            let s = build_session(&env, &cls_path, actual_device)?;
            Some(s)
        } else {
            if cfg.use_cls {
                tracing::info!("未找到 {}，方向分类已禁用", cls_path.display());
            }
            None
        };

        let det_input = input_name(&det);
        let rec_input = input_name(&rec);
        let cls_input = cls.as_ref().map(input_name).unwrap_or_default();

        Ok(Self {
            det,
            rec,
            cls,
            chars,
            det_input,
            rec_input,
            cls_input,
            cfg,
            actual_device,
        })
    }

    pub fn health(&self) -> HealthInfo {
        HealthInfo {
            level: self.cfg.level,
            requested_device: self.cfg.device,
            actual_device: self.actual_device,
            det_loaded: true,
            rec_loaded: true,
            cls_loaded: self.cls.is_some(),
            dict_size: self.chars.len(),
        }
    }

    pub fn chars(&self) -> &[String] {
        &self.chars
    }

    /// 当前引擎配置。HTTP 层据此判断是否需要按请求参数重载模型。
    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    pub fn actual_device(&self) -> Device {
        self.actual_device
    }

    /// 对整图做 OCR，返回按阅读顺序排列的文本行。
    /// 调试/对齐探针：对**已准备好的 rec 输入张量**跑推理，返回逐帧概率 `[T, C]`。
    ///
    /// 存在的理由：全流程置信度对不上时，需要把「预处理」和「推理+解码」两段切开。
    /// 喂 Python dump 的张量进来，若概率仍不一致，问题就在会话/解码而不是预处理。
    pub fn rec_probe(
        &mut self,
        tensor: &ndarray::Array3<f32>,
    ) -> OrtResult<ndarray::Array2<f32>> {
        // 模型要的是 [N,3,H,W]，调用方给的是无 batch 的 [3,H,W]
        let batched = tensor.clone().insert_axis(ndarray::Axis(0));
        let out = self.rec.run(ort::inputs![
            self.rec_input.as_str() => Tensor::from_array(batched)?
        ])?;
        let p = out[0].try_extract_array::<f32>()?;
        let p_batch = p.index_axis(ndarray::Axis(0), 0);
        let p2 = p_batch
            .into_dimensionality::<ndarray::Ix2>()
            .map_err(|e| ort::Error::new(format!("rec 输出维度异常: {e}")))?;
        Ok(p2.to_owned())
    }

    pub fn recognize(&mut self, img: &DynamicImage) -> OrtResult<Vec<TextBox>> {
        if img.width() == 0 || img.height() == 0 {
            return Ok(Vec::new());
        }

        // ---- det ----
        let (tensor, ratio) = det_preprocess(img, 960, DetLimitType::Max);
        let det_out = self.det.run(ort::inputs![
            self.det_input.as_str() => Tensor::from_array(tensor)?
        ])?;
        let prob = det_out[0].try_extract_array::<f32>()?;
        let prob4 = prob
            .view()
            .into_dimensionality::<ndarray::Ix4>()
            .map_err(|e| ort::Error::new(format!("det 输出维度异常: {e}")))?;

        let boxes: Vec<DetBox> = db_postprocess(&prob4, ratio, &self.cfg.det_cfg);

        // ---- rec ----
        let mut results = Vec::with_capacity(boxes.len());
        for b in boxes {
            let Some(crop) = warp_perspective(img, b.points) else {
                continue;
            };

            // ---- cls（可选）----
            let mut rotated = false;
            let crop = if let Some(cls_sess) = self.cls.as_mut() {
                let cls_tensor = cls_preprocess(&crop);
                let out = cls_sess.run(ort::inputs![
                    self.cls_input.as_str() => Tensor::from_array(cls_tensor)?
                ])?;
                let p = out[0].try_extract_array::<f32>()?;
                rotated = cls_should_rotate(&p, self.cfg.cls_thresh);
                if rotated {
                    crop.rotate180()
                } else {
                    crop
                }
            } else {
                crop
            };

            let (rec_tensor, valid_w) = rec_preprocess(&crop);
            if valid_w == 0 {
                continue;
            }
            let out = self.rec.run(ort::inputs![
                self.rec_input.as_str() => Tensor::from_array(rec_tensor)?
            ])?;
            let p = out[0].try_extract_array::<f32>()?;
            // rec 输出是 [N, T, C]；CTC 解码要的是 [T, C]，先去 batch 维再转静态二维。
            // 必须分两步绑定：index_axis 的结果借自上一个临时值。
            let p_batch = p.index_axis(ndarray::Axis(0), 0);
            let p2 = p_batch
                .into_dimensionality::<ndarray::Ix2>()
                .map_err(|e| ort::Error::new(format!("rec 输出维度异常: {e}")))?;
            let RecResult { text, score, .. } = ctc_decode(&p2, &self.chars);
            // 调试钩子：设 OCR_DEBUG_REC=1 时打印每个框的 rec 逐帧 argmax 与非 blank 帧。
            // 用于定位「文本对但置信度对不上」这类问题——能把 det 框、rec 输入、
            // 逐帧概率三层一次拉开。默认关闭，不产生任何输出。
            if std::env::var("OCR_DEBUG_REC").is_ok() {
                let mut nb = Vec::new();
                let mut prev = usize::MAX;
                for r in p2.rows() {
                    let mut bi = 0usize;
                    let mut bv = f32::MIN;
                    for (j, v) in r.iter().enumerate() {
                        if *v > bv {
                            bv = *v;
                            bi = j;
                        }
                    }
                    if bi != 0 && bi != prev && bi >= 1 && bi <= self.chars.len() {
                        nb.push((bi, bv));
                    }
                    prev = bi;
                }
                eprintln!(
                    "[rec dbg] #{} valid_w={} score={score:.4} pts={:?} nb={nb:?}",
                    results.len(),
                    valid_w,
                    b.points
                );
            }
            let points = order_points(b.points);
            results.push(TextBox {
                points,
                text,
                score,
                det_score: b.score,
                rotated,
            });
        }

        Ok(results)
    }
}

/// 让 ort 的 `load-dynamic` 加载到**随包的那份** `onnxruntime.dll`。
///
/// 背景：ort 用 libloading 按 Windows 标准顺序找 `onnxruntime.dll`
/// （exe 目录 → System32 → PATH）。本机 `C:\WINDOWS\system32` 里恰好躺着一份
/// 1.17.1 的 onnxruntime.dll，版本低于 ort 要求（>=1.24.x），会直接把进程带崩。
/// 打包后 DLL 就在 exe 旁边所以没事，但 `tauri dev` / `cargo test` 时 exe 在
/// `target/debug`，必须显式指定路径。
///
/// 幂等：已设置 `ORT_DYLIB_PATH` 时不动。返回是否最终有可用路径。
pub fn ensure_ort_dylib_path(workspace_root: &Path) -> bool {
    if std::env::var("ORT_DYLIB_PATH").ok().filter(|s| !s.is_empty()).is_some() {
        return true;
    }
    let dll = workspace_root
        .join("src-tauri")
        .join("runtime")
        .join("onnxruntime.dll");
    if !dll.exists() {
        return false;
    }
    // ort 在首次使用 API 时才读取该变量，此刻尚未初始化，可以安全设置
    unsafe {
        std::env::set_var("ORT_DYLIB_PATH", &dll);
    }
    true
}

fn level_dir(root: &Path, level: ModelLevel) -> PathBuf {
    root.join(level.dir_name())
}

/// 读取字典。约定：每行一个字符，空行不计入。
///
/// 注意与 PaddleOCR 的差异：PaddleOCR 把所有行拼接成一个字符串再 `list()`，
/// 效果等价于「跳过空行后的字符序列」，与本实现一致。
fn load_dict(path: &Path) -> OrtResult<Vec<String>> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ort::Error::new(format!("读取字典失败 {}: {e}", path.display()))
    })?;
    // 去掉可能存在的 BOM
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    let chars: Vec<String> = content
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    if chars.is_empty() {
        return Err(ort::Error::new(format!("字典为空: {}", path.display())));
    }
    Ok(chars)
}

fn build_session(
    env: &Environment,
    path: &Path,
    device: Device,
) -> OrtResult<Session> {
    let mut builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?;
    builder = match device {
        Device::Directml => builder.with_execution_providers([
            DirectML::default().build(),
            CPU::default().build(),
        ])?,
        Device::Cpu => builder
            .with_execution_providers([CPU::default().build()])?,
    };
    builder.commit_from_file(path)
}

fn build_pair(
    env: &Environment,
    det_path: &Path,
    rec_path: &Path,
    device: Device,
) -> OrtResult<(Session, Session)> {
    Ok((
        build_session(env, det_path, device)?,
        build_session(env, rec_path, device)?,
    ))
}

/// 用最小张量跑一次 det/rec，确认所选 EP 真的能推理。
///
/// DirectML 注册成功但设备不可用时，错误只会在 run 阶段暴露，
/// 所以这一步是必须的，不能只靠 `with_execution_providers` 的返回值。
fn warmup_directml(det: &mut Session, rec: &mut Session) -> OrtResult<()> {
    // det: [1,3,32,32]
    let det_t = Array4::<f32>::zeros((1, 3, 32, 32));
    let name = input_name(det);
    det.run(ort::inputs![name.as_str() => Tensor::from_array(det_t)?])?;

    // rec: [1,3,48,320]
    let rec_t = Array4::<f32>::zeros((1, 3, 48, 320));
    let name = input_name(rec);
    rec.run(ort::inputs![name.as_str() => Tensor::from_array(rec_t)?])?;
    Ok(())
}

fn input_name(sess: &Session) -> String {
    sess.inputs()
        .first()
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "x".to_string())
}

/// cls 预处理：输入固定 [N,3,80,160]。
fn cls_preprocess(img: &DynamicImage) -> Array4<f32> {
    let rgb = super::preprocess::resize_bilinear(&img.to_rgb8(), CLS_IMG_W, CLS_IMG_H);
    let mut t = Array4::<f32>::zeros((1, 3, CLS_IMG_H as usize, CLS_IMG_W as usize));
    for (x, y, px) in rgb.enumerate_pixels() {
        let [r, g, b] = px.0;
        t[[0, 0, y as usize, x as usize]] = (r as f32 / 255.0 - 0.5) / 0.5;
        t[[0, 1, y as usize, x as usize]] = (g as f32 / 255.0 - 0.5) / 0.5;
        t[[0, 2, y as usize, x as usize]] = (b as f32 / 255.0 - 0.5) / 0.5;
    }
    t
}

/// 是否应旋转 180°。
///
/// 规则：输出 `[N, 2]`，索引 1 为「旋转 180°」的概率。
/// 当它同时是 argmax 且超过 `cls_thresh` 时才翻转。
///
/// 注意：此阈值语义需与 RapidOCR / PaddleOCR 逐一对齐后才能视为已验证。
/// 当前测试图为正向文本，该分支不会触发，不影响基线比对。
fn cls_should_rotate(p: &ndarray::ArrayViewD<f32>, thresh: f32) -> bool {
    if p.ndim() < 2 || p.shape()[1] < 2 {
        return false;
    }
    let straight = p[[0, 0]];
    let rotated = p[[0, 1]];
    rotated > straight && rotated >= thresh
}

/// 便捷函数：从给定目录直接构造引擎（给测试和 CLI 用）。
pub fn engine_from_dir(dir: &Path, level: ModelLevel, device: Device) -> OrtResult<OcrEngine> {
    engine_from_dir_with(dir, level, device, true)
}

/// 同 `engine_from_dir`，但可显式控制方向分类开关。
pub fn engine_from_dir_with(
    dir: &Path,
    level: ModelLevel,
    device: Device,
    use_cls: bool,
) -> OrtResult<OcrEngine> {
    OcrEngine::load(EngineConfig {
        models_root: dir.to_path_buf(),
        level,
        device,
        use_cls,
        ..Default::default()
    })
}

/// 共享指针类型，供 Tauri 状态管理使用。
pub type SharedEngine = Arc<std::sync::Mutex<OcrEngine>>;
