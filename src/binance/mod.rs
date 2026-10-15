//! Binance 接入层：市场定义、端点、REST 客户端、WebSocket、数据模型与量化规则

pub mod client;
pub mod de;
pub mod model;
pub mod symbols;
pub mod ws;

/// 交易市场（MVP 覆盖现货与 U 本位合约）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Market {
    Spot,
    UmFutures,
}

impl Market {
    pub fn label(&self) -> &'static str {
        match self {
            Market::Spot => crate::i18n::tr("现货"),
            Market::UmFutures => crate::i18n::tr("U本位合约"),
        }
    }

    pub fn all() -> [Market; 2] {
        [Market::Spot, Market::UmFutures]
    }

    /// REST 路径前缀（合约的 v2 端点单独写全路径）
    pub fn api_prefix(&self) -> &'static str {
        match self {
            Market::Spot => "/api/v3",
            Market::UmFutures => "/fapi/v1",
        }
    }

    /// listenKey 端点（现货与合约路径不同）
    pub fn listen_key_path(&self) -> &'static str {
        match self {
            Market::Spot => "/api/v3/userDataStream",
            Market::UmFutures => "/fapi/v1/listenKey",
        }
    }
}

impl std::fmt::Display for Market {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub rest: String,
    pub ws: String,
}

/// 解析端点（内置默认 + 配置覆盖）
pub fn endpoints(market: Market, testnet: bool, cfg: &crate::config::AppConfig) -> Endpoints {
    match (market, testnet) {
        (Market::Spot, false) => Endpoints {
            rest: non_empty(&cfg.override_spot_rest, "https://api.binance.com"),
            ws: non_empty(&cfg.override_spot_ws, "wss://stream.binance.com:9443"),
        },
        (Market::Spot, true) => Endpoints {
            rest: non_empty(&cfg.override_spot_rest, "https://testnet.binance.vision"),
            ws: non_empty(
                &cfg.override_spot_ws,
                "wss://stream.testnet.binance.vision:9443",
            ),
        },
        (Market::UmFutures, false) => Endpoints {
            rest: non_empty(&cfg.override_fut_rest, "https://fapi.binance.com"),
            ws: non_empty(&cfg.override_fut_ws, "wss://fstream.binance.com"),
        },
        (Market::UmFutures, true) => Endpoints {
            rest: non_empty(&cfg.override_fut_rest, "https://testnet.binancefuture.com"),
            ws: non_empty(&cfg.override_fut_ws, "wss://stream.binancefuture.com"),
        },
    }
}

fn non_empty(s: &str, default: &str) -> String {
    let t = s.trim();
    if t.is_empty() {
        return default.to_string();
    }
    let t = t.trim_end_matches('/');
    // 覆盖端点必须加密：http/ws 会把 X-MBX-APIKEY 头、签名 query 与 listenKey
    // 全部明文暴露给链路上的观察者，一律拒绝并回退内置默认
    if !t.starts_with("https://") && !t.starts_with("wss://") {
        // 拒绝的 URL 可能内嵌 user:pass@，落盘日志前必须脱敏
        let redacted = match t.split_once("://") {
            Some((scheme, rest)) => {
                let host = rest.rsplit('@').next().unwrap_or("");
                format!("{scheme}//***@{host}")
            }
            None => "***".to_string(),
        };
        tracing::warn!("忽略不安全的端点覆盖（须为 https:// 或 wss://）：{redacted}");
        return default.to_string();
    }
    t.to_string()
}

/// 合约 WS-API（下单通道）端点：独立于行情/用户流端点，不走覆盖配置
/// （实测 testnet 的 ws-api 在 testnet.binancefuture.com 主机下而非 stream 子域）
pub fn ws_api_order(testnet: bool) -> &'static str {
    if testnet {
        "wss://testnet.binancefuture.com/ws-fapi/v1"
    } else {
        "wss://ws-fapi.binance.com/ws-fapi/v1"
    }
}

/// K 线周期
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Interval {
    M1,
    M3,
    M5,
    #[default]
    M15,
    M30,
    H1,
    H2,
    H4,
    H6,
    H12,
    D1,
    D3,
    W1,
    MO1,
}

