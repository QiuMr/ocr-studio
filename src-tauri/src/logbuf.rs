//! 日志环形缓冲 + 实时转发。
//!
//! 后端所有 `tracing` 事件除了写文件，还会进这里：
//! - 环形缓冲保留最近 [`CAP`] 条，供前端首次进「运行日志」页时一次性拉取；
//! - 同时通过 broadcast 推给转发线程，由它批量 `emit` 成 Tauri 事件。
//!
//! 为什么不用「前端定时拉」：识别/找图是毫秒级的，轮询要么太密要么太慢。
//! 为什么要攒批：HTTP 服务每个请求都会打几条日志，一条一个事件会让 IPC 变成瓶颈。

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// 缓冲容量。日志页一次最多显示这么多，再多也没人看。
pub const CAP: usize = 2000;

/// 一条日志。字段名与前端 `types.ts` 的 `LogLine` 严格对应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// Unix 毫秒。前端负责格式化成当地时间
    pub ts_ms: u64,
    /// trace / debug / info / warn / error（小写）
    pub level: String,
    /// 产生日志的模块路径
    pub target: String,
    pub message: String,
}

static BUF: OnceLock<Mutex<VecDeque<LogLine>>> = OnceLock::new();
static SENDER: OnceLock<broadcast::Sender<LogLine>> = OnceLock::new();

fn buf() -> &'static Mutex<VecDeque<LogLine>> {
    BUF.get_or_init(|| Mutex::new(VecDeque::with_capacity(CAP)))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn push(line: LogLine) {
    let mut guard = buf().lock().unwrap_or_else(|e| e.into_inner());
    if guard.len() >= CAP {
        guard.pop_front();
    }
    guard.push_back(line);
}

/// 最近 [`CAP`] 条日志（旧的在前）。
pub fn snapshot() -> Vec<LogLine> {
    let guard = buf().lock().unwrap_or_else(|e| e.into_inner());
    guard.iter().cloned().collect()
}

/// broadcast 发送端。`init_layer` 之前调用会返回 None。
pub fn sender() -> Option<broadcast::Sender<LogLine>> {
    SENDER.get().cloned()
}

/// 只摘 `message` 字段，其它字段（字段名、文件行号）对用户没意义。
#[derive(Default)]
struct MsgVisitor {
    message: String,
}

impl Visit for MsgVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // `{:?}` 会带引号，去掉更接近用户直觉
            let s = format!("{value:?}");
            self.message = s.trim_matches('"').to_string();
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }
}

/// tracing Layer：把事件灌进缓冲与 broadcast。
pub struct BufferLayer {
    tx: broadcast::Sender<LogLine>,
}

impl BufferLayer {
    /// 建 layer 并登记全局 sender。**必须在 `tracing_subscriber` init 之前调用。**
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(1024);
        let _ = SENDER.set(tx.clone());
        Self { tx }
    }
}

impl Default for BufferLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Subscriber> Layer<S> for BufferLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MsgVisitor::default();
        event.record(&mut visitor);
        // 没有 message 字段的事件（纯 span 标记）跳过，前端显示空白行没意义
        if visitor.message.is_empty() {
            return;
        }
        let meta = event.metadata();
        let line = LogLine {
            ts_ms: now_ms(),
            level: meta.level().as_str().to_lowercase(),
            target: meta.target().to_string(),
            message: visitor.message,
        };
        push(line.clone());
        // 没有订阅者时 send 会返回 Err，属正常情况
        let _ = self.tx.send(line);
    }
}

/// 起一个线程，把 broadcast 里的日志攒批推给前端。
///
/// 事件名 `log-lines`，payload 是 `LogLine[]`。
pub fn spawn_forwarder(app: tauri::AppHandle) {
    use tauri::Emitter;

    let Some(tx) = sender() else {
        return;
    };
    let mut rx = tx.subscribe();

    std::thread::Builder::new()
        .name("log-forwarder".into())
        .spawn(move || {
            let mut batch: Vec<LogLine> = Vec::with_capacity(32);
            loop {
                match rx.blocking_recv() {
                    Ok(line) => {
                        batch.push(line);
                        // 把积压的一次取完，避免一条一个 IPC 往返
                        while batch.len() < 200 {
                            match rx.try_recv() {
                                Ok(l) => batch.push(l),
                                Err(_) => break,
                            }
                        }
                        // 前端还没加载/窗口已关都会失败，忽略即可
                        let _ = app.emit("log-lines", &batch);
                        batch.clear();
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // 消费不过来就丢，日志不值得反压业务
                        let _ = n;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .ok();
}
