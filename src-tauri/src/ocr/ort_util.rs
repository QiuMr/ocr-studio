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
    // ONNX Runtime 的日志会整条转发进 tracing。SLANet 的输入维度是动态的，
    // 每推理一次就刷几十条 "Expected shape from model of {} does not match"
    // 的 WARN —— 实测一天写出 120 万行日志，真正有用的 INFO 全被埋掉，
    // 而且同步写盘会拖慢推理。只留 ERROR：DML 注册失败、算子不支持这类
    // 关键信息本身就是 ERROR 级，不会丢。
    env.set_log_level(ort::logging::LogLevel::Error);
    let _ = ORT_ENV.set(env);
    Ok(ORT_ENV.get().expect("ORT_ENV 刚被设置"))
}

/// 当前 `onnxruntime.dll` 是否真的带 DirectML 执行提供器。
///
/// 为什么必须显式查：`SessionBuilder::with_execution_providers` 对**单个** EP
/// 注册失败只打一条 `ERROR ort::ep` 日志然后跳过，整体仍返回 `Ok`。
/// 于是「建会话成功」完全不能证明 DML 生效——2026-10-22 就踩到这个坑：
/// 界面上写着 GPU/DirectML，日志里四条 "DirectML is not supported in this
/// build of ONNX Runtime"，实际一直在 CPU 上跑。
///
/// 判据用 ort 自己的 `directml::api()`（内部是 `GetExecutionProviderApi("DML")`），
/// 返回 `None` 就是这份 DLL 没编 DML。
pub fn directml_available() -> bool {
    ort::ep::directml::api().is_some()
}

/// 按设备偏好建会话。DirectML 不可用时**直接走 CPU**，不再注册必然失败的 EP。
pub fn build_session(env: &Environment, path: &Path, device: Device) -> OrtResult<Session> {
    let mut builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?;
    let use_dml = device == Device::Directml;
    if use_dml && !directml_available() {
        tracing::warn!(
            "{} 请求 DirectML，但当前 onnxruntime.dll 不带 DML 执行提供器，改用 CPU",
            path.display()
        );
    }
    if use_dml && directml_available() {
        builder = builder.with_execution_providers([
            DirectML::default().build(),
            CPU::default().build(),
        ])?;
    } else {
        builder = builder.with_execution_providers([CPU::default().build()])?;
    }
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
    if device == Device::Directml && directml_available() {
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
