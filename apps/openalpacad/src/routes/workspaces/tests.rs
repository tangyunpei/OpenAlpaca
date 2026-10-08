//! The verification cell for `/v1/workspaces` (§4.8, "Project moved").
//!
//! The transaction belongs to `ArtifactStore::repoint_rows` and the re-point
//! to `project_life`; both are proved in `openalpaca_storage`. What is proved
//! here is what the route owns — which refusal becomes which status code, that
//! every refusal happens before a row or a folder changes, and what the JSON
//! carries.

use super::*;

use axum::body::to_bytes;
use openalpaca_core::memory::service::{
    Attribution, Capabilities, MemoryAccessContext, ReadInput, StoreInput, StoreOutput, Surface,
    WorkspaceHint,
};
use openalpaca_storage::IdentityRepository;
use openalpaca_storage::memory_files::{
    ControlSnapshot, NoteKind, Partition, PartitionHealth, list_snapshots,
};
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::store::projects;
use openalpaca_storage::{ArtifactKind, NewArtifact};
use tempfile::TempDir;

use openalpaca_core::context::SharedContext;

use crate::test_util::HomeStoreGuard;

const OWNER: &str = "owner-1";

// ============================================================================
// Harness
// ============================================================================

struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    /// The directory both project roots are created under, so neither is an
    /// ancestor of the other.
    projects: TempDir,
    db: Database,
    /// The live lane registry the purge's in-flight guard consults. Empty
    /// unless a test says otherwise.
    ctx: SharedContext,
    /// Where the purge's `session_changed{deleted}` frames land.
    bus: openalpaca_core::bus::EventBus,
    /// The home store's `sessions/` — where a purged session's directory is.
    sessions_root: std::path::PathBuf,
    /// The live writers, as the daemon holds them: the purge stands each purged
    /// session's writer down before removing its directory.
    log: std::sync::Arc<openalpaca_core::session_log::SessionLogService>,
    /// The memory service the move quiesces and re-keys through, over this
    /// fixture's home store and an in-memory index.
    memory: std::sync::Arc<MemoryService>,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home root");
        // The home root is `<tmp>/.openalpaca`, exactly as it is in a real
        // install — so `<tmp>` stands in for `$HOME` and both shapes of the
        // home fold are reachable from a test.
        let home_store = home
            .path()
            .canonicalize()
            .expect("canonicalize home")
            .join(store::STORE_DIR_NAME);
        std::fs::create_dir_all(&home_store).expect("home store");
        let env = HomeStoreGuard::set(&home_store);
        let projects = tempfile::tempdir().expect("projects");
        let db_dir = tempfile::tempdir().expect("db dir");
        let db = Database::open(&db_dir.path().join("test.db")).expect("open db");
        // The memory service takes an owner only from a `global_user` row.
        IdentityRepository::new(&db)
            .create_global_user(OWNER, None)
            .expect("owner row");
        let memory = MemoryService::new(
            Some(db.clone()),
            MemoryIndex::open_in_memory().expect("in-memory index"),
        );
        memory.rebuild_index().expect("rebuild an empty index");
        let memory = std::sync::Arc::new(memory);
        let sessions_root = home_store.join("sessions");
        std::fs::create_dir_all(&sessions_root).expect("sessions root");
        let log = openalpaca_core::session_log::SessionLogService::new(
            sessions_root.clone(),
            None,
            openalpaca_core::session_log::SessionLogLimits::default(),
            "test".to_string(),
        )
        .into_arc();
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            projects,
            db,
            ctx: SharedContext::new(),
            bus: openalpaca_core::bus::EventBus::default(),
            sessions_root,
            log,
            memory,
        }
    }

    // ── memory fixtures ──────────────────────────────────────────

    /// A user-directed context on `root` for `owner` — what `/v1/memories`
    /// builds. `owner` must be a `global_user` row.
    fn memory_ctx_as(&self, owner: &str, root: &str) -> MemoryAccessContext {
        self.memory
            .context(
                owner,
                WorkspaceHint::Path(std::path::Path::new(root)),
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .expect("the owner is a global_user row")
    }

    /// Another `global_user` row, so the service takes `id` as an owner.
    fn global_user(&self, id: &str) {
        IdentityRepository::new(&self.db)
            .create_global_user(id, None)
            .expect("owner row");
    }

    /// One project note under `root`, through the service: the partition,
    /// its snapshot and the index row all exist afterwards.
    fn note(&self, root: &str, title: &str) -> StoreOutput {
        self.note_as(OWNER, root, title)
    }

    /// One project note under `root` in `owner`'s partition.
    fn note_as(&self, owner: &str, root: &str, title: &str) -> StoreOutput {
        self.memory
            .store(
                &self.memory_ctx_as(owner, root),
                StoreInput {
                    title: title.to_string(),
                    body: format!("# {title}\n\nTag, then run the release script.\n"),
                    kind: Some(NoteKind::Procedure),
                    ..Default::default()
                },
            )
            .expect("store a project note")
    }

    fn partition(&self, root: &str) -> Partition {
        Partition::open(&StoreScope::Project(std::path::PathBuf::from(root)), OWNER)
            .expect("open the partition")
    }

    /// The owner's partition snapshot at `root`, if the partition has one.
    fn project_snapshot(&self, root: &str) -> Option<ControlSnapshot> {
        self.partition(root).snapshot()
    }

    /// The note files under `root`'s partition, sorted.
    fn note_files(&self, root: &str) -> Vec<std::path::PathBuf> {
        let dir = self.partition(root).notes_dir().to_path_buf();
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        files
    }

    /// A project directory: a git repository (what makes it a root since the
    /// storage design's resolver, §3), registered under this fixture's home
    /// store, as the daemon's first turn there would register it.
    fn project(&self, name: &str) -> String {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(dir.join(".git")).expect("project dir");
        let dir = dir.canonicalize().expect("canonicalize project");
        projects::register(&dir).expect("register the project");
        dir.to_string_lossy().into_owned()
    }

    /// A project directory with no store at all.
    fn bare_dir(&self, name: &str) -> String {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(&dir).expect("dir");
        dir.canonicalize()
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned()
    }

    /// One produced artifact addressed under `root`.
    fn artifact(&self, root: &str, title: &str) {
        let scope = StoreScope::Project(std::path::PathBuf::from(root));
        let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, title, b"body\n");
        new.created = chrono::Utc::now();
        ArtifactStore::new(&self.db).put(new).expect("put");
    }

    fn task(&self, id: &str, root: &str, status: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane, workspace_id, status)
                     VALUES (?1, ?1, 'test', 'test', ?2, ?3)",
                    [id, root, status],
                )?;
                Ok(())
            })
            .expect("insert task");
    }

    fn session(&self, id: &str, root: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO session (id, lane_key, source, workspace_id)
                     VALUES (?1, ?1, 'gui', ?2)",
                    [id, root],
                )?;
                Ok(())
            })
            .expect("insert session");
    }

    async fn get(&self, path: Option<&str>) -> (StatusCode, serde_json::Value) {
        split(get_workspace(
            &self.db,
            WorkspaceQuery {
                path: path.map(str::to_string),
            },
        ))
        .await
    }

    async fn patch(&self, old: &str, new: &str) -> (StatusCode, serde_json::Value) {
        self.patch_as(OWNER, old, new).await
    }

    async fn patch_as(&self, owner: &str, old: &str, new: &str) -> (StatusCode, serde_json::Value) {
        split(rebase_workspace(
            &self.db,
            &self.memory,
            owner,
            RebaseRequest {
                old_path: old.to_string(),
                new_path: new.to_string(),
            },
        ))
        .await
    }

    /// The home store root itself — `~/.openalpaca`.
    fn home_store(&self) -> String {
        self._home
            .path()
            .canonicalize()
            .expect("canonicalize home")
            .join(store::STORE_DIR_NAME)
            .to_string_lossy()
            .into_owned()
    }

    /// `$HOME`: the directory whose project store *is* the home store.
    fn home(&self) -> String {
        self._home
            .path()
            .canonicalize()
            .expect("canonicalize home")
            .to_string_lossy()
            .into_owned()
    }

    /// One produced artifact under `root`, owned by somebody else.
    fn artifact_owned(&self, root: &str, title: &str, owner: &str) {
        let scope = StoreScope::Project(std::path::PathBuf::from(root));
        let mut new = NewArtifact::new(owner, &scope, ArtifactKind::Markdown, title, b"body\n");
        new.created = chrono::Utc::now();
        ArtifactStore::new(&self.db).put(new).expect("put");
    }

    // ── purge fixtures ───────────────────────────────────────────

    /// A session under `root`, with the log directory a real one would have.
    fn session_with_log(&self, id: &str, root: &str) -> std::path::PathBuf {
        self.session(id, root);
        let dir = self.sessions_root.join(id);
        std::fs::create_dir_all(&dir).expect("session dir");
        std::fs::write(dir.join("log.jsonl"), "{}\n").expect("session log");
        dir
    }

    fn message(&self, session_id: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO conversation_messages (lane_key, role, content, session_id)
                     VALUES (?1, 'user', 'hi', ?1)",
                    [session_id],
                )?;
                Ok(())
            })
            .expect("insert message");
    }

    /// An upload addressed under `root`, with its bytes where the row says.
    fn upload(&self, root: &str, name: &str) -> std::path::PathBuf {
        let dir = store::store_root(&StoreScope::Project(std::path::PathBuf::from(root)))
            .expect("the project's store")
            .join("uploads");
        std::fs::create_dir_all(&dir).expect("uploads dir");
        let path = dir.join(name);
        std::fs::write(&path, b"bytes").expect("upload bytes");
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO file_assets
                         (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path,
                          origin, project_root)
                     VALUES (?1, ?2, 'sha', ?1, 'text/plain', 5, ?3, 'upload', ?4)",
                    [name, OWNER, &path.to_string_lossy(), root],
                )?;
                Ok(())
            })
            .expect("insert upload");
        path
    }

    fn purge_deps(&self) -> PurgeDeps<'_> {
        PurgeDeps {
            db: &self.db,
            ctx: &self.ctx,
            bus: &self.bus,
            owner: OWNER,
            memory: &self.memory,
            sessions_root: Some(self.sessions_root.clone()),
            session_log: Some(self.log.clone()),
        }
    }

    async fn purge(
        &self,
        path: Option<&str>,
        all: bool,
        dry_run: bool,
    ) -> (StatusCode, serde_json::Value) {
        split(
            purge_workspaces(
                &self.purge_deps(),
                PurgeRequest {
                    path: path.map(str::to_string),
                    all,
                    dry_run,
                },
            )
            .await,
        )
        .await
    }

    fn row_count(&self, sql: &str) -> i64 {
        self.db
            .with_connection(|conn| Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?))
            .expect("count")
    }
}

