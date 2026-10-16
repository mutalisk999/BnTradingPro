//! serde 反序列化助手：Binance 返回的数值字段可能带引号（字符串）也可能不带（JSON number），
//! 统一反序列化为 `rust_decimal::Decimal`。

use rust_decimal::Decimal;
use serde::de::{Deserializer, Error as _, Unexpected};
use serde::Deserialize;
use serde_json::Value;

pub fn dec<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    let v = Value::deserialize(d)?;
    value_to_dec(&v)
        .ok_or_else(|| D::Error::invalid_type(Unexpected::Other("非数值"), &"数字或字符串数字"))
}

pub fn opt_dec<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
    let v = Option::<Value>::deserialize(d)?;
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(v) => value_to_dec(&v).map(Some).ok_or_else(|| {
            D::Error::invalid_type(Unexpected::Other("非数值"), &"数字或字符串数字")
        }),
    }
}

/// 数值字段容错归零：null / 空串 / 非数值一律得 0（用于「0 = 未设置」语义的
/// 可缺省数值字段，如 positionRisk 的仓位止盈止损触发价）。
/// 不可用 `dec`（null 直接报错、会拖垮整个列表的解析）也不可用
/// `default`（只兜底键缺失，兜不住键存在但值为 null 的情形）
pub fn dec0<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match &v {
        Value::Null => Decimal::ZERO,
        Value::String(s) if s.trim().is_empty() => Decimal::ZERO,
        _ => value_to_dec(&v).unwrap_or(Decimal::ZERO),
    })
}

pub fn value_to_dec(v: &Value) -> Option<Decimal> {
    match v {
        Value::String(s) => Decimal::from_str_exact(s.trim()).ok(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Decimal::from(i))
            } else {
                Decimal::from_str_exact(&n.to_string()).ok()
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct T {
        #[serde(deserialize_with = "dec")]
        a: Decimal,
        #[serde(deserialize_with = "opt_dec")]
        b: Option<Decimal>,
        #[serde(deserialize_with = "opt_dec")]
        c: Option<Decimal>,
    }

    #[test]
    fn accepts_string_number_and_null() {
        let t: T = serde_json::from_str(r#"{"a":"30000.12","b":"0.5","c":null}"#).unwrap();
        assert_eq!(t.a.to_string(), "30000.12");
        assert_eq!(t.b.unwrap().to_string(), "0.5");
        assert!(t.c.is_none());

        let t: T = serde_json::from_str(r#"{"a":30000.12,"b":3,"c":"-1.5"}"#).unwrap();
        assert_eq!(t.a.to_string(), "30000.12");
        assert_eq!(t.b.unwrap().to_string(), "3");
        assert_eq!(t.c.unwrap().to_string(), "-1.5");
    }
}
