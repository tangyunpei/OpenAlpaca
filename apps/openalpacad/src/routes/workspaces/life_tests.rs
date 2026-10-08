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

use openalpaca_core::memory::service::{
    Attribution, Capabilities, StoreInput, Surface, WorkspaceHint,
};
use openalpaca_storage::memory_files::{NoteKind, list_snapshots};
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{ArtifactKind, FileAssetRepository, NewArtifact, NewUpload, UploadStore};

const OWNER: &str = "owner-1";
const OTHER: &str = "owner-2";

struct World {
    tmp: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    db: Database,
    memory: Arc<MemoryService>,
    /// The live lane registry the delete's guard consults.
    ctx: SharedContext,
    /// Where `session_changed{deleted}` frames land.
    bus: openalpaca_core::bus::EventBus,
    sessions_root: PathBuf,
    log: Arc<openalpaca_core::session_log::SessionLogService>,
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
        let sessions_root = home.join("sessions");
        std::fs::create_dir_all(&sessions_root).expect("sessions root");
        let log = openalpaca_core::session_log::SessionLogService::new(
            sessions_root.clone(),
            None,
            openalpaca_core::session_log::SessionLogLimits::default(),
            "test".to_string(),
        )
        .into_arc();
        Self {
            tmp,
            _env: env,
            home,
            db,
            memory: Arc::new(memory),
            ctx: SharedContext::new(),
            bus: openalpaca_core::bus::EventBus::default(),
            sessions_root,
            log,
        }
    }

    fn home_session(&self, id: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO session (id, lane_key, source) VALUES (?1, ?1, 'gui')",
                    [id],
                )?;
                Ok(())
            })
            .expect("insert session");
    }

    fn home_message(&self, session: &str) -> i64 {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO conversation_messages (lane_key, role, content, session_id)
                     VALUES (?1, 'user', 'hi', ?1)",
                    [session],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .expect("insert message")
    }

    fn upload(&self, project: &Project, name: &str) -> String {
        let scope = StoreScope::Project(project.root.clone());
        UploadStore::new(&self.db)
            .put(NewUpload {
                owner_id: OWNER,
                filename: name,
                mime_type: "text/plain",
                data: name.as_bytes(),
                scope: &scope,
                created: chrono::Utc::now(),
            })
            .expect("upload")
            .asset
            .id
    }

    fn by_product(&self, project: &Project, title: &str, owner: &str) -> String {
        let scope = StoreScope::Project(project.root.clone());
        ArtifactStore::new(&self.db)
            .put(NewArtifact::new(
                owner,
                &scope,
                ArtifactKind::Markdown,
                title,
                b"body\n",
            ))
            .expect("put")
            .0
            .id
    }

    fn link(&self, message: i64, file: &str) {
        FileAssetRepository::new(&self.db)
            .link_to_message(message, file, 0, None)
            .expect("link");
    }

    fn note(&self, owner: &str, root: Option<&std::path::Path>, title: &str) {
        let ctx = self
            .memory
            .context(
                owner,
                root.map_or(WorkspaceHint::None, WorkspaceHint::Path),
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .expect("the owner is a global_user row");
        self.memory
            .store(
                &ctx,
                StoreInput {
                    title: title.to_string(),
                    body: format!("# {title}\n\nA fact worth keeping.\n"),
                    kind: Some(NoteKind::Fact),
                    ..Default::default()
                },
            )
            .expect("store a note");
    }

    fn count(&self, sql: &str) -> i64 {
        self.db
            .with_connection(|conn| Ok(conn.query_row(sql, [], |row| row.get::<_, i64>(0))?))
            .expect("count")
    }

    fn deps(&self) -> PurgeDeps<'_> {
        PurgeDeps {
            db: &self.db,
            ctx: &self.ctx,
            bus: &self.bus,
            owner: OWNER,
            memory: &self.memory,
            sessions_root: Some(self.sessions_root.clone()),
            session_log: Some(self.log.clone()),
            learning: None,
        }
    }

    async fn delete(
        &self,
        path: &std::path::Path,
        dry_run: bool,
    ) -> (StatusCode, serde_json::Value) {
        split(
            delete_workspace_data(
                &self.deps(),
                PurgeRequest {
                    path: text(path),
                    dry_run,
                },
            )
            .await,
        )
        .await
    }

    /// `p` to delete, with something on every leg; `q` a bystander.
    fn scene(&self) -> Scene {
        let p = self.project("doomed");
        let q = self.project("bystander");
        let (pr, qr) = (text(&p.root), text(&q.root));
        self.session("s-p", &pr);
        let session_dir = self
            .sessions_root
            .join(openalpaca_core::session_log::session_dir_name("s-p"));
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(session_dir.join("log.jsonl"), "{}\n").expect("session log");
        let m_p = self.message("s-p", &pr);
        self.task("t-p", &pr, "completed");
        self.session("s-q", &qr);
        self.message("s-q", &qr);
        self.home_session("s-h");
        let m_h = self.home_message("s-h");
        let u_alone = self.upload(&p, "alone.txt");
        let u_shared = self.upload(&p, "shared.txt");
        self.by_product(&p, "Report", OWNER);
        self.link(m_p, &u_alone);
        self.link(m_p, &u_shared);
        self.link(m_h, &u_shared);
        self.note(OWNER, Some(p.root.as_path()), "Release on Fridays");
        self.note(OTHER, Some(p.root.as_path()), "Staging is blue");
        self.note(OWNER, Some(q.root.as_path()), "Bystander note");
        self.note(OWNER, None, "Kept at home");
        std::fs::write(p.root.join("README.md"), b"the user's own file").expect("own file");
        Scene {
            p,
            q,
            u_shared,
            session_dir,
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

    /// Project A registered at `repos/<from>`, its folder then moved to
    /// `repos/<to>` with a link left at the old path — on macOS a case-only
    /// rename does the same: the recorded root no longer canonicalizes to
    /// itself — and project B registered where the link leads, as the next
    /// turn in the moved folder would.
    #[cfg(unix)]
    fn moved_behind_a_link(&self, from: &str, to: &str) -> (Project, Project) {
        let a = self.project(from);
        let moved = a.root.with_file_name(to);
        std::fs::rename(&a.root, &moved).expect("the user moves the folder");
        std::os::unix::fs::symlink(&moved, &a.root).expect("and leaves a link at the old path");
        let b = projects::register(&moved).expect("a turn in the moved folder registers it");
        assert_ne!(a.id, b.id);
        (a, b)
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

struct Scene {
    p: Project,
    q: Project,
    u_shared: String,
    session_dir: PathBuf,
}

/// Control snapshots carrying `id`.
fn project_snapshots(id: &ProjectId) -> usize {
    list_snapshots()
        .expect("snapshots")
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .filter(|s| s.project_id.as_ref() == Some(id))
        .count()
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

// ── POST /v1/workspaces/purge — delete its data (ST-19) ─────────

#[test]
fn a_delete_request_with_no_dry_run_field_is_a_dry_run_and_all_is_gone() {
    let request: PurgeRequest =
        serde_json::from_value(serde_json::json!({"path": "/p"})).expect("parses");
    assert!(
        request.dry_run,
        "the destructive reading of a missing field is never taken"
    );
    assert!(
        serde_json::from_value::<PurgeRequest>(serde_json::json!({"all": true})).is_err(),
        "`path` is required and `all` is gone"
    );
}

#[tokio::test]
async fn the_plan_names_every_entry_counts_the_kept_files_and_changes_nothing() {
    let w = World::new();
    let s = w.scene();

    let (status, body) = w.delete(&s.p.root, true).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["dry_run"], true);
    assert_eq!(body["applied"], false);
    assert!(body["removed"].is_null());
    assert_eq!(
        body["project"],
        serde_json::to_value(ProjectView::from(&s.p)).unwrap()
    );
    assert_eq!(
        body["counts"],
        serde_json::json!({
            "sessions": 1, "messages": 1, "tool_calls": 0, "followups": 0, "tasks": 1,
            "spans": 0, "run_events": 0, "uploads": 1, "artifacts": 1, "memory_notes": 2,
        })
    );
    assert_eq!(
        body["kept"],
        serde_json::json!({"uploads": 1, "artifacts": 0})
    );
    let folder = format!("projects/{}/", s.p.name);
    let lines: Vec<(String, String)> = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| {
            assert!(
                !e["retention"].as_str().unwrap_or_default().is_empty(),
                "{e}"
            );
            (
                e["entry"].as_str().unwrap().to_string(),
                e["action"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("sessions/".to_string(), "delete"),
        ("runs (database)".to_string(), "delete"),
        (format!("{folder}uploads/"), "delete"),
        (format!("{folder}artifacts/"), "delete"),
        (format!("{folder}memory/"), "delete"),
        (folder.clone(), "delete"),
        ("uploads/ (root store)".to_string(), "keep"),
        ("artifacts/ (root store)".to_string(), "keep"),
        (text(&s.p.root), "keep"),
    ]
    .into_iter()
    .map(|(entry, action)| (entry, action.to_string()))
    .collect();
    assert_eq!(lines, expected);

    // Nothing happened.
    assert!(s.p.dir.is_dir());
    assert!(s.session_dir.exists());
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 3);
    assert_eq!(project_snapshots(&s.p.id), 2);
}

#[tokio::test]
async fn a_real_delete_removes_rows_logs_every_owners_notes_and_the_folder_and_nothing_else() {
    let w = World::new();
    let s = w.scene();
    let bystander_snapshots = project_snapshots(&s.q.id);

    let (status, body) = w.delete(&s.p.root, false).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], true);
    assert_eq!(
        body["removed"],
        serde_json::json!({"session_dirs": 1, "memory_partitions": 2, "project_folder": true})
    );
    assert_eq!(body["counts"]["messages"], 1);
    assert_eq!(body["counts"]["memory_notes"], 2);
    assert_eq!(
        body["kept"],
        serde_json::json!({"uploads": 1, "artifacts": 0})
    );

    // The project's rows, log, partitions and folder are gone.
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 2);
    assert_eq!(w.count("SELECT COUNT(*) FROM task"), 0);
    assert!(!s.session_dir.exists());
    assert!(!s.p.dir.exists());
    assert_eq!(projects::lookup(&s.p.root).expect("lookup"), None);
    assert_eq!(project_snapshots(&s.p.id), 0);
    // Every owner's notes: a read from the deleted root is an empty project (R12).
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, Some(s.p.root.as_path())),
        vec!["Kept at home".to_string()]
    );
    assert!(crate::test_util::note_titles(&w.memory, OTHER, Some(s.p.root.as_path())).is_empty());
    // The upload a home conversation still shows reads from the root store.
    let shared = FileAssetRepository::new(&w.db)
        .get_by_id(&s.u_shared)
        .expect("read")
        .expect("kept");
    assert!(
        std::path::Path::new(&shared.storage_path)
            .starts_with(w.home.join("uploads").join(project_life::KEPT_DIR)),
        "{}",
        shared.storage_path
    );
    assert_eq!(
        std::fs::read(&shared.storage_path).expect("bytes"),
        b"shared.txt"
    );

    // Nothing else: the bystander, the home notes, the project's own folder.
    assert!(s.q.dir.is_dir());
    assert_eq!(project_snapshots(&s.q.id), bystander_snapshots);
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, Some(s.q.root.as_path())),
        vec!["Bystander note".to_string(), "Kept at home".to_string()]
    );
    assert_eq!(
        std::fs::read(s.p.root.join("README.md")).expect("own file"),
        b"the user's own file"
    );
    assert!(s.p.root.join(".git").is_dir());
}

