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

use crate::test_util::HomeStoreGuard;

/// Ollama is on and owns a `[models]` row; the default model is Anthropic's,
/// so turning Ollama off is allowed. No provider carries a key, so nothing
/// here can reach a provider's API.
const CONFIG: &str = r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true

[providers.ollama]
enabled = true
base_url = "http://localhost:11434/v1"

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
        let home = TempDir::new().expect("home");
        let env = HomeStoreGuard::set_with_master_key(home.path());
        let dir = TempDir::new().expect("config");
        let path = dir.path().join("llm.toml");
        std::fs::write(&path, CONFIG).expect("seed llm.toml");

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

    fn ollama_models(&self) -> Option<ProviderType> {
        self.router.model_registry().resolve_provider("llama3.1")
    }
}

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

#[tokio::test]
async fn a_hand_edit_still_reloads() {
    let h = Harness::new();
    let edited = format!("{CONFIG}\n[web_search]\ntimeout_secs = 42\n");
    std::fs::write(&h.path, &edited).expect("hand edit");

    assert!(h.tick(), "a hand edit is not the daemon's write");
    assert_eq!(h.web_search.load().timeout_secs, 42);
}