/// The plan line for one entry, or `None` when the plan does not mention it —
/// which is itself a failure worth naming, because a plan that omits an entry
/// makes no promise about it.
fn entry<'a>(body: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    body["projects"][0]["entries"]
        .as_array()?
        .iter()
        .find(|e| e["entry"] == name)
}

fn action(body: &serde_json::Value, name: &str) -> String {
    entry(body, name).unwrap_or_else(|| panic!("the plan never mentions {name}"))["action"]
        .as_str()
        .unwrap_or("<none>")
        .to_string()
}

/// Every control snapshot that parses.
fn snapshots() -> Vec<ControlSnapshot> {
    list_snapshots()
        .expect("list the control snapshots")
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .collect()
}

/// Split a `Response` into its status and its JSON body.
async fn split(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("read the response body");
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

fn error_code(body: &serde_json::Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("<no code>")
}

/// What an earlier build's in-project `.layout` says, written by hand. The
/// GET still reads it until Stage 1 step (5) removes `store_present`,
/// `recorded_root` and `moved` (interfaces C8).
fn earlier_build_marker(root: &str, recorded: &str) {
    let dir = std::path::Path::new(root).join(store::STORE_DIR_NAME);
    std::fs::create_dir_all(&dir).expect("store dir");
    std::fs::write(dir.join(".layout"), format!("1\nproject_root={recorded}\n")).expect("marker");
}

fn project_at(root: &str) -> projects::Project {
    projects::lookup(std::path::Path::new(root))
        .expect("lookup")
        .expect("a registered project")
}

// ============================================================================
// GET /v1/workspaces
// ============================================================================

#[tokio::test]
async fn the_path_is_required_and_must_be_absolute() {
    let f = Fixture::new();

    let (status, body) = f.get(None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "MISSING_PATH");

    for relative in ["", "   ", "relative/project", "./here"] {
        let (status, body) = f.get(Some(relative)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{relative:?}");
        assert_eq!(error_code(&body), "INVALID_PATH", "{relative:?}");
    }
}

#[tokio::test]
async fn a_store_whose_marker_names_another_root_reads_as_moved() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");
    f.session("session-1", &old);
    f.task("task-1", &old, "completed");
    let new = f.bare_dir("new-project");
    earlier_build_marker(&new, &old);

    let (status, body) = f.get(Some(&new)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["path"], new);
    assert_eq!(body["store_present"], true);
    assert_eq!(body["recorded_root"], old);
    assert_eq!(body["moved"], true);
    assert_eq!(body["rows"]["artifacts"], 0);

    let (status, body) = f.get(Some(&old)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rows"]["artifacts"], 1);
    assert_eq!(body["rows"]["sessions"], 1);
    assert_eq!(body["rows"]["tasks"], 1);
    assert_eq!(body["active_tasks"], 0);
}

#[tokio::test]
async fn a_store_standing_where_it_was_seeded_is_not_moved() {
    let f = Fixture::new();
    let root = f.project("project");
    earlier_build_marker(&root, &root);

    let (status, body) = f.get(Some(&root)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["recorded_root"], root);
    assert_eq!(body["moved"], false, "it is exactly where it was seeded");
    assert_eq!(body["store_present"], true);
}

#[tokio::test]
async fn the_runs_in_flight_are_reported() {
    let f = Fixture::new();
    let root = f.project("project");
    f.task("done", &root, "completed");
    f.task("queued", &root, "queued");
    f.task("live", &root, "running");

    let (_, body) = f.get(Some(&root)).await;
    assert_eq!(body["rows"]["tasks"], 3);
    assert_eq!(body["active_tasks"], 1);
    // D7: the purge refuses on `queued` as well, so the read that a client
    // offers a purge from has to report that set too — otherwise a
    // `WORKSPACE_BUSY` arrives with `active_tasks: 0` and nothing to explain it.
    assert_eq!(body["queued_tasks"], 1);
}

// ============================================================================
// PATCH /v1/workspaces — the refusals
// ============================================================================

#[tokio::test]
async fn re_basing_a_root_nothing_names_is_a_404() {
    let f = Fixture::new();
    let old = f.bare_dir("old-project");
    let new = f.bare_dir("new-project");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
}

#[tokio::test]
async fn re_pointing_onto_a_project_or_a_path_rows_name_is_a_409_naming_both_ways_out() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.project("new-project");
    f.artifact(&old, "Mine");
    f.artifact(&new, "Theirs");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_EXISTS");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("delete that project's data first")
            && message.contains("re-point onto another path"),
        "ST-20: {message}"
    );

    // A path only rows name is the same merge.
    let named = f.bare_dir("named-by-rows");
    f.session("session-named", &named);
    let (status, body) = f.patch(&old, &named).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_EXISTS");

    // Refused before a row changed.
    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1);
    let (_, new_body) = f.get(Some(&new)).await;
    assert_eq!(new_body["rows"]["artifacts"], 1);
}