#[tokio::test]
async fn a_queued_run_or_a_live_lane_refuses_the_delete_before_anything_moves() {
    let w = World::new();
    let s = w.scene();
    w.task("t-queued", &text(&s.p.root), "queued");

    let (status, body) = w.delete(&s.p.root, false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");

    // A run dispatched with no workspace of its own, from one of p's
    // conversations: only the lane registry knows it is live.
    w.db.with_connection(|conn| {
        conn.execute("DELETE FROM task WHERE id = 't-queued'", [])?;
        conn.execute(
            "INSERT INTO task (id, title, created_by, source_lane, session_id, status)
             VALUES ('t-live', 't', 'test', 'test', 's-p', 'running')",
            [],
        )?;
        Ok(())
    })
    .expect("a live run");
    w.ctx.register_workflow_for_lane("s-p", "t-live");

    let (status, body) = w.delete(&s.p.root, false).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("s-p"),
        "{body}"
    );
    assert!(s.p.dir.is_dir());
    assert_eq!(w.count("SELECT COUNT(*) FROM session WHERE id = 's-p'"), 1);
}

/// Waits out the real `QUIESCE_RETRY_FOR` (3 s), as the re-point's twin does.
#[tokio::test]
async fn a_held_memory_slot_answers_busy_and_deletes_nothing() {
    let w = World::new();
    let s = w.scene();
    let _held = w.memory.quiesce(&s.p.id).expect("the slot is idle");

    let (status, body) = w.delete(&s.p.root, false).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 3);
    assert!(s.p.dir.is_dir());
}

