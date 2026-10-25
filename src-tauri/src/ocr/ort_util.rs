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

/// 当前 CPU 是否满足这份 `onnxruntime.dll` 的指令集基线。
///
/// 为什么必须自己先问一句：微软的 Windows x64 预编译包从 1.18 起把 **AVX2 当最低基线**
/// （x86-64 psABI v3：AVX2 + FMA + BMI1/2 + F16C）。老 CPU 上 Session 构造时会裸跑
/// AVX2 指令，抛 `0xC000001D`（STATUS_ILLEGAL_INSTRUCTION）被 Windows 直接干掉 ——
/// 现象是「点了某个功能程序瞬间消失、零日志、零对话框」，只在事件查看器里留一条
/// Application Error。2012~2013 的至强 E5 v1/v2（Sandy/Ivy Bridge）正是这一档，
/// 它们是廉价服务器里存量最大的一批。
/// ORT 的 MLAS 计算内核倒是有 CPUID 逐 ISA 分发，裸跑 AVX2 的只是 Eigen/protobuf/
/// 会话装配那部分非分发代码，所以不是「一定崩」—— 但崩起来没有任何现场可查。
/// 在建会话前拦一道，把「静默闪退」变成一句人能看懂的话。
pub fn cpu_supports_ort_baseline() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// 缺 AVX2 时给用户看的一句话。
pub const CPU_BASELINE_HINT: &str = "当前 CPU 不支持 AVX2 指令集。本程序自带的 ONNX Runtime \
    要求 Haswell（2013）及以后的处理器；至强 E5 v1/v2（Sandy/Ivy Bridge，2012~2013）不满足，\
    会直接导致程序闪退。请更换处理器，或联系我们提供不含 AVX2 的专用构建。";

/// 按设备偏好建会话。DirectML 不可用时**直接走 CPU**，不再注册必然失败的 EP。
pub fn build_session(env: &Environment, path: &Path, device: Device) -> OrtResult<Session> {
    if !cpu_supports_ort_baseline() {
        // 必须在任何 ORT C API 之前返回：Session 构造才是崩溃点，别让它走到那儿
        return Err(ort::Error::new(format!(
            "{}（{} 请求在 {:?} 上）",
            CPU_BASELINE_HINT,
            path.display(),
            device
        )));
    }
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
    // 先查指令集，再碰 DLL：`directml_available()` 会调进 onnxruntime.dll，
    // 老 CPU 上任何一次进入都不值得冒险。
    if !cpu_supports_ort_baseline() {
        return Err(ort::Error::new(format!(
            "{}（{}）",
            CPU_BASELINE_HINT,
            path.display()
        )));
    }
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
