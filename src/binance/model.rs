//! Binance 数据模型（现货 + U本位合约，REST 与 WebSocket 共用）
//!
//! 字段集合刻意保持完整（含当前 UI 未展示的字段），故豁免 dead_code。
#![allow(dead_code)]

use crate::binance::de::{dec, opt_dec};
use rust_decimal::Decimal;
use serde::Deserialize;

/// 24 小时行情（REST ticker/24hr 为长键，WS @ticker 为短键，用 alias 兼容）
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Ticker24 {
    pub symbol: String,
    #[serde(rename = "lastPrice", alias = "c", deserialize_with = "dec")]
    pub last_price: Decimal,
    #[serde(rename = "openPrice", alias = "o", deserialize_with = "dec")]
    pub open_price: Decimal,
    #[serde(rename = "highPrice", alias = "h", deserialize_with = "dec")]
    pub high_price: Decimal,
    #[serde(rename = "lowPrice", alias = "l", deserialize_with = "dec")]
    pub low_price: Decimal,
    #[serde(rename = "priceChangePercent", alias = "P", deserialize_with = "dec")]
    pub price_change_percent: Decimal,
    #[serde(rename = "quoteVolume", alias = "q", deserialize_with = "dec")]
    pub quote_volume: Decimal,
}

/// 盘口档位
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub price: Decimal,
    pub qty: Decimal,
}

impl<'de> Deserialize<'de> for Level {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let arr = <[serde_json::Value; 2]>::deserialize(d)?;
        Ok(Level {
            price: crate::binance::de::value_to_dec(&arr[0])
                .ok_or_else(|| serde::de::Error::custom("盘口价格解析失败"))?,
            qty: crate::binance::de::value_to_dec(&arr[1])
                .ok_or_else(|| serde::de::Error::custom("盘口数量解析失败"))?,
        })
    }
}

/// 盘口快照（REST depth 与 partial-depth WS 流）
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Depth {
    #[serde(alias = "bids", alias = "b")]
    pub bids: Vec<Level>,
    #[serde(alias = "asks", alias = "a")]
    pub asks: Vec<Level>,
}

/// 逐笔/聚合成交（REST aggTrades 与 `@aggTrade` 流均为短键）
#[derive(Debug, Clone, Deserialize)]
pub struct TradeTick {
    #[serde(rename = "a", alias = "id", default)]
    pub id: i64,
    #[serde(rename = "p", alias = "price", deserialize_with = "dec")]
    pub price: Decimal,
    #[serde(rename = "q", alias = "qty", deserialize_with = "dec")]
    pub qty: Decimal,
    #[serde(rename = "T", alias = "time", default)]
    pub time: i64,
    #[serde(rename = "m", alias = "isBuyerMaker", default)]
    pub buyer_maker: bool,
}

/// K 线
#[derive(Debug, Clone)]
pub struct Candle {
    pub t: i64,
    pub o: Decimal,
    pub h: Decimal,
    pub l: Decimal,
    pub c: Decimal,
    pub v: Decimal,
    pub closed: bool,
}

impl Candle {
    pub fn f(&self) -> [f64; 4] {
        [
            self.o.to_string().parse().unwrap_or(0.0),
            self.h.to_string().parse().unwrap_or(0.0),
            self.l.to_string().parse().unwrap_or(0.0),
            self.c.to_string().parse().unwrap_or(0.0),
        ]
    }
}

/// REST klines 返回的数组行解析：
/// `[openTime, "open", "high", "low", "close", "volume", closeTime, ...]`
pub fn parse_kline_row(row: &[serde_json::Value]) -> Option<Candle> {
    if row.len() < 6 {
        return None;
    }
    Some(Candle {
        t: row[0].as_i64()?,
        o: crate::binance::de::value_to_dec(&row[1])?,
        h: crate::binance::de::value_to_dec(&row[2])?,
        l: crate::binance::de::value_to_dec(&row[3])?,
        c: crate::binance::de::value_to_dec(&row[4])?,
        v: crate::binance::de::value_to_dec(&row[5])?,
        closed: false,
    })
}

