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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub level: ModelLevel,
    pub device: Device,
    pub port: u16,
    pub template_threshold: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            level: ModelLevel::default(),
            device: Device::default(),
            port: 8848,
            template_threshold: 0.9,
        }
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