#[tokio::test]
async fn re_basing_out_from_under_a_run_in_flight_is_a_409() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.bare_dir("new-project");
    f.artifact(&old, "Notes");
    f.task("live", &old, "running");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");

    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1, "nothing moved");
}

#[tokio::test]
async fn re_pointing_onto_the_home_store_is_a_409_and_from_it_a_404() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");

    for destination in [f.home_store(), f.home()] {
        let (status, body) = f.patch(&old, &destination).await;
        assert_eq!(status, StatusCode::CONFLICT, "{destination}: {body}");
        assert_eq!(error_code(&body), "WORKSPACE_IS_HOME", "{destination}");
    }

    // The home store is never a project (R13): as the source it is a path no
    // project owns.
    let new = f.bare_dir("new-project");
    let (status, body) = f.patch(&f.home(), &new).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");

    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1, "nothing moved");
}

/// R13: a folder inside the home store is no project either. A re-base
/// onto `~/.openalpaca/artifacts` would hand a project the home store's own
/// identity space, as the store itself would.
#[tokio::test]
async fn re_basing_onto_a_folder_inside_the_home_store_is_a_409() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");
    let inside = std::path::Path::new(&f.home_store()).join("artifacts");
    std::fs::create_dir_all(&inside).expect("a folder inside the home store");

    let (status, body) = f.patch(&old, &inside.to_string_lossy()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_IS_HOME");
    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1, "nothing moved");
}

#[tokio::test]
async fn re_basing_rows_another_owner_holds_is_a_404() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.bare_dir("new-project");
    f.artifact_owned(&old, "Theirs", "owner-2");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");

    // A root the caller only partly owns is refused too: the transaction moves
    // sessions and tasks, which carry no owner, so a partial re-base would
    // strand the other owner's artifacts under a path nothing else names.
    f.artifact(&old, "Mine");
    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");

    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 2, "nothing moved");
}

#[tokio::test]
async fn a_destination_that_is_not_a_directory_is_a_422_and_is_not_created() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");
    let mistyped = f
        .projects
        .path()
        .join("mistyped")
        .to_string_lossy()
        .into_owned();

    let (status, body) = f.patch(&old, &mistyped).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_A_DIRECTORY");
    assert!(
        !std::path::Path::new(&mistyped).exists(),
        "a mistyped path is not created"
    );
    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1);
}

