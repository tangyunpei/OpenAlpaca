//! Test-only helpers shared across this binary's test modules.

use std::ffi::OsString;
use std::path::Path;
use std::sync::MutexGuard;

/// Serializes every test in this binary that re-points `OPENALPACA_HOME_STORE`.
///
/// The variable is process-global and every store accessor reads it on each
/// call, so two modules holding *separate* locks would still race. This is the
/// binary's one lock — mirroring `openalpaca_core::test_util` — and every test
/// that re-points the home store takes it through [`HomeStoreGuard`].
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Points `OPENALPACA_HOME_STORE` at a temp root for the guard's lifetime.
/// No test ever touches the real `~/.openalpaca`.
pub(crate) struct HomeStoreGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
    /// The master key this guard set, if any — restored the same way.
    prev_master_key: Option<Option<OsString>>,
}

/// A fixed 32-byte key. `KeyEncryptor::from_env` is what a
/// `LlmSettingsService` is built through, and a test must never reach for the
/// owner's real one.
const TEST_MASTER_KEY: &str =
    "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

impl HomeStoreGuard {
    pub(crate) fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by ENV_LOCK — the binary's only writer.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self {
            _lock: lock,
            prev,
            prev_master_key: None,
        }
    }

    /// As [`Self::set`], plus a throwaway `OPENALPACA_MASTER_KEY` for the tests
    /// that build an `LlmSettingsService`. The variable is what
    /// `KeyEncryptor::from_env` falls back to: nothing in this binary calls
    /// `KeyEncryptor::install_process_key` (the daemon's preamble does, in
    /// `main`), and nothing may — the installed key is process-wide and would
    /// change the key under every service another test already built (F3).
    pub(crate) fn set_with_master_key(path: &Path) -> Self {
        let mut guard = Self::set(path);
        guard.prev_master_key = Some(std::env::var_os("OPENALPACA_MASTER_KEY"));
        // SAFETY: as above — still holding ENV_LOCK.
        unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", TEST_MASTER_KEY) };
        guard
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding ENV_LOCK.
        if let Some(prev) = self.prev_master_key.take() {
            match prev {
                Some(v) => unsafe { std::env::set_var("OPENALPACA_MASTER_KEY", v) },
                None => unsafe { std::env::remove_var("OPENALPACA_MASTER_KEY") },
            }
        }
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}

/// A guarded temp home store and one temp project folder registered under it
/// — core's `GuardedProject`'s twin (storage design Stage 2): every daemon
/// test that writes into a project, or records a result file, does it here.
/// `_guard` is declared first so it drops first (the variable is restored
/// before the folders go).
pub(crate) struct GuardedProject {
    _guard: HomeStoreGuard,
    pub(crate) home: tempfile::TempDir,
    /// Held so the project folder lives as long as the test.
    #[allow(dead_code)]
    pub(crate) project: tempfile::TempDir,
    /// The project's canonical root, as `projects::register` recorded it.
    pub(crate) root: std::path::PathBuf,
}

impl GuardedProject {
    pub(crate) fn new() -> Self {
        let home = tempfile::tempdir().expect("a temp home store");
        let project = tempfile::tempdir().expect("a temp project folder");
        let guard = HomeStoreGuard::set(&home.path().canonicalize().expect("canonical home"));
        let root = openalpaca_storage::store::projects::register(
            &project.path().canonicalize().expect("canonical project"),
        )
        .expect("register the project")
        .root;
        Self {
            _guard: guard,
            home,
            project,
            root,
        }
    }

    /// The home store, canonical.
    pub(crate) fn home_root(&self) -> std::path::PathBuf {
        self.home.path().canonicalize().expect("canonical home")
    }
}

