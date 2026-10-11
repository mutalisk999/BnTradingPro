//! 交易规则（exchangeInfo）解析与下单数值量化

use rust_decimal::Decimal;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SymbolRules {
    pub symbol: String,
    pub base: String,
    pub quote: String,
    /// 价格最小变动
    pub tick: Decimal,
    /// 数量最小步进
    pub step: Decimal,
    pub min_qty: Decimal,
    pub max_qty: Decimal,
    pub min_notional: Decimal,
    /// 价格精度（合约 pricePrecision / 现货 quotePrecision）
    pub price_precision: u32,
    /// 数量精度（合约 quantityPrecision / 现货 baseAssetPrecision）
    pub qty_precision: u32,
    /// 支持的订单类型
    pub order_types: Vec<String>,
    /// 现货权限（SPOT/MARGIN…）
    pub permissions: Vec<String>,
    /// 状态（TRADING/BREAK…）
    pub status: String,
    // ---- 合约特有 ----
    /// 合约类型（PERPETUAL/CURRENT_QUARTER…）
    pub contract_type: String,
    /// 保证金资产
    pub margin_asset: String,
    /// 标的类型（COINMARGIN? 实为 underlyingType，如 COIN/INDEX）
    pub underlying_type: String,
    pub underlying_sub_types: Vec<String>,
    /// 上架时间（毫秒时间戳，0 表示未知）
    pub onboard_date: i64,
}

impl SymbolRules {
    /// tick 的小数位数（用于展示）
    pub fn price_dp(&self) -> u32 {
        self.tick.normalize().scale()
    }

    pub fn qty_dp(&self) -> u32 {
        self.step.normalize().scale()
    }
}

