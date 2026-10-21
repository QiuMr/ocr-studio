//! HTTP 服务共享状态：引擎实例、配置、停机信号。
//!
//! 关键决策：
//! - 引擎放在 `std::sync::Mutex<Option<OcrEngine>>` 而不是 `tokio::sync::RwLock`：
//!   推理是阻塞式计算（几十到几百毫秒），用 `spawn_blocking` 跑更合适；
//!   `Option` 允许模型加载失败时服务照样起来，接口返回 503 而不是进程退出。
//! - 配置用 `std::sync::RwLock`：临界区内没有 `.await`，无需异步锁。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tokio::sync::oneshot;

use crate::config::AppConfig;
use crate::ocr::{
    engine_from_dir_with, Device, HealthInfo, LineDetail, ModelLevel, OcrEngine, TextBox,
};

use super::dto::{ApiError, OcrOptions};

/// 仓库根目录（`src-tauri` 的上一级）。
///
/// `CARGO_MANIFEST_DIR` 是编译期常量，指向 lib crate 所在目录 `src-tauri`，
/// 因此其父目录就是仓库根。这比运行时猜 exe 位置可靠。
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 解析模型根目录（其下应有 tiny/small/medium 子目录）。
///
/// 优先级：环境变量 `OCR_MODELS_ROOT` → **exe 旁的 models** → 编译期仓库根
/// → exe 向上 1~4 级 → 当前工作目录。逐个校验 `small/det.onnx` 存在，
/// 避免猜错目录后报一堆「找不到模型」的误导性错误。
///
/// 为什么 exe 旁要排在仓库根之前：免安装分发时用户解压出来就是
/// `OCR Studio.exe + models/`，那才是**这份拷贝**该用的模型。若让编译期常量
/// `CARGO_MANIFEST_DIR` 抢先，开发机上会把分发包偷偷指回源码目录的 models，
/// 测起来一切正常、到别人机器上就找不到——典型的「只在本机能跑」。
pub fn resolve_models_root() -> PathBuf {
    if let Ok(p) = std::env::var("OCR_MODELS_ROOT") {
        if !p.trim().is_empty() {
            return PathBuf::from(p.trim());
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("models"));
            // 打包/开发时 exe 在 target/{debug,release}，模型在仓库根的上两级
            for up in 1..=4 {
                let mut p = dir.to_path_buf();
                for _ in 0..up {
                    p = p.join("..");
                }
                candidates.push(p.join("models"));
            }
        }
    }

    candidates.push(workspace_root().join("models"));
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("models"));
    }

    for c in &candidates {
        if c.join("small").join("det.onnx").exists() {
            return c.clone();
        }
    }
    // 都找不到：返回第一个候选，让加载阶段报出具体路径
    candidates.into_iter().next().unwrap_or_default()
}

/// 解析设备参数。容忍 `gpu` 作为 `directml` 的别名。
fn parse_device(s: &str) -> Option<Device> {
    match s.trim().to_ascii_lowercase().as_str() {
        "directml" | "gpu" => Some(Device::Directml),
        "cpu" => Some(Device::Cpu),
        _ => None,
    }
}

