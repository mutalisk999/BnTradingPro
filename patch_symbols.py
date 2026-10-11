# -*- coding: utf-8 -*-
p = 'src/binance/symbols.rs'
s = open(p, encoding='utf-8').read()

# 1. SymbolRules 扩展属性字段
old = """#[derive(Debug, Clone)]
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
}"""
new = """#[derive(Debug, Clone)]
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
}"""
assert old in s, "struct"
s = s.replace(old, new)

# 2. 解析器扩展
old = """        let mut tick = Decimal::ZERO;
        let mut step = Decimal::ZERO;
        let mut min_qty = Decimal::ZERO;
        let mut min_notional = Decimal::ZERO;

        if let Some(filters) = s.get("filters").and_then(|f| f.as_array()) {"""
new = """        let mut tick = Decimal::ZERO;
        let mut step = Decimal::ZERO;
        let mut min_qty = Decimal::ZERO;
        let mut max_qty = Decimal::ZERO;
        let mut min_notional = Decimal::ZERO;

        if let Some(filters) = s.get("filters").and_then(|f| f.as_array()) {"""
assert old in s, "filter vars"
s = s.replace(old, new)

old = """                match ft {
                    "PRICE_FILTER" => tick = num("tickSize"),
                    "LOT_SIZE" => {
                        step = num("stepSize");
                        min_qty = num("minQty");
                    }"""
new = """                match ft {
                    "PRICE_FILTER" => tick = num("tickSize"),
                    "LOT_SIZE" => {
                        step = num("stepSize");
                        min_qty = num("minQty");
                        max_qty = num("maxQty");
                    }"""
assert old in s, "lot"
s = s.replace(old, new)

old = """        if symbol.is_empty() || tick.is_zero() || step.is_zero() {
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
}"""
new = """        if symbol.is_empty() || tick.is_zero() || step.is_zero() {
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
}"""
assert old in s, "push"
s = s.replace(old, new)

# 3. 测试构造更新 + 新属性断言
old = """    fn rules() -> SymbolRules {
        SymbolRules {
            symbol: "BTCUSDT".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick: Decimal::new(1, 2),           // 0.01
            step: Decimal::new(1, 5),           // 0.00001
            min_qty: Decimal::new(1, 5),        // 0.00001
            min_notional: Decimal::new(500, 2), // 5.00
        }
    }"""
new = """    fn rules() -> SymbolRules {
        SymbolRules {
            symbol: "BTCUSDT".into(),
            base: "BTC".into(),
            quote: "USDT".into(),
            tick: Decimal::new(1, 2),           // 0.01
            step: Decimal::new(1, 5),           // 0.00001
            min_qty: Decimal::new(1, 5),        // 0.00001
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
    }"""
assert old in s, "test rules"
s = s.replace(old, new)

# 4. exchangeInfo 解析测试补属性断言
old = """        let rules = parse_exchange_info(&v);
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert_eq!(r.base, "BTC");
        assert_eq!(r.tick.to_string(), "0.01");
        assert_eq!(r.min_notional.to_string(), "5.00");
    }"""
new = """        let rules = parse_exchange_info(&v);
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
    }"""
assert old in s, "spot test"
s = s.replace(old, new)

old = """        let rules = parse_exchange_info(&v);
        assert_eq!(rules[0].min_notional.to_string(), "100");
        assert_eq!(rules[0].tick.to_string(), "0.10");
    }"""
new = """        let rules = parse_exchange_info(&v);
        assert_eq!(rules[0].min_notional.to_string(), "100");
        assert_eq!(rules[0].tick.to_string(), "0.10");
        assert_eq!(rules[0].contract_type, "PERPETUAL");
        assert_eq!(rules[0].margin_asset, "USDT");
        assert_eq!(rules[0].price_precision, 2);
        assert_eq!(rules[0].qty_precision, 3);
        assert_eq!(rules[0].onboard_date, 1569888000000);
    }"""
assert old in s, "fut test"
s = s.replace(old, new)

# 5. 两个测试样本 JSON 补充属性字段
old = """              "filters":[
                {"filterType":"PRICE_FILTER","minPrice":"0.01","maxPrice":"1000000","tickSize":"0.01"},
                {"filterType":"LOT_SIZE","minQty":"0.00001","maxQty":"9000","stepSize":"0.00001"},
                {"filterType":"NOTIONAL","minNotional":"5.00","applyMinToMarket":true}
              ]},"""
new = """              "quotePrecision":8,"baseAssetPrecision":6,"orderTypes":["LIMIT","MARKET"],
              "permissions":["SPOT","MARGIN"],
              "filters":[
                {"filterType":"PRICE_FILTER","minPrice":"0.01","maxPrice":"1000000","tickSize":"0.01"},
                {"filterType":"LOT_SIZE","minQty":"0.00001","maxQty":"9000","stepSize":"0.00001"},
                {"filterType":"NOTIONAL","minNotional":"5.00","applyMinToMarket":true}
              ]},"""
assert old in s, "spot json"
s = s.replace(old, new)

old = """        let v: Value = serde_json::from_str(
            r#"{"symbols":[{"symbol":"BTCUSDT","status":"TRADING","baseAsset":"BTC","quoteAsset":"USDT","filters":[
              {"filterType":"PRICE_FILTER","minPrice":"0.10","tickSize":"0.10"},
              {"filterType":"LOT_SIZE","minQty":"0.001","stepSize":"0.001"},
              {"filterType":"MIN_NOTIONAL","notional":"100"}]}]}"#,
        )
        .unwrap();"""
new = """        let v: Value = serde_json::from_str(
            r#"{"symbols":[{"symbol":"BTCUSDT","status":"TRADING","baseAsset":"BTC","quoteAsset":"USDT",
              "contractType":"PERPETUAL","marginAsset":"USDT","pricePrecision":2,"quantityPrecision":3,
              "underlyingType":"COIN","underlyingSubType":["POW"],"onboardDate":1569888000000,
              "filters":[
              {"filterType":"PRICE_FILTER","minPrice":"0.10","tickSize":"0.10"},
              {"filterType":"LOT_SIZE","minQty":"0.001","stepSize":"0.001"},
              {"filterType":"MIN_NOTIONAL","notional":"100"}]}]}"#,
        )
        .unwrap();"""
assert old in s, "fut json"
s = s.replace(old, new)

open(p, 'w', encoding='utf-8').write(s)
print("symbols done")