impl Interval {
    pub fn as_str(&self) -> &'static str {
        match self {
            Interval::M1 => "1m",
            Interval::M3 => "3m",
            Interval::M5 => "5m",
            Interval::M15 => "15m",
            Interval::M30 => "30m",
            Interval::H1 => "1h",
            Interval::H2 => "2h",
            Interval::H4 => "4h",
            Interval::H6 => "6h",
            Interval::H12 => "12h",
            Interval::D1 => "1d",
            Interval::D3 => "3d",
            Interval::W1 => "1w",
            Interval::MO1 => "1M",
        }
    }

    pub fn all() -> [Interval; 14] {
        [
            Interval::M1,
            Interval::M3,
            Interval::M5,
            Interval::M15,
            Interval::M30,
            Interval::H1,
            Interval::H2,
            Interval::H4,
            Interval::H6,
            Interval::H12,
            Interval::D1,
            Interval::D3,
            Interval::W1,
            Interval::MO1,
        ]
    }

    pub fn from_str(s: &str) -> Interval {
        Interval::all()
            .into_iter()
            .find(|i| i.as_str() == s)
            .unwrap_or(Interval::M15)
    }

    /// 周期对应的毫秒数（1M 按自然月近似为 30 天，仅用于K线窗口判断）
    pub fn ms(&self) -> i64 {
        match self {
            Interval::M1 => 60 * 1000,
            Interval::M3 => 3 * 60 * 1000,
            Interval::M5 => 5 * 60 * 1000,
            Interval::M15 => 15 * 60 * 1000,
            Interval::M30 => 30 * 60 * 1000,
            Interval::H1 => 60 * 60 * 1000,
            Interval::H2 => 2 * 60 * 60 * 1000,
            Interval::H4 => 4 * 60 * 60 * 1000,
            Interval::H6 => 6 * 60 * 60 * 1000,
            Interval::H12 => 12 * 60 * 60 * 1000,
            Interval::D1 => 24 * 60 * 60 * 1000,
            Interval::D3 => 3 * 24 * 60 * 60 * 1000,
            Interval::W1 => 7 * 24 * 60 * 60 * 1000,
            Interval::MO1 => 30 * 24 * 60 * 60 * 1000,
        }
    }

    pub fn label(&self) -> &'static str {
        self.as_str()
    }
}

/// 订单方向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Side::Buy => crate::i18n::tr("买入"),
            Side::Sell => crate::i18n::tr("卖出"),
        }
    }
}

/// 合约持仓方向（双向模式下使用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutPosSide {
    Both,
    Long,
    Short,
}

impl FutPosSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            FutPosSide::Both => "BOTH",
            FutPosSide::Long => "LONG",
            FutPosSide::Short => "SHORT",
        }
    }

    pub fn parse(s: &str) -> FutPosSide {
        match s {
            "LONG" => FutPosSide::Long,
            "SHORT" => FutPosSide::Short,
            _ => FutPosSide::Both,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            FutPosSide::Both => crate::i18n::tr("单向"),
            FutPosSide::Long => crate::i18n::tr("做多"),
            FutPosSide::Short => crate::i18n::tr("做空"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_defaults_and_override() {
        let mut cfg = crate::config::AppConfig::default();
        assert_eq!(
            endpoints(Market::Spot, false, &cfg).rest,
            "https://api.binance.com"
        );
        assert_eq!(
            endpoints(Market::UmFutures, true, &cfg).ws,
            "wss://stream.binancefuture.com"
        );
        cfg.override_fut_rest = "https://my-proxy.example.com/".into();
        assert_eq!(
            endpoints(Market::UmFutures, true, &cfg).rest,
            "https://my-proxy.example.com"
        );
    }

    #[test]
    fn insecure_override_rejected() {
        // http/ws 覆盖会把 API Key 头与签名 query 明文暴露，必须回退默认
        let mut cfg = crate::config::AppConfig {
            override_spot_rest: "http://evil.example.com".into(),
            ..Default::default()
        };
        assert_eq!(
            endpoints(Market::Spot, false, &cfg).rest,
            "https://api.binance.com"
        );
        cfg.override_spot_ws = "ws://evil.example.com".into();
        assert_eq!(
            endpoints(Market::Spot, false, &cfg).ws,
            "wss://stream.binance.com:9443"
        );
    }

    #[test]
    fn interval_parse() {
        assert_eq!(Interval::from_str("1h"), Interval::H1);
        assert_eq!(Interval::from_str("xx"), Interval::M15);
    }
}
