//! `PUT /v1/settings/llm/providers/{provider}/enabled` (GAP-15): the status
//! codes the route owns, the body it answers with, and the two things that are
//! only observable end to end — that the write really goes through the one
//! atomic writer (a rotated copy appears under `state/backups/`), and that a
//! keyless provider can actually be turned on, which is the case the toggle
//! exists for.
//!
//! The handler itself is one `state.llm_settings_service` lookup over
//! `provider_enabled_response`, which is what these drive: an `AppState` is
//! not constructible in a unit test, and the 503 for a missing service is the
//! only thing that lookup decides.

use super::*;

use axum::body::to_bytes;
use openalpaca_llm::ProviderType;
use tempfile::TempDir;

use crate::test_util::HomeStoreGuard;

/// Anthropic is the default model's provider (so it is the 409 case), Ollama
/// is off and keyless (so it is the enable case). Neither carries a key, so
/// nothing here can reach a provider's API.
const CONFIG: &str = r#"[orchestrator]
model = "claude-haiku-4-5-20251001"

[providers.anthropic]
enabled = true
strategy = "round_robin"

[providers.ollama]
enabled = false
base_url = "http://localhost:11434/v1"

[models."llama3.1"]
provider = "ollama"
context = 8192
"#;

/// As `CONFIG`, but the default model places nowhere: the registry has never
/// seen it, `[models]` does not declare it, and its shape names no provider.
const UNPLACEABLE_DEFAULT: &str = r#"[orchestrator]
model = "my-local-thing"

[providers.anthropic]
enabled = true
strategy = "round_robin"

[providers.ollama]
enabled = false
base_url = "http://localhost:11434/v1"
"#;

struct Harness {
    home: TempDir,
    _env: HomeStoreGuard,
    _config: TempDir,
    path: std::path::PathBuf,
    seed: &'static str,
    service: Arc<openalpaca_llm::LlmSettingsService>,
}

impl Harness {
    fn new() -> Self {
        Self::with_config(CONFIG)
    }

    fn with_config(seed: &'static str) -> Self {
        let home = TempDir::new().expect("home");
        let env = HomeStoreGuard::set_with_master_key(home.path());
        let config = TempDir::new().expect("config");
        let path = config.path().join("llm.toml");
        std::fs::write(&path, seed).expect("seed llm.toml");

        let router = Arc::new(openalpaca_llm::build_router(&path).expect("router"));
        let secret_store: Arc<dyn openalpaca_llm::SecretStore> =
            Arc::new(openalpaca_llm::MemorySecretStore::new());
        let service = openalpaca_llm::LlmSettingsService::new_with_secret_store(
            router,
            path.clone(),
            secret_store,
        )
        .expect("settings service")
        // The daemon's own hook — the point of the test is that this is the
        // writer the route uses.
        .with_config_writer(crate::services::llm::atomic_config_writer(
            crate::hot_reload::new_config_hashes(),
        ));

        Self {
            home,
            _env: env,
            _config: config,
            path,
            seed,
            service: Arc::new(service),
        }
    }

    async fn put(&self, provider: &str, enabled: bool) -> (StatusCode, serde_json::Value) {
        let response = provider_enabled_response(&self.service, provider, enabled).await;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.expect("body");
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).expect("read llm.toml")
    }

    fn backups(&self) -> Vec<String> {
        let dir = self.home.path().join("state").join("backups");
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[tokio::test]
async fn turning_a_keyless_provider_on_answers_the_row_and_loads_it() {
    let h = Harness::new();
    assert!(
        !h.service
            .router()
            .configured_providers()
            .contains(&ProviderType::Ollama),
        "the fixture starts with Ollama off"
    );

    let (status, body) = h.put("ollama", true).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({
            "id": "ollama",
            "enabled": true,
            "loaded": true,
            "warning": null,
        })
    );
    assert!(
        h.service
            .router()
            .configured_providers()
            .contains(&ProviderType::Ollama),
        "an enable re-registers — a keyless provider needs no key to load"
    );
    assert!(h.text().contains("enabled = true"));
}

#[tokio::test]
async fn the_write_rotates_a_backup_because_it_uses_the_one_atomic_writer() {
    let h = Harness::new();
    assert!(h.backups().is_empty());

    h.put("ollama", true).await;

    let kept = h.backups();
    assert_eq!(kept.len(), 1, "the replaced version is kept: {kept:?}");
    assert!(kept[0].starts_with("llm.toml.bak."), "{kept:?}");
    let replaced = std::fs::read_to_string(
        h.home.path().join("state").join("backups").join(&kept[0]),
    )
    .expect("read the backup");
    assert_eq!(replaced, CONFIG, "the backup is the version it replaced");
}

