//! The `llm.toml` watcher's dedup ring (R58a).
//!
//! The daemon's own write produces exactly the filesystem event a hand edit
//! does. Without the ring the poll watcher re-runs the whole reload one
//! interval after every settings write — and step 2 of that reload puts the
//! config's `[models]` rows back, so a provider the owner had just disabled
//! would have its models re-listed while the provider itself stayed unloaded:
//! a model the picker offers and the router cannot serve.

use super::*;

use openalpaca_llm::ProviderType;
use tempfile::TempDir;

use crate::test_util::{HomeStoreGuard, MockOllama};

/// Ollama is on and owns a `[models]` row; the default model is Anthropic's,
/// so turning Ollama off is allowed. No provider carries a key, so nothing
/// here can reach a provider's API — and Ollama's `base_url` is a loopback
/// port nothing can bind, so a discovery pass reaches no real Ollama on the
/// developer's machine.
const CONFIG: &str = r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true

[providers.ollama]
enabled = true
base_url = "http://127.0.0.1:1/v1"

[models."llama3.1"]
provider = "ollama"
context = 8192
"#;

struct Harness {
    _home: TempDir,
    _env: HomeStoreGuard,
    _dir: TempDir,
    path: std::path::PathBuf,
    hashes: ConfigHashes,
    router: Arc<openalpaca_llm::LlmRouter>,
    secret_store: Arc<dyn openalpaca_llm::SecretStore>,
    service: openalpaca_llm::LlmSettingsService,
    web_search: ArcSwap<openalpaca_llm::WebSearchConfig>,
}

impl Harness {
    fn new() -> Self {
        Self::with_config(CONFIG)
    }

    fn with_config(seed: impl Into<String>) -> Self {
        let seed = seed.into();
        let home = TempDir::new().expect("home");
        let env = HomeStoreGuard::set_with_master_key(home.path());
        let dir = TempDir::new().expect("config");
        let path = dir.path().join("llm.toml");
        std::fs::write(&path, &seed).expect("seed llm.toml");

        let router = Arc::new(openalpaca_llm::build_router(&path).expect("router"));
        let secret_store: Arc<dyn openalpaca_llm::SecretStore> =
            Arc::new(openalpaca_llm::MemorySecretStore::new());
        let hashes = new_config_hashes();
        let service = openalpaca_llm::LlmSettingsService::new_with_secret_store(
            router.clone(),
            path.clone(),
            secret_store.clone(),
        )
        .expect("settings service")
        // The daemon's own hook — the writer that records what it wrote.
        .with_config_writer(crate::services::llm::atomic_config_writer(hashes.clone()));

        Self {
            _home: home,
            _env: env,
            _dir: dir,
            path,
            hashes,
            router,
            secret_store,
            service,
            web_search: ArcSwap::from_pointee(openalpaca_llm::WebSearchConfig::default()),
        }
    }

    fn tick(&self) -> bool {
        llm_config_watcher_tick(
            &self.hashes,
            &self.router,
            &*self.secret_store,
            &self.web_search,
            &self.path,
        )
    }

    /// The whole watcher event, L13's registration included — what the
    /// daemon really runs for one `llm.toml` change.
    async fn reload(&self) -> bool {
        llm_config_watcher_reload(
            &self.hashes,
            &self.router,
            Some(&self.service),
            &*self.secret_store,
            &self.web_search,
            &self.path,
        )
        .await
    }

    fn ollama_models(&self) -> Option<ProviderType> {
        self.router.model_registry().resolve_provider("llama3.1")
    }
}

/// Anthropic only: Ollama is not in the file at all, so the router boots
/// without it — the shape L13 is about.
const NO_OLLAMA: &str = r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true
"#;

#[tokio::test]
async fn a_toggles_own_write_does_not_put_the_disabled_providers_models_back() {
    let h = Harness::new();
    assert_eq!(h.ollama_models(), Some(ProviderType::Ollama));

    h.service
        .set_provider_enabled("ollama", false)
        .await
        .expect("disable");
    assert_eq!(h.ollama_models(), None, "the disable stripped them");

    let reloaded = h.tick();

    assert!(!reloaded, "the daemon's own write is swallowed, not reloaded");
    assert_eq!(
        h.ollama_models(),
        None,
        "one poll interval later the models are still gone"
    );
}

#[tokio::test]
async fn the_ring_swallows_one_event_per_write_and_no_more() {
    let h = Harness::new();
    h.service
        .set_provider_enabled("ollama", false)
        .await
        .expect("disable");

    assert!(!h.tick(), "the first event is the daemon's own");
    assert!(
        h.tick(),
        "a second event for the same bytes is a real one and is applied"
    );
    assert_eq!(
        h.ollama_models(),
        None,
        "and even applied, the reload leaves a disabled provider out (R58b)"
    );
}