/// The destination is taken as named: a folder inside a repository nobody
/// chose is re-pointed onto, and the repository above it is not made a
/// project.
#[tokio::test]
async fn a_destination_inside_another_repository_is_taken_as_named() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");
    let mono = f.bare_dir("mono");
    std::fs::create_dir_all(std::path::Path::new(&mono).join(".git")).expect("marker");
    let inside = std::path::Path::new(&mono).join("sub").join("proj");
    std::fs::create_dir_all(&inside).expect("destination");
    let inside = inside.to_string_lossy().into_owned();

    let (status, body) = f.patch(&old, &inside).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["new_path"], inside);
    assert!(
        projects::lookup(std::path::Path::new(&mono))
            .unwrap()
            .is_none()
    );
    let (_, moved) = f.get(Some(&inside)).await;
    assert_eq!(moved["rows"]["artifacts"], 1);
}

#[tokio::test]
async fn re_basing_a_root_onto_itself_is_a_400() {
    let f = Fixture::new();
    let root = f.project("project");
    let (status, body) = f.patch(&root, &root).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_PATH");
}

// ============================================================================
// PATCH /v1/workspaces — the re-base
// ============================================================================

/// The user moved the project's folder with `mv`: the re-point moves its rows
/// and renames its folder under `projects/`; the id stays (ST-13).
#[tokio::test]
async fn a_project_moved_by_hand_is_re_pointed_with_its_rows_and_its_folder() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let before = project_at(&old);
    f.artifact(&old, "Notes");
    f.session("session-1", &old);
    f.task("task-1", &old, "completed");
    let new_dir = f.projects.path().join("new-project");
    std::fs::rename(&old, &new_dir).expect("the user's mv");
    let new = new_dir
        .canonicalize()
        .expect("canonicalize")
        .to_string_lossy()
        .into_owned();

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["old_path"], old);
    assert_eq!(body["new_path"], new);
    assert_eq!(body["moved"]["artifacts"], 1);
    assert_eq!(body["moved"]["sessions"], 1);
    assert_eq!(body["moved"]["tasks"], 1);
    assert_eq!(
        body["store_moved"], true,
        "the folder under projects/ was renamed"
    );

    let after = project_at(&new);
    assert_eq!(after.id, before.id);
    assert!(!before.dir.exists());
    assert!(
        projects::lookup(std::path::Path::new(&old))
            .unwrap()
            .is_none()
    );
    let (_, at_new) = f.get(Some(&new)).await;
    assert_eq!(at_new["rows"]["artifacts"], 1);
    assert_eq!(at_new["rows"]["sessions"], 1);
    let (_, at_old) = f.get(Some(&old)).await;
    assert_eq!(at_old["rows"]["artifacts"], 0);
    let record = ArtifactStore::new(&f.db)
        .list(&openalpaca_storage::ArtifactQuery::new(OWNER))
        .expect("list")
        .0
        .remove(0);
    assert!(
        std::path::Path::new(&record.storage_path).exists(),
        "{} does not exist",
        record.storage_path
    );
}

/// The caller asks the daemon to re-point a project whose folder is still
/// where it was: the rows move to the new path and the folder under
/// `projects/` takes the new path's name.
#[tokio::test]
async fn a_re_point_renames_the_projects_folder_under_projects() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let before = project_at(&old);
    let new = f.bare_dir("new-project");
    f.artifact(&old, "Notes");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["store_moved"], true);
    let after = project_at(&new);
    assert_eq!(after.id, before.id);
    assert_eq!(
        after.name,
        projects::folder_name(std::path::Path::new(&new))
    );
    assert!(!before.dir.exists());
    let (_, moved) = f.get(Some(&new)).await;
    assert_eq!(moved["rows"]["artifacts"], 1);

    // The bytes are where the rows now say they are: the folder moved with
    // the project.
    let record = ArtifactStore::new(&f.db)
        .list(&openalpaca_storage::ArtifactQuery::new(OWNER))
        .expect("list")
        .0
        .remove(0);
    assert!(
        record
            .storage_path
            .starts_with(after.dir.to_string_lossy().as_ref())
    );
    assert!(
        std::path::Path::new(&record.storage_path).exists(),
        "{} does not exist",
        record.storage_path
    );
}

/// A re-point is refused while a memory write for the project is in flight,
/// and nothing changes; once it lands, the same re-point goes through.
#[tokio::test]
async fn a_re_point_while_a_memory_write_is_pending_is_refused() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let before = project_at(&old);
    let new = f.bare_dir("new-project");
    f.artifact(&old, "Notes");

    let pending = f
        .memory
        .quiesce(
            &store::projects::lookup(std::path::Path::new(&old))
                .unwrap()
                .expect("a registered project")
                .id,
        )
        .expect("nothing is writing yet");
    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("memory"),
        "{body}"
    );
    assert_eq!(project_at(&old), before, "project.json was never marked");
    let (_, rows) = f.get(Some(&old)).await;
    assert_eq!(rows["rows"]["artifacts"], 1);

    drop(pending);
    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// R18 through the route (fixture 17 steps 1–5): a re-point is refused
