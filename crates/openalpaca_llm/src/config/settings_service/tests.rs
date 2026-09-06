//! GAP-15's two halves: the write (`persist_only`, routed through whatever
//! atomic writer the host injected) and the hot path (`deregister_provider` on
//! disable, re-register + `refresh_models` on enable).
//!
//! This crate compiles with **no provider features** under
//! `cargo test -p openalpaca_llm`, so nothing here may depend on a real
//! Anthropic/OpenAI/Ollama provider existing. The router is driven with a stub
//! provider through the public `register_provider`, and the enable half's
//! *registration* is proved in `openalpacad`, which does compile them.

use super::*;

use crate::keys::key_encryption::KeyEncryptor;
use crate::routing::cost_tracker::CostTracker;
use crate::routing::model_registry::ModelRegistry;
use crate::{ChatRequest, ChatResponse, LlmProvider, error::LlmError};
use std::path::Path;
use std::sync::Mutex;

/// A hand-authored `llm.toml` carrying an encrypted secret — the bytes that
/// must survive every rewrite verbatim.
const SECRET: &str = "enc:v1:YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXo=";

fn hand_authored() -> String {
    format!(
        r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true
strategy = "round_robin"

[[providers.anthropic.keys]]
id = "key_one"
secret_encrypted = "{SECRET}"
priority = "primary"

[providers.openai]
enabled = true
base_url = "https://api.openai.com/v1"
strategy = "round_robin"
"#
    )
}

/// The stub the router registers, so `deregister_provider` has something real
/// to remove without any provider feature being compiled in.
struct StubProvider;

#[async_trait::async_trait]
impl LlmProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }
    fn supports_tools(&self) -> bool {
        false
    }
    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::NotConfigured)
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    path: PathBuf,
    service: LlmSettingsService,
    router: Arc<LlmRouter>,
    /// Every `(path, contents)` the injected writer was handed.
    writes: Arc<Mutex<Vec<(PathBuf, String)>>>,
    /// Flipped to make the next write fail, for the write-first proof.
    fail: Arc<Mutex<bool>>,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("llm.toml");
        std::fs::write(&path, hand_authored()).unwrap();

        let router = Arc::new(LlmRouter::new(
            HashMap::new(),
            ModelRegistry::with_defaults(),
            HashMap::new(),
            Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
            "claude-haiku-4-5-20251001".to_string(),
        ));

        let writes = Arc::new(Mutex::new(Vec::new()));
        let fail = Arc::new(Mutex::new(false));
        let (seen, failing) = (writes.clone(), fail.clone());

        let encryptor = KeyEncryptor::load_or_generate_at(dir.path()).unwrap();
        let service = LlmSettingsService::for_tests(router.clone(), path.clone(), encryptor)
            .with_config_writer(Arc::new(move |p: &Path, contents: &str| {
                if *failing.lock().unwrap() {
                    return Err("disk is full".to_string());
                }
                seen.lock()
                    .unwrap()
                    .push((p.to_path_buf(), contents.to_string()));
                std::fs::write(p, contents).map_err(|e| e.to_string())
            }));

        Self {
            _dir: dir,
            path,
            service,
            router,
            writes,
            fail,
        }
    }

    /// Register a stub under `provider_type` so the router has a live entry.
    fn register_stub(&self, provider_type: ProviderType) {
        self.router.register_provider(
            provider_type,
            Arc::new(StubProvider),
            KeyPool::new(vec![], SelectionStrategy::RoundRobin),
        );
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }
}

// ── The write ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_write_goes_through_the_injected_atomic_writer() {
    let h = Harness::new();

    h.service.set_provider_enabled("openai", false).await.unwrap();

    let writes = h.writes.lock().unwrap();
    assert_eq!(writes.len(), 1, "one write per toggle");
    assert_eq!(writes[0].0, h.path, "the writer is handed llm.toml itself");
    assert!(
        writes[0].1.contains("[providers.openai]"),
        "the writer is handed the whole rendered document"
    );
    drop(writes);
    assert!(h.text().contains("enabled = false"));
}