/// 解析现货/合约 exchangeInfo，只保留 TRADING 状态的交易对
pub fn parse_exchange_info(v: &Value) -> Vec<SymbolRules> {
    let mut out = Vec::new();
    let Some(symbols) = v.get("symbols").and_then(|s| s.as_array()) else {
        return out;
    };
    for s in symbols {
        let symbol = match s.get("symbol").and_then(|x| x.as_str()) {
            Some(x) => x.to_string(),
            None => continue,
        };
        if let Some(status) = s.get("status").and_then(|x| x.as_str()) {
            if status != "TRADING" {
                continue;
            }
        }
        let base = s
            .get("baseAsset")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let quote = s
            .get("quoteAsset")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();

        let mut tick = Decimal::ZERO;
        let mut step = Decimal::ZERO;
        let mut min_qty = Decimal::ZERO;
        let mut max_qty = Decimal::ZERO;
        let mut min_notional = Decimal::ZERO;

        if let Some(filters) = s.get("filters").and_then(|f| f.as_array()) {
            for f in filters {
                let ft = f.get("filterType").and_then(|x| x.as_str()).unwrap_or("");
                let num = |k: &str| -> Decimal {
                    f.get(k)
                        .and_then(crate::binance::de::value_to_dec)
                        .unwrap_or(Decimal::ZERO)
                };
                match ft {
                    "PRICE_FILTER" => tick = num("tickSize"),
                    "LOT_SIZE" => {
                        step = num("stepSize");
                        min_qty = num("minQty");
                        max_qty = num("maxQty");
                    }
                    // 现货新版 NOTIONAL 与旧版 MIN_NOTIONAL；合约 MIN_NOTIONAL.notional
                    "NOTIONAL" => min_notional = num("minNotional"),
                    "MIN_NOTIONAL" => {
                        let n = num("minNotional");
                        let n2 = num("notional");
                        min_notional = if n.is_zero() { n2 } else { n };
                    }
                    _ => {}
                }
            }
        }
        if symbol.is_empty() || tick.is_zero() || step.is_zero() {
            continue;
        }
        let arr_str = |key: &str| -> Vec<String> {
            s.get(key)
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(|x| x.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let u32_of = |keys: &[&str]| -> u32 {
            for k in keys {
                if let Some(v) = s.get(*k).and_then(crate::binance::de::value_to_dec) {
                    if let Ok(n) = v.to_string().parse::<u32>() {
                        return n;
                    }
                }
            }
            0
        };
        out.push(SymbolRules {
            price_precision: u32_of(&["pricePrecision", "quotePrecision"]),
            qty_precision: u32_of(&["quantityPrecision", "baseAssetPrecision"]),
            order_types: arr_str("orderTypes"),
            permissions: arr_str("permissions"),
            status: s
                .get("status")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            contract_type: s
                .get("contractType")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            margin_asset: s
                .get("marginAsset")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            underlying_type: s
                .get("underlyingType")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            underlying_sub_types: arr_str("underlyingSubType"),
            onboard_date: s.get("onboardDate").and_then(|x| x.as_i64()).unwrap_or(0),
            symbol,
            base,
            quote,
            tick,
            step,
            min_qty,
            max_qty,
            min_notional,
        });
    }
    out
}

/// 交易对规则簿
#[derive(Debug, Clone, Default)]
pub struct RuleBook {
    pub by_symbol: HashMap<String, SymbolRules>,
    pub symbols: Vec<String>,
}

impl RuleBook {
    pub fn from_rules(rules: Vec<SymbolRules>) -> Self {
        let mut symbols: Vec<String> = rules.iter().map(|r| r.symbol.clone()).collect();
        symbols.sort();
        Self {
            by_symbol: rules.into_iter().map(|r| (r.symbol.clone(), r)).collect(),
            symbols,
        }
    }

    pub fn get(&self, symbol: &str) -> Option<&SymbolRules> {
        self.by_symbol.get(symbol)
    }
}

/// 价格向下取整到 tick 的整数倍（溢出时返回 None，防 rust_decimal panic）
pub fn quantize_price(price: Decimal, tick: Decimal) -> Option<Decimal> {
    if tick.is_zero() {
        return Some(price);
    }
    let n = price.checked_div(tick)?.floor();
    n.checked_mul(tick)
}

/// 数量向下取整到 step 的整数倍（溢出时返回 None）
pub fn quantize_qty(qty: Decimal, step: Decimal) -> Option<Decimal> {
    if step.is_zero() {
        return Some(qty);
    }
    let n = qty.checked_div(step)?.floor();
    n.checked_mul(step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> SymbolRules {
        SymbolRules {
            symbol: "BTCUSDT".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick: Decimal::new(1, 2),    // 0.01
            step: Decimal::new(1, 5),    // 0.00001
            min_qty: Decimal::new(1, 5), // 0.00001
            max_qty: Decimal::ZERO,
            min_notional: Decimal::new(500, 2), // 5.00
            price_precision: 2,
            qty_precision: 5,
            order_types: vec![],
            permissions: vec![],
            status: "TRADING".into(),
            contract_type: String::new(),
            margin_asset: String::new(),
            underlying_type: String::new(),
            underlying_sub_types: vec![],
            onboard_date: 0,
        }
    }

    #[test]
    fn quantize_boundaries() {
        let r = rules();
        assert_eq!(
            quantize_price(Decimal::new(30000123, 3), r.tick)
                .unwrap()
                .to_string(),
            "30000.12"
        );
        assert_eq!(
            quantize_price(Decimal::new(12345, 2), Decimal::new(5, 1))
                .unwrap()
                .to_string(),
            "123.0"
        );
        assert_eq!(
            quantize_qty(Decimal::new(1234567, 6), r.step)
                .unwrap()
                .to_string(),
            "1.23456"
        );
        // 恰好为整数倍时保持不变
        assert_eq!(
            quantize_qty(Decimal::new(1, 0), r.step)
                .unwrap()
                .to_string(),
            "1.00000"
        );
        // 溢出防护：超大值返回 None 而非 panic
        let huge = Decimal::from_str_exact("79228162514264337593543950335").unwrap();
        assert!(quantize_price(huge, r.tick).is_none());
        assert!(quantize_qty(huge, r.step).is_none());
    }

    #[test]
    fn decimal_places() {
        let r = rules();
        assert_eq!(r.price_dp(), 2);
        assert_eq!(r.qty_dp(), 5);
    }

    #[test]
    fn parse_spot_exchange_info() {
        let v: Value = serde_json::from_str(
            r#"{"timezone":"UTC","symbols":[{
              "symbol":"BTCUSDT","status":"TRADING","baseAsset":"BTC","quoteAsset":"USDT",
              "quotePrecision":8,"baseAssetPrecision":6,"orderTypes":["LIMIT","MARKET"],
              "permissions":["SPOT","MARGIN"],
              "filters":[
                {"filterType":"PRICE_FILTER","minPrice":"0.01","maxPrice":"1000000","tickSize":"0.01"},
                {"filterType":"LOT_SIZE","minQty":"0.00001","maxQty":"9000","stepSize":"0.00001"},
                {"filterType":"NOTIONAL","minNotional":"5.00","applyMinToMarket":true}
              ]},
              {"symbol":"OFF","status":"BREAK","baseAsset":"X","quoteAsset":"Y","filters":[]}]}"#,
        )
        .unwrap();
        let rules = parse_exchange_info(&v);
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert_eq!(r.base, "BTC");
        assert_eq!(r.tick.to_string(), "0.01");
        assert_eq!(r.min_notional.to_string(), "5.00");
        assert_eq!(r.status, "TRADING");
        assert!(r.order_types.contains(&"LIMIT".to_string()));
        assert!(r.permissions.contains(&"SPOT".to_string()));
        assert_eq!(r.price_precision, 8);
        assert_eq!(r.qty_precision, 6);
    }

    #[test]
    fn parse_futures_exchange_info_min_notional() {
        let v: Value = serde_json::from_str(
            r#"{"symbols":[{"symbol":"BTCUSDT","status":"TRADING","baseAsset":"BTC","quoteAsset":"USDT",
              "contractType":"PERPETUAL","marginAsset":"USDT","pricePrecision":2,"quantityPrecision":3,
              "underlyingType":"COIN","underlyingSubType":["POW"],"onboardDate":1569888000000,
              "filters":[
              {"filterType":"PRICE_FILTER","minPrice":"0.10","tickSize":"0.10"},
              {"filterType":"LOT_SIZE","minQty":"0.001","stepSize":"0.001"},
              {"filterType":"MIN_NOTIONAL","notional":"100"}]}]}"#,
        )
        .unwrap();
        let rules = parse_exchange_info(&v);
        assert_eq!(rules[0].min_notional.to_string(), "100");
        assert_eq!(rules[0].tick.to_string(), "0.10");
        assert_eq!(rules[0].contract_type, "PERPETUAL");
        assert_eq!(rules[0].margin_asset, "USDT");
        assert_eq!(rules[0].price_precision, 2);
        assert_eq!(rules[0].qty_precision, 3);
        assert_eq!(rules[0].onboard_date, 1569888000000);
    }
}
