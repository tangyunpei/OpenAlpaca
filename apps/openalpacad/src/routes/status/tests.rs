//! `GET /v1/status` — the store roots, the caller's project, and the run's own
//! numbers (§4.7 item 4, §4.8, GAP-14).
//!
//! The property under test is that nothing here is invented: every path comes
//! from `openalpaca_storage::store` rather than a literal joined onto a root,
//! `schema_version` comes from the open database, the byte totals come from
//! `file_assets`, the session numbers come from the `SessionLogService` the
//! runner holds, and `project_root` is the *request's* project — `null` unless
//! the caller actually named one.

use super::*;

use crate::test_util::HomeStoreGuard;
use axum::body::to_bytes;
use chrono::TimeDelta;
use openalpaca_core::daemon_config::{RoutingConfig, SessionsConfig};
use openalpaca_core::session_log::{SessionLogLimits, SessionLogService};
use openalpaca_storage::{Database, FileAssetRepository};
use openalpaca_storage::models::file_asset::{FileAsset, FileAssetStatus};
use tempfile::TempDir;

fn headers_with(path: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(path) = path {
        headers.insert("x-workspace-path", path.parse().unwrap());
    }
    headers
}

/// A real database, migrated: `schema_version` has to be read from the thing
/// the daemon actually opened, so the tests open one too.
fn test_db(dir: &TempDir) -> Database {
    Database::open(&dir.path().join("status-test.db")).expect("database should open")
}

/// `managed_log: true` and the config's own defaults — the ordinary case for
/// every test that is not exercising those two fields specifically.
fn inputs<'a>(db: &'a Database, started_at: DateTime<Utc>) -> StatusInputs<'a> {
    StatusInputs {
        started_at,
        now: started_at,
        db,
        session_log: None,
        managed_log: true,
        sessions_config: SessionsConfig::default(),
        routing_config: RoutingConfig::default(),
    }
}

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Status only — the body is not JSON on the error paths worth asserting.
async fn body_for(db: &Database, headers: &HeaderMap) -> serde_json::Value {
    let response = status_response(&inputs(db, Utc::now()), headers);
    assert_eq!(response.status(), StatusCode::OK);
    body_of(response).await
}

#[tokio::test]
async fn reports_the_three_store_roots_from_the_store_module() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join(".openalpaca");
    let _guard = HomeStoreGuard::set(&home);
    let db = test_db(&tmp);

    let body = body_for(&db, &headers_with(None)).await;

    assert_eq!(body["home_root"], home.to_string_lossy().as_ref());
    assert_eq!(
        body["state_dir"],
        home.join("state").to_string_lossy().as_ref()
    );
    assert_eq!(
        body["db_path"],
        home.join("state")
            .join("openalpaca.db")
            .to_string_lossy()
            .as_ref()
    );
    // No header, no project — and the key is present, not omitted, so a client
    // can tell "no project" from "old daemon".
    assert!(body.get("project_root").is_some());
    assert!(body["project_root"].is_null());
}

#[tokio::test]
async fn reports_the_project_the_request_names() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home").join(".openalpaca"));
    let db = test_db(&tmp);

    let project = root.join("checkout");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(project.join(".git")).unwrap();
    let nested = project.join("crates").join("core");
    std::fs::create_dir_all(&nested).unwrap();

    // A path *inside* the project resolves up to the project root — the same
    // walk a chat turn does, so the answer names where artifacts would land.
    let body = body_for(&db, &headers_with(nested.to_str())).await;
    assert_eq!(body["project_root"], project.to_string_lossy().as_ref());
}

#[tokio::test]
async fn a_path_that_is_no_project_reports_null() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home").join(".openalpaca"));
    let db = test_db(&tmp);

    let loose = root.join("Downloads");
    std::fs::create_dir(&loose).unwrap();

    let body = body_for(&db, &headers_with(loose.to_str())).await;
    assert!(
        body["project_root"].is_null(),
        "a directory with no marker above it is not a project: {body}"
    );
}

/// The T24 carry-over, visible from the route: `$HOME` is not a project, so a
/// header pointing under it reports `null` rather than claiming the whole home
/// directory.
#[tokio::test]
async fn a_path_under_the_home_store_reports_null() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(home.join(".openalpaca")).unwrap();
    let _guard = HomeStoreGuard::set(&home.join(".openalpaca"));
    let db = test_db(&tmp);

    let documents = home.join("Documents");
    std::fs::create_dir(&documents).unwrap();

    let body = body_for(&db, &headers_with(documents.to_str())).await;
    assert!(
        body["project_root"].is_null(),
        "the home store is not a project: {body}"
    );
}

