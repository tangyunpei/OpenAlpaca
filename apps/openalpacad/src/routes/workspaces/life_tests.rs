//! `/v1/workspaces` after Stage 1 (design §6, §9): the list, `?path=`,
//! choosing a folder, the re-point's answer and ST-19's delete. Every store
//! is a tempdir behind the binary's one `HomeStoreGuard`; every project is a
//! git repository made for the test and registered through `store::projects`.

use super::*;

use axum::body::to_bytes;
use openalpaca_storage::store::projects::{self, Project};
use std::path::PathBuf;
use tempfile::TempDir;

use crate::test_util::HomeStoreGuard;

const OWNER: &str = "owner-1";
const OTHER: &str = "owner-2";

struct World {
    tmp: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    db: Database,
    memory: Arc<MemoryService>,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("temp root");
        let home = tmp
            .path()
            .canonicalize()
            .expect("canonical temp")
            .join("home");
        std::fs::create_dir_all(&home).expect("home store");
        let env = HomeStoreGuard::set(&home);
        let db = Database::open(&tmp.path().join("test.db")).expect("open db");
        for owner in [OWNER, OTHER] {
            openalpaca_storage::IdentityRepository::new(&db)
                .create_global_user(owner, None)
                .expect("owner row");
        }
        let memory = MemoryService::new(
            Some(db.clone()),
            openalpaca_storage::memory_index::MemoryIndex::open_in_memory().expect("index"),
        );
        memory.rebuild_index().expect("an empty index");
        Self {
            tmp,
            _env: env,
            home,
            db,
            memory: Arc::new(memory),
        }
    }

    /// A directory under the test's tempdir, canonical.
    fn dir(&self, name: &str) -> PathBuf {
        let dir = self
            .tmp
            .path()
            .canonicalize()
            .expect("canonical temp")
            .join(name);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    /// A git repository made for this test, not registered.
    fn repo(&self, name: &str) -> PathBuf {
        let root = self.dir(&format!("repos/{name}"));
        std::fs::create_dir_all(root.join(".git")).expect("git dir");
        root
    }

    fn project(&self, name: &str) -> Project {
        projects::register(&self.repo(name)).expect("register")
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

    fn message(&self, session: &str, root: &str) -> i64 {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO conversation_messages
                         (lane_key, role, content, session_id, workspace_root)
                     VALUES (?1, 'user', 'hi', ?1, ?2)",
                    [session, root],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .expect("insert message")
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

    async fn get(&self, path: &str) -> (StatusCode, serde_json::Value) {
        split(get_workspace(
            &self.db,
            WorkspaceQuery {
                path: Some(path.to_string()),
            },
        ))
        .await
    }

    async fn repoint(
        &self,
        old: &std::path::Path,
        new: &std::path::Path,
    ) -> (StatusCode, serde_json::Value) {
        split(
            rebase_workspace(
                &self.db,
                &self.memory,
                OWNER,
                RebaseRequest {
                    old_path: text(old),
                    new_path: text(new),
                },
            )
            .await,
        )
        .await
    }
}

fn text(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

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

fn zero_rows() -> serde_json::Value {
    serde_json::json!({"artifacts": 0, "sessions": 0, "tasks": 0, "messages": 0, "followups": 0})
}

// ── GET /v1/workspaces ──────────────────────────────────────────

#[tokio::test]
async fn the_list_names_every_project_and_marks_a_missing_one() {
    let w = World::new();
    let kept = w.project("kept");
    let gone = w.project("gone");
    std::fs::remove_dir_all(&gone.root).expect("the user deletes the folder");

    let (status, body) = split(get_workspace(&w.db, WorkspaceQuery::default())).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let list = body.as_array().expect("a bare array");
    assert_eq!(list.len(), 2, "{body}");
    let entry = |p: &Project| {
        list.iter()
            .find(|e| e["id"] == p.id.to_string())
            .cloned()
            .unwrap_or_else(|| panic!("{} is not listed: {body}", p.root.display()))
    };
    assert_eq!(
        entry(&kept),
        serde_json::json!({
            "id": kept.id.to_string(),
            "root": text(&kept.root),
            "name": kept.name.clone(),
            "missing": false,
        })
    );
    assert_eq!(entry(&gone)["missing"], true);
}

#[tokio::test]
async fn the_list_is_empty_and_creates_nothing_before_any_project() {
    let w = World::new();

    let (status, body) = split(get_workspace(&w.db, WorkspaceQuery::default())).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));
    assert!(
        !w.home.join(projects::PROJECTS_DIR).exists(),
        "a read created projects/"
    );
}

// ── GET /v1/workspaces?path= ────────────────────────────────────

#[tokio::test]
async fn a_path_inside_a_project_answers_the_project_and_its_five_counts() {
    let w = World::new();
    let p = w.project("app");
    let src = p.root.join("src");
    std::fs::create_dir_all(&src).expect("src");
    let root = text(&p.root);
    w.session("s-1", &root);
    w.message("s-1", &root);
    w.task("t-1", &root, "queued");

    let (status, body) = w.get(&text(&src)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    // Exact: `store_present`, `recorded_root` and `moved` are gone (§9).
    assert_eq!(
        body,
        serde_json::json!({
            "path": text(&src),
            "root": root,
            "id": p.id.to_string(),
            "name": p.name.clone(),
            "missing": false,
            "rows": {"artifacts": 0, "sessions": 1, "tasks": 1, "messages": 1, "followups": 0},
            "active_tasks": 0,
            "queued_tasks": 1,
        })
    );
}

#[tokio::test]
async fn an_unused_git_repository_answers_its_root_with_no_id_and_registers_nothing() {
    let w = World::new();
    let repo = w.repo("fresh");

    let (status, body) = w.get(&text(&repo)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["root"], text(&repo));
    assert!(body["id"].is_null(), "{body}");
    assert!(body["name"].is_null(), "{body}");
    assert_eq!(body["missing"], false);
    assert!(
        !w.home.join(projects::PROJECTS_DIR).exists(),
        "a read registered the repository"
    );
}

#[tokio::test]
async fn a_path_in_no_project_answers_a_null_root_and_zero_rows() {
    let w = World::new();
    let plain = w.dir("plain");

    let (status, body) = w.get(&text(&plain)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        serde_json::json!({
            "path": text(&plain),
            "root": null,
            "id": null,
            "name": null,
            "missing": false,
            "rows": zero_rows(),
            "active_tasks": 0,
            "queued_tasks": 0,
        })
    );
}

#[tokio::test]
async fn a_missing_project_resolves_to_itself_and_reads_missing() {
    let w = World::new();
    let p = w.project("moved-away");
    std::fs::remove_dir_all(&p.root).expect("the user moves the folder");

    let (status, body) = w.get(&text(&p.root)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["root"], text(&p.root));
    assert_eq!(body["id"], p.id.to_string());
    assert_eq!(body["missing"], true);
}

#[tokio::test]
async fn a_relative_or_empty_path_is_invalid() {
    let w = World::new();
    for bad in ["relative/dir", "", "   "] {
        let (status, body) = w.get(bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
        assert_eq!(error_code(&body), "INVALID_PATH");
    }
}

// ── POST /v1/workspaces ─────────────────────────────────────────

async fn choose(path: &str) -> (StatusCode, serde_json::Value) {
    split(choose_workspace(ChooseWorkspaceRequest {
        path: path.to_string(),
    }))
    .await
}

#[tokio::test]
async fn choosing_a_folder_inside_a_git_repository_registers_the_repository() {
    let w = World::new();
    let repo = w.repo("mono");
    let sub = repo.join("packages").join("web");
    std::fs::create_dir_all(&sub).expect("sub");

    let (status, body) = choose(&text(&sub)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let registered = projects::lookup(&repo)
        .expect("lookup")
        .expect("registered");
    assert_eq!(
        body,
        serde_json::json!({
            "id": registered.id.to_string(),
            "root": text(&repo),
            "name": registered.name.clone(),
            "missing": false,
        })
    );
    assert!(
        projects::lookup(&sub).expect("lookup").is_none(),
        "the subfolder is no project of its own"
    );
}

#[tokio::test]
async fn choosing_a_folder_with_no_git_makes_it_a_project_once() {
    let w = World::new();
    let notes = w.dir("notes");

    let (first_status, first) = choose(&text(&notes)).await;
    let (second_status, second) = choose(&text(&notes)).await;

    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert_eq!(second_status, StatusCode::OK, "{second}");
    assert_eq!(first["root"], text(&notes));
    assert_eq!(first, second, "a second choice answers the same project");
    let name = first["name"].as_str().expect("a name");
    assert!(
        w.home
            .join(projects::PROJECTS_DIR)
            .join(name)
            .join(projects::PROJECT_FILE)
            .is_file()
    );
    assert_eq!(projects::list().expect("list").len(), 1);
}

#[tokio::test]
async fn choosing_the_home_store_or_a_folder_holding_it_is_refused() {
    let w = World::new();
    let inside = w.home.join("artifacts");
    std::fs::create_dir_all(&inside).expect("inside");
    let holding = w.tmp.path().canonicalize().expect("canonical temp");
    for path in [w.home.clone(), inside, holding] {
        let (status, body) = choose(&text(&path)).await;
        assert_eq!(status, StatusCode::CONFLICT, "{}: {body}", path.display());
        assert_eq!(error_code(&body), "WORKSPACE_IS_HOME");
    }
    assert!(projects::list().expect("list").is_empty());
}

#[tokio::test]
async fn choosing_a_file_or_a_missing_path_is_unprocessable() {
    let w = World::new();
    let loose = w.dir("loose");
    let file = loose.join("note.txt");
    std::fs::write(&file, b"a file").expect("file");
    for path in [file, loose.join("no-such-folder")] {
        let (status, body) = choose(&text(&path)).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}: {body}",
            path.display()
        );
        assert_eq!(error_code(&body), "WORKSPACE_NOT_A_DIRECTORY");
    }
    assert!(projects::list().expect("list").is_empty());
}

#[tokio::test]
async fn choosing_a_relative_path_is_invalid() {
    let _w = World::new();
    let (status, body) = choose("some/folder").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(error_code(&body), "INVALID_PATH");
}

// ── PATCH /v1/workspaces ────────────────────────────────────────

#[tokio::test]
async fn a_re_point_answers_the_project_under_its_new_root_and_five_counts() {
    let w = World::new();
    let p = w.project("old-home");
    let root = text(&p.root);
    w.session("s-1", &root);
    w.message("s-1", &root);
    w.task("t-1", &root, "completed");
    let new = w.dir("new-home");

    let (status, body) = w.repoint(&p.root, &new).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let after = projects::lookup(&new)
        .expect("lookup")
        .expect("the project is at its new root");
    assert_eq!(after.id, p.id);
    assert_eq!(
        body,
        serde_json::json!({
            "old_path": root,
            "new_path": text(&new),
            "moved": {"artifacts": 0, "sessions": 1, "tasks": 1, "messages": 1, "followups": 0},
            "project": {
                "id": p.id.to_string(),
                "root": text(&new),
                "name": after.name.clone(),
                "missing": false,
            },
        })
    );
}

/// `quiesce` refuses rather than waits, and the 30 s reconciliation and an
/// index rebuild take the same slot, so the route tries for a while (§5).
/// The slot is let go after 150 ms, well inside `QUIESCE_RETRY_FOR`.
#[tokio::test]
async fn a_re_point_waits_for_a_held_memory_slot_and_then_answers() {
    let w = World::new();
    let p = w.project("held-briefly");
    let new = w.dir("free-at-last");
    let held = w.memory.quiesce(&p.id).expect("the slot is idle");
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
    });

    let (status, body) = w.repoint(&p.root, &new).await;

    release.join().expect("the releasing thread");
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["root"], text(&new));
}