/// while a memory write of the project holds its slot — the hold named by
/// the project's id — and once it lands the partition keeps its key, its
/// snapshot file byte for byte and its note, read at the same revision
/// from the new root.
#[tokio::test]
async fn a_repoint_keeps_the_projects_memory_under_its_key() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.bare_dir("new-project");
    let project = store::projects::register(std::path::Path::new(&old)).expect("register");
    let note = f.note(&old, "Release step");
    let key = f.project_snapshot(&old).expect("a snapshot").partition_key;
    let snapshot_file = openalpaca_storage::memory_files::snapshot_path_for(&key).unwrap();
    let before = std::fs::read(&snapshot_file).unwrap();

    let pending = f
        .memory
        .quiesce(&project.id)
        .expect("nothing is writing yet");
    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    drop(pending);

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        f.project_snapshot(&new)
            .expect("bound by the project id")
            .partition_key,
        key
    );
    assert_eq!(
        std::fs::read(&snapshot_file).unwrap(),
        before,
        "a re-point touches no snapshot"
    );
    assert_eq!(f.partition(&new).health(), PartitionHealth::Ready);
    let read = f
        .memory
        .read(
            &f.memory_ctx_as(OWNER, &new),
            ReadInput {
                id: note.id,
                expected_revision: None,
                section: None,
            },
        )
        .expect("the note reads from the new root");
    assert_eq!(read.revision, note.revision);
}

/// A registered project no row names yet is a project: it re-points.
#[tokio::test]
async fn a_registered_project_with_no_rows_can_be_re_pointed() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.bare_dir("new-project");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["moved"]["artifacts"], 0);
    assert!(
        projects::lookup(std::path::Path::new(&new))
            .unwrap()
            .is_some()
    );
}

/// D6 — the `422`'s remediation has to be followable.
///
/// `WORKSPACE_NOT_A_ROOT` tells the caller to give the path a project marker of
/// its own; the marker walk's process-lifetime cache used to pin the ancestor
/// answer that the refusal's own lookup had just written, so the retry after
/// `git init` answered the same `422` until the daemon restarted. Same
/// process, same path, marker created in between.
#[tokio::test]
async fn a_marker_created_after_the_422_is_seen_by_the_next_request() {
    let f = Fixture::new();
    let root = f.project("mono-cached");
    f.artifact(&root, "Notes");
    let inside = f.bare_dir("mono-cached/sub");

    let (status, body) = f.purge(Some(&inside), false, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_A_ROOT");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains(&root),
        "the refusal names the ancestor and says to give the path a marker",
    );

    // Exactly what the message asked for.
    std::fs::create_dir_all(std::path::Path::new(&inside).join(".git")).expect("marker");

    let (status, body) = f.purge(Some(&inside), false, true).await;
    assert_ne!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the ancestor answer was cached: {body}",
    );
    // It is its own root now — and nothing is recorded under it, which is the
    // ordinary `404`, not a refusal to look.
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");

    // The GET resolves the same way, so a picker sees the new root too.
    let (status, described) = f.get(Some(&inside)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(described["path"], inside);
}

// ============================================================================
// POST /v1/workspaces/purge
// ============================================================================

