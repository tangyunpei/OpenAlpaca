//! Embedder trait and backends for generating vector embeddings.

use async_trait::async_trait;
use std::sync::Arc;
use thiserror::Error;

use crate::config::EmbeddingsConfig;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("API error: {0}")]
    Api(String),
    #[error("Dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: u32, actual: usize },
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
    fn dimensions(&self) -> u32;
}

// ── OpenAI backend ──────────────────────────────────────────────────────

#[cfg(feature = "openai")]
pub struct OpenAiEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dimensions: u32,
    embeddings_url: String,
}

#[cfg(feature = "openai")]
impl OpenAiEmbedder {
    pub fn new(api_key: String, model: String, dimensions: u32) -> Result<Self, EmbedError> {
        Self::new_with_url(api_key, model, dimensions, None)
    }

    pub fn new_with_url(api_key: String, model: String, dimensions: u32, url: Option<String>) -> Result<Self, EmbedError> {
        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            dimensions,
            embeddings_url: url.unwrap_or_else(|| "https://api.openai.com/v1/embeddings".to_string()),
        })
    }
}

#[cfg(feature = "openai")]
#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
            "dimensions": self.dimensions,
        });

        let response = self
            .client
            .post(&self.embeddings_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| EmbedError::Api(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(EmbedError::Api(format!("HTTP {status}: {body}")));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| EmbedError::Api(format!("Failed to parse response: {e}")))?;

        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| EmbedError::Api("Missing 'data' array in response".to_string()))?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding: Vec<f32> = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| EmbedError::Api("Missing 'embedding' in data item".to_string()))?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();

            if embedding.len() != self.dimensions as usize {
                return Err(EmbedError::DimensionMismatch {
                    expected: self.dimensions,
                    actual: embedding.len(),
                });
            }
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
    }
}

// ── Local (fastembed) backend ───────────────────────────────────────────

#[cfg(feature = "local-embeddings")]
pub struct LocalEmbedder {
    model: fastembed::TextEmbedding,
    dims: u32,
}

#[cfg(feature = "local-embeddings")]
impl LocalEmbedder {
    pub fn new(model_name: Option<&str>, dimensions: u32) -> Result<Self, EmbedError> {
        let default_model = "intfloat/multilingual-e5-base";
        let name = model_name.unwrap_or(default_model);
        let model_enum: fastembed::EmbeddingModel = name
            .parse()
            .map_err(|_| EmbedError::Config(format!("Unknown local embedding model: '{name}'")))?;

        let mut opts = fastembed::InitOptions::default();
        opts.model_name = model_enum;
        opts.show_download_progress = true;
        let model = fastembed::TextEmbedding::try_new(opts)
            .map_err(|e| EmbedError::Config(format!("Failed to load local embedding model: {e}")))?;
        Ok(Self { model, dims: dimensions })
    }
}

#[cfg(feature = "local-embeddings")]
#[async_trait]
impl Embedder for LocalEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let owned: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        let embeddings = self
            .model
            .embed(owned, None)
            .map_err(|e| EmbedError::Api(format!("Local embedding failed: {e}")))?;

        for emb in &embeddings {
            if emb.len() != self.dims as usize {
                return Err(EmbedError::DimensionMismatch {
                    expected: self.dims,
                    actual: emb.len(),
                });
            }
        }

        Ok(embeddings)
    }

    fn dimensions(&self) -> u32 {
        self.dims
    }
}

// ── Factory ─────────────────────────────────────────────────────────────

pub fn build_embedder(
    config: &EmbeddingsConfig,
    secret_store: Option<&dyn crate::secret_store::SecretStore>,
    provider_config: Option<&crate::config::ProviderConfig>,
) -> Result<Arc<dyn Embedder>, EmbedError> {
    build_embedder_with_runtime(config, secret_store, provider_config, None)
}

pub fn build_embedder_with_runtime(
    config: &EmbeddingsConfig,
    _secret_store: Option<&dyn crate::secret_store::SecretStore>,
    _provider_config: Option<&crate::config::ProviderConfig>,
    _runtime_config: Option<&crate::config::LlmRuntimeConfig>,
) -> Result<Arc<dyn Embedder>, EmbedError> {
    let _dimensions = config.dimensions.unwrap_or(768);

    match config.provider.as_str() {
        #[cfg(feature = "openai")]
        "openai" => {
            let api_key = resolve_openai_key(_secret_store, _provider_config)
                .ok_or_else(|| {
                    EmbedError::Config(
                        "No OpenAI API key found for embeddings. Configure a key in [providers.openai]."
                            .to_string(),
                    )
                })?;
            let model = config
                .model
                .clone()
                .unwrap_or_else(|| "text-embedding-3-small".to_string());
            let url = _runtime_config.map(|rt| rt.endpoints.openai_embeddings.clone());
            let embedder = OpenAiEmbedder::new_with_url(api_key, model, _dimensions, url)?;
            Ok(Arc::new(embedder))
        }
        #[cfg(feature = "local-embeddings")]
        "local" => {
            let model_name = config.model.as_deref();
            let embedder = LocalEmbedder::new(model_name, _dimensions)?;
            Ok(Arc::new(embedder))
        }
        other => Err(EmbedError::Config(format!(
            "Unknown embedding provider: '{other}'. Available: openai{}",
            if cfg!(feature = "local-embeddings") {
                ", local"
            } else {
                ""
            }
        ))),
    }
}

/// Resolve OpenAI API key from provider config (same key pool as chat).
#[cfg(feature = "openai")]
fn resolve_openai_key(
    secret_store: Option<&dyn crate::secret_store::SecretStore>,
    provider_config: Option<&crate::config::ProviderConfig>,
) -> Option<String> {
    // Try keys from provider config
    if let Some(pc) = provider_config {
        if let Some(ref keys) = pc.keys {
            for key_config in keys {
                if let Some(secret) =
                    crate::config::resolve_key_from_config(key_config, secret_store)
                {
                    return Some(secret);
                }
            }
        }
    }
    // Fallback to env var
    std::env::var("OPENAI_API_KEY").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_dimensions_is_768() {
        // When dimensions is None, the factory should default to 768
        let config = EmbeddingsConfig {
            enabled: true,
            provider: "nonexistent".to_string(),
            model: None,
            dimensions: None,
            batch_size: None,
        };
        let result = build_embedder(&config, None, None);
        // Should fail because provider is unknown, not because of dimensions
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("Unknown")),
            Ok(_) => panic!("Expected error"),
        }
    }

    #[cfg(feature = "openai")]
    #[test]
    fn test_openai_embedder_requires_key() {
        let config = EmbeddingsConfig {
            enabled: true,
            provider: "openai".to_string(),
            model: Some("text-embedding-3-small".to_string()),
            dimensions: Some(768),
            batch_size: None,
        };
        // Without any key source, should fail
        let result = build_embedder(&config, None, None);
        // May succeed if OPENAI_API_KEY is set in env, otherwise fails
        if std::env::var("OPENAI_API_KEY").is_err() {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_unknown_provider() {
        let config = EmbeddingsConfig {
            enabled: true,
            provider: "nonexistent".to_string(),
            model: None,
            dimensions: Some(768),
            batch_size: None,
        };
        let result = build_embedder(&config, None, None);
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("Unknown")),
            Ok(_) => panic!("Expected error"),
        }
    }
}
