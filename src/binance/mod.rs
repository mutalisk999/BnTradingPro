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
            Market::Spot => "现货",
            Market::UmFutures => "U本位合约",
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
    if s.trim().is_empty() {
        default.to_string()
    } else {
        s.trim().trim_end_matches('/').to_string()
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
            Side::Buy => "买入",
            Side::Sell => "卖出",
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
            FutPosSide::Both => "单向",
            FutPosSide::Long => "做多",
            FutPosSide::Short => "做空",
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
    fn interval_parse() {
        assert_eq!(Interval::from_str("1h"), Interval::H1);
        assert_eq!(Interval::from_str("xx"), Interval::M15);
    }
}