/// The route's checks run before it waits for the memory slot, and neither
/// sees a run the lane registry does not know yet — one dispatched from a
/// conversation of the project with no workspace of its own, or queued while
/// the delete waited. The transaction counts the runs again and refuses:
/// `409`, nothing deleted, the kept copies taken back.
#[tokio::test]
async fn a_run_the_routes_checks_did_not_see_is_refused_by_the_transaction() {
    let w = World::new();
    let s = w.scene();
    w.db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO task (id, title, created_by, source_lane, session_id, status)
             VALUES ('t-dispatching', 't', 'test', 'test', 's-p', 'queued')",
            [],
        )?;
        Ok(())
    })
    .expect("a run not yet in the lane registry");

    let (status, body) = w.delete(&s.p.root, false).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing was deleted"),
        "{body}"
    );
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 3);
    assert_eq!(w.count("SELECT COUNT(*) FROM task"), 2);
    assert!(s.p.dir.is_dir());
    assert_eq!(project_snapshots(&s.p.id), 2);
    assert!(
        tree(&w.home.join("uploads").join(project_life::KEPT_DIR)).is_empty(),
        "the copy of the kept upload was taken back"
    );
}

#[tokio::test]
async fn a_project_holding_another_owners_rows_is_a_404() {
    let w = World::new();
    let p = w.project("shared-desk");
    w.by_product(&p, "Theirs", OTHER);

    let (status, body) = w.delete(&p.root, true).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_NOT_FOUND");
    assert_eq!(
        body["error"]["message"],
        format!("nothing of yours is recorded under {}", text(&p.root))
    );
}

