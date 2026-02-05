use crate::error::LlmError;
use crate::LlmProvider;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
}

impl LlmConfig {
    pub fn from_file(path: &std::path::Path) -> Result<Self, LlmError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| LlmError::Config(format!("Failed to read {}: {}", path.display(), e)))?;
        toml::from_str(&content)
            .map_err(|e| LlmError::Config(format!("Failed to parse {}: {}", path.display(), e)))
    }

    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(ref key) = self.api_key {
            return Some(key.clone());
        }
        let env_var = match self.provider.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            _ => return None,
        };
        std::env::var(env_var).ok()
    }
}

pub fn build_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    match config.provider.as_str() {
        #[cfg(feature = "anthropic")]
        "anthropic" => {
            let api_key = config
                .resolve_api_key()
                .ok_or(LlmError::Config("Anthropic API key not configured. Set api_key in config or ANTHROPIC_API_KEY env var.".into()))?;
            let provider = crate::providers::anthropic::AnthropicProvider::new(
                api_key,
                config.model.clone(),
                config.max_tokens,
            );
            Ok(Box::new(provider))
        }
        #[cfg(feature = "openai")]
        "openai" => {
            let api_key = config
                .resolve_api_key()
                .ok_or(LlmError::Config("OpenAI API key not configured. Set api_key in config or OPENAI_API_KEY env var.".into()))?;
            let provider = crate::providers::openai::OpenAiProvider::new(
                api_key,
                config.model.clone(),
                config.base_url.clone(),
                config.max_tokens,
            );
            Ok(Box::new(provider))
        }
        #[cfg(feature = "ollama")]
        "ollama" => {
            let provider = crate::providers::ollama::OllamaProvider::new(
                config.model.clone().unwrap_or_else(|| "llama3".to_string()),
                config.base_url.clone(),
            );
            Ok(Box::new(provider))
        }
        other => Err(LlmError::UnknownProvider(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_from_toml() {
        let toml_str = r#"
provider = "anthropic"
model = "claude-sonnet-4-5-20250929"
max_tokens = 4096
"#;
        let config: LlmConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert_eq!(config.max_tokens, Some(4096));
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_resolve_api_key_env() {
        let config = LlmConfig {
            provider: "anthropic".to_string(),
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
        };
        // Without env var set, resolve returns None (unless env is set externally)
        // We just verify the method doesn't panic
        let _ = config.resolve_api_key();
    }

    #[test]
    fn test_resolve_api_key_config_value() {
        let config = LlmConfig {
            provider: "anthropic".to_string(),
            model: None,
            api_key: Some("sk-test-key".to_string()),
            base_url: None,
            max_tokens: None,
        };
        assert_eq!(config.resolve_api_key(), Some("sk-test-key".to_string()));
    }

    #[test]
    fn test_build_provider_unknown() {
        let config = LlmConfig {
            provider: "unknown_provider".to_string(),
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
        };
        let result = build_provider(&config);
        assert!(result.is_err());
        let err = result.err().unwrap();
        match err {
            LlmError::UnknownProvider(name) => assert_eq!(name, "unknown_provider"),
            other => panic!("Expected UnknownProvider, got: {:?}", other),
        }
    }
}
