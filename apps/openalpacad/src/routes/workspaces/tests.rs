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
        let dir = std::path::Path::new(root)
            .join(store::STORE_DIR_NAME)
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

    fn memory(&self, root: &str, content: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO memory (owner_id, kind, scope, scope_id, source, content,
                                         content_hash)
                     VALUES (?1, 'fact', 'workspace', ?2, 'test', ?3, ?3)",
                    [OWNER, root, content],
                )?;
                Ok(())
            })
            .expect("insert memory");
    }

    fn purge_deps(&self) -> PurgeDeps<'_> {
        PurgeDeps {
            db: &self.db,
            ctx: &self.ctx,
            bus: &self.bus,
            owner: OWNER,
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
async fn a_destination_inside_another_project_is_a_422() {
    let f = Fixture::new();
    let old = f.project("old-project");
    f.artifact(&old, "Notes");

    // A monorepo with its own marker, and a fresh directory inside it.
    let mono = f.bare_dir("mono");
    std::fs::create_dir_all(std::path::Path::new(&mono).join(".git")).expect("marker");
    let inside = std::path::Path::new(&mono).join("sub").join("proj");
    std::fs::create_dir_all(&inside).expect("destination");
    let inside = inside.to_string_lossy().into_owned();

    let (status, body) = f.patch(&old, &inside).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_A_ROOT");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&mono), "the ancestor is named: {message}");

    // Nothing moved, and `/mono/.openalpaca` was not created on the way past.
    let (_, old_body) = f.get(Some(&old)).await;
    assert_eq!(old_body["rows"]["artifacts"], 1);
    assert!(
        !std::path::Path::new(&mono)
            .join(store::STORE_DIR_NAME)
            .exists()
    );

    // A destination that is a root of its own is taken as given.
    let sibling = f.bare_dir("sibling");
    std::fs::create_dir_all(std::path::Path::new(&sibling).join(".git")).expect("marker");
    let (status, body) = f.patch(&old, &sibling).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["new_path"], sibling);
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

/// D5 — the PATCH twin of `a_root_rows_still_name_purges_literally_under_anothers_marker`:
/// rows are the proof of a root on the **source** side too.
///
/// A project moved out from under a monorepo's `.git`, its own `.openalpaca`
/// gone with it, is nested under a marker the ordinary walk resolves it to. The
/// walk therefore used to answer `/mono` for `old_path`, and the re-base either
/// refused ("nothing of yours is recorded under /mono" — the moved project could
/// not be re-attached by the path it came from) or, with rows under `/mono`,
/// re-addressed *those*. A row naming the literal path is the proof, so the
/// literal path is what is re-based.
#[tokio::test]
async fn a_recorded_root_under_anothers_marker_re_bases_by_its_literal_path() {
    let f = Fixture::new();
    let ancestor = f.projects.path().join("mono-rebase");
    std::fs::create_dir_all(ancestor.join(".git")).expect(".git marker");
    // No marker of its own — and no artifact either, because putting one seeds
    // a store (`.openalpaca`) at the root and the walk would then resolve the
    // path to itself, which is not the shape under test. Session and task rows
    // name it exactly; that is the proof R75 is about.
    let moved_away = f.bare_dir("mono-rebase/moved-away");
    f.session("session-moved", &moved_away);
    f.task("task-moved", &moved_away, "completed");

    // Its new home, outside the monorepo.
    let new = f.bare_dir("re-homed");

    let (status, body) = f.patch(&moved_away, &new).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["old_path"], moved_away,
        "the literal path the rows name, not the monorepo root",
    );
    assert_eq!(body["new_path"], new);
    assert_eq!(body["moved"]["sessions"], 1);
    assert_eq!(body["moved"]["tasks"], 1);

    let (_, after) = f.get(Some(&new)).await;
    assert_eq!(after["rows"]["sessions"], 1);
    assert_eq!(after["rows"]["tasks"], 1);
}

/// D6 — the `422`'s remediation has to be followable.
///
/// `WORKSPACE_NOT_A_ROOT` tells the caller to give the path a project marker of
/// its own; the marker walk's process-lifetime cache used to pin the ancestor
/// answer that the refusal's own lookup had just written, so the retry after
/// `mkdir .openalpaca` answered the same `422` until the daemon restarted. Same
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
    std::fs::create_dir_all(std::path::Path::new(&inside).join(store::STORE_DIR_NAME))
        .expect("marker");

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
    f.memory(&root, "the build takes four minutes");
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
    assert_eq!(body["projects"][0]["kept"]["memories"], 1);

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
    f.memory(&root, "the build takes four minutes");
    let upload = f.upload(&root, "notes.txt");
    // A name the store did not create, in the store root itself.
    let unknown = std::path::Path::new(&root)
        .join(store::STORE_DIR_NAME)
        .join("notes-of-my-own");
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
    assert_eq!(f.row_count("SELECT COUNT(*) FROM memory"), 1);
    assert!(
        unknown.exists(),
        "the store deleted a name it did not create"
    );
    assert_eq!(action(&body, "notes-of-my-own"), "keep");
    let artifacts = std::path::Path::new(&root)
        .join(store::STORE_DIR_NAME)
        .join("artifacts");
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
    assert!(
        state["retention"]
            .as_str()
            .unwrap()
            .contains("factory reset")
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