/// `started_at` is the run's own stamp and `uptime_secs` is measured against
/// it — not against the process table, and never negative.
#[tokio::test]
async fn reports_when_the_run_began_and_how_long_it_has_been_up() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let started_at = Utc::now() - TimeDelta::seconds(3_725);
    let mut inputs = inputs(&db, started_at);
    inputs.now = started_at + TimeDelta::seconds(3_725);

    let body = body_of(status_response(&inputs, &headers_with(None))).await;
    assert_eq!(body["started_at"], started_at.to_rfc3339());
    assert_eq!(body["uptime_secs"], 3_725);

    // A clock that stepped backwards reports zero, not a negative age.
    inputs.now = started_at - TimeDelta::seconds(10);
    let body = body_of(status_response(&inputs, &headers_with(None))).await;
    assert_eq!(body["uptime_secs"], 0);
}

/// The DB is the truth: the version reported is the one the open database is
/// at, not the compile-time count of migration files.
#[tokio::test]
async fn reports_the_schema_version_the_open_database_is_at() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let body = body_for(&db, &headers_with(None)).await;
    assert_eq!(body["schema_version"], db.schema_version().unwrap());
    assert!(
        body["schema_version"].as_i64().unwrap() > 0,
        "a migrated database is past 0: {body}"
    );
}

/// §4.8's two numbers, never one: an agent's output must not appear as upload
/// traffic, and neither total may swallow the other.
#[tokio::test]
async fn splits_upload_bytes_from_produced_bytes() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let repo = FileAssetRepository::new(&db);
    repo.insert(&asset("upload-1", 900)).unwrap();
    repo.insert(&asset("produced-1", 4_100)).unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE file_assets SET origin = 'produced' WHERE id = 'produced-1'",
            [],
        )?;
        Ok(())
    })
    .unwrap();

    let body = body_for(&db, &headers_with(None)).await;
    assert_eq!(body["upload_bytes"], 900);
    assert_eq!(body["produced_bytes"], 4_100);
}

/// N2, resolved: serve the path — but only when the CLI actually wrote the
/// file. A `cargo run` daemon has no `daemon.log`, and `null` says so.
#[tokio::test]
async fn names_the_daemon_log_only_once_the_file_exists() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join(".openalpaca");
    let _guard = HomeStoreGuard::set(&home);
    let db = test_db(&tmp);

    let body = body_for(&db, &headers_with(None)).await;
    assert!(
        body["log_path"].is_null(),
        "no CLI-written log means no path: {body}"
    );

    let log = openalpaca_storage::store::daemon_log_path().unwrap();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, b"boot\n").unwrap();

    let body = body_for(&db, &headers_with(None)).await;
    assert_eq!(body["log_path"], log.to_string_lossy().as_ref());
}

/// Important #3 (T44 fix round 1): the file existing is not enough — a
/// daemon this run did not launch (the GUI sidecar, a bare `cargo run`) must
/// not claim an earlier CLI daemon's leftover `daemon.log`, even though it is
/// sitting at the exact path this daemon would also write to.
#[tokio::test]
async fn an_unmanaged_daemon_reports_null_even_when_a_log_file_exists() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join(".openalpaca");
    let _guard = HomeStoreGuard::set(&home);
    let db = test_db(&tmp);

    let log = openalpaca_storage::store::daemon_log_path().unwrap();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    std::fs::write(&log, b"a previous CLI daemon's run\n").unwrap();

    let mut unmanaged = inputs(&db, Utc::now());
    unmanaged.managed_log = false;
    let body = body_of(status_response(&unmanaged, &headers_with(None))).await;
    assert!(
        body["log_path"].is_null(),
        "this run never opened the file, so it is not this run's to report: {body}"
    );
}