#[tokio::test]
async fn a_purge_names_exactly_one_of_a_path_and_all() {
    let f = Fixture::new();
    let root = f.project("proj");

    let (status, body) = f.purge(Some(&root), true, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_REQUEST");

    let (status, body) = f.purge(None, false, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_REQUEST");
}

#[tokio::test]
async fn a_relative_path_is_refused_before_anything_is_counted() {
    let f = Fixture::new();
    let (status, body) = f.purge(Some("relative/proj"), false, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_PATH");
}

#[tokio::test]
async fn the_home_store_is_not_a_project_and_cannot_be_purged() {
    let f = Fixture::new();
    for path in [f.home_store(), f.home()] {
        let (status, body) = f.purge(Some(&path), false, true).await;
        assert_eq!(status, StatusCode::CONFLICT, "{path}");
        assert_eq!(error_code(&body), "WORKSPACE_IS_HOME", "{path}");
    }
}

#[tokio::test]
async fn a_root_nothing_of_yours_names_is_a_404() {
    let f = Fixture::new();
    let empty = f.bare_dir("empty");
    let (status, body) = f.purge(Some(&empty), false, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
}

#[tokio::test]
async fn a_root_holding_another_owners_rows_is_a_404_not_a_403() {
    let f = Fixture::new();
    let root = f.project("theirs");
    f.artifact_owned(&root, "theirs.md", "someone-else");

    let (status, body) = f.purge(Some(&root), false, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
    // D9: the message is fixed, and the same one an empty root gets. It used to
    // read "1 row(s) under <root> belong to another owner", which disclosed both
    // that someone else has a history there and how big it is — the two things a
    // `404`-not-`403` exists to withhold. The count is in the debug log instead.
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert_eq!(message, format!("nothing of yours is recorded under {root}"));
    assert!(
        !message.contains("owner"),
        "no hint of whose rows: {message}",
    );

    // An empty root is indistinguishable, which is the point.
    let empty = f.project("empty-twin");
    let (empty_status, empty_body) = f.purge(Some(&empty), false, true).await;
    assert_eq!(empty_status, StatusCode::NOT_FOUND);
    assert_eq!(
        empty_body["error"]["message"].as_str().unwrap_or_default(),
        format!("nothing of yours is recorded under {empty}"),
    );
}

/// Ruling R72: a destructive verb takes `path` almost literally — it never
/// silently walks a subdirectory up to the project root the way a re-base's
/// *old* path does. `purge /repo/src` must be refused naming `/repo`, not
/// carried out against it.
#[tokio::test]
async fn a_path_inside_a_project_is_refused_naming_the_root_not_purged() {
    let f = Fixture::new();
    let root = f.project("mono");
    f.session_with_log("s-mono", &root);

    let inside = std::path::Path::new(&root).join("src");
    std::fs::create_dir_all(&inside).expect("nested dir");
    let inside = inside.to_string_lossy().into_owned();

    let (status, body) = f.purge(Some(&inside), false, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_A_ROOT");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&root), "the project root is named: {message}");

    // Refused before anything was counted, let alone deleted — even a `-y`
    // real run must not carry this out.
    let (status, _) = f.purge(Some(&inside), false, false).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 1);

    // The root itself is exactly what `--all` and an explicit root are for,
    // and still works.
    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 0);
}

/// Ruling R75: rows are the proof of a root. A project moved out from under
/// a monorepo's `.git` — its own `.openalpaca` gone with it — is nested under
/// another marker the ordinary walk would resolve it to, but a row still
/// names it exactly, so `resolve_purge_root` takes the literal path as given
/// before ever consulting the walk.
#[tokio::test]
async fn a_root_rows_still_name_purges_literally_under_anothers_marker() {
    let f = Fixture::new();
    let ancestor = f.projects.path().join("mono");
    std::fs::create_dir_all(ancestor.join(".git")).expect(".git marker");
    // No `.openalpaca` of its own: the marker walk would otherwise resolve
    // this to `ancestor` through the `.git` it finds there.
    let moved_away = f.bare_dir("mono/moved-away");
    f.session_with_log("s-moved", &moved_away);

    let (status, body) = f.purge(Some(&moved_away), false, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 0);
}

/// The same shape with nothing recorded under the nested path still gets the
/// ordinary ancestor refusal — R75 only takes a row's word for a root rows
/// actually name; a path nothing names is still subject to the walk.
#[tokio::test]
async fn a_path_under_anothers_marker_with_no_rows_is_still_refused_naming_the_ancestor() {
    let f = Fixture::new();
    let ancestor = f.projects.path().join("mono2");
    std::fs::create_dir_all(ancestor.join(".git")).expect(".git marker");
    let never_recorded = f.bare_dir("mono2/never-recorded");
    let ancestor = ancestor
        .canonicalize()
        .expect("canonicalize ancestor")
        .to_string_lossy()
        .into_owned();

    let (status, body) = f.purge(Some(&never_recorded), false, true).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_A_ROOT");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&ancestor), "the ancestor is named: {message}");
}

#[tokio::test]
async fn a_run_in_flight_refuses_the_whole_purge() {
    let f = Fixture::new();
    let root = f.project("busy");
    f.session_with_log("s-busy", &root);
    f.task("t-busy", &root, "running");

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    // Refused before a row moved: the session is still there.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 1);
}

/// Minor #2: a `queued` run has not reached `running` yet, but it already
/// named this root when it was dispatched — the purge's busy predicate is
/// stricter than `active_tasks` for exactly this row.
#[tokio::test]
async fn a_queued_run_under_the_root_refuses_the_purge() {
    let f = Fixture::new();
    let root = f.project("queued-busy");
    f.session_with_log("s-queued", &root);
    f.task("t-queued", &root, "queued");

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 1);
}

#[tokio::test]
async fn a_dry_run_deletes_nothing_and_lists_both_verdicts() {
    let f = Fixture::new();
    let root = f.project("proj");
    let dir = f.session_with_log("s-1", &root);
    f.message("s-1");
    f.task("t-1", &root, "completed");
    f.artifact(&root, "report.md");
    let upload = f.upload(&root, "notes.txt");

    let (status, body) = f.purge(Some(&root), false, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["applied"], false);
    assert_eq!(body["projects"][0]["path"], root);

    // Counted, in the plan's own terms.
    assert_eq!(body["projects"][0]["counts"]["sessions"], 1);
    assert_eq!(body["projects"][0]["counts"]["messages"], 1);
    assert_eq!(body["projects"][0]["counts"]["tasks"], 1);
    assert_eq!(body["projects"][0]["counts"]["uploads"], 1);
    assert_eq!(body["projects"][0]["kept"]["artifacts"], 1);
    assert!(
        body["projects"][0]["kept"].get("memories").is_none(),
        "the purge counts no memory rows — notes are files under memory/: {body}"
    );

    // Both verdicts, each with the retention class it comes from.
    assert_eq!(action(&body, "sessions/"), "delete");
    assert_eq!(action(&body, "runs (database)"), "delete");
    assert_eq!(action(&body, "uploads/"), "delete");
    assert_eq!(action(&body, "artifacts/"), "keep");
    assert_eq!(action(&body, "memory/"), "keep");
    assert_eq!(action(&body, "skills/, config/"), "keep");
    assert!(
        entry(&body, "artifacts/").unwrap()["retention"]
            .as_str()
            .unwrap()
            .contains("never garbage-collected")
    );

    // And nothing happened.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 1);
    assert_eq!(f.row_count("SELECT COUNT(*) FROM task"), 1);
    assert_eq!(f.row_count("SELECT COUNT(*) FROM file_assets"), 2);
    assert!(dir.exists());
    assert!(upload.exists());
    assert!(body["projects"][0]["removed"].is_null());
}

#[tokio::test]
async fn a_real_purge_removes_the_rows_and_the_bytes_it_named() {
    let f = Fixture::new();
    let root = f.project("proj");
    let dir = f.session_with_log("s-1", &root);
    f.message("s-1");
    f.task("t-1", &root, "completed");
    f.artifact(&root, "report.md");
    let upload = f.upload(&root, "notes.txt");
    // A name the store did not create, in the project's folder.
    let folder = store::store_root(&StoreScope::Project(std::path::PathBuf::from(&root)))
        .expect("the project's folder");
    let unknown = folder.join("notes-of-my-own");
    std::fs::create_dir_all(&unknown).expect("unknown dir");

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["applied"], true);
    assert_eq!(body["projects"][0]["counts"]["sessions"], 1);
    assert_eq!(body["projects"][0]["removed"]["session_dirs"], 1);
    assert_eq!(body["projects"][0]["removed"]["upload_files"], 1);

    // Gone.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 0);
    assert_eq!(f.row_count("SELECT COUNT(*) FROM conversation_messages"), 0);
    assert_eq!(f.row_count("SELECT COUNT(*) FROM task"), 0);
    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM file_assets WHERE origin = 'upload'"),
        0
    );
    assert!(!dir.exists());
    assert!(!upload.exists());

    // Kept, and said so in the plan.
    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM file_assets WHERE origin = 'produced'"),
        1
    );
    assert!(
        unknown.exists(),
        "the store deleted a name it did not create"
    );
    assert_eq!(action(&body, "notes-of-my-own"), "keep");
    assert!(
        entry(&body, "project.json").is_none(),
        "project.json is the folder's own file, not an unknown one"
    );
    assert!(
        !std::path::Path::new(&root)
            .join(store::STORE_DIR_NAME)
            .exists(),
        "nothing in the project's own folder (ST-1)"
    );
    let artifacts = folder.join("artifacts");
    assert!(artifacts.is_dir());
}