#[tokio::test]
async fn a_path_no_project_records_is_a_404_and_registers_nothing() {
    let w = World::new();
    let repo = w.repo("unused");
    let plain = w.dir("plain");
    for (path, says) in [
        (&repo, "is not a project OpenAlpaca knows"),
        (&plain, "is in no project"),
    ] {
        let (status, body) = w.delete(path, false).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(says),
            "{body}"
        );
    }
    assert!(
        !w.home.join(projects::PROJECTS_DIR).exists(),
        "a delete registered something"
    );
}

/// The lead's R-S8: deleting a project's data takes the project exactly —
/// its root as registered. A path inside it is refused before anything is
/// counted, the answer naming the root, on a dry run and a real one alike;
/// the root itself is then deleted.
#[tokio::test]
async fn a_path_inside_a_project_is_refused_naming_its_root_and_nothing_is_deleted() {
    let w = World::new();
    let s = w.scene();
    let root = text(&s.p.root);
    let inside = s.p.root.join("src");
    std::fs::create_dir_all(&inside).expect("a folder inside the project");

    for dry_run in [true, false] {
        let (status, body) = w.delete(&inside, dry_run).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(error_code(&body), "WORKSPACE_NOT_A_ROOT");
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&format!("inside the project rooted at {root}")),
            "{message}"
        );
        assert!(
            message.contains(&format!("Delete the data of {root}")),
            "{message}"
        );
    }
    assert!(s.p.dir.is_dir());
    assert!(s.session_dir.exists());
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 3);

    let (status, body) = w.delete(&s.p.root, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!s.p.dir.exists());
}

/// The delete removes `projects/<name>/` as one directory; a row whose
/// `storage_path` points anywhere else is never followed.
#[tokio::test]
async fn the_delete_never_follows_a_storage_path_out_of_the_project_folder() {
    let w = World::new();
    let p = w.project("tampered");
    let file = w.by_product(&p, "Report", OWNER);
    let decoy = w.dir("elsewhere").join("decoy.md");
    std::fs::write(&decoy, b"not OpenAlpaca's").expect("decoy");
    w.db.with_connection(|conn| {
        conn.execute(
            "UPDATE file_assets SET storage_path = ?1 WHERE id = ?2",
            [text(&decoy).as_str(), file.as_str()],
        )?;
        Ok(())
    })
    .expect("tamper");

    let (status, body) = w.delete(&p.root, false).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(std::fs::read(&decoy).expect("decoy"), b"not OpenAlpaca's");
    assert!(!p.dir.exists());
}

