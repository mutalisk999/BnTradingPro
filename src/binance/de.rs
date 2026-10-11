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