/// WS kline 事件负载（现货与合约同构）
#[derive(Debug, Clone, Deserialize)]
pub struct WsKline {
    #[serde(rename = "k")]
    pub k: WsKlineInner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsKlineInner {
    #[serde(rename = "t", default)]
    pub start_time: i64,
    #[serde(rename = "i", default)]
    pub interval: String,
    #[serde(rename = "o", deserialize_with = "dec")]
    pub open: Decimal,
    #[serde(rename = "h", deserialize_with = "dec")]
    pub high: Decimal,
    #[serde(rename = "l", deserialize_with = "dec")]
    pub low: Decimal,
    #[serde(rename = "c", deserialize_with = "dec")]
    pub close: Decimal,
    #[serde(rename = "v", deserialize_with = "dec")]
    pub volume: Decimal,
    #[serde(rename = "x", default)]
    pub closed: bool,
}

/// WS markPrice 事件（合约）
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MarkPriceUpdate {
    #[serde(rename = "p", deserialize_with = "dec", default)]
    pub mark_price: Decimal,
    #[serde(rename = "r", deserialize_with = "opt_dec", default)]
    pub funding_rate: Option<Decimal>,
    #[serde(rename = "T", default)]
    pub next_funding_time: i64,
}

/// 账户余额（spot）
#[derive(Debug, Clone, Deserialize)]
pub struct Balance {
    pub asset: String,
    #[serde(deserialize_with = "dec")]
    pub free: Decimal,
    #[serde(rename = "locked", alias = "l", deserialize_with = "dec", default)]
    pub locked: Decimal,
}

/// 现货账户（GET /api/v3/account）
#[derive(Debug, Clone, Deserialize)]
pub struct SpotAccount {
    #[serde(rename = "canTrade", default)]
    pub can_trade: bool,
    pub balances: Vec<Balance>,
}

/// 合约资产（/fapi/v2/account assets[]）
#[derive(Debug, Clone, Deserialize)]
pub struct FutBalance {
    #[serde(rename = "asset")]
    pub asset: String,
    #[serde(rename = "walletBalance", deserialize_with = "dec", default)]
    pub wallet: Decimal,
    #[serde(rename = "unrealizedProfit", deserialize_with = "dec", default)]
    pub unrealized: Decimal,
    #[serde(rename = "marginBalance", deserialize_with = "dec", default)]
    pub margin: Decimal,
    #[serde(rename = "availableBalance", deserialize_with = "dec", default)]
    pub available: Decimal,
}

/// 合约账户持仓（/fapi/v2/account positions[]）
#[derive(Debug, Clone, Deserialize)]
pub struct FutAccountPos {
    #[serde(rename = "symbol")]
    pub symbol: String,
    #[serde(rename = "positionAmt", deserialize_with = "dec", default)]
    pub amt: Decimal,
    #[serde(rename = "entryPrice", deserialize_with = "dec", default)]
    pub entry: Decimal,
    #[serde(rename = "unrealizedProfit", deserialize_with = "dec", default)]
    pub unrealized: Decimal,
    #[serde(rename = "leverage", deserialize_with = "dec", default)]
    pub leverage: Decimal,
}

/// 合约账户（GET /fapi/v2/account）
#[derive(Debug, Clone, Deserialize)]
pub struct FutAccount {
    #[serde(rename = "totalWalletBalance", deserialize_with = "dec", default)]
    pub total_wallet: Decimal,
    #[serde(rename = "totalUnrealizedProfit", deserialize_with = "dec", default)]
    pub total_unrealized: Decimal,
    #[serde(rename = "totalMarginBalance", deserialize_with = "dec", default)]
    pub total_margin: Decimal,
    #[serde(rename = "availableBalance", deserialize_with = "dec", default)]
    pub available: Decimal,
    #[serde(default)]
    pub assets: Vec<FutBalance>,
    #[serde(default)]
    pub positions: Vec<FutAccountPos>,
}

/// 持仓风险（GET /fapi/v2/positionRisk）
#[derive(Debug, Clone, Deserialize)]
pub struct PositionRisk {
    #[serde(rename = "symbol")]
    pub symbol: String,
    #[serde(rename = "positionAmt", deserialize_with = "dec", default)]
    pub amt: Decimal,
    #[serde(rename = "entryPrice", deserialize_with = "dec", default)]
    pub entry_price: Decimal,
    #[serde(rename = "markPrice", deserialize_with = "dec", default)]
    pub mark_price: Decimal,
    #[serde(rename = "unRealizedProfit", deserialize_with = "dec", default)]
    pub unrealized: Decimal,
    #[serde(rename = "liquidationPrice", deserialize_with = "dec", default)]
    pub liquidation_price: Decimal,
    #[serde(rename = "leverage", deserialize_with = "dec", default)]
    pub leverage: Decimal,
    #[serde(rename = "positionSide", default)]
    pub side: String,
}

/// 双向持仓模式状态（GET /fapi/v1/positionSide/dual）
#[derive(Debug, Clone, Deserialize)]
pub struct DualSideMode {
    #[serde(rename = "dualSidePosition", default)]
    pub dual: bool,
}

/// 杠杆信息（GET /fapi/v1/leverage）
#[derive(Debug, Clone, Deserialize)]
pub struct LeverageInfo {
    #[serde(rename = "symbol")]
    pub symbol: String,
    #[serde(rename = "leverage", deserialize_with = "dec", default)]
    pub leverage: Decimal,
}

/// listenKey
#[derive(Debug, Clone, Deserialize)]
pub struct ListenKey {
    #[serde(rename = "listenKey")]
    pub key: String,
}

/// 订单（现货/合约 REST 通用，字段为 camelCase 全名）
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct OrderInfo {
    pub symbol: String,
    #[serde(rename = "orderId", default)]
    pub order_id: i64,
    #[serde(rename = "clientOrderId", default)]
    pub client_order_id: String,
    pub price: Option<Decimal>,
    #[serde(rename = "origQty", default)]
    pub orig_qty: Decimal,
    #[serde(rename = "executedQty", default)]
    pub executed_qty: Decimal,
    #[serde(rename = "status", default)]
    pub status: String,
    #[serde(rename = "side", default)]
    pub side: String,
    #[serde(rename = "type", default)]
    pub order_type: String,
    #[serde(rename = "stopPrice", deserialize_with = "opt_dec", default)]
    pub stop_price: Option<Decimal>,
    #[serde(rename = "time", default)]
    pub time: i64,
    #[serde(rename = "updateTime", default)]
    pub update_time: i64,
    #[serde(rename = "positionSide", default)]
    pub position_side: String,
    #[serde(rename = "reduceOnly", default)]
    pub reduce_only: bool,
}

/// 成交明细（spot myTrades / fapi userTrades 同构）
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MyTrade {
    #[serde(rename = "id", default)]
    pub id: i64,
    #[serde(rename = "orderId", default)]
    pub order_id: i64,
    #[serde(rename = "symbol", default)]
    pub symbol: String,
    #[serde(deserialize_with = "dec", default)]
    pub price: Decimal,
    #[serde(rename = "qty", default)]
    pub qty: Decimal,
    #[serde(rename = "quoteQty", default)]
    pub quote_qty: Decimal,
    #[serde(rename = "commission", default)]
    pub commission: Decimal,
    #[serde(rename = "commissionAsset", default)]
    pub commission_asset: String,
    #[serde(rename = "time", default)]
    pub time: i64,
    #[serde(rename = "isBuyer", default)]
    pub is_buyer: bool,
    #[serde(rename = "isMaker", default)]
    pub is_maker: bool,
}

// ---------- 用户数据流（WebSocket 推送） ----------

/// 现货 executionReport 关键字段
#[derive(Debug, Clone, Deserialize)]
pub struct ExecReport {
    #[serde(rename = "E", default)]
    pub event_time: i64,
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub client_order_id: String,
    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "o")]
    pub order_type: String,
    #[serde(rename = "q", deserialize_with = "dec")]
    pub orig_qty: Decimal,
    #[serde(rename = "p", deserialize_with = "dec")]
    pub price: Decimal,
    #[serde(rename = "x")]
    pub exec_type: String,
    #[serde(rename = "X")]
    pub status: String,
    #[serde(rename = "i", default)]
    pub order_id: i64,
    #[serde(rename = "l", deserialize_with = "dec", default)]
    pub last_qty: Decimal,
    #[serde(rename = "z", deserialize_with = "dec", default)]
    pub cum_qty: Decimal,
    #[serde(rename = "L", deserialize_with = "opt_dec", default)]
    pub last_price: Option<Decimal>,
    #[serde(rename = "n", deserialize_with = "opt_dec", default)]
    pub commission: Option<Decimal>,
    #[serde(rename = "N", default)]
    pub commission_asset: Option<String>,
    #[serde(rename = "T", default)]
    pub trade_time: i64,
    #[serde(rename = "m", default)]
    pub is_maker: bool,
}

