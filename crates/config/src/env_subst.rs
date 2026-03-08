use std::sync::LazyLock;

use regex::Regex;

static ENV_VAR_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$\{([^}:]+)(?::-(.*?))?\}").unwrap());

/// Replace `${VAR}` and `${VAR:-default}` patterns with environment variable values.
pub fn substitute_env_vars(input: &str) -> String {
    ENV_VAR_PATTERN
        .replace_all(input, |caps: &regex::Captures| {
            let var_name = &caps[1];
            let default_value = caps.get(2).map(|m| m.as_str());
            std::env::var(var_name).unwrap_or_else(|_| default_value.unwrap_or("").to_string())
        })
        .to_string()
}

/// Walk a YAML value tree and substitute env vars in all string values.
pub fn substitute_env_vars_in_value(value: &mut serde_yml::Value) {
    match value {
        serde_yml::Value::String(s) => {
            *s = substitute_env_vars(s);
        }
        serde_yml::Value::Mapping(map) => {
            for (_, v) in map.iter_mut() {
                substitute_env_vars_in_value(v);
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for v in seq.iter_mut() {
                substitute_env_vars_in_value(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_simple_var() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::set_var("TEST_SUBST_VAR", "hello") };
        assert_eq!(substitute_env_vars("${TEST_SUBST_VAR}"), "hello");
        unsafe { std::env::remove_var("TEST_SUBST_VAR") };
    }

    #[test]
    fn test_substitute_with_default() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::remove_var("NONEXISTENT_VAR_12345") };
        assert_eq!(
            substitute_env_vars("${NONEXISTENT_VAR_12345:-fallback}"),
            "fallback"
        );
    }

    #[test]
    fn test_substitute_missing_no_default() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::remove_var("NONEXISTENT_VAR_67890") };
        assert_eq!(substitute_env_vars("${NONEXISTENT_VAR_67890}"), "");
    }

    #[test]
    fn test_substitute_in_context() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::set_var("TEST_PORT", "8080") };
        assert_eq!(
            substitute_env_vars("http://localhost:${TEST_PORT}/api"),
            "http://localhost:8080/api"
        );
        unsafe { std::env::remove_var("TEST_PORT") };
    }

    #[test]
    fn test_no_substitution_needed() {
        assert_eq!(substitute_env_vars("plain text"), "plain text");
    }

    #[test]
    fn test_substitute_in_yaml_value() {
        // SAFETY: test-only env var manipulation
        unsafe { std::env::set_var("TEST_YAML_TOKEN", "secret123") };
        let mut value =
            serde_yml::from_str::<serde_yml::Value>("token: \"${TEST_YAML_TOKEN}\"\nport: 3000")
                .unwrap();
        substitute_env_vars_in_value(&mut value);

        let mapping = value.as_mapping().unwrap();
        let token = mapping
            .get(&serde_yml::Value::String("token".into()))
            .unwrap();
        assert_eq!(token.as_str().unwrap(), "secret123");
        unsafe { std::env::remove_var("TEST_YAML_TOKEN") };
    }
}
