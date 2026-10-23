//! 配置加载与保存
//!
//! 配置存放于应用数据目录下的 `config.json`：
//! - 模型档位（tiny / small / medium）
//! - 推理设备（directml / cpu）
//! - HTTP 服务端口
//! - 模板匹配默认阈值

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ModelLevel {
    Tiny,
    Small,
    Medium,
}

impl Default for ModelLevel {
    fn default() -> Self {
        ModelLevel::Small
    }
}

impl ModelLevel {
    /// 从字符串解析档位，大小写不敏感。
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "tiny" => Some(ModelLevel::Tiny),
            "small" => Some(ModelLevel::Small),
            "medium" => Some(ModelLevel::Medium),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Directml,
    Cpu,
}

impl Default for Device {
    fn default() -> Self {
        Device::Directml
    }
}

impl Device {
    /// 从字符串解析设备，大小写不敏感。
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "directml" | "gpu" => Some(Device::Directml),
            "cpu" => Some(Device::Cpu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub level: ModelLevel,
    pub device: Device,
    pub port: u16,
    pub template_threshold: f32,
    /// 兼容渲染模式：给 WebView2 加 `--disable-gpu-compositing`。
    ///
    /// 部分机器（典型是较老的 AMD/Intel 核显驱动）上 WebView2 的 GPU 进程会
    /// 启动即崩，Chromium 判定 "GPU process isn't usable" 后直接终止浏览器进程，
    /// 表现为**白窗 + msedgewebview2 has stopped working**。开启本项改走软件
    /// 合成，界面恢复正常（OCR 推理不受影响，仍在 DirectML/CPU 上跑）。
    /// 首次崩溃后会自动置为 true 并持久化，无需用户手动处理。
    pub webview_compat: bool,
    /// 用户自己加的检测模型：onnx 的**绝对路径**，可以是任意目录。
    ///
    /// 目标检测模型天然是「用户自己的」——不同人训的模型放在哪都有可能，
    /// 要求他们把文件搬进 `models/` 不现实，所以这里只记路径、不复制文件
    /// （模型动辄几十上百 MB，复制一份纯属浪费磁盘）。
    /// 目录 `models/detect/yolo/` 下扫到的模型不需要记在这里。
    pub detect_models: Vec<String>,
    /// 允许浏览器页面调用（CORS）。
    ///
    /// 默认 **false**：这些接口能读本机屏幕（`/ocr/pingmuzhaotu*`），
    /// 默认放行任意网页等于把「截屏 + 识别」交给任何打开着的网页去驱动。
    /// 需要在本地 HTML / Electron 页面里 fetch 时才打开。
    pub allow_browser: bool,
    /// 是否开放 AI 操控接口（`/ai/v1/*`）。
    ///
    /// 默认 **false**：这是会改变程序行为的对外接口，一律让用户显式打开。
    /// 只绑回环地址，所以能调它的只有跑在本机的 AI。
    pub ai_enabled: bool,
    /// AI 接口的连接码（`Authorization: Bearer <码>`）。
    ///
    /// 首次启动生成并落盘；用户可在设置页重置（重置后旧码立刻失效）。
    pub ai_token: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            level: ModelLevel::default(),
            device: Device::default(),
            port: 8848,
            template_threshold: 0.9,
            webview_compat: false,
            detect_models: Vec::new(),
            allow_browser: false,
            ai_enabled: false,
            ai_token: String::new(),
        }
    }
}

impl AppConfig {
    /// 确保 AI 连接码存在（没有就生成一个）。返回是否变过（变过要落盘）。
    ///
    /// 为什么在加载时补而不是用到再生成：用户一旦看到界面上的连接码，
    /// 它就必须是稳定能用的 —— 每次启动换一个，粘给 AI 的那份立刻作废。
    pub fn ensure_ai_token(&mut self) -> bool {
        if self.ai_token.trim().is_empty() {
            self.ai_token = crate::api::ai::new_ai_token();
            return true;
        }
        false
    }
}

impl AppConfig {
    /// 配置文件路径：<应用数据目录>/config.json
    pub fn path(app_data_dir: &std::path::Path) -> PathBuf {
        app_data_dir.join("config.json")
    }

    /// 读取配置，不存在或解析失败时返回默认配置
    pub fn load(app_data_dir: &std::path::Path) -> Self {
        let path = Self::path(app_data_dir);
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("配置文件解析失败，使用默认值: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// 保存配置（原子写）
    pub fn save(&self, app_data_dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(app_data_dir)?;
        let path = Self::path(app_data_dir);
        let tmp = path.with_extension("json.tmp");
        let s = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}
