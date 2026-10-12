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
use openalpaca_storage::memory_files::{ControlSnapshot, NoteKind, Partition, PartitionHealth};
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::store::{self, StoreScope, projects};
use openalpaca_storage::{ArtifactKind, NewArtifact};
use tempfile::TempDir;

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
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            projects,
            db,
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
        split(
            rebase_workspace(
                &self.db,
                &self.memory,
                owner,
                RebaseRequest {
                    old_path: old.to_string(),
                    new_path: new.to_string(),
                },
            )
            .await,
        )
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

    /// `$HOME`: the directory that holds the home store, so never a project
    /// (R13).
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

fn project_at(root: &str) -> projects::Project {
    projects::lookup(std::path::Path::new(root))
        .expect("lookup")
        .expect("a registered project")
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
