//! ONNX Runtime 的进程级公共设施：环境单例、会话构建、输入名获取。
//!
//! 抽出来的唯一原因：M3.1 起不止一种引擎（PP-OCR / DDDDOCR / YOLO）需要建会话，
//! 而 `ort::Environment` 是**进程级单例**——`ort::init().build()` 调第二次直接报
//! "only one environment is allowed per process"。若每个引擎各写一份，
//! 迟早会有人在别处再 build 一次，然后遇到一个「只有同时开两个引擎才崩」的 bug。
//!
//! 这里只是把原有实现**原样搬过来**，逻辑一字未改；PP-OCR 的行为不受影响。

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use ort::environment::Environment;
use ort::ep::{CPU, DirectML};
use ort::error::Result as OrtResult;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use crate::config::Device;

static ORT_ENV: OnceLock<Environment> = OnceLock::new();
/// 串行化首次环境创建，避免两个线程同时 build 时其中一个白跑。
static ORT_ENV_INIT: Mutex<()> = Mutex::new(());

/// 取得进程级 ORT 环境，必要时创建。
pub fn ort_env() -> OrtResult<&'static Environment> {
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

/// 按设备偏好建会话。DirectML 注册失败时由调用方决定是否降级。
pub fn build_session(env: &Environment, path: &Path, device: Device) -> OrtResult<Session> {
    let mut builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?;
    builder = match device {
        Device::Directml => builder.with_execution_providers([
            DirectML::default().build(),
            CPU::default().build(),
        ])?,
        Device::Cpu => builder.with_execution_providers([CPU::default().build()])?,
    };
    builder.commit_from_file(path)
}

/// 模型的第一个输入名。
///
/// 不硬编码 "x"：当前几个模型都叫 x，但换模型时不该因此炸掉。
pub fn input_name(sess: &Session) -> String {
    sess.inputs()
        .first()
        .map(|i| i.name().to_string())
        .unwrap_or_else(|| "x".to_string())
}

/// 用一次最小张量推理确认所选 EP 真的能跑。
///
/// DirectML 在部分 AMD 核显上**注册成功但推理失败**，错误只在 run 阶段暴露，
/// 所以光看 `with_execution_providers` 的返回值不够。
pub fn warmup(sess: &mut Session, input: &str, shape: [usize; 4]) -> OrtResult<()> {
    let t = ndarray::Array4::<f32>::zeros(shape);
    sess.run(ort::inputs![input => ort::value::Tensor::from_array(t)?])?;
    Ok(())
}

/// 按 `请求设备 → 失败降级 CPU` 的顺序建会话，并做一次预热。
///
/// 返回 `(会话, 实际生效设备)`。降级会打 warn，方便用户从日志里看出
/// 「我明明选了 GPU，为什么 actual_device 是 cpu」。
pub fn build_session_with_fallback(
    env: &Environment,
    path: &Path,
    device: Device,
    warmup_shape: [usize; 4],
) -> OrtResult<(Session, Device)> {
    if device == Device::Directml {
        match build_session(env, path, Device::Directml) {
            Ok(mut s) => {
                let name = input_name(&s);
                match warmup(&mut s, &name, warmup_shape) {
                    Ok(()) => return Ok((s, Device::Directml)),
                    Err(e) => tracing::warn!(
                        "{} 在 DirectML 上预热失败，降级 CPU: {e}",
                        path.display()
                    ),
                }
            }
            Err(e) => tracing::warn!(
                "{} 构建 DirectML 会话失败，降级 CPU: {e}",
                path.display()
            ),
        }
    }
    Ok((build_session(env, path, Device::Cpu)?, Device::Cpu))
}