#[tokio::test]
async fn a_delete_stands_the_writers_down_and_announces_every_conversation() {
    use openalpaca_core::session_log::{Record, RecordType, session_dir_name};

    let w = World::new();
    let p = w.project("narrated");
    let root = text(&p.root);
    w.session("s-a", &root);
    w.session("s-b", &root);
    let handle = w.log.open("s-a", Some("owner-1:gui"), Some("gui"), None);
    assert!(handle.emit(Record::new(RecordType::UserMsg)));
    assert!(handle.flush().await);
    let dir = w.sessions_root.join(session_dir_name("s-a"));
    assert!(dir.is_dir(), "the writer created {}", dir.display());
    let mut rx = w.bus.subscribe();

    let (status, _) = w.delete(&p.root, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(rx.try_recv().is_err(), "a dry run announced a deletion");

    let (status, body) = w.delete(&p.root, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!dir.exists());
    assert!(handle.is_closed(), "the writer was not stood down");
    handle.emit(Record::new(RecordType::AssistantMsg));
    handle.flush().await;
    assert!(!dir.exists(), "a late record re-created {}", dir.display());

    let mut deleted = Vec::new();
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

/// A directory nobody may write until this is dropped — then `0700` again,
/// as `create_private_dir` made it, so a failed assertion never leaves the
/// test's tempdir unremovable.
#[cfg(unix)]
struct ReadOnlyDir(PathBuf);

#[cfg(unix)]
impl ReadOnlyDir {
    fn set(dir: PathBuf) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .expect("make the directory read-only");
        Self(dir)
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyDir {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
    }
}

/// `remove_project` stops at the first partition it cannot drop. The folder
/// stays then — the project still registered, `project_folder: false` — and
/// deleting its data again finds no row, drops the partitions left and then
/// the folder. Removed at once, it would strand those snapshots where nothing
/// lists them (R-S11) and no delete can reach them.
#[cfg(unix)]
#[tokio::test]
async fn a_partition_that_cannot_be_dropped_keeps_the_folder_and_a_second_delete_finishes() {
    let w = World::new();
    let s = w.scene();
    let control = w
        .home
        .join("state")
        .join(openalpaca_storage::memory_files::CONTROL_DIR);

    // No snapshot can be removed from a directory nobody may write.
    let read_only = ReadOnlyDir::set(control);
    let (status, body) = w.delete(&s.p.root, false).await;
    drop(read_only);

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], true);
    assert_eq!(
        body["removed"],
        serde_json::json!({"session_dirs": 1, "memory_partitions": 0, "project_folder": false})
    );
    assert_eq!(
        body["counts"]["memory_notes"], 0,
        "the notes are still on disk in the kept folder: none is reported deleted"
    );
    assert_eq!(w.count("SELECT COUNT(*) FROM session"), 2, "the rows went");
    assert!(s.p.dir.is_dir(), "the folder is kept");
    assert_eq!(
        projects::lookup(&s.p.root)
            .expect("lookup")
            .map(|found| found.id),
        Some(s.p.id),
        "the project stays registered"
    );
    assert_eq!(project_snapshots(&s.p.id), 2);

    let (status, body) = w.delete(&s.p.root, false).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["counts"]["sessions"], 0, "no row was left");
    assert_eq!(
        body["removed"],
        serde_json::json!({"session_dirs": 0, "memory_partitions": 2, "project_folder": true})
    );
    assert_eq!(
        body["counts"]["memory_notes"], 2,
        "the notes went with the folder"
    );
    assert!(!s.p.dir.exists());
    assert_eq!(projects::lookup(&s.p.root).expect("lookup"), None);
    assert_eq!(project_snapshots(&s.p.id), 0);
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, Some(s.p.root.as_path())),
        vec!["Kept at home".to_string()]
    );
}

/// The delete on a thread of its own with its own runtime, so a hang fails
/// the test instead of hanging the suite (a stuck thread is left behind when
/// the process exits). No session writer: the test's runtime is blocked
/// while it waits.
fn delete_on_its_own_thread(
    w: &World,
    path: &std::path::Path,
) -> std::sync::mpsc::Receiver<(StatusCode, serde_json::Value)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let db = w.db.clone();
    let memory = w.memory.clone();
    let bus = w.bus.clone();
    let sessions_root = w.sessions_root.clone();
    let path = text(path);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let answer = runtime.block_on(async {
            let ctx = SharedContext::new();
            let deps = PurgeDeps {
                db: &db,
                ctx: &ctx,
                bus: &bus,
                owner: OWNER,
                memory: &memory,
                sessions_root: Some(sessions_root),
                session_log: None,
                learning: None,
            };
            split(
                delete_workspace_data(
                    &deps,
                    PurgeRequest {
                        path,
                        dry_run: false,
                    },
                )
                .await,
            )
            .await
        });
        let _ = tx.send(answer);
    });
    rx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_forgets_its_conversations_home_summaries_without_waiting_on_its_own_slot() {
    let w = World::new();
    let p = w.project("summarised");
    let root = text(&p.root);
    w.session("s-1", &root);
    let row = w.message("s-1", &root);
    crate::test_util::extracted_note(
        &w.memory,
        OWNER,
        None,
        "Conversation on 2026-10-01: at home",
        &[row],
        true,
    );
    crate::test_util::extracted_note(
        &w.memory,
        OWNER,
        Some(p.root.as_path()),
        "Conversation on 2026-10-01: in the project",
        &[row],
        true,
    );
    w.note(OWNER, None, "Kept at home");

    let (status, body) = delete_on_its_own_thread(&w, &p.root)
        .recv_timeout(Duration::from_secs(20))
        .expect("the delete did not finish: a forget into the project's partition waits on the slot the delete holds");

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, None),
        vec!["Kept at home".to_string()],
        "the home summary of the deleted conversation is forgotten"
    );
    assert_eq!(
        body["removed"]["memory_partitions"], 1,
        "the project's own summary went with its partition"
    );
}

