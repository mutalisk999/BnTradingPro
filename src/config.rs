//! 应用配置：持久化到系统数据目录（Windows: %APPDATA%\BnTradingPro\config.toml）

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 代理配置（REST 与 WebSocket 均生效）
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub enabled: bool,
    /// 支持 http://host:port 与 socks5://[user:pass@]host:port
    pub url: String,
}

impl std::fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // url 可能内嵌 user:pass@：Debug 输出一律脱敏，
        // 防止 cfg 的 {:?} 进日志时带出代理口令（明文落盘 config.toml 是既定取舍）
        let redacted = match self.url.split_once("://") {
            Some((scheme, rest)) => {
                let host = rest.rsplit('@').next().unwrap_or("");
                format!("{scheme}//***@{host}")
            }
            None => self.url.clone(),
        };
        f.debug_struct("ProxyConfig")
            .field("enabled", &self.enabled)
            .field("url", &redacted)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// true = Binance 测试网（强烈建议默认，防误下单到实盘）
    pub testnet: bool,
    /// 默认交易对
    pub default_symbol: String,
    /// 默认 K 线周期（如 "15m"）
    pub default_interval: String,
    /// 签名请求 recvWindow（毫秒）
    pub recv_window_ms: u64,
    /// HTTP 超时（秒）
    pub http_timeout_secs: u64,
    /// 日志级别（trace/debug/info/warn/error）
    pub log_level: String,
    /// 界面语言（zh/en）
    pub lang: String,
    /// 主题（dark/light）
    pub theme_mode: String,
    /// 现货自选交易对（置顶显示）
    pub favorites_spot: Vec<String>,
    /// 合约自选交易对
    pub favorites_futures: Vec<String>,
    /// 端点覆盖：留空使用内置默认
    pub override_spot_rest: String,
    pub override_spot_ws: String,
    pub override_fut_rest: String,
    pub override_fut_ws: String,
    pub proxy: ProxyConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            testnet: true,
            default_symbol: "BTCUSDT".to_string(),
            default_interval: "15m".to_string(),
            recv_window_ms: 5000,
            http_timeout_secs: 15,
            log_level: "info".to_string(),
            lang: "zh".to_string(),
            theme_mode: "dark".to_string(),
            favorites_spot: Vec::new(),
            favorites_futures: Vec::new(),
            override_spot_rest: String::new(),
            override_spot_ws: String::new(),
            override_fut_rest: String::new(),
            override_fut_ws: String::new(),
            proxy: ProxyConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("解析配置失败({e})，使用默认配置: {}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, dir: &std::path::Path) {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("config.toml");
        if let Ok(s) = toml::to_string_pretty(self) {
            if let Err(e) = std::fs::write(&path, s) {
                tracing::error!("保存配置失败: {e}");
            }
        }
    }
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("BnTradingPro")
}
