//! HTTP 服务共享状态：引擎实例、配置、停机信号。
//!
//! 关键决策：
//! - 引擎放在 `std::sync::Mutex<Option<OcrEngine>>` 而不是 `tokio::sync::RwLock`：
//!   推理是阻塞式计算（几十到几百毫秒），用 `spawn_blocking` 跑更合适；
//!   `Option` 允许模型加载失败时服务照样起来，接口返回 503 而不是进程退出。
//!   **顺带这是 ONNX Runtime 多线程崩溃的官方推荐解法**：一个会话 + 互斥锁串行，
//!   Session 全程复用，线程数只由会话数决定、与并发请求数无关。
//! - 在途请求另有 `infer_slots` 信号量封顶（见该字段）：串行化只保证不崩，
//!   不保证内存有界 —— 排队期间每张已解码位图都活着。
//! - 配置用 `std::sync::RwLock`：临界区内没有 `.await`，无需异步锁。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tokio::sync::oneshot;

use crate::config::AppConfig;
use crate::engine::{
    load_engine, Capabilities, DetectOptions, Detection, EngineKind, EngineSpec, RecognizeOptions,
    TextLine, VisionEngine, VisionError,
};
use crate::ocr::{
    engine_from_dir_with, Device, HealthInfo, LineDetail, ModelLevel, OcrEngine,
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

/// 额外引擎槽（PP-OCR 之外）。
///
/// 只留一个槽是刻意的：这些模型动辄几十 MB（ddddocr common 54MB），
/// 同时驻留多个纯属浪费显存/内存。请求换引擎时先丢旧的再加载新的，
/// 行为等价于「单槽 LRU」。PP-OCR 自己仍走 `engine` 字段，
/// 因为它的配置（档位/设备/cls）与重载语义已经被现有代码和测试依赖。
struct AltEngine {
    spec: EngineSpec,
    /// 实际请求的设备。请求里换了设备也要重载——否则 `device=cpu` 会静默无效。
    device: Device,
    engine: Box<dyn VisionEngine>,
}

/// 请求要用的引擎：`None` 表示沿用当前 PP-OCR 配置。
///
/// `engine=ppocr:tiny` 这类写法也归到 `None`——它由 `level` 参数和时间已久
/// 的 `ensure_engine` 重载路径处理，没必要让同一个引擎走两条加载代码。
fn alt_spec_of(opts: &OcrOptions) -> Result<Option<EngineSpec>, ApiError> {
    let Some(raw) = opts.engine.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let spec = EngineSpec::parse(raw).map_err(|e| match e {
        VisionError::UnknownEngine(m) => ApiError::UnknownEngine(m),
        other => ApiError::BadRequest(other.to_string()),
    })?;
    if matches!(spec.kind, EngineKind::PpOcr) {
        return Ok(None);
    }
    Ok(Some(spec))
}

/// 把引擎层错误映射成 API 错误（`Unsupported`/`UnknownEngine` 带专用 code）。
pub fn map_engine_err(e: VisionError) -> ApiError {
    map_engine_err_ctx(e, None)
}

/// 同上，但可以给消息加一层上下文（例如「提示找图失败」）。
///
/// 为什么要单独有这个：`/vision/prompt` 的两种提示方式在**运行时**才可能发现
/// 「这套模型没带」——那时抛的是 `VisionError::Unsupported`，必须原样映射成
/// code 1002，不能像早期那样一律拍成 `Internal`（500）。脚本靠 1002 区分
/// 「引擎做不到，换条路」和「服务出故障了」，两者混在一起就没法自动降级。
pub fn map_engine_err_ctx(e: VisionError, ctx: Option<&str>) -> ApiError {
    let with = |m: String| match ctx {
        Some(c) => format!("{c}: {m}"),
        None => m,
    };
    match e {
        VisionError::Unsupported { capability, .. } => {
            ApiError::Unsupported(with(format!("该引擎不支持 {capability}")))
        }
        VisionError::UnknownEngine(m) => ApiError::UnknownEngine(with(m)),
        VisionError::BadRequest(m) => ApiError::BadRequest(with(m)),
        other => ApiError::Internal(with(other.to_string())),
    }
}

/// HTTP 服务共享状态。
pub struct AppState {
    pub models_root: PathBuf,
    /// 应用数据目录（配置存放处）
    pub data_dir: PathBuf,
    /// 引擎实例。`None` 表示尚未成功加载（接口会返回 503）。
    pub engine: Mutex<Option<OcrEngine>>,
    /// PP-OCR 之外的引擎（当前只会是 ddddocr），单槽。
    alt: Mutex<Option<AltEngine>>,
    /// 运行时提示引擎（YOLOE 开放词汇的图找图），单槽、按需加载。
    /// 检测器 + 视觉编码器几百 MB，不能跟主引擎一起常驻。
    prompt: Mutex<Option<crate::engine::prompt::PromptEngine>>,
    /// 中文视觉参考库（`<数据目录>/prompt_refs/`）。
    ///
    /// 独立于提示引擎：教一个新目标只是存图 + 写索引，不该为了加一条记录
    /// 就去加载检测器。`mode=ref` 查找时才把参考图送进视觉编码器。
    pub refs: Mutex<crate::engine::prompt_refs::RefStore>,
    /// AI 接口的限流窗口（最近一分钟内的调用时刻）。
    pub ai_rate: Mutex<std::collections::VecDeque<std::time::Instant>>,
    /// AI 接口的幂等缓存（`request_id` → 上次的响应）。
    pub ai_idem: Mutex<super::ai::AiIdem>,
    pub cfg: RwLock<AppConfig>,
    pub started_at: Instant,
    /// 停机信号发送端。`Some` 表示服务正在运行。
    pub shutdown: Mutex<Option<oneshot::Sender<()>>>,
    /// 在途推理请求的名额。拿不到就当场 429，不排队。
    ///
    /// 为什么需要：推理被 `engine` 互斥锁串行化，所以并发请求**只会排队** ——
    /// 而排队期间每张已解码的位图都活着（32MB 的 body 能解出 200MB 位图），
    /// 在途数不设上限时内存随并发线性上涨，直到被系统 OOM killer 干掉
    /// （现象：闪退、零日志）。实测 48 路并发 + 2400×1600 的图，工作集
    /// 从 204MB 涨到 493MB；200 路并发时光是排队就把最后一个请求拖到 52 秒，
    /// 客户端早超时了，白占内存。
    /// 放在 `Arc` 里是为了能 `acquire_owned()`：许可跟着请求走，
    /// 不用把 `&self` 的借用拖进 handler。
    pub infer_slots: Arc<tokio::sync::Semaphore>,
}

/// 在途推理请求的上限。
///
/// 32 的来历：推理是串行的，超过这个数只是排队；而每张在途位图按最坏情况
/// （`MAX_DECODE_PIXELS` = 3200 万像素 ≈ 96MB）算，32 路也就是 3GB 量级的上限，
/// 真实截图（1080p~4K，2~8M 像素）则只有几百 MB。
pub const MAX_INFLIGHT_INFER: usize = 32;

impl AppState {
    pub fn new(models_root: PathBuf, cfg: AppConfig) -> Self {
        Self::with_data_dir(models_root, cfg, PathBuf::from("."))
    }

    pub fn with_data_dir(models_root: PathBuf, cfg: AppConfig, data_dir: PathBuf) -> Self {
        // 先算好再搬 `data_dir`：struct 字面量里字段是按书写顺序求值的，
        // `data_dir` 一旦移动进去，后面就没法再借它拼路径了。
        let refs = crate::engine::prompt_refs::RefStore::load(&data_dir.join("prompt_refs"));
        let mut cfg = cfg;
        // 连接码「首次生成即落盘」：用户看到界面上那个码之后，它就必须稳定可用。
        if cfg.ensure_ai_token() {
            if let Err(e) = cfg.save(&data_dir) {
                tracing::warn!("AI 连接码没能落盘（下次启动会变）：{e}");
            }
        }
        Self {
            models_root,
            data_dir,
            engine: Mutex::new(None),
            alt: Mutex::new(None),
            prompt: Mutex::new(None),
            refs: Mutex::new(refs),
            cfg: RwLock::new(cfg),
            started_at: Instant::now(),
            ai_rate: Mutex::new(std::collections::VecDeque::new()),
            ai_idem: Mutex::new(super::ai::AiIdem::default()),
            shutdown: Mutex::new(None),
            infer_slots: Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT_INFER)),
        }
    }

    /// 拿一个在途推理名额。
    ///
    /// **不排队**：`try_acquire` 拿不到就立刻返回 429 + `Retry-After`。
    /// 排队看起来「客气」，实际上是把客户端的超时和自己的内存一起耗光。
    /// 许可由调用方持有到推理结束（`OwnedSemaphorePermit` drop 时自动释放）。
    pub fn acquire_infer_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        match self.infer_slots.clone().try_acquire_owned() {
            Ok(permit) => Ok(permit),
            Err(_) => Err(ApiError::TooManyRequests(format!(
                "在途推理请求已达上限 {MAX_INFLIGHT_INFER}，请稍后重试（响应带 Retry-After）。\
                 推理是逐张串行处理的，继续加并发只会让队列更长、不会更快。"
            ))),
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

    /// 确保请求指定的额外引擎就绪。换引擎/换设备时丢弃上一个（单槽）。
    pub async fn ensure_alt_engine(
        self: &Arc<Self>,
        spec: EngineSpec,
        device: Option<Device>,
    ) -> Result<(), ApiError> {
        let need_load = {
            let guard = self.alt.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(a) => a.spec != spec || device.is_some_and(|d| d != a.device),
                None => true,
            }
        };
        if !need_load {
            return Ok(());
        }
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || st.load_alt_blocking(spec, device))
            .await
            .map_err(|e| ApiError::Internal(format!("引擎加载任务崩溃: {e}")))?
            .map_err(ApiError::EngineNotReady)
    }

    fn load_alt_blocking(
        &self,
        spec: EngineSpec,
        device: Option<Device>,
    ) -> Result<(), String> {
        let cfg = self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
        let device = device.unwrap_or(cfg.device);
        let eng = load_engine(
            &spec,
            &self.models_root,
            cfg.level,
            device,
            true,
            &cfg.detect_models,
        )
        .map_err(|e| format!("加载 {spec} 失败: {e}"))?;
        let mut guard = self.alt.lock().unwrap_or_else(|e| e.into_inner());
        // 先赋新值再让旧的 drop：旧引擎的释放（几十 MB）在锁内完成，
        // 但这是单槽设计下唯一安全的位置——放到锁外可能出现两个实例同时活着。
        *guard = Some(AltEngine {
            spec,
            device,
            engine: eng,
        });
        Ok(())
    }

    /// 在阻塞线程里跑 PP-OCR。走 trait 入口，与 `recognize_detailed` 等价。
    async fn run_pp(
        self: &Arc<Self>,
        img: image::DynamicImage,
        want_chars: bool,
    ) -> Result<Vec<TextLine>, ApiError> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.engine.lock().unwrap_or_else(|e| e.into_inner());
            let eng = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
            eng.infer(&img, &RecognizeOptions { want_chars })
                .map_err(|e| map_engine_err_ctx(e, Some("推理失败")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("推理任务崩溃: {e}")))?
    }

    /// 在阻塞线程里跑额外引擎。
    async fn run_alt(
        self: &Arc<Self>,
        img: image::DynamicImage,
        want_chars: bool,
    ) -> Result<Vec<TextLine>, ApiError> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.alt.lock().unwrap_or_else(|e| e.into_inner());
            let alt = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
            alt.engine
                .infer(&img, &RecognizeOptions { want_chars })
                .map_err(|e| map_engine_err_ctx(e, Some("推理失败")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("推理任务崩溃: {e}")))?
    }

    /// 运行时提示找图（图找图 / 按名字找）。
    ///
    /// 引擎按需加载、单槽常驻：文本塔是 250MB 级的权重，不能每次请求都读盘；
    /// 但也不跟主 OCR 引擎一起加载——不用这个功能的人不该付这份内存。
    pub async fn prompt_find(
        self: &Arc<Self>,
        input: crate::engine::prompt::PromptInput,
        target: image::DynamicImage,
        opts: DetectOptions,
    ) -> Result<Vec<Detection>, ApiError> {
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let device = st.cfg.read().unwrap_or_else(|e| e.into_inner()).device;
            let mut guard = st.prompt.lock().unwrap_or_else(|e| e.into_inner());
            if guard.is_none() {
                let eng = crate::engine::prompt::PromptEngine::load(&st.models_root, device)
                    .map_err(|e| ApiError::EngineNotReady(format!("提示引擎加载失败: {e}")))?;
                *guard = Some(eng);
            }
            let eng = guard.as_mut().expect("上面刚赋值过");
            // 参考库的锁要在调用期间一直拿着：参考图路径是从索引里读的，
            // 中途被删掉会读不到文件。它和 prompt 是两把不同的锁，不会互等。
            let refs = st.refs.lock().unwrap_or_else(|e| e.into_inner());
            // 走 map_engine_err_ctx 而不是直接 Internal：缺 MobileCLIP2-B 权重 /
            // 缺视觉编码器时抛的是 Unsupported，必须映射成 1002 而不是 500。
            eng.run(&target, &input, &opts, &refs)
                .map_err(|e| map_engine_err_ctx(e, Some("提示找图失败")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("提示找图任务崩溃: {e}")))?
    }

    /// 已加载的提示引擎的说明（给 `/engine/list` 与状态页看能力差异）。
    pub fn prompt_loaded_note(&self) -> Option<String> {
        let guard = self.prompt.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|p| p.describe())
    }

    /// 统一识别入口：按 `opts.engine` 选引擎，返回文本行。
    ///
    /// 未指定 `engine`（或指定 `ppocr:*`）时走原来的 PP-OCR 路径，
    /// 参数语义、重载规则、精度全部不变。
    pub async fn recognize_lines(
        self: &Arc<Self>,
        opts: &OcrOptions,
        img: image::DynamicImage,
    ) -> Result<Vec<TextLine>, ApiError> {
        match alt_spec_of(opts)? {
            None => {
                self.ensure_engine(opts).await?;
                self.run_pp(img, false).await
            }
            Some(spec) => {
                let dev = opts.device.as_deref().and_then(parse_device);
                self.ensure_alt_engine(spec, dev).await?;
                self.run_alt(img, false).await
            }
        }
    }

    /// 同上，但带逐字框（`/ocr/danzijson`、`ocr_danzijson` 用）。
    pub async fn recognize_details(
        self: &Arc<Self>,
        opts: &OcrOptions,
        img: image::DynamicImage,
    ) -> Result<Vec<LineDetail>, ApiError> {
        let lines = match alt_spec_of(opts)? {
            None => {
                self.ensure_engine(opts).await?;
                self.run_pp(img, true).await?
            }
            Some(spec) => {
                let dev = opts.device.as_deref().and_then(parse_device);
                self.ensure_alt_engine(spec, dev).await?;
                self.run_alt(img, true).await?
            }
        };
        Ok(lines.into_iter().map(LineDetail::from).collect())
    }

    /// 当前额外引擎的标识、真实设备与**加载后才知道的能力**。
    ///
    /// 带 capabilities 是为了让 `/engine/list` 能回填类名：检测模型的类别名写在
    /// ONNX metadata 里，扫描阶段不加载模型（几秒 + 吃显存）就读不到，
    /// 所以「加载过一次」的引擎要把类名补回列表，前端才能显示「共几类」。
    pub fn alt_loaded(&self) -> Option<(EngineSpec, Option<Device>, Capabilities)> {
        let guard = self.alt.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|a| {
            let st = a.engine.status();
            (a.spec.clone(), st.actual_device, st.capabilities)
        })
    }

    /// 扫描引擎 + 回填「已加载」信息（真实设备、加载后才知道的类名）。
    ///
    /// `/engine/list` 与 Tauri `list_engines` **共用这一份**：两份实现很容易
    /// 只改一处，前端就会出现「HTTP 说有 GPU、IPC 说没有」这类难查的不一致。
    pub fn engine_status_list(&self) -> Vec<crate::engine::EngineStatus> {
        let cfg = self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone();
        let mut engines =
            crate::engine::catalog::discover(&self.models_root, &cfg.detect_models);

        // PP-OCR：设备从已加载实例读
        if let Some(info) = self.health() {
            let key = EngineSpec::ppocr(info.level).key();
            for e in engines.iter_mut() {
                if e.spec.key() == key {
                    e.actual_device = Some(info.actual_device);
                }
            }
        }
        // 额外引擎（ddddocr / yolo）：设备 + 类名
        if let Some((spec, device, caps)) = self.alt_loaded() {
            for e in engines.iter_mut() {
                if e.spec != spec {
                    continue;
                }
                e.actual_device = device;
                if caps.classes.is_empty() {
                    continue;
                }
                // 类名写在 ONNX metadata 里，扫描阶段（不加载模型）读不到；
                // 加载过一次就把结果补进列表并改掉 note 里的「待读取」说法
                e.capabilities.classes = caps.classes.clone();
                e.capabilities.detect = caps.detect;
                if let Some(note) = e.note.as_mut() {
                    if !note.contains("已加载") {
                        // 把「类名要等加载才知道」的说法换掉——现在知道了。
                        // 留着会和后面的「已加载 N 类」互相打脸。
                        let cleaned = note
                            .replace("（类名在加载时从模型元数据读取）", "")
                            .replace(
                                &format!("（类名来自标签文件，共 {} 个）", caps.classes.len()),
                                "",
                            );
                        *note = format!("{cleaned} · 已加载 {} 类", caps.classes.len());
                    }
                }
            }
        }
        engines
    }

    /// 通用目标检测入口（`/vision/detect`）。
    ///
    /// 引擎能力差异由 trait 默认实现兜底：OCR 类引擎返回 `Unsupported`（code 1002），
    /// 这里不做提前判断——`capabilities()` 才是给前端看的，路由层保持薄。
    pub async fn detect_objects(
        self: &Arc<Self>,
        opts: &OcrOptions,
        img: image::DynamicImage,
        detect: DetectOptions,
    ) -> Result<Vec<Detection>, ApiError> {
        self.detect_with(opts, img, detect, None).await
    }

    /// 检测的公共实现。`spec_override` 给 pipeline 用（显式指定检测引擎）。
    async fn detect_with(
        self: &Arc<Self>,
        opts: &OcrOptions,
        img: image::DynamicImage,
        detect: DetectOptions,
        spec_override: Option<EngineSpec>,
    ) -> Result<Vec<Detection>, ApiError> {
        let spec = match spec_override {
            Some(s) => s,
            None => match alt_spec_of(opts)? {
                // 缺省 = PP-OCR（会走 1002），与 /vision/detect 的语义一致
                None => {
                    self.ensure_engine(opts).await?;
                    let st = Arc::clone(self);
                    return tokio::task::spawn_blocking(move || {
                        let mut guard = st.engine.lock().unwrap_or_else(|e| e.into_inner());
                        let eng = guard
                            .as_mut()
                            .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
                        eng.detect(&img, &detect).map_err(map_engine_err)
                    })
                    .await
                    .map_err(|e| ApiError::Internal(format!("检测任务崩溃: {e}")))?;
                }
                Some(s) => s,
            },
        };
        let dev = opts.device.as_deref().and_then(parse_device);
        self.ensure_alt_engine(spec, dev).await?;
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.alt.lock().unwrap_or_else(|e| e.into_inner());
            let alt = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;
            alt.engine.detect(&img, &detect).map_err(map_engine_err)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("检测任务崩溃: {e}")))?
    }

    /// 检测 + 识别串联（`/vision/pipeline`）：检测引擎找区域，PP-OCR 在每个
    /// 区域内认字。
    ///
    /// 两把引擎锁**顺序持有，绝不嵌套**：detect 拿 alt 锁、识别拿 engine 锁，
    /// 分别在不同的 spawn_blocking 里完成，避免锁序问题。
    ///
    /// 验收口径（重要）：COCO 类别与「文本区域」基本不对齐——用 yolov8n+coco
    /// 跑文档截图，检出的多半是人/车而不是文字块，这是**预期行为**。pipeline
    /// 面向的是自训检测模型（例如「按钮/标题/表格」检测器），COCO 图只用来
    /// 验证链路通不通。
    pub async fn run_pipeline(
        self: &Arc<Self>,
        opts: &OcrOptions,
        img: image::DynamicImage,
        detect: DetectOptions,
    ) -> Result<Vec<(Detection, Vec<TextLine>)>, ApiError> {
        use crate::engine::EngineKind;
        use image::GenericImageView;

        // 检测引擎：显式指定 > 请求 engine 字段 > 默认（唯一的目标检测引擎）
        let detect_spec = match opts
            .detect_engine
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => EngineSpec::parse(raw).map_err(|e| match e {
                VisionError::UnknownEngine(m) => ApiError::UnknownEngine(m),
                other => ApiError::BadRequest(other.to_string()),
            })?,
            None => match alt_spec_of(opts)? {
                Some(s) if matches!(s.kind, EngineKind::Yolo) => s,
                _ => EngineSpec::yolo(),
            },
        };
        if matches!(detect_spec.kind, EngineKind::PpOcr | EngineKind::DdddOcr) {
            return Err(ApiError::Unsupported(
                "pipeline 的检测引擎必须是目标检测引擎（OCR 引擎不会找目标区域）".to_string(),
            ));
        }

        // 1) 检测
        let dets = self
            .detect_with(opts, img.clone(), detect, Some(detect_spec))
            .await?;

        // 2) PP-OCR 识别（区域为空就直接返回，别为了空结果加载 OCR）
        if dets.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_engine(opts).await?;
        let st = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut guard = st.engine.lock().unwrap_or_else(|e| e.into_inner());
            let eng = guard
                .as_mut()
                .ok_or_else(|| ApiError::EngineNotReady("引擎未加载".to_string()))?;

            let (w, h) = img.dimensions();
            let mut out = Vec::with_capacity(dets.len());
            for d in dets {
                // 轴对齐裁剪，钳到图内；退化区域直接跳过
                let x0 = d.points[0].0.floor().max(0.0) as u32;
                let y0 = d.points[0].1.floor().max(0.0) as u32;
                let x1 = (d.points[2].0.ceil() as u32).min(w);
                let y1 = (d.points[2].1.ceil() as u32).min(h);
                if x1 <= x0 + 2 || y1 <= y0 + 2 {
                    continue;
                }
                let crop = img.crop_imm(x0, y0, x1 - x0, y1 - y0);
                let lines = match eng.infer(
                    &crop,
                    &RecognizeOptions { want_chars: false },
                ) {
                    Ok(v) => v,
                    Err(e) => {
                        // 单区域失败不该拖垮整张图：记日志，该区域标记为无文字
                        tracing::warn!("pipeline 区域识别失败（{:?}）：{e}", d.class_name);
                        Vec::new()
                    }
                };
                out.push((d, lines));
            }
            Ok(out)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("pipeline 任务崩溃: {e}")))?
    }

    /// 按 spec 立即加载额外引擎（Tauri 命令用，不经过请求参数）。
    pub async fn load_alt(
        self: &Arc<Self>,
        spec: EngineSpec,
        device: Option<Device>,
    ) -> Result<(), ApiError> {
        self.ensure_alt_engine(spec, device).await
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 并发闸门：名额用光时必须**当场 429**，不能排队。
    ///
    /// 排队看着客气，实际是把客户端的超时和自己的内存一起耗光 —— 实测 200 路并发
    /// 无闸门时最后一个要等 52 秒，客户端早超时了，位图还白占着。
    #[test]
    fn infer_slot_gate_fails_fast_instead_of_queueing() {
        let st = AppState::new(PathBuf::from("."), AppConfig::default());
        // 先把名额全部占住
        let mut held: Vec<_> = (0..MAX_INFLIGHT_INFER)
            .map(|_| st.acquire_infer_slot().expect("名额不该用完"))
            .collect();
        assert_eq!(held.len(), MAX_INFLIGHT_INFER);

        let e = st.acquire_infer_slot().unwrap_err();
        assert_eq!(e.code(), crate::api::dto::CODE_TOO_MANY_REQUESTS);
        assert_eq!(e.code(), 429);
        // 消息必须说清「稍后重试」，不能让人以为服务坏了
        assert!(e.message().contains("请稍后重试"));

        // 还回去一个，立刻又能拿到（不能有泄漏）
        drop(held.pop());
        assert!(st.acquire_infer_slot().is_ok());
    }

    /// 引擎层错误 → API 错误的映射，必须逐档对上，不能一律拍成 500。
    ///
    /// 这条曾经真的错：`/vision/prompt` 缺 MobileCLIP2-B 权重时返回的是 500
    /// 而不是 1002，脚本没法区分「这个引擎做不到」和「服务出故障了」。
    /// 打包验收（`scripts/verify_packaged.py`）跑出来的就是这个。
    #[test]
    fn engine_error_mapping_keeps_special_codes() {
        let unsupported = map_engine_err(VisionError::Unsupported {
            engine: "yoloe-prompt".into(),
            capability: "text（缺 MobileCLIP2-B 权重 / 词表 / 提示头）",
        });
        assert_eq!(unsupported.code(), 1002);
        assert!(unsupported.message().contains("MobileCLIP2-B"));

        assert_eq!(
            map_engine_err(VisionError::UnknownEngine("nope".into())).code(),
            1003
        );
        assert_eq!(
            map_engine_err(VisionError::BadRequest("参数不对".into())).code(),
            400
        );
        assert_eq!(
            map_engine_err(VisionError::Inference("炸了".into())).code(),
            500
        );
        assert_eq!(
            map_engine_err(VisionError::NotReady("模型没加载".into())).code(),
            500
        );
    }

    /// 带上下文的版本：code 不变，消息前面多一层「提示找图失败」。
    #[test]
    fn engine_error_mapping_ctx_prefixes_message_only() {
        let e = map_engine_err_ctx(
            VisionError::Unsupported {
                engine: "yoloe-prompt".into(),
                capability: "visual（缺 *_vpe.onnx）",
            },
            Some("提示找图失败"),
        );
        assert_eq!(e.code(), 1002);
        assert!(e.message().starts_with("提示找图失败: "), "{}", e.message());
        assert!(e.message().contains("visual"), "{}", e.message());
    }
}