/// Every file under `dir`, with its bytes, sorted.
fn tree(dir: &std::path::Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().expect("type").is_dir() {
            out.extend(tree(&path));
        } else {
            out.push((path.clone(), std::fs::read(&path).expect("bytes")));
        }
    }
    out.sort();
    out
}

/// Fixture 17, the delete step (ST-19): deleting `project_d`'s data drops
/// its partition — note, snapshot, index rows — and leaves `moved`'s and
/// `project_c`'s partitions and the home partition byte for byte, so the
/// factory reset and `adopt` steps after it still find them.
#[tokio::test]
async fn fixture_17_deleting_a_projects_data_drops_its_partition_and_no_other() {
    let w = World::new();
    let moved = w.project("moved");
    let c = w.project("project_c");
    let d = w.project("project_d");
    w.note(OWNER, None, "Global note");
    w.note(OWNER, Some(moved.root.as_path()), "Project A release step");
    w.note(OWNER, Some(c.root.as_path()), "Project C note");
    w.note(OWNER, Some(d.root.as_path()), "Project D note");
    let others_before: Vec<_> = [&moved.dir, &c.dir, &w.home.join("memory")]
        .into_iter()
        .flat_map(|dir| tree(dir))
        .collect();
    // The parsed snapshots, by key: `list_snapshots` skips anything in
    // `state/memory-control/` that is not one.
    let snapshots = || {
        let mut all: Vec<_> = list_snapshots()
            .expect("snapshots")
            .into_iter()
            .filter_map(|(_, parsed)| parsed.ok())
            .collect();
        all.sort_by(|a, b| a.partition_key.cmp(&b.partition_key));
        all
    };
    let snapshots_before = snapshots();
    assert_eq!(snapshots_before.len(), 4);

    let (status, body) = w.delete(&d.root, false).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !d.dir.exists(),
        "part:alice_project_d is gone with projects/<name>/"
    );
    assert_eq!(
        project_snapshots(&d.id),
        0,
        "snapshot_missing: alice_project_d"
    );
    // Compared before any read below can touch a partition.
    let others_after: Vec<_> = [&moved.dir, &c.dir, &w.home.join("memory")]
        .into_iter()
        .flat_map(|dir| tree(dir))
        .collect();
    assert_eq!(
        others_after, others_before,
        "no other partition's files changed"
    );
    let snapshots_after = snapshots();
    assert_eq!(snapshots_after.len(), 3);
    assert!(
        snapshots_after
            .iter()
            .all(|snapshot| snapshots_before.contains(snapshot)),
        "no other snapshot changed"
    );
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, Some(d.root.as_path())),
        vec!["Global note".to_string()],
        "notes_not_on_disk: the project's note is no longer found"
    );
}

// ── A recorded root that no longer canonicalizes to itself (review R1) ──

/// The Projects list prints A's recorded root, which now leads, through a
/// link, to B's. The read, the plan and the delete that name it are A's;
/// B's rows, notes and folder are untouched.
#[cfg(unix)]
#[tokio::test]
async fn a_root_now_linking_to_another_project_names_its_own_for_the_read_and_the_delete() {
    let w = World::new();
    let (a, b) = w.moved_behind_a_link("a", "b");
    let b_root = text(&b.root);
    w.session("s-b", &b_root);
    w.message("s-b", &b_root);
    w.note(OWNER, Some(b.root.as_path()), "B's own note");

    let (status, body) = w.get(&text(&a.root)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], a.id.to_string(), "{body}");
    assert_eq!(body["root"], text(&a.root), "{body}");

    let (status, body) = w.delete(&a.root, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["id"], a.id.to_string(), "{body}");
    assert_eq!(body["project"]["root"], text(&a.root), "{body}");

    let (status, body) = w.delete(&a.root, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["id"], a.id.to_string(), "{body}");
    assert!(!a.dir.exists(), "A's folder under projects/ went");
    assert!(b.dir.is_dir(), "B's folder under projects/ stays");
    assert_eq!(
        projects::lookup(&b.root).expect("lookup").map(|p| p.id),
        Some(b.id)
    );
    assert_eq!(w.count("SELECT COUNT(*) FROM session WHERE id = 's-b'"), 1);
    assert_eq!(project_snapshots(&b.id), 1, "B's partition stays");
}