/// HTTP 服务共享状态。
pub struct AppState {
    pub models_root: PathBuf,
    /// 应用数据目录（配置存放处）
    pub data_dir: PathBuf,
    /// 引擎实例。`None` 表示尚未成功加载（接口会返回 503）。
    pub engine: Mutex<Option<OcrEngine>>,
    pub cfg: RwLock<AppConfig>,
    pub started_at: Instant,
    /// 停机信号发送端。`Some` 表示服务正在运行。
    pub shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl AppState {
    pub fn new(models_root: PathBuf, cfg: AppConfig) -> Self {
        Self::with_data_dir(models_root, cfg, PathBuf::from("."))
    }

    pub fn with_data_dir(models_root: PathBuf, cfg: AppConfig, data_dir: PathBuf) -> Self {
        Self {
            models_root,
            data_dir,
            engine: Mutex::new(None),
            cfg: RwLock::new(cfg),
            started_at: Instant::now(),
            shutdown: Mutex::new(None),
        }
    }

    pub fn port(&self) -> u16 {
        self.cfg
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .port
    }

    /// HTTP 服务是否正在运行。
    pub fn is_running(&self) -> bool {
        self.shutdown
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// 阻塞式加载/重载引擎。加载耗时以秒计，调用方应放在 `spawn_blocking` 里。
    pub fn load_engine_blocking(
        &self,
        level: Option<ModelLevel>,
        device: Option<Device>,
        use_cls: Option<bool>,
    ) -> Result<HealthInfo, String> {
        let base = self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
        let level = level.unwrap_or(base.level);
        let device = device.unwrap_or(base.device);
        // cls 开关缺省跟随 EngineConfig 默认值（true）
        let use_cls = use_cls.unwrap_or(true);

        let eng = engine_from_dir_with(&self.models_root, level, device, use_cls)
            .map_err(|e| format!("模型加载失败（{}）: {e}", self.models_root.display()))?;
        let health = eng.health();

        let mut guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(eng);
        Ok(health)
    }

    /// 按请求参数确保引擎就绪；参数与当前引擎不一致时重载。
    pub async fn ensure_engine(self: &Arc<Self>, opts: &OcrOptions) -> Result<(), ApiError> {
        let want_level = opts.level.as_deref().and_then(ModelLevel::from_str);
        let want_device = opts.device.as_deref().and_then(parse_device);
        let want_cls = opts.use_cls;

        let need_reload = {
            let guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                None => true,
                Some(e) => {
                    let c = e.config();
                    want_level.is_some_and(|l| l != c.level)
                        || want_device.is_some_and(|d| d != c.device)
                        || want_cls.is_some_and(|u| u != c.use_cls)
                }
            }
        };

        if need_reload {
            let st = Arc::clone(self);
            tokio::task::spawn_blocking(move || st.load_engine_blocking(want_level, want_device, want_cls))
                .await
                .map_err(|e| ApiError::Internal(format!("模型加载任务崩溃: {e}")))?
                .map_err(ApiError::EngineNotReady)?;
        }
        Ok(())
    }

    /// 在阻塞线程里执行识别，避免占住 tokio worker。
    pub async fn run_recognize(
        self: &Arc<Self>,
        img: image::DynamicImage,
    ) -> Result<Vec<TextBox>, ApiError> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.engine.lock().unwrap_or_else(|e| e.into_inner());
            let eng = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
            eng.recognize(&img)
                .map_err(|e| ApiError::Internal(format!("推理失败: {e}")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("推理任务崩溃: {e}")))?
    }

    /// 识别并返回逐字框（`/ocr/danzijson` 用）。
    pub async fn run_recognize_detailed(
        self: &Arc<Self>,
        img: image::DynamicImage,
    ) -> Result<Vec<LineDetail>, ApiError> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.engine.lock().unwrap_or_else(|e| e.into_inner());
            let eng = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
            eng.recognize_detailed(&img)
                .map_err(|e| ApiError::Internal(format!("推理失败: {e}")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("推理任务崩溃: {e}")))?
    }

    /// 读取引擎健康信息；未加载时返回 `None`。
    pub fn health(&self) -> Option<HealthInfo> {
        let guard = self.engine.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|e| e.health())
    }

    /// 在阻塞线程里重载引擎（供 Tauri 命令用，避免卡住 async worker）。
    pub async fn reload_engine(
        self: &Arc<Self>,
        level: Option<ModelLevel>,
        device: Option<Device>,
        use_cls: Option<bool>,
    ) -> Result<HealthInfo, String> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || st.load_engine_blocking(level, device, use_cls))
            .await
            .map_err(|e| format!("模型加载任务崩溃: {e}"))?
    }
}
