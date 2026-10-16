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
            // 无 scheme 但内嵌 userinfo 的写法同样脱敏
            None if self.url.contains('@') => {
                format!("***@{}", self.url.rsplit('@').next().unwrap_or(""))
            }
            None => self.url.clone(),
        };
        f.debug_struct("ProxyConfig")
            .field("enabled", &self.enabled)
            .field("url", &redacted)
            .finish()
    }
}

/// 本地价格预警（客户端自行判断触发，不依赖交易所条件单）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlertConfig {
    /// 0 = 现货，1 = U 本位合约（与行情 tickers 槽位一致）
    pub market: u8,
    pub symbol: String,
    /// 目标价（Decimal 精确值，旧版 config.toml 以 f64 形式落盘，兼容读入）
    #[serde(default, deserialize_with = "dec_compat")]
    pub price: rust_decimal::Decimal,
    /// true = 上破触发（最新价 ≥ 目标价），false = 下破触发（≤）
    pub above: bool,
}

/// Decimal 反序列化兼容层：字符串（新版写入形式）、f64/i64（旧版 config.toml
/// 的 `price = 86997.7` 形式）都可读入，f64 走字符串中转避免吸入双精度残差
fn dec_compat<'de, D: serde::Deserializer<'de>>(d: D) -> Result<rust_decimal::Decimal, D::Error> {
    use std::str::FromStr;

    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = rust_decimal::Decimal;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "a price as string or number")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<rust_decimal::Decimal, E> {
            rust_decimal::Decimal::from_str(v).map_err(E::custom)
        }

        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<rust_decimal::Decimal, E> {
            rust_decimal::Decimal::from_str(&format!("{v}")).map_err(E::custom)
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<rust_decimal::Decimal, E> {
            Ok(rust_decimal::Decimal::from(v))
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<rust_decimal::Decimal, E> {
            Ok(rust_decimal::Decimal::from(v))
        }
    }

    d.deserialize_any(V)
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
    /// 涨跌颜色方向：green_up=绿涨红跌（默认），red_up=红涨绿跌
    pub price_dir: String,
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
    /// 本地价格预警列表
    pub alerts: Vec<AlertConfig>,
    /// 合约下单走 WS-API 通道（失败自动回退 REST；默认关闭）
    pub ws_order: bool,
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
            price_dir: "green_up".to_string(),
            favorites_spot: Vec::new(),
            favorites_futures: Vec::new(),
            override_spot_rest: String::new(),
            override_spot_ws: String::new(),
            override_fut_rest: String::new(),
            override_fut_ws: String::new(),
            proxy: ProxyConfig::default(),
            alerts: Vec::new(),
            ws_order: false,
        }
    }
}

impl AppConfig {
    pub fn load(dir: &std::path::Path) -> Self {
        let path = dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                // 解析失败回退默认前，先把损坏文件改名备份：否则下一次
                // PersistConfig 会用默认值直接覆盖，用户的自选列表/端点配置
                // 将永久丢失且无从恢复
                let bak = dir.join("config.toml.bak");
                match std::fs::rename(&path, &bak) {
                    Ok(()) => tracing::warn!(
                        "解析配置失败({e})，原文件已备份为 {}，使用默认配置",
                        bak.display()
                    ),
                    Err(be) => tracing::warn!(
                        "解析配置失败({e})，且备份原文件失败({be})，使用默认配置"
                    ),
                }
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// 落盘失败返回错误描述（路径 + 原因）：UI 据此向用户说明，
    /// 而不是无条件宣称"已保存"——磁盘满/权限拒绝时配置实际并未写入
    pub fn save(&self, dir: &std::path::Path) -> Result<(), String> {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("config.toml");
        let s = toml::to_string_pretty(self).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Err(e) = std::fs::write(&path, s) {
            return Err(format!("{}: {e}", path.display()));
        }
        // unix 下收紧权限：config.toml 虽无密钥，但保持与凭据库同等的 0600 纪律
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

/// 应用数据目录（Windows: %APPDATA%\BnTradingPro）。
/// 会话级不变：首次调用推导一次（dirs 查询 %APPDATA% 环境变量并分配
/// PathBuf），之后命中 OnceLock——设置/配钥页每帧调用，裸推导即每帧
/// 一次环境变量查询。不回退 temp_dir：/tmp 全员可读，fail fast 优于
/// 静默降级到不安全位置
pub fn data_dir() -> &'static std::path::Path {
    static DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DATA_DIR.get_or_init(|| {
        dirs::data_dir()
            .expect("无法确定应用数据目录（请检查 HOME/XDG_DATA_HOME 环境变量）")
            .join("BnTradingPro")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "btp_config_test_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// save 成功后 load 必须原样读回：Result 契约的 happy path
    #[test]
    fn save_roundtrips_through_load() {
        let dir = tmp_dir("roundtrip");
        let cfg = AppConfig {
            testnet: false,
            default_symbol: "ETHUSDT".into(),
            favorites_spot: vec!["BTCUSDT".into(), "ETHUSDT".into()],
            alerts: vec![AlertConfig {
                market: 1,
                symbol: "BTCUSDT".into(),
                price: rust_decimal::Decimal::from_str_exact("86997.7").unwrap(),
                above: true,
            }],
            ..AppConfig::default()
        };
        cfg.save(&dir).expect("save 应当成功");

        let path = dir.join("config.toml");
        assert!(path.exists(), "save 成功后 config.toml 必须存在");
        let back = AppConfig::load(&dir);
        assert!(!back.testnet);
        assert_eq!(back.default_symbol, "ETHUSDT");
        assert_eq!(back.favorites_spot, cfg.favorites_spot);
        assert_eq!(back.alerts.len(), 1);
        assert_eq!(back.alerts[0].symbol, "BTCUSDT");
        assert_eq!(back.alerts[0].price.to_string(), "86997.7");
    }

    /// save 的失败路径必须返回 Err 而非 panic 或静默吞错：
    /// 目标路径已被一个普通文件占据时 create_dir_all/write 必然失败
    #[test]
    fn save_returns_err_when_blocked() {
        let parent = tmp_dir("blocked");
        // 占位"目录"其实是一个普通文件 → join 后的 config.toml 无法写入
        let blocker = parent.join("blocked_dir");
        std::fs::write(&blocker, "not a directory").unwrap();
        let cfg = AppConfig::default();
        assert!(cfg.save(&blocker).is_err(), "写入被阻截时 save 必须返回 Err");
    }

    /// data_dir 会话级稳定：每帧调用必须命中同一分配，
    /// 否则 OnceLock 缓存失效、设置页各处拿到不同路径
    #[test]
    fn data_dir_is_stable_within_session() {
        assert!(std::ptr::eq(data_dir(), data_dir()));
    }
}
