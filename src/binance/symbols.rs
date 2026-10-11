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
    pub min_notional: Decimal,
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
        out.push(SymbolRules {
            symbol,
            base,
            quote,
            tick,
            step,
            min_qty,
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

/// 价格向下取整到 tick 的整数倍
pub fn quantize_price(price: Decimal, tick: Decimal) -> Decimal {
    if tick.is_zero() {
        return price;
    }
    (price / tick).floor() * tick
}

/// 数量向下取整到 step 的整数倍
pub fn quantize_qty(qty: Decimal, step: Decimal) -> Decimal {
    if step.is_zero() {
        return qty;
    }
    (qty / step).floor() * step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> SymbolRules {
        SymbolRules {
            symbol: "BTCUSDT".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick: Decimal::new(1, 2),           // 0.01
            step: Decimal::new(1, 5),           // 0.00001
            min_qty: Decimal::new(1, 5),        // 0.00001
            min_notional: Decimal::new(500, 2), // 5.00
        }
    }

    #[test]
    fn quantize_boundaries() {
        let r = rules();
        assert_eq!(
            quantize_price(Decimal::new(30000123, 3), r.tick).to_string(),
            "30000.12"
        );
        assert_eq!(
            quantize_price(Decimal::new(12345, 2), Decimal::new(5, 1)).to_string(),
            "123.0"
        );
        assert_eq!(
            quantize_qty(Decimal::new(1234567, 6), r.step).to_string(),
            "1.23456"
        );
        // 恰好为整数倍时保持不变
        assert_eq!(
            quantize_qty(Decimal::new(1, 0), r.step).to_string(),
            "1.00000"
        );
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
    }

    #[test]
    fn parse_futures_exchange_info_min_notional() {
        let v: Value = serde_json::from_str(
            r#"{"symbols":[{"symbol":"BTCUSDT","status":"TRADING","baseAsset":"BTC","quoteAsset":"USDT","filters":[
              {"filterType":"PRICE_FILTER","minPrice":"0.10","tickSize":"0.10"},
              {"filterType":"LOT_SIZE","minQty":"0.001","stepSize":"0.001"},
              {"filterType":"MIN_NOTIONAL","notional":"100"}]}]}"#,
        )
        .unwrap();
        let rules = parse_exchange_info(&v);
        assert_eq!(rules[0].min_notional.to_string(), "100");
        assert_eq!(rules[0].tick.to_string(), "0.10");
    }
}
