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

struct World {
    tmp: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    db: Database,
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
        Self {
            tmp,
            _env: env,
            home,
            db,
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