#[tokio::test]
async fn a_disable_strips_the_models_and_a_second_enable_puts_them_back() {
    let h = Harness::new();
    h.put("ollama", true).await;

    let (status, body) = h.put("ollama", false).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({
            "id": "ollama",
            "enabled": false,
            "loaded": false,
            "warning": null,
        }),
        "a disable is not loaded — that is the point of it"
    );
    assert!(
        !h.service
            .router()
            .configured_providers()
            .contains(&ProviderType::Ollama)
    );
    assert_eq!(
        h.service.router().model_registry().resolve_provider("llama3.1"),
        None,
        "a disabled provider's models leave the registry with it"
    );

    // …and come back, or the model picker would stay empty for a provider the
    // owner just switched back on until the daemon restarted.
    let (status, _) = h.put("ollama", true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        h.service.router().model_registry().resolve_provider("llama3.1"),
        Some(ProviderType::Ollama),
        "the config's own [models] rows are re-applied on enable"
    );
}

#[tokio::test]
async fn an_unknown_provider_is_a_404() {
    let h = Harness::new();

    let (status, body) = h.put("groq", false).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "PROVIDER_NOT_FOUND");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("groq")
    );
    assert!(body["error"].get("status").is_none(), "§7: no duplicated status");
    assert_eq!(h.text(), h.seed, "nothing was written");
}

#[tokio::test]
async fn disabling_the_default_models_provider_is_a_409() {
    let h = Harness::new();

    let (status, body) = h.put("anthropic", false).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "PROVIDER_IS_DEFAULT");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("claude-haiku-4-5-20251001"),
        "the refusal names the model that is in the way: {body}"
    );
    assert_eq!(h.text(), h.seed, "nothing was written");
    assert!(h.backups().is_empty(), "a refusal rotates nothing");
}

#[tokio::test]
async fn enabling_the_default_models_provider_is_never_refused() {
    let h = Harness::new();

    let (status, body) = h.put("anthropic", true).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "anthropic");
    assert_eq!(body["enabled"], true);
}

/// R60. The fixture's Anthropic row carries no key, so the file can be written
/// and the router still cannot load the provider. That used to answer
/// `200 {id, enabled: true}` with the failure only in the daemon log: nothing
/// on the wire, and nothing in `GET /v1/settings/llm`, told "on and serving"
/// apart from "on and inert". A 500 would be worse — the write did happen and
/// a restart reaches the same state — so the disposition is reported instead.
#[tokio::test]
async fn an_enable_the_router_cannot_load_says_so_on_the_wire() {
    let h = Harness::new();

    let (status, body) = h.put("anthropic", true).await;

    assert_eq!(status, StatusCode::OK, "the write happened; this is not a failure");
    assert_eq!(body["id"], "anthropic");
    assert_eq!(body["enabled"], true, "the file says what the owner asked for");
    assert_eq!(body["loaded"], false, "and the body says it did not load: {body}");
    let warning = body["warning"].as_str().unwrap_or_default();
    assert!(
        warning.contains("No keys"),
        "the reason travels with it: {body}"
    );
    assert!(
        !h.service
            .router()
            .configured_providers()
            .contains(&ProviderType::Anthropic)
    );
}

/// R61's second arm, at the wire. When the default model resolves to nothing,
/// *every* disable is refused — there would be no way to tell whether the one
/// being turned off is the one that was going to answer — and the message names
/// the model so the owner knows what to fix.
///
/// R61a: its **own** code word. The two arms share a remedy but not a fact:
/// `PROVIDER_IS_DEFAULT` asserts that this provider serves the default model,
/// which is exactly what this arm could not establish. A client that renders
/// per code (the GUI does) would otherwise say a false thing about the provider
/// the owner just tried to turn off.
#[tokio::test]
async fn a_default_model_that_places_nowhere_gets_its_own_code_word() {
    let h = Harness::with_config(UNPLACEABLE_DEFAULT);

    let (status, body) = h.put("anthropic", false).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "DEFAULT_MODEL_UNRESOLVED");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("my-local-thing"),
        "the refusal names the default it could not place: {body}"
    );
    assert_eq!(h.text(), h.seed, "nothing was written");
    assert!(h.backups().is_empty(), "a refusal rotates nothing");
}

/// The other half of R61a: the arm that *can* name the provider keeps
/// `PROVIDER_IS_DEFAULT`, so the two are told apart on the wire and not only in
/// the prose of the message.
///
/// The two fixtures are built and dropped one at a time: `HomeStoreGuard` holds
/// a process-wide `ENV_LOCK` for its lifetime, so two live at once deadlock.
#[tokio::test]
async fn the_two_409_arms_answer_different_code_words() {
    let (placed_status, placed_body) = {
        let h = Harness::new();
        h.put("anthropic", false).await
    };
    let (unplaced_status, unplaced_body) = {
        let h = Harness::with_config(UNPLACEABLE_DEFAULT);
        h.put("anthropic", false).await
    };

    assert_eq!(placed_status, StatusCode::CONFLICT);
    assert_eq!(unplaced_status, StatusCode::CONFLICT);
    assert_eq!(placed_body["error"]["code"], "PROVIDER_IS_DEFAULT");
    assert_eq!(unplaced_body["error"]["code"], "DEFAULT_MODEL_UNRESOLVED");
    assert_ne!(
        placed_body["error"]["code"], unplaced_body["error"]["code"],
        "same status, different fact — the client renders per code"
    );
}