/// D1's purge half: a live writer is stood down before its directory goes, so a
/// record emitted afterwards on a handle a turn captured earlier cannot
/// re-create it (`SessionLogWriter::open` does `create_dir_all`).
#[tokio::test]
async fn a_purge_stands_the_writers_down_so_a_late_record_re_creates_nothing() {
    use openalpaca_core::session_log::{Record, RecordType};

    let f = Fixture::new();
    let root = f.project("narrated");
    f.session("s-live", &root);
    f.artifact(&root, "report.md");

    let handle = f.log.open("s-live", Some("owner-1:gui"), Some("gui"), None);
    assert!(handle.emit(Record::new(RecordType::UserMsg)));
    assert!(handle.flush().await);
    let dir = f
        .sessions_root
        .join(openalpaca_core::session_log::session_dir_name("s-live"));
    assert!(dir.is_dir(), "the writer created {}", dir.display());

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["projects"][0]["removed"]["session_dirs"], 1);
    assert!(!dir.exists());
    assert!(
        handle.is_closed(),
        "the writer was not stood down, so its next record reopens the log",
    );

    handle.emit(Record::new(RecordType::AssistantMsg));
    handle.flush().await;
    assert!(
        !dir.exists(),
        "a record emitted after the purge re-created {}",
        dir.display()
    );
}

/// D8: every client's sidebar learns that a conversation is gone from
/// `session_changed{status:"deleted"}` — the frame `DELETE /v1/sessions/{id}`
/// publishes. The purge deletes conversations by the handful and used to say
/// nothing at all.
#[tokio::test]
async fn a_purge_announces_every_conversation_it_deleted() {
    let f = Fixture::new();
    let root = f.project("announced");
    f.session("s-a", &root);
    f.session("s-b", &root);
    f.artifact(&root, "report.md");
    let mut rx = f.bus.subscribe();

    // A dry run deletes nothing, so it announces nothing.
    let (status, _) = f.purge(Some(&root), false, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rx.try_recv().is_err(), "a dry run announced a deletion");

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut deleted: Vec<String> = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let openalpaca_core::events::SystemEvent::SessionChanged {
            session_id,
            lane_key,
            status,
            ..
        } = event
        {
            assert_eq!(status, "deleted");
            assert_eq!(lane_key, session_id, "the fixture's lane key is the id");
            deleted.push(session_id);
        }
    }
    deleted.sort();
    assert_eq!(deleted, vec!["s-a".to_string(), "s-b".to_string()]);
}

/// D10: `--all` was all-or-nothing only for its refusals. A DB failure on the
/// second root used to leave the first one purged and the `500` named neither.
/// One transaction for every root's rows.
#[tokio::test]
async fn a_failure_on_the_second_root_leaves_the_first_untouched() {
    let f = Fixture::new();
    let first = f.project("first");
    let second = f.project("second");
    f.session("s-first", &first);
    f.session("s-second", &second);
    f.artifact(&first, "first.md");
    f.artifact(&second, "second.md");

    // The second root's session refuses to be deleted — the shape of any
    // mid-loop DB failure, from a trigger rather than from luck.
    f.db
        .with_connection(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER refuse_second BEFORE DELETE ON session
                   WHEN OLD.id = 's-second'
                   BEGIN SELECT RAISE(ABORT, 'no'); END;",
            )?;
            Ok(())
        })
        .expect("trigger");

    let (status, body) = f.purge(None, true, false).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(error_code(&body), "DB_ERROR");

    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM session"),
        2,
        "the first root's rows were rolled back with the second's",
    );
    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM file_assets WHERE origin = 'produced'"),
        2,
    );
}

#[tokio::test]
async fn purging_everything_lists_each_root_and_keeps_the_home_scope() {
    let f = Fixture::new();
    let one = f.project("one");
    let two = f.project("two");
    f.session_with_log("s-one", &one);
    f.session_with_log("s-two", &two);
    // A conversation with no project: the home scope, which is not a project.
    f.db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session (id, lane_key, source) VALUES ('s-home', 's-home', 'gui')",
            [],
        )?;
        Ok(())
    })
    .expect("home session");

    let (status, body) = f.purge(None, true, false).await;
    assert_eq!(status, StatusCode::OK);
    let paths: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&one.as_str()) && paths.contains(&two.as_str()),
        "{paths:?}"
    );

    let home = body["home_scope"].as_array().expect("home_scope is a list");
    let no_project = home
        .iter()
        .find(|e| e["entry"] == "no project")
        .expect("the home scope's own conversations are named");
    assert_eq!(no_project["action"], "keep");
    assert!(no_project["holds"].as_str().unwrap().contains("1 conversations"));
    let state = home
        .iter()
        .find(|e| e["entry"] == "state/")
        .expect("the home store's state/ is named as kept too");
    assert_eq!(state["action"], "keep");
    // The projects' sessions went; the home scope's stayed.
    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM session WHERE workspace_id IS NULL"),
        1
    );
    assert_eq!(
        f.row_count("SELECT COUNT(*) FROM session WHERE workspace_id IS NOT NULL"),
        0
    );
}