#[tokio::test]
async fn a_disable_enable_cycle_changes_only_the_enabled_key() {
    let h = Harness::new();

    // The encrypted secret survives the very first rewrite, which is the one
    // that normalises a hand-authored file into the serialiser's own layout.
    h.service.set_provider_enabled("openai", false).await.unwrap();
    let off = h.text();
    assert!(off.contains(SECRET), "the encrypted key is copied verbatim");

    h.service.set_provider_enabled("openai", true).await.unwrap();
    let on = h.text();
    assert!(on.contains(SECRET));

    let changed: Vec<(&str, &str)> = off
        .lines()
        .zip(on.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(
        off.lines().count(),
        on.lines().count(),
        "the document may not grow or shrink:\n--- off\n{off}\n--- on\n{on}"
    );
    assert_eq!(
        changed,
        vec![("enabled = false", "enabled = true")],
        "exactly one key may move:\n--- off\n{off}\n--- on\n{on}"
    );
}

#[tokio::test]
async fn a_failed_write_leaves_the_router_untouched() {
    let h = Harness::new();
    h.register_stub(ProviderType::OpenAI);
    *h.fail.lock().unwrap() = true;

    let err = h
        .service
        .set_provider_enabled("openai", false)
        .await
        .expect_err("a refused write is not a toggle");
    assert!(matches!(err, SetProviderEnabledError::Persist(_)), "{err:?}");

    assert_eq!(h.text(), hand_authored(), "the file is byte-identical");
    assert!(
        h.router.configured_providers().contains(&ProviderType::OpenAI),
        "write-first: the provider is still loaded"
    );
    assert!(
        h.router
            .model_registry()
            .resolve_provider("gpt-5.2")
            .is_some(),
        "and its models are still registered"
    );
}

// ── The refusals ────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_unknown_provider_is_refused_before_anything_is_written() {
    let h = Harness::new();

    let err = h
        .service
        .set_provider_enabled("groq", false)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, SetProviderEnabledError::UnknownProvider(name) if name == "groq"),
        "{err:?}"
    );
    assert!(h.writes.lock().unwrap().is_empty());
    assert_eq!(h.text(), hand_authored());
}

#[tokio::test]
async fn the_default_models_provider_cannot_be_disabled() {
    let h = Harness::new();
    h.register_stub(ProviderType::Anthropic);

    // `[orchestrator] model` is a Claude model, so Anthropic is the default's
    // provider and the switch must refuse rather than strand the daemon.
    let err = h
        .service
        .set_provider_enabled("anthropic", false)
        .await
        .unwrap_err();
    match &err {
        SetProviderEnabledError::IsDefaultProvider { provider, model } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(model, "claude-haiku-4-5-20251001");
        }
        other => panic!("{other:?}"),
    }

    assert!(h.writes.lock().unwrap().is_empty());
    assert!(h.router.configured_providers().contains(&ProviderType::Anthropic));

    // Enabling it is never refused — the guard is about turning the default off.
    h.service
        .set_provider_enabled("anthropic", true)
        .await
        .unwrap();
}

// ── The hot path ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_disable_unloads_the_provider_and_strips_its_models() {
    let h = Harness::new();
    h.register_stub(ProviderType::OpenAI);
    assert!(
        h.router
            .model_registry()
            .resolve_provider("gpt-5.2")
            .is_some()
    );

    let outcome = h.service.set_provider_enabled("openai", false).await.unwrap();

    assert_eq!(outcome.id, "openai");
    assert!(!outcome.enabled);
    assert!(
        outcome.removed_models.iter().any(|m| m == "gpt-5.2"),
        "the stripped models are reported: {:?}",
        outcome.removed_models
    );
    assert!(
        !h.router.configured_providers().contains(&ProviderType::OpenAI),
        "the provider is unloaded, so new calls fall through to the chain"
    );
    assert!(
        h.router
            .model_registry()
            .resolve_provider("gpt-5.2")
            .is_none()
    );
    // Anthropic is untouched.
    assert!(
        h.router
            .model_registry()
            .resolve_provider("claude-sonnet-4-6")
            .is_some()
    );
}

#[tokio::test]
async fn an_enable_restores_the_catalogue_the_disable_stripped() {
    let h = Harness::new();
    h.register_stub(ProviderType::OpenAI);
    h.service.set_provider_enabled("openai", false).await.unwrap();
    assert!(
        h.router
            .model_registry()
            .resolve_provider("gpt-5.2")
            .is_none()
    );

    let outcome = h.service.set_provider_enabled("openai", true).await.unwrap();

    assert!(outcome.enabled);
    assert!(
        outcome.restored_models > 0,
        "re-enabling puts the provider's catalogue back"
    );
    assert!(
        h.router
            .model_registry()
            .resolve_provider("gpt-5.2")
            .is_some(),
        "otherwise the model picker stays empty until the daemon restarts"
    );
    // No provider feature is compiled in here, so registration itself cannot
    // succeed — and that is reported rather than swallowed or 500'd.
    assert!(outcome.warning.is_some(), "{outcome:?}");
    assert!(h.text().contains("enabled = true"));
}