/// The session-log extras 7b left for this route, read from the service that
/// owns them — and present, with `last_sweep: null`, on a daemon that has no
/// session log at all.
#[tokio::test]
async fn reports_the_boot_sweep_and_the_records_the_writers_dropped() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let body = body_for(&db, &headers_with(None)).await;
    assert!(
        body["sessions"].is_object(),
        "the object is always present: {body}"
    );
    assert!(body["sessions"]["last_sweep"].is_null());
    assert_eq!(body["sessions"]["dropped_records"], 0);

    let service = SessionLogService::new(
        tmp.path().join("sessions"),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    )
    .with_last_sweep(SweepReport {
        sessions_visited: 12,
        sessions_evicted: 3,
        files_removed: 7,
        bytes_freed: 4_096,
        bytes_before: 9_000,
        bytes_after: 4_904,
        over_cap_after: true,
        index_rows_cleared: 2,
    });

    let mut with_log = inputs(&db, Utc::now());
    with_log.session_log = Some(&service);
    let body = body_of(status_response(&with_log, &headers_with(None))).await;

    let sweep = &body["sessions"]["last_sweep"];
    assert_eq!(sweep["sessions_visited"], 12);
    assert_eq!(sweep["sessions_evicted"], 3);
    assert_eq!(sweep["files_removed"], 7);
    assert_eq!(sweep["bytes_freed"], 4_096);
    assert_eq!(sweep["bytes_before"], 9_000);
    assert_eq!(sweep["bytes_after"], 4_904);
    assert_eq!(sweep["index_rows_cleared"], 2);
    assert_eq!(
        sweep["over_cap_after"], true,
        "a root still over its cap has to be able to say so: {body}"
    );
    assert_eq!(body["sessions"]["dropped_records"], 0);
}

/// Important #1 (T44 fix round 1): the brief's `retention` block (§1, P-27),
/// read from `orchestrator.sessions` — the values the boot sweep and the
/// writer's per-session trim actually enforce, not a hardcoded copy.
#[tokio::test]
async fn serves_the_retention_limits_the_daemon_actually_enforces() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let mut with_config = inputs(&db, Utc::now());
    with_config.sessions_config = SessionsConfig {
        log_max_session_bytes: 111,
        log_max_total_bytes: 222,
        log_retention_days: 3,
        tool_result_inline_bytes: 4_096,
        snapshot_max_bytes: 8_192,
    };

    let body = body_of(status_response(&with_config, &headers_with(None))).await;
    assert_eq!(body["retention"]["log_max_session_bytes"], 111);
    assert_eq!(body["retention"]["log_max_total_bytes"], 222);
    assert_eq!(body["retention"]["log_retention_days"], 3);
    // Only the three fields the brief names — `tool_result_inline_bytes`
    // governs tool-result spill and `snapshot_max_bytes` the pre-edit image,
    // neither of which is retention, and neither is part of this block.
    assert!(body["retention"].get("tool_result_inline_bytes").is_none());
    assert!(body["retention"].get("snapshot_max_bytes").is_none());
}

/// §5.6c — a client cannot decide whether to offer `Resume` on an interrupted
/// run without knowing whether this daemon would honour it, and the flag lives
/// in `daemon.toml`. So the daemon says, and the GUI hides the control rather
/// than offering one that answers `409 RESUME_DISABLED`.
#[tokio::test]
async fn serves_whether_replay_resume_is_enabled() {
    let tmp = TempDir::new().unwrap();
    let _guard = HomeStoreGuard::set(&tmp.path().join(".openalpaca"));
    let db = test_db(&tmp);

    let body = body_of(status_response(&inputs(&db, Utc::now()), &headers_with(None))).await;
    assert_eq!(
        body["routing"]["resume_enabled"], false,
        "S2 is opt-in, so the default answer is no: {body}"
    );

    let mut on = inputs(&db, Utc::now());
    on.routing_config = RoutingConfig {
        resume_enabled: true,
        ..RoutingConfig::default()
    };
    let body = body_of(status_response(&on, &headers_with(None))).await;
    assert_eq!(body["routing"]["resume_enabled"], true);
}

fn asset(id: &str, size_bytes: i64) -> FileAsset {
    FileAsset {
        id: id.to_string(),
        owner_id: "owner-1".to_string(),
        sha256: format!("sha-{id}"),
        filename: format!("{id}.md"),
        mime_type: "text/markdown".to_string(),
        size_bytes,
        storage_path: format!("/tmp/{id}.md"),
        status: FileAssetStatus::Ready,
        extracted_text: None,
        extract_error: None,
        metadata_json: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}