/// 现货 outboundAccountPosition
#[derive(Debug, Clone, Deserialize)]
pub struct OutboundAccountPosition {
    #[serde(rename = "E", default)]
    pub event_time: i64,
    #[serde(rename = "B")]
    pub balances: Vec<Balance>,
}

/// 合约 ORDER_TRADE_UPDATE 的 o 字段
#[derive(Debug, Clone, Deserialize)]
pub struct FutExec {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "c")]
    pub client_order_id: String,
    #[serde(rename = "S")]
    pub side: String,
    #[serde(rename = "o")]
    pub order_type: String,
    #[serde(rename = "q", deserialize_with = "dec")]
    pub orig_qty: Decimal,
    #[serde(rename = "p", deserialize_with = "opt_dec", default)]
    pub price: Option<Decimal>,
    #[serde(rename = "X")]
    pub status: String,
    #[serde(rename = "i", default)]
    pub order_id: i64,
    #[serde(rename = "l", deserialize_with = "opt_dec", default)]
    pub last_qty: Option<Decimal>,
    #[serde(rename = "z", deserialize_with = "dec", default)]
    pub cum_qty: Decimal,
    #[serde(rename = "L", deserialize_with = "opt_dec", default)]
    pub last_price: Option<Decimal>,
    #[serde(rename = "n", deserialize_with = "opt_dec", default)]
    pub commission: Option<Decimal>,
    #[serde(rename = "N", default)]
    pub commission_asset: Option<String>,
    #[serde(rename = "T", default)]
    pub trade_time: i64,
    #[serde(rename = "ps", default)]
    pub position_side: String,
    #[serde(rename = "R", default)]
    pub reduce_only: bool,
    #[serde(rename = "rp", deserialize_with = "opt_dec", default)]
    pub realized_profit: Option<Decimal>,
}