/// R62. The ring is only safe because no hot path depends on the watcher
/// running. `PUT /v1/orchestrator/config` used to be the exception: it wrote
/// unlocked, the ring never saw the hash, and step 3 of the tick
/// (`router.set_default_model` — the only caller in the workspace) applied the
/// picked model one poll interval later. Routed through `persist_only`, the
/// ring began swallowing that event, and the model reached the file and nothing
/// else.
///
/// So the handler applies its own effect after the write. This asserts both
/// halves at once: the router has already moved when the tick runs, and the
/// tick still declines to reload.
#[tokio::test]
async fn a_model_pick_reaches_the_router_though_the_ring_swallows_its_event() {
    let h = Harness::new();
    assert_eq!(h.router.default_model(), "claude-haiku-4-5-20251001");

    h.service
        .update_orchestrator_config(openalpaca_llm::UpdateOrchestratorRequest {
            model: "claude-sonnet-4-6".to_string(),
            fallback_models: vec![],
        })
        .expect("pick a model");

    assert_eq!(
        h.router.default_model(),
        "claude-sonnet-4-6",
        "the handler applied it, not the watcher"
    );
    assert!(!h.tick(), "the daemon's own write is still swallowed");
    assert_eq!(
        h.router.default_model(),
        "claude-sonnet-4-6",
        "and a swallowed event takes nothing back"
    );
}

/// The other side of the same coin: an edit the daemon did not make is exactly
/// what the watcher is for, and step 3 still applies it.
#[tokio::test]
async fn an_external_edit_still_moves_the_default_model_through_the_watcher() {
    let h = Harness::new();
    let edited = CONFIG.replace("claude-haiku-4-5-20251001", "claude-opus-4-6");
    std::fs::write(&h.path, &edited).expect("hand edit");

    assert!(h.tick(), "a hand edit is not the daemon's write");
    assert_eq!(h.router.default_model(), "claude-opus-4-6");
}

#[tokio::test]
async fn a_hand_edit_still_reloads() {
    let h = Harness::new();
    let edited = format!("{CONFIG}\n[web_search]\ntimeout_secs = 42\n");
    std::fs::write(&h.path, &edited).expect("hand edit");

    assert!(h.tick(), "a hand edit is not the daemon's write");
    assert_eq!(h.web_search.load().timeout_secs, 42);
}

/// **L13.** Hand-editing `llm.toml` to enable a provider the daemon booted
/// without used to do nothing until a restart: the tick reloads runtime
/// config, models, the default and the key pools, and every one of those
/// needs the provider to already be in the router. Now the edit registers it
/// live — the same registration the toggle route performs, discovery
/// included, so both ways of saying "on" mean the same thing.
#[tokio::test]
async fn a_hand_edit_that_enables_a_provider_registers_it_live() {
    let ollama = MockOllama::start(&["qwen3:8b"], 32_768).await;
    let h = Harness::with_config(NO_OLLAMA);
    assert!(
        !h.router.has_provider(&ProviderType::Ollama),
        "the daemon booted without it"
    );

    std::fs::write(
        &h.path,
        format!(
            "{NO_OLLAMA}\n[providers.ollama]\nenabled = true\nbase_url = \"{}\"\n",
            ollama.base_url
        ),
    )
    .expect("hand edit");

    assert!(h.reload().await, "a hand edit is not the daemon's write");

    assert!(
        h.router.has_provider(&ProviderType::Ollama),
        "the provider the file now enables is loaded"
    );
    assert_eq!(
        h.router.model_registry().resolve_provider("qwen3:8b"),
        Some(ProviderType::Ollama),
        "and its installed models were discovered, not waited for"
    );
}

/// A provider that is already loaded is left exactly as it is: the reload must
/// not tear down and rebuild a live provider on every unrelated edit.
#[tokio::test]
async fn an_edit_does_not_re_register_a_provider_that_is_already_loaded() {
    let h = Harness::new();
    assert!(h.router.has_provider(&ProviderType::Ollama));

    let edited = format!("{CONFIG}\n[web_search]\ntimeout_secs = 42\n");
    std::fs::write(&h.path, &edited).expect("hand edit");

    assert!(h.reload().await);
    assert!(h.router.has_provider(&ProviderType::Ollama));
    assert_eq!(
        h.ollama_models(),
        Some(ProviderType::Ollama),
        "its declared [models] row is untouched"
    );
    assert_eq!(h.web_search.load().timeout_secs, 42);
}