/// Minor #3: `state/` is never a purge target at all, and `--all` says so —
/// on a dry run just as much as a real one, since the plan is what a reader
/// checks before passing `-y`.
#[tokio::test]
async fn an_all_dry_run_names_the_home_stores_state_as_kept() {
    let f = Fixture::new();
    let root = f.project("proj");
    f.session_with_log("s-1", &root);

    let (status, body) = f.purge(None, true, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dry_run"], true);
    let home = body["home_scope"].as_array().expect("home_scope is a list");
    let state = home
        .iter()
        .find(|e| e["entry"] == "state/")
        .expect("the plan never mentions state/");
    assert_eq!(state["action"], "keep");
    let retention = state["retention"].as_str().unwrap();
    assert!(retention.contains("factory reset"));
    // A factory reset deletes the database, not the directory: telling a user
    // that deleting `state/` is one would cost them the master key, the memory
    // control snapshots and the search index too.
    assert!(
        retention.contains("only the database"),
        "the plan must say a factory reset deletes only the database: {retention}"
    );
    // Nothing happened — it is still a dry run.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 1);
}

#[tokio::test]
async fn one_busy_root_refuses_all_of_them() {
    let f = Fixture::new();
    let quiet = f.project("quiet");
    let busy = f.project("busy");
    f.session_with_log("s-quiet", &quiet);
    f.session_with_log("s-busy", &busy);
    f.task("t-busy", &busy, "paused");

    let (status, body) = f.purge(None, true, false).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    // Never half applied: the quiet root's session is still there too.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 2);
}

#[tokio::test]
async fn a_purge_with_no_dry_run_field_is_a_dry_run() {
    // The route's own default, not the CLI's: a caller that forgets the field
    // gets the plan.
    let request: PurgeRequest =
        serde_json::from_value(serde_json::json!({ "path": "/somewhere" })).expect("parse");
    assert!(request.dry_run);
    assert!(!request.all);
}

// ============================================================================
// POST /v1/workspaces/purge and the memory partition (plan 3.7)
// ============================================================================

/// Design 09 §13.3 (LQ4): a real purge deletes conversations, and their
/// summaries go with them — found by the message ids the transaction
/// deleted — while every other note of the project stays, and the plan says
/// so on its `memory/` line, still a `keep`.
#[tokio::test]
async fn a_purge_forgets_the_summaries_of_the_conversations_it_deletes() {
    let f = Fixture::new();
    let root = f.project("proj");
    f.session_with_log("s-1", &root);
    f.message("s-1");
    let row = f.row_count("SELECT id FROM conversation_messages WHERE session_id = 's-1'");
    let project = std::path::Path::new(&root);
    crate::test_util::extracted_note(
        &f.memory,
        OWNER,
        Some(project),
        "Conversation on 2026-09-20: the build",
        &[row],
        true,
    );
    f.note(&root, "Release procedure");

    let (status, body) = f.purge(Some(&root), false, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(action(&body, "memory/"), "keep");
    assert!(
        entry(&body, "memory/").unwrap()["holds"]
            .as_str()
            .unwrap()
            .contains("episode notes summarising the conversations it"),
        "{body}"
    );
    assert_eq!(
        crate::test_util::note_titles(&f.memory, OWNER, Some(project)).len(),
        2,
        "a dry run forgets nothing"
    );

    let (status, body) = f.purge(Some(&root), false, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        crate::test_util::note_titles(&f.memory, OWNER, Some(project)),
        vec!["Release procedure".to_string()]
    );
}

/// A project the database has no row for is still a project when its memory
/// partition exists: `--all` lists it (where the `memory` UNION used to),
/// its plan keeps `memory/` and counts nothing, and a real purge — of it, or
/// of a project with rows — leaves every note file and its snapshot byte for
/// byte.
#[tokio::test]
async fn a_purge_lists_and_keeps_a_project_known_only_by_its_memory() {
    let f = Fixture::new();
    let with_rows = f.project("with-rows");
    f.session_with_log("s-rows", &with_rows);
    f.note(&with_rows, "Rows note");
    let notes_only = f.project("notes-only");
    f.note(&notes_only, "Notes only");
    let files_before: Vec<(std::path::PathBuf, Vec<u8>)> = [&with_rows, &notes_only]
        .into_iter()
        .flat_map(|root| f.note_files(root))
        .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
        .collect();
    assert_eq!(files_before.len(), 2);
    let snapshots_before = snapshots();
    assert_eq!(snapshots_before.len(), 2);

    // A single-root purge of the notes-only project answers a plan, not a 404.
    let (status, body) = f.purge(Some(&notes_only), false, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(action(&body, "memory/"), "keep");
    assert_eq!(body["projects"][0]["counts"]["sessions"], 0);

    let (status, body) = f.purge(None, true, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let mut paths: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    paths.sort();
    let mut expected = [with_rows.as_str(), notes_only.as_str()];
    expected.sort();
    assert_eq!(paths, expected);
    for project in body["projects"].as_array().unwrap() {
        let memory = project["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["entry"] == "memory/")
            .expect("memory/ is in every plan");
        assert_eq!(memory["action"], "keep");
    }

    // The rows went; the memory stayed, byte for byte.
    assert_eq!(f.row_count("SELECT COUNT(*) FROM session"), 0);
    for (path, bytes) in &files_before {
        assert_eq!(
            std::fs::read(path).unwrap_or_else(|e| panic!("{} is gone: {e}", path.display())),
            *bytes
        );
    }
    let mut after = snapshots();
    let mut before = snapshots_before;
    after.sort_by(|a, b| a.partition_key.cmp(&b.partition_key));
    before.sort_by(|a, b| a.partition_key.cmp(&b.partition_key));
    assert_eq!(after, before, "a purge never touches a control snapshot");
}

/// The partition roots the listing adds are the **caller's**: a project
/// another owner knows only by their memory partition is not named in this
/// owner's `--all` plan, and purging it by path answers the same `404` a
/// root holding somebody else's rows does (R40) — the row path never names
/// a foreign root, and the partition path must not either.
#[tokio::test]
async fn an_all_purge_lists_only_the_callers_memory_roots() {
    let f = Fixture::new();
    let mine = f.project("mine");
    f.note(&mine, "Mine");
    f.global_user("owner-2");
    let theirs = f.project("theirs");
    f.note_as("owner-2", &theirs, "Theirs");

    let (status, body) = f.purge(None, true, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let paths: Vec<&str> = body["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths,
        [mine.as_str()],
        "the plan names a root only another owner's partition knows"
    );

    let (status, body) = f.purge(Some(&theirs), false, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
}