/// The same state, re-pointed: the PATCH naming A's recorded root moves A,
/// and B stays where it is.
#[cfg(unix)]
#[tokio::test]
async fn a_re_point_naming_a_root_that_now_links_to_another_project_moves_that_project() {
    let w = World::new();
    let (a, b) = w.moved_behind_a_link("a", "b");
    let new = w.dir("a-new-home");

    let (status, body) = w.repoint(&a.root, &new).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["project"]["id"], a.id.to_string(), "{body}");
    assert_eq!(body["project"]["root"], text(&new), "{body}");
    let still = projects::lookup(&b.root)
        .expect("lookup")
        .expect("B is where it was");
    assert_eq!((still.id, still.dir), (b.id, b.dir));
}

// ── A re-point that has not finished (review R11) ───────────────────────

/// A re-point whose rename failed leaves the project listed at its new root
/// with its folder under the old name; `lookup` finds it under neither path
/// until the next start finishes it. Re-point and Delete data on its row —
/// or on its old root — answer `409 WORKSPACE_BUSY` naming that restart, not
/// a `404` saying no folder records it, and nothing moves.
#[tokio::test]
async fn a_project_whose_re_point_has_not_finished_answers_busy_naming_the_restart() {
    let w = World::new();
    let p = w.project("half-moved");
    let new = w.dir("half-moved-new");
    project_life::repoint_with(&w.db, &p.root, &new, &mut |at| {
        at != project_life::RepointStep::RowsRewritten
    })
    .expect_err("stopped after its rows moved");
    let elsewhere = w.dir("elsewhere");

    for path in [&new, &p.root] {
        for (what, (status, body)) in [
            ("re-point", w.repoint(path, &elsewhere).await),
            ("delete", w.delete(path, false).await),
        ] {
            assert_eq!(
                status,
                StatusCode::CONFLICT,
                "{what} {}: {body}",
                text(path)
            );
            assert_eq!(error_code(&body), "WORKSPACE_BUSY", "{body}");
            let message = body["error"]["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("next start of the daemon finishes it"),
                "{what} {}: {message}",
                text(path)
            );
        }
    }
    let marked = projects::list().expect("list");
    assert_eq!(marked.len(), 1, "nothing was registered or removed");
    assert_eq!(marked[0].id, p.id);
    assert_eq!(marked[0].dir, p.dir, "the folder was not touched");
    assert_eq!(marked[0].root, new);
}

/// The re-point on a thread of its own with its own runtime, as
/// [`delete_on_its_own_thread`] runs the delete.
fn repoint_on_its_own_thread(
    w: &World,
    old: &std::path::Path,
    new: &std::path::Path,
) -> std::sync::mpsc::Receiver<(StatusCode, serde_json::Value)> {
    let (tx, rx) = std::sync::mpsc::channel();
    let db = w.db.clone();
    let memory = w.memory.clone();
    let request = RebaseRequest {
        old_path: text(old),
        new_path: text(new),
    };
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let answer = runtime
            .block_on(async { split(rebase_workspace(&db, &memory, OWNER, request).await).await });
        let _ = tx.send(answer);
    });
    rx
}

/// Long enough for a route on its own thread to pass its checks and start
/// waiting for the memory slot the test holds.
const INTO_THE_WAIT: Duration = Duration::from_millis(400);

/// §6: refused while a run is in flight — queued, running or paused — and
/// counted before the wait as well as inside step 2's transaction.
#[tokio::test]
async fn a_queued_running_or_paused_run_refuses_the_re_point() {
    for status in ["queued", "running", "paused"] {
        let w = World::new();
        let p = w.project("busy");
        w.task("t-live", &text(&p.root), status);
        let new = w.dir("later");

        let (code, body) = w.repoint(&p.root, &new).await;

        assert_eq!(code, StatusCode::CONFLICT, "{status}: {body}");
        assert_eq!(error_code(&body), "WORKSPACE_BUSY", "{status}");
        assert_eq!(
            projects::lookup(&p.root)
                .expect("lookup")
                .map(|found| found.id),
            Some(p.id),
            "{status}: nothing moved"
        );
    }
}

/// A run dispatched while the re-point waits for the project's memory slot
/// has resolved the project by its old root; the transaction counts it and
/// refuses — `409`, nothing moved — rather than moving its row and not the
/// process.
#[tokio::test]
async fn a_run_dispatched_while_the_re_point_waits_refuses_it_and_nothing_moves() {
    let w = World::new();
    let p = w.project("racing");
    let root = text(&p.root);
    w.session("s-1", &root);
    let new = w.dir("racing-moved");
    let held = w.memory.quiesce(&p.id).expect("the slot is idle");

    let answer = repoint_on_its_own_thread(&w, &p.root, &new);
    std::thread::sleep(INTO_THE_WAIT);
    w.task("t-dispatched", &root, "running");
    drop(held);
    let (status, body) = answer
        .recv_timeout(Duration::from_secs(20))
        .expect("the re-point answered");

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(&root), "{message}");
    assert_eq!(
        projects::lookup(&p.root).expect("lookup"),
        Some(p.clone()),
        "the project is where it was, with no marker"
    );
    assert_eq!(projects::lookup(&new).expect("lookup"), None);
    assert_eq!(
        w.count(&format!(
            "SELECT COUNT(*) FROM session WHERE workspace_id = '{root}'"
        )),
        1,
        "no row moved"
    );
}

