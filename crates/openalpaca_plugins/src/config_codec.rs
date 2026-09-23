//! Plugin configuration values as JSON.
//!
//! A plugin's config is stored as TOML and travels as JSON — to the plugin in
//! `initialize`, and to a client from `GET /v1/extensions/{kind}/{id}/config`.
//! One conversion serves both, so the two cannot drift. The reverse direction
//! (`json_to_toml`, the config write) has its own null and number policy and
//! lives with the route that owns it.

use serde_json::Value;

/// Convert a `toml::Value` to a `serde_json::Value`.
///
/// A datetime becomes its TOML string form, a non-finite float becomes `null`
/// (JSON has no NaN or infinity), and arrays and tables convert recursively.
pub fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<String, Value> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_to_json_primitives() {
        assert_eq!(
            toml_to_json(&toml::Value::String("hello".into())),
            Value::String("hello".into())
        );
        assert_eq!(
            toml_to_json(&toml::Value::Integer(42)),
            Value::Number(42.into())
        );
        assert_eq!(toml_to_json(&toml::Value::Boolean(true)), Value::Bool(true));
    }

    #[test]
    fn test_toml_to_json_nested() {
        let mut tbl = toml::map::Map::new();
        tbl.insert("key".into(), toml::Value::String("val".into()));
        tbl.insert(
            "arr".into(),
            toml::Value::Array(vec![toml::Value::Integer(1), toml::Value::Integer(2)]),
        );
        let json = toml_to_json(&toml::Value::Table(tbl));
        assert!(json.is_object());
        assert_eq!(json["key"], "val");
        assert_eq!(json["arr"], serde_json::json!([1, 2]));
    }

    #[test]
    fn toml_to_json_floats_datetimes_and_nesting() {
        assert_eq!(
            toml_to_json(&toml::Value::Float(1.5)),
            serde_json::json!(1.5)
        );
        for non_finite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(toml_to_json(&toml::Value::Float(non_finite)), Value::Null);
        }
        let parsed: toml::Table = toml::from_str(
            "when = 1979-05-27T07:32:00Z\n\
             day = 1979-05-27\n\
             at = 07:32:00\n\
             nested = [[1, 2.5], [{ inner = 1979-05-27T00:32:00-07:00 }]]\n",
        )
        .unwrap();
        assert_eq!(
            toml_to_json(&toml::Value::Table(parsed)),
            serde_json::json!({
                "when": "1979-05-27T07:32:00Z",
                "day": "1979-05-27",
                "at": "07:32:00",
                "nested": [[1, 2.5], [{ "inner": "1979-05-27T00:32:00-07:00" }]],
            })
        );
    }
}
