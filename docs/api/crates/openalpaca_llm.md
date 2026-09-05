# `openalpaca_llm`

> Generated from source by `python3 scripts/gen_api_docs.py`.

## Overview

- Member path: `crates/openalpaca_llm`
- Entry: `crates/openalpaca_llm/src/lib.rs`

## Modules

- `cli_backend` (crates/openalpaca_llm/src/cli_backend/mod.rs)
- `config` (crates/openalpaca_llm/src/config/mod.rs)
- `context_management` (crates/openalpaca_llm/src/context_management.rs)
- `embedder` (crates/openalpaca_llm/src/embedder.rs)
- `error` (crates/openalpaca_llm/src/error.rs)
- `keys` (crates/openalpaca_llm/src/keys/mod.rs)
- `providers` (crates/openalpaca_llm/src/providers/mod.rs)
- `routing` (crates/openalpaca_llm/src/routing/mod.rs)
- `streaming` (crates/openalpaca_llm/src/streaming.rs)
- `types` (crates/openalpaca_llm/src/types.rs)

## Re-exports

- `pub use cli_backend::{ ClaudeCodeCliProvider, CliBackendConfig, CliBackendStatus, CliBackendsConfig, CodexCliProvider, detect_cli_backends, };`
- `pub use config::llm_config::{ EmbeddingsConfig, EndpointsConfig, EnvVarsConfig, KeyConfig, LlmRouterConfig, LlmRuntimeConfig, ModelConfigEntry, OrchestratorLlmConfig, ProviderConfig, ProviderDefaults, SecurityConfig, TimeoutsConfig, WebSearchConfig, build_router, build_router_with_secret_store, collect_secret_refs, migrate_llm_secrets, read_config, resolve_key_from_config, reverse_migrate_llm_secrets, write_config, };`
- `pub use config::settings_service::{ LlmSettingsService, OrchestratorConfigResponse, UpdateOrchestratorRequest, };`
- `pub use embedder::{EmbedError, Embedder, build_embedder, build_embedder_with_runtime};`
- `pub use error::LlmError;`
- `pub use keys::credential_discovery::{ CredentialDiscoveryConfig, CredentialSource, DiscoveredCredential, DiscoveredCredentialInfo, OAuthToken, TokenManager, };`
- `pub use keys::key_pool::{ ApiKey, CallResult, KeyGuard, KeyHealthStatus, KeyPool, KeyPoolError, KeyPriority, KeySource, KeyStatus, ProviderType, SelectionStrategy, mask_secret, };`
- `pub use keys::secret_store::{ CachingSecretStore, KeyringSecretStore, MemorySecretStore, SecretStore, };`
- `pub use routing::cost_tracker::{ CacheStats, CallRecord, CostSnapshot, CostTracker, ModelUsageStats, UsageStats, };`
- `pub use routing::model_registry::{ModelEntry, ModelInfo, ModelRegistry, PricingInfo};`
- `pub use routing::provider_usage::{ExternalUsage, ProviderUsageSummary, ProviderUsageTracker};`
- `pub use routing::rate_limiter::{ CircuitState, RateLimitConfig, RateLimiterRegistry, backoff_with_jitter, };`
- `pub use routing::router::{ LlmCapacityInfo, LlmRouter, LlmRouterError, ProviderEntry, RequestContext, RouterRequest, };`
- `pub use streaming::collect_stream;`
- `pub use types::*;`

## Related Links

- [API Index](../README.md)