/// A delete that waits for the project's memory slot while a re-point of the
/// same project holds it acts on nothing it read before the wait: once the
/// slot is its own, the project is read again by its id, and a project that
/// moved (or went) meanwhile is a `409` — no row, no control snapshot and no
/// folder of the re-pointed project is touched, and its notes still read.
#[tokio::test]
async fn a_delete_that_waited_out_a_re_point_of_its_project_deletes_nothing() {
    let w = World::new();
    let s = w.scene();
    let snapshots = project_snapshots(&s.p.id);
    let new = w.dir("doomed-moved");
    let held = w.memory.quiesce(&s.p.id).expect("the slot is idle");

    let answer = delete_on_its_own_thread(&w, &s.p.root);
    std::thread::sleep(INTO_THE_WAIT);
    let moved = project_life::repoint(&w.db, &s.p.root, &new).expect("the re-point");
    drop(held);
    let (status, body) = answer
        .recv_timeout(Duration::from_secs(20))
        .expect("the delete answered");

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("nothing was deleted"), "{message}");
    assert_eq!(project_snapshots(&s.p.id), snapshots, "no snapshot went");
    assert!(moved.after.dir.is_dir(), "the re-pointed folder is kept");
    assert_eq!(
        w.count(&format!(
            "SELECT COUNT(*) FROM session WHERE workspace_id = '{}'",
            text(&new)
        )),
        1,
        "its conversation is kept"
    );
    assert_eq!(
        crate::test_util::note_titles(&w.memory, OWNER, Some(new.as_path())),
        vec!["Kept at home".to_string(), "Release on Fridays".to_string()],
        "its notes still read under the new root"
    );
}

/// The re-point's twin: one that waits out another re-point of the same
/// project answers `409` rather than acting on the root it read before the
/// wait.
#[tokio::test]
async fn a_re_point_that_waited_out_another_re_point_of_its_project_moves_nothing() {
    let w = World::new();
    let p = w.project("wandering");
    let first = w.dir("first-stop");
    let second = w.dir("second-stop");
    let held = w.memory.quiesce(&p.id).expect("the slot is idle");

    let answer = repoint_on_its_own_thread(&w, &p.root, &second);
    std::thread::sleep(INTO_THE_WAIT);
    project_life::repoint(&w.db, &p.root, &first).expect("the first re-point");
    drop(held);
    let (status, body) = answer
        .recv_timeout(Duration::from_secs(20))
        .expect("the re-point answered");

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(error_code(&body), "WORKSPACE_BUSY");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("nothing was re-pointed"), "{message}");
    assert_eq!(
        projects::lookup(&first)
            .expect("lookup")
            .map(|found| found.id),
        Some(p.id),
        "the project stays where the first re-point put it"
    );
    assert_eq!(projects::lookup(&second).expect("lookup"), None);
}

/// S5a M1: a real delete's plan lines are what was done — built from the
/// same outcome as its counts — not the plan read before the wait for the
/// memory slot. A conversation started in the project meanwhile is in both.
#[tokio::test]
async fn a_real_deletes_entries_say_what_its_counts_say() {
    let w = World::new();
    let s = w.scene();
    let held = w.memory.quiesce(&s.p.id).expect("the slot is idle");

    let answer = delete_on_its_own_thread(&w, &s.p.root);
    std::thread::sleep(INTO_THE_WAIT);
    w.session("s-late", &text(&s.p.root));
    drop(held);
    let (status, body) = answer
        .recv_timeout(Duration::from_secs(20))
        .expect("the delete answered");

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["counts"]["sessions"], 2, "{body}");
    let sessions_line = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["entry"] == "sessions/")
        .expect("the sessions/ line");
    let holds = sessions_line["holds"].as_str().unwrap_or_default();
    assert!(holds.starts_with("2 conversations"), "{holds}");
    let memory_line = body["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| {
            entry["entry"]
                .as_str()
                .is_some_and(|name| name.ends_with("/memory/"))
        })
        .expect("the memory/ line");
    let holds = memory_line["holds"].as_str().unwrap_or_default();
    assert!(
        holds.starts_with(&format!(
            "{} memory notes of every owner, in {} partitions",
            body["counts"]["memory_notes"], body["removed"]["memory_partitions"]
        )),
        "{holds}"
    );
}
