//! Plugin configuration values shared by initialization and the settings API.

use serde_json::Value;

/// Convert a `toml::Value` to a `serde_json::Value`.
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
    fn converts_nested_configuration_and_toml_only_values() {
        let value: toml::Value = toml::from_str(
            r#"
            name = "hello"
            count = 42
            enabled = true
            fraction = 1.25
            created = 2026-09-21T10:20:30Z
            items = [1, 2]
            [nested]
            token = "secret"
        "#,
        )
        .unwrap();
        let json = toml_to_json(&value);
        assert_eq!(
            json,
            serde_json::json!({
                "name": "hello", "count": 42, "enabled": true, "fraction": 1.25,
                "created": "2026-09-21T10:20:30Z", "items": [1, 2], "nested": {"token": "secret"},
            })
        );
        for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(toml_to_json(&toml::Value::Float(number)), Value::Null);
        }
    }
}
