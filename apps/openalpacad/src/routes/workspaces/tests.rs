//! The verification cell for `/v1/workspaces` (§4.8, "Project moved").
//!
//! The transaction itself belongs to `ArtifactStore::rebase_project` and is
//! proved in `openalpaca_storage::artifacts::tests`; the store-directory move
//! belongs to `store::migrate` and is proved there. What is proved here is what
//! the route owns — which refusal becomes which status code, that every refusal
//! happens *before* a row changes, and what the JSON carries.

use super::*;

use axum::body::to_bytes;
use openalpaca_storage::store::StoreScope;
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
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            projects,
            db,
        }
    }

    /// A project directory carrying a real `.openalpaca` store — seeded through
    /// `ensure_store`, so its `.layout` records the root it was seeded at.
    fn project(&self, name: &str) -> String {
        let dir = self.projects.path().join(name);
        std::fs::create_dir_all(&dir).expect("project dir");
        let dir = dir.canonicalize().expect("canonicalize project");
        store::ensure_store(&StoreScope::Project(dir.clone())).expect("seed store");
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

/// The user's `mv`: the whole store travels to the new root, marker and all,
/// so it still records the root it was seeded at. Nothing in the database moves
/// — which is precisely the state a re-base exists to fix.
fn move_the_store_by_hand(old: &str, new: &str) {
    let from = std::path::Path::new(old).join(store::STORE_DIR_NAME);
    let to = std::path::Path::new(new).join(store::STORE_DIR_NAME);
    std::fs::create_dir_all(new).expect("new project dir");
    std::fs::rename(&from, &to).expect("move the store");
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
    let new_dir = f.projects.path().join("new-project");

    // Everything is recorded under the root the project had while it was there.
    f.artifact(&old, "Notes");
    f.session("session-1", &old);
    f.task("task-1", &old, "completed");

    let new = {
        move_the_store_by_hand(&old, &new_dir.to_string_lossy());
        new_dir
            .canonicalize()
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned()
    };

    let (status, body) = f.get(Some(&new)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["path"], new);
    assert_eq!(body["store_present"], true);
    assert_eq!(body["recorded_root"], old);
    assert_eq!(body["moved"], true);
    // Nothing is recorded at the *new* root yet — that is what a re-base fixes.
    assert_eq!(body["rows"]["artifacts"], 0);

    // And the old root is where the history is.
    let (status, body) = f.get(Some(&old)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["store_present"], false);
    assert_eq!(body["recorded_root"], serde_json::Value::Null);
    assert_eq!(body["moved"], false);
    assert_eq!(body["rows"]["artifacts"], 1);
    assert_eq!(body["rows"]["sessions"], 1);
    assert_eq!(body["rows"]["tasks"], 1);
    assert_eq!(body["active_tasks"], 0);
}

#[tokio::test]
async fn a_store_standing_where_it_was_seeded_is_not_moved() {
    let f = Fixture::new();
    let root = f.project("project");

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
async fn re_basing_onto_a_root_that_already_has_rows_is_a_409() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.project("new-project");
    f.artifact(&old, "Mine");
    f.artifact(&new, "Theirs");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT);
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
async fn two_stores_block_the_re_base_before_the_transaction() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.project("new-project");
    f.artifact(&old, "Notes");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "WORKSPACE_MOVE_BLOCKED");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("two stores"),
        "{body}"
    );

    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(
        old_body["rows"]["artifacts"], 1,
        "the rows are untouched — the move is decided first"
    );
}

#[tokio::test]
async fn re_basing_to_or_from_the_home_store_is_a_409() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");

    // Both shapes of the fold: the home store root itself, and the `$HOME`
    // whose project store *is* that root.
    for destination in [f.home_store(), f.home()] {
        let (status, body) = f.patch(&old, &destination).await;
        assert_eq!(status, StatusCode::CONFLICT, "{destination}: {body}");
        assert_eq!(error_code(&body), "WORKSPACE_IS_HOME", "{destination}");
    }

    // And as the source, which would record the home store as a project just
    // as surely.
    let new = f.bare_dir("new-project");
    let (status, body) = f.patch(&f.home(), &new).await;
    assert_eq!(status, StatusCode::CONFLICT);
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

/// P-12's own shape: the user moved the project with `mv`, so the store is
/// already at the new root and only the rows are behind.
#[tokio::test]
async fn a_project_moved_by_hand_re_bases_its_four_members() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new_dir = f.projects.path().join("new-project");

    f.artifact(&old, "Notes");
    f.session("session-1", &old);
    f.task("task-1", &old, "completed");

    move_the_store_by_hand(&old, &new_dir.to_string_lossy());
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
    assert_eq!(body["moved"]["memories"], 0);
    assert_eq!(
        body["store_moved"], false,
        "the directory was already moved by hand"
    );

    // The store now says where it lives, so a second look does not offer the
    // same re-base all over again.
    let (_, after) = f.get(Some(&new)).await;
    assert_eq!(after["recorded_root"], new);
    assert_eq!(after["moved"], false);
    assert_eq!(after["rows"]["artifacts"], 1);
    assert_eq!(after["rows"]["sessions"], 1);

    let (_, old_after) = f.get(Some(&old)).await;
    assert_eq!(old_after["rows"]["artifacts"], 0);
}

/// The other shape: the caller is asking the daemon to *do* the move.
#[tokio::test]
async fn a_store_still_at_the_old_root_is_moved_after_the_transaction() {
    let f = Fixture::new();
    let old = f.project("old-project");
    let new = f.bare_dir("new-project");
    f.artifact(&old, "Notes");

    let (status, body) = f.patch(&old, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["store_moved"], true);

    assert!(
        !std::path::Path::new(&old)
            .join(store::STORE_DIR_NAME)
            .exists()
    );
    let moved_store = std::path::Path::new(&new).join(store::STORE_DIR_NAME);
    assert!(moved_store.is_dir());
    assert_eq!(
        store::recorded_project_root(&moved_store)
            .unwrap()
            .as_deref(),
        Some(new.as_str())
    );

    // The bytes are where the rows now say they are.
    let (_, after) = f.get(Some(&new)).await;
    assert_eq!(after["rows"]["artifacts"], 1);
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
    assert!(record.storage_path.starts_with(&new));
}