/// 合约 ACCOUNT_UPDATE
#[derive(Debug, Clone, Deserialize)]
pub struct FutAccountUpdate {
    #[serde(rename = "a")]
    pub a: FutAccountUpdateInner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FutAccountUpdateInner {
    #[serde(rename = "m", default)]
    pub reason: String,
    #[serde(rename = "B", default)]
    pub balances: Vec<FutWsBalance>,
    #[serde(rename = "P", default)]
    pub positions: Vec<FutWsPosition>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FutWsBalance {
    #[serde(rename = "a")]
    pub asset: String,
    #[serde(rename = "wb", deserialize_with = "dec", default)]
    pub wallet: Decimal,
    #[serde(rename = "cw", deserialize_with = "dec", default)]
    pub cross_wallet: Decimal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FutWsPosition {
    #[serde(rename = "s")]
    pub symbol: String,
    #[serde(rename = "pa", deserialize_with = "dec", default)]
    pub amt: Decimal,
    #[serde(rename = "ep", deserialize_with = "dec", default)]
    pub entry: Decimal,
    #[serde(rename = "up", deserialize_with = "dec", default)]
    pub unrealized: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ticker() {
        let t: Ticker24 = serde_json::from_str(
            r#"{"symbol":"BTCUSDT","priceChange":"9.5","priceChangePercent":"0.10","weightedAvgPrice":"1",
                "lastPrice":"96000.50","openPrice":"95991.0","highPrice":"97000","lowPrice":"95000",
                "volume":"1","quoteVolume":"123.45","openTime":1,"closeTime":2,"firstId":1,"lastId":2,"count":2}"#,
        )
        .unwrap();
        assert_eq!(t.last_price.to_string(), "96000.50");
        assert_eq!(t.price_change_percent.to_string(), "0.10");
        // WS 短键格式
        let t: Ticker24 = serde_json::from_str(
            r#"{"e":"24hrTicker","E":1,"s":"BTCUSDT","p":"9.5","P":"0.10","w":"1","c":"96000.50","Q":"1",
                "o":"95991.0","h":"97000","l":"95000","v":"1","q":"123.45","O":1,"C":2,"F":1,"L":2,"n":2}"#,
        )
        .unwrap();
        assert_eq!(t.last_price.to_string(), "96000.50");
        assert_eq!(t.price_change_percent.to_string(), "0.10");
        assert_eq!(t.quote_volume.to_string(), "123.45");
    }

    #[test]
    fn parse_depth_both_key_styles() {
        let d: Depth = serde_json::from_str(
            r#"{"lastUpdateId":1,"bids":[["100.1","2.0"]],"asks":[["101.0","1.5"]]}"#,
        )
        .unwrap();
        assert_eq!(d.bids[0].price.to_string(), "100.1");
        let d: Depth =
            serde_json::from_str(r#"{"E":1,"T":1,"b":[["100.1","2.0"]],"a":[["101.0","1.5"]]}"#)
                .unwrap();
        assert_eq!(d.asks[0].qty.to_string(), "1.5");
    }

    #[test]
    fn parse_kline_row_sample() {
        let v: Vec<serde_json::Value> = serde_json::from_str(
            r#"[1668445200000,"17000","17100","16900","17050","12.3",1668445259999,"209700",38,"6.9","117400",0]"#,
        )
        .unwrap();
        let c = parse_kline_row(&v).unwrap();
        assert_eq!(c.t, 1668445200000);
        assert_eq!(c.c.to_string(), "17050");
        assert_eq!(c.v.to_string(), "12.3");
    }

    #[test]
    fn parse_ws_kline_event() {
        let raw = r#"{"stream":"btcusdt@kline_1m","data":{"e":"kline","E":1,"s":"BTCUSDT","k":{"t":1,"T":2,"s":"BTCUSDT","i":"1m","o":"1","c":"2","h":"3","l":"0.5","v":"9","x":false,"q":"1","V":"1","Q":"1","n":1}}}"#;
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let k: WsKline = serde_json::from_value(v["data"].clone()).unwrap();
        assert_eq!(k.k.close.to_string(), "2");
        assert!(!k.k.closed);
    }

    #[test]
    fn parse_spot_execution_report() {
        let raw = r#"{
          "e":"executionReport","E":1568014460893,"s":"BTCUSDT","c":"abc","S":"BUY","o":"LIMIT","f":"GTC",
          "q":"1.00000000","p":"0.10264446","x":"TRADE","X":"PARTIALLY_FILLED","r":"NONE","i":891,
          "l":"0.10000000","z":"0.30000000","L":"0.10264446","n":"0.0005","N":"BTC","T":1568014460891,
          "t":1234,"I":8647,"w":true,"m":false,"M":true,"O":1568014460893,"Z":"0.03079333","Y":"0.01026444","Q":"0.10000000"
        }"#;
        let r: ExecReport = serde_json::from_str(raw).unwrap();
        assert_eq!(r.status, "PARTIALLY_FILLED");
        assert_eq!(r.cum_qty.to_string(), "0.30000000");
        assert_eq!(r.commission_asset.as_deref(), Some("BTC"));
    }

    #[test]
    fn parse_fut_order_trade_update() {
        let raw = r#"{
          "e":"ORDER_TRADE_UPDATE","E":1568014460893,"T":1568014460893,
          "o":{"s":"BTCUSDT","c":"TEST","S":"SELL","o":"TRAILING_STOP_MARKET","f":"GTC","q":"2","p":"0",
               "ap":"0","X":"NEW","i":8888888,"l":"0","z":"0","L":"0","N":"USDT","n":"0","T":1568014460893,
               "t":0,"b":"0","a":"0","m":false,"R":false,"wt":"CONTRACT_PRICE","ot":"TRAILING_STOP_MARKET",
               "ps":"LONG","cp":false,"AP":"94700","cr":"5","rp":"0"}
        }"#;
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(rename = "o")]
            o: FutExec,
        }
        let w: Wrap = serde_json::from_str(raw).unwrap();
        assert_eq!(w.o.symbol, "BTCUSDT");
        assert_eq!(w.o.position_side, "LONG");
        assert_eq!(w.o.orig_qty.to_string(), "2");
        assert_eq!(w.o.status, "NEW");
    }

    #[test]
    fn parse_fut_account_update() {
        let raw = r#"{"e":"ACCOUNT_UPDATE","E":1564797666,"T":1564797667,"a":{
          "m":"ORDER","B":[{"a":"USDT","wb":"122624.12345678","cw":"100.12345678"}],
          "P":[{"s":"BTCUSDT","pa":"1","ep":"90787.6","cr":"0.0","up":"0.0","mt":"cross","iw":"1.0","ps":"LONG","ma":"USDT"}]}}"#;
        let u: FutAccountUpdate = serde_json::from_str(raw).unwrap();
        assert_eq!(u.a.balances[0].asset, "USDT");
        assert_eq!(u.a.positions[0].symbol, "BTCUSDT");
    }

    #[test]
    fn parse_position_risk_and_account() {
        let raw = r#"{"symbol":"BTCUSDT","positionAmt":0.010,"entryPrice":22185.2,"markPrice":21123.0,
            "unRealizedProfit":-0.1062210,"liquidationPrice":0,"leverage":10,"positionSide":"BOTH",
            "updateTime":0,"maxNotionalValue":"1000","isolated":true,"isolatedWallet":99.9,"notional":"-211.23","isAutoAddMargin":"false"}"#;
        let p: PositionRisk = serde_json::from_str(raw).unwrap();
        assert_eq!(p.amt.to_string(), "0.01");
        assert_eq!(p.side, "BOTH");

        let raw = r#"{"feeTier":0,"canTrade":true,"totalWalletBalance":"105.1","totalUnrealizedProfit":"-1.2",
            "totalMarginBalance":"103.9","availableBalance":"88.5","maxWithdrawAmount":"77",
            "totalInitialMargin":"10","totalPositionInitialMargin":"9","totalOpenOrderInitialMargin":"1",
            "totalPositionMargin":"0","assets":[{"asset":"USDT","walletBalance":"105.1","unrealizedProfit":"-1.2","marginBalance":"103.9","maintMargin":"1","initialMargin":"10","positionInitialMargin":"9","openOrderInitialMargin":"1","crossWalletBalance":"100","crossUnPnl":"0","availableBalance":"88.5","maxWithdrawAmount":"77","marginAvailable":true,"updateTime":1}],
            "positions":[{"symbol":"BTCUSDT","initialMargin":"0","maintMargin":"0","positionInitialMargin":"0","openOrderInitialMargin":"0","leverage":"10","isolated":false,"entryPrice":"22185.2","maxNotional":"1000","positionSide":"BOTH","positionAmt":"0.010","notional":"-211.23","isolatedWallet":"0","unrealizedProfit":"-0.106","autoAddMargin":"0"}]}"#;
        let a: FutAccount = serde_json::from_str(raw).unwrap();
        assert_eq!(a.total_wallet.to_string(), "105.1");
        assert_eq!(a.positions[0].symbol, "BTCUSDT");
    }
}