/// A stand-in for a locally installed Ollama, on a loopback port.
///
/// Discovery speaks Ollama's **native** API — `GET /api/tags` for what is
/// installed, `POST /api/show` per tag for its context length and capabilities
/// — so a test of the daemon's discovery paths has to answer both. Never the
/// real Ollama: a developer's machine has whatever it has, and a test that
/// read it would assert a different thing on every machine.
pub(crate) struct MockOllama {
    /// `http://127.0.0.1:<port>/v1` — the `base_url` an `llm.toml` names.
    pub(crate) base_url: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for MockOllama {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl MockOllama {
    /// Serve `tags` as the installed list; every `/api/show` answers with
    /// `context_length` and the `capabilities` given.
    pub(crate) async fn start(tags: &[&str], context_length: u64) -> Self {
        let installed: Vec<serde_json::Value> = tags
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect();
        let tags_body = serde_json::json!({ "models": installed });
        let show_body = serde_json::json!({
            "capabilities": ["completion", "tools"],
            "model_info": { "qwen3.context_length": context_length },
        });

        let app = axum::Router::new()
            .route(
                "/api/tags",
                axum::routing::get(move || {
                    let body = tags_body.clone();
                    async move { axum::Json(body) }
                }),
            )
            .route(
                "/api/show",
                axum::routing::post(move || {
                    let body = show_body.clone();
                    async move { axum::Json(body) }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Self {
            base_url: format!("http://{addr}/v1"),
            server,
        }
    }
}

/// A learning queue over `db` in manual mode — nothing starts a flush on its
/// own — for the routes that drop a conversation's queue state (LQ4). Its
/// router has no provider and its memory index lives in memory: the routes
/// only ever call `forget_session`.
pub(crate) fn learning_queue(
    db: &openalpaca_storage::Database,
) -> std::sync::Arc<openalpaca_core::memory::learning::LearningQueue> {
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    let router = openalpaca_llm::LlmRouter::new(
        std::collections::HashMap::new(),
        openalpaca_llm::ModelRegistry::with_defaults(),
        std::collections::HashMap::new(),
        std::sync::Arc::new(openalpaca_llm::CostTracker::new(
            openalpaca_llm::ModelRegistry::with_defaults(),
        )),
        "none".to_string(),
    );
    let index = openalpaca_storage::memory_index::MemoryIndex::open_in_memory()
        .expect("an in-memory index");
    LearningQueue::new(
        LearningDeps {
            db: db.clone(),
            router: std::sync::Arc::new(router),
            daemon_config: std::sync::Arc::new(arc_swap::ArcSwap::from_pointee(
                openalpaca_core::daemon_config::DaemonConfig::default(),
            )),
            memory_service: std::sync::Arc::new(
                openalpaca_core::memory::service::MemoryService::new(Some(db.clone()), index),
            ),
        },
        QueueMode::Manual,
    )
}

/// One finished, user-authored turn of `lane` in `session_id`, as the
/// gateway's hook records it: the queue holds it as not yet learned.
pub(crate) fn finish_turn(
    queue: &std::sync::Arc<openalpaca_core::memory::learning::LearningQueue>,
    lane: &str,
    session_id: &str,
) {
    let guard = queue.turn_started(uuid::Uuid::new_v4(), lane, true);
    guard.attach_session(session_id);
}

/// A memory service over a temp home store, with `owner` a `global_user`
/// row; the guard serializes the home store for the test's life. For the
/// routes that forget a deleted conversation's summaries (design 09 §13.3).
pub(crate) fn memory_service(
    db: &openalpaca_storage::Database,
    owner: &str,
) -> (
    tempfile::TempDir,
    HomeStoreGuard,
    std::sync::Arc<openalpaca_core::memory::service::MemoryService>,
) {
    let home = tempfile::tempdir().expect("home");
    let store = home
        .path()
        .canonicalize()
        .expect("canonical")
        .join(".openalpaca");
    std::fs::create_dir_all(&store).expect("home store");
    let guard = HomeStoreGuard::set(&store);
    openalpaca_storage::IdentityRepository::new(db)
        .create_global_user(owner, None)
        .expect("owner row");
    let service = openalpaca_core::memory::service::MemoryService::new(
        Some(db.clone()),
        openalpaca_storage::memory_index::MemoryIndex::open_in_memory().expect("index"),
    );
    service.rebuild_index().expect("rebuild");
    (home, guard, std::sync::Arc::new(service))
}

/// One note through the extractor's surface citing `rows`: a conversation
/// summary (`episode`, provenance `Summary`) or a note learned from the
/// user's words.
pub(crate) fn extracted_note(
    service: &openalpaca_core::memory::service::MemoryService,
    owner: &str,
    root: Option<&std::path::Path>,
    title: &str,
    rows: &[i64],
    summary: bool,
) -> openalpaca_storage::memory_files::NoteId {
    use openalpaca_core::memory::service::{
        Attribution, Capabilities, ExtractionProvenance, StoreInput, Surface, WorkspaceHint,
    };
    use openalpaca_storage::memory_files::{NoteKind, NoteSource, SourceType};
    let ctx = service
        .context(
            owner,
            root.map_or(WorkspaceHint::None, WorkspaceHint::Root),
            Surface::Extraction,
            Capabilities::EXTRACTION,
            Attribution::default(),
        )
        .expect("the owner is a global_user row");
    service
        .store(
            &ctx,
            StoreInput {
                title: title.to_string(),
                body: format!("# {title}\n\n## Knowledge\n{title}.\n"),
                kind: Some(if summary {
                    NoteKind::Episode
                } else {
                    NoteKind::Fact
                }),
                sources: rows
                    .iter()
                    .map(|id| NoteSource {
                        kind: SourceType::Message,
                        id: id.to_string(),
                        observed_at: None,
                    })
                    .collect(),
                provenance: Some(if summary {
                    ExtractionProvenance::Summary
                } else {
                    ExtractionProvenance::UserWords
                }),
                ..Default::default()
            },
        )
        .expect("store the note")
        .id
}

/// The titles of the owner's active notes (home partition, and `root`'s).
pub(crate) fn note_titles(
    service: &openalpaca_core::memory::service::MemoryService,
    owner: &str,
    root: Option<&std::path::Path>,
) -> Vec<String> {
    use openalpaca_core::memory::service::{
        Attribution, Capabilities, ListInput, Surface, WorkspaceHint,
    };
    let ctx = service
        .context(
            owner,
            root.map_or(WorkspaceHint::None, WorkspaceHint::Root),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .expect("owner");
    let mut titles: Vec<String> = service
        .list(&ctx, ListInput::default())
        .expect("list")
        .items
        .into_iter()
        .map(|i| i.title)
        .collect();
    titles.sort();
    titles
}

/// Storage's `store::projects::register` refuses an unguarded home store only
/// in a build that carries its `test-home-guard` feature, which this crate
/// turns on through `[dev-dependencies]` (storage design, Stage 1, S1.9).
#[test]
fn the_storage_test_home_guard_is_armed_in_this_test_build() {
    assert!(openalpaca_storage::store::projects::test_home_guard_armed());
}