/// Waits out the real `QUIESCE_RETRY_FOR` (3 s): the daemon's tests have
/// no paused tokio clock.
#[tokio::test]
async fn a_re_point_answers_busy_when_the_memory_slot_stays_held() {
    let w = World::new();
    let p = w.project("held");
    let new = w.dir("never-reached");
    let _held = w.memory.quiesce(&p.id).expect("the slot is idle");

    let started = std::time::Instant::now();
    let (status, body) = w.repoint(&p.root, &new).await;

    assert!(
        started.elapsed() >= QUIESCE_RETRY_FOR,
        "the route gave up before its window"
    );
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert_eq!(
        projects::lookup(&p.root)
            .expect("lookup")
            .map(|found| found.id),
        Some(p.id),
        "nothing moved"
    );
}

#[tokio::test]
async fn a_running_run_refuses_the_re_point() {
    let w = World::new();
    let p = w.project("running");
    w.task("t-run", &text(&p.root), "running");
    let new = w.dir("later");

    let (status, body) = w.repoint(&p.root, &new).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
}

/// R40, unchanged: rows another owner holds answer the same 404 an
/// unknown root does, and say nothing about them.
#[tokio::test]
async fn rows_of_another_owner_answer_404() {
    let w = World::new();
    let p = w.project("theirs");
    w.db
        .with_connection(|conn| {
            conn.execute(
                "INSERT INTO file_assets
                     (id, owner_id, sha256, filename, mime_type, size_bytes, storage_path, origin, project_root)
                 VALUES ('f-other', ?1, 'sha', 'x.txt', 'text/plain', 1, '/nowhere', 'upload', ?2)",
                [OTHER, text(&p.root).as_str()],
            )?;
            Ok(())
        })
        .expect("a row of another owner");
    let new = w.dir("mine-now");

    let (status, body) = w.repoint(&p.root, &new).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["error"]["message"],
        format!("nothing of yours is recorded under {}", text(&p.root))
    );
}

#[tokio::test]
async fn a_re_point_of_a_path_no_project_records_is_a_404() {
    let w = World::new();
    let repo = w.repo("never-used");
    let new = w.dir("anywhere");

    let (status, body) = w.repoint(&repo, &new).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("is not a project OpenAlpaca knows"),
        "{body}"
    );
}

#[tokio::test]
async fn a_relative_path_on_either_side_of_a_re_point_is_invalid() {
    let w = World::new();
    let p = w.project("absolute");
    let rel = std::path::Path::new("relative/dir");
    for (old, new) in [(rel, p.root.as_path()), (p.root.as_path(), rel)] {
        let (status, body) = w.repoint(old, new).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(error_code(&body), "INVALID_PATH");
    }
}
