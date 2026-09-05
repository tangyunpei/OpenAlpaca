//! The Phase 3 verification cell for `/v1/artifacts*` and the two content
//! routes GAP-11 moves off the bearer middleware.
//!
//! These are route tests, not store tests: the write protocol, the version
//! rotation and the `missing_since` bookkeeping all belong to `ArtifactStore`
//! and are proved in `openalpaca_storage::artifacts::tests`. What is proved
//! here is what the routes own — which refusal becomes which status code, what
//! the JSON carries, and that the inline `?token=`/`Authorization` check
//! accepts exactly the two forms it is meant to.

use super::*;

use axum::body::to_bytes;
use axum::http::header::AUTHORIZATION;
use chrono::Utc;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{
    ArtifactRecord, ArtifactStore, Database, NewArtifact, NewUpload, UploadStore,
};
use tempfile::TempDir;

use crate::test_util::HomeStoreGuard;

const OWNER: &str = "owner-1";
const OTHER: &str = "owner-2";
const TOKEN: &str = "s3cret-token";

// ============================================================================
// Harness
// ============================================================================

/// A temp home root, a temp project root and a temp database — nothing here
/// ever touches a real store.
struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    project: TempDir,
    db: Database,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home root");
        let env = HomeStoreGuard::set(&home.path().canonicalize().expect("canonicalize home"));
        let project = tempfile::tempdir().expect("project root");
        let db_dir = tempfile::tempdir().expect("db dir");
        let db = Database::open(&db_dir.path().join("test.db")).expect("open db");
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            project,
            db,
        }
    }

    fn scope(&self) -> StoreScope {
        StoreScope::Project(self.project.path().canonicalize().expect("canonicalize"))
    }

    /// A `task` row, so `file_assets.task_id`'s foreign key is satisfiable.
    fn task(&self, id: &str, title: &str) {
        self.db
            .with_connection(|conn| {
                conn.execute(
                    "INSERT INTO task (id, title, created_by, source_lane)
                     VALUES (?1, ?2, 'test', 'test')",
                    [id, title],
                )?;
                Ok(())
            })
            .expect("insert task");
    }

    /// One produced artifact, owned by `owner`.
    fn put(&self, owner: &str, title: &str, body: &str, task_id: Option<&str>) -> ArtifactRecord {
        let scope = self.scope();
        let mut new = NewArtifact::new(owner, &scope, ArtifactKind::Markdown, title, body.as_bytes());
        new.task_id = task_id;
        new.task_title = task_id.map(|_| "A run");
        new.created = Utc::now();
        ArtifactStore::new(&self.db).put(new).expect("put").0
    }

    /// Supersede an existing artifact — same title, same kind, new bytes.
    fn put_again(&self, owner: &str, title: &str, body: &str, note: Option<&str>) -> ArtifactRecord {
        let scope = self.scope();
        let mut new = NewArtifact::new(owner, &scope, ArtifactKind::Markdown, title, body.as_bytes());
        new.note = note;
        ArtifactStore::new(&self.db).put(new).expect("put").0
    }

    /// One upload, written by the *other* writer — `UploadStore` — so its
    /// `kind` is whatever R25's projection made of the MIME type the client
    /// sent, not something an agent declared.
    fn upload(&self, owner: &str, filename: &str, mime: &str) -> String {
        let scope = self.scope();
        UploadStore::new(&self.db)
            .put(NewUpload {
                owner_id: owner,
                filename,
                mime_type: mime,
                data: filename.as_bytes(),
                scope: &scope,
                created: Utc::now(),
            })
            .expect("put upload")
            .asset
            .id
    }

    /// Rows inserted straight into `file_assets`, bypassing both writers: the
    /// shape of a row that predates migration 036 — `kind` NULL, no `rel_path`
    /// — and the cheapest way to fill a page. Returns the ids in insert order.
    fn null_kind_rows(&self, owner: &str, mime: &str, count: usize) -> Vec<String> {
        let ids: Vec<String> = (0..count).map(|n| format!("raw-{n:05}")).collect();
        self.db
            .with_connection(|conn| {
                let tx = conn.unchecked_transaction()?;
                for (n, id) in ids.iter().enumerate() {
                    let filename = format!("raw-{n}.dat");
                    let path = format!("/nonexistent/{filename}");
                    tx.execute(
                        "INSERT INTO file_assets
                            (id, owner_id, sha256, filename, mime_type, size_bytes,
                             storage_path, status)
                         VALUES (?1, ?2, 'sha', ?3, ?4, 0, ?5, 'uploaded')",
                        [id.as_str(), owner, &filename, mime, &path],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .expect("insert rows with no kind");
        ids
    }

    fn record(&self, id: &str, owner: &str) -> ArtifactRecord {
        ArtifactStore::new(&self.db)
            .get(id, owner)
            .expect("get")
            .expect("record present")
    }
}

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().expect("header value"),
    );
    headers
}

fn no_headers() -> HeaderMap {
    HeaderMap::new()
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

/// Split a `Response` into its status, its `Referrer-Policy` and its raw bytes.
async fn split_bytes(response: Response) -> (StatusCode, Option<String>, Vec<u8>) {
    let status = response.status();
    let referrer = response
        .headers()
        .get("referrer-policy")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let bytes = to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("read the response body");
    (status, referrer, bytes.to_vec())
}

fn error_code(body: &serde_json::Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("<no code>")
}

// ============================================================================
// GAP-11 — the inline check on the two content routes
// ============================================================================

#[tokio::test]
async fn a_query_token_is_accepted_on_the_artifact_content_route() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "hello\n", None);

    let (status, referrer, bytes) = split_bytes(
        artifact_content(
            &f.db,
            OWNER,
            TOKEN,
            &no_headers(),
            Some(TOKEN),
            &row.id,
            None,
        )
        .await,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"hello\n");
    // §9's mitigation for a bearer token that can now ride in a URL.
    assert_eq!(referrer.as_deref(), Some("no-referrer"));
}

#[tokio::test]
async fn a_bearer_header_is_accepted_on_the_artifact_content_route() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "hello\n", None);

    let (status, _, bytes) = split_bytes(
        artifact_content(&f.db, OWNER, TOKEN, &bearer(TOKEN), None, &row.id, None).await,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "the GUI's `apiFetchBlob` still works");
    assert_eq!(bytes, b"hello\n");
}

#[tokio::test]
async fn a_wrong_or_missing_token_is_a_401_on_the_artifact_content_route() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "hello\n", None);

    for (headers, query) in [
        (no_headers(), None),
        (no_headers(), Some("wrong")),
        (bearer("wrong"), None),
    ] {
        let response =
            artifact_content(&f.db, OWNER, TOKEN, &headers, query, &row.id, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        // The same plain-text body `/v1/chat/stream` answers with.
        assert_eq!(&bytes[..], b"Invalid token");
    }
}

#[tokio::test]
async fn both_token_forms_are_accepted_on_the_file_content_route() {
    let f = Fixture::new();
    // A produced row is still a `file_assets` row, so `/v1/files/{id}/content`
    // serves it — the route reads the table, not the origin.
    let row = f.put(OWNER, "Notes", "file bytes\n", None);

    for (headers, query) in [(no_headers(), Some(TOKEN)), (bearer(TOKEN), None)] {
        let (status, referrer, bytes) = split_bytes(
            crate::routes::files::file_content(&f.db, OWNER, TOKEN, &headers, query, &row.id).await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bytes, b"file bytes\n");
        assert_eq!(referrer.as_deref(), Some("no-referrer"));
    }
}

#[tokio::test]
async fn a_wrong_token_is_a_401_on_the_file_content_route() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "file bytes\n", None);

    let response =
        crate::routes::files::file_content(&f.db, OWNER, TOKEN, &no_headers(), None, &row.id).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// Owner scoping — 404, never 403
// ============================================================================

#[tokio::test]
async fn another_owners_artifact_is_a_404_on_every_route() {
    let f = Fixture::new();
    let row = f.put(OTHER, "Theirs", "not yours\n", None);

    let (status, body) = split(get_artifact(&f.db, OWNER, &row.id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "ARTIFACT_NOT_FOUND");

    let (status, _) = split(list_artifact_versions(&f.db, OWNER, &row.id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = split(artifact_diff(&f.db, OWNER, &row.id, 1, 2)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = split(pin_artifact(&f.db, OWNER, &row.id, true)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Authenticated, but not authorised — and the answer is still 404.
    let (status, body) = split(
        artifact_content(
            &f.db,
            OWNER,
            TOKEN,
            &bearer(TOKEN),
            None,
            &row.id,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "ARTIFACT_NOT_FOUND");

    // …and their row is not in this owner's list.
    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert_eq!(body["total"], 0);
    assert_eq!(body["artifacts"].as_array().unwrap().len(), 0);
}

// ============================================================================
// 410 — the bytes are gone
// ============================================================================

#[tokio::test]
async fn deleted_bytes_are_a_410_and_stamp_missing_since() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "hello\n", None);
    assert!(f.record(&row.id, OWNER).missing_since.is_none());

    std::fs::remove_file(&row.storage_path).expect("delete the head file");

    let (status, body) = split(
        artifact_content(&f.db, OWNER, TOKEN, &bearer(TOKEN), None, &row.id, None).await,
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(error_code(&body), "ARTIFACT_GONE");

    assert!(
        f.record(&row.id, OWNER).missing_since.is_some(),
        "the 410 must record that the head is gone"
    );

    // And the row leaves the default list, which hides missing rows (§4.8).
    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert_eq!(body["total"], 0);

    let params = ListArtifactsParams {
        include_missing: true,
        ..Default::default()
    };
    let (_, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["artifacts"][0]["missing"], true);
}

// ============================================================================
// The list
// ============================================================================

#[tokio::test]
async fn the_list_filters_by_task_id() {
    let f = Fixture::new();
    f.task("task-a", "Run A");
    f.task("task-b", "Run B");
    f.put(OWNER, "From A", "a\n", Some("task-a"));
    f.put(OWNER, "From B", "b\n", Some("task-b"));
    f.put(OWNER, "Loose", "c\n", None);

    let params = ListArtifactsParams {
        task_id: Some("task-a".to_string()),
        ..Default::default()
    };
    let (status, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    let rows = body["artifacts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["task_id"], "task-a");
    // §4.9's `task_title`, joined from `task`.
    assert_eq!(rows[0]["task_title"], "Run A");

    // The unfiltered list is the whole page plus its unpaged total.
    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert_eq!(body["total"], 3);
    assert_eq!(body["artifacts"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_loose_artifacts_task_title_is_null() {
    let f = Fixture::new();
    f.put(OWNER, "Loose", "c\n", None);

    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert!(body["artifacts"][0]["task_id"].is_null());
    assert!(body["artifacts"][0]["task_title"].is_null());
}

#[tokio::test]
async fn an_unparseable_kind_or_origin_is_a_400() {
    let f = Fixture::new();

    let params = ListArtifactsParams {
        kind: Some("diff".to_string()),
        ..Default::default()
    };
    let (status, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_KIND");

    let params = ListArtifactsParams {
        origin: Some("generated".to_string()),
        ..Default::default()
    };
    let (status, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_ORIGIN");
}

#[tokio::test]
async fn the_limit_and_offset_page_the_list() {
    let f = Fixture::new();
    for n in 0..3 {
        f.put(OWNER, &format!("Note {n}"), "x\n", None);
    }

    let params = ListArtifactsParams {
        limit: Some(2),
        ..Default::default()
    };
    let (_, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(body["total"], 3, "the total is unpaged");
    assert_eq!(body["artifacts"].as_array().unwrap().len(), 2);

    let params = ListArtifactsParams {
        limit: Some(2),
        offset: Some(2),
        ..Default::default()
    };
    let (_, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(body["artifacts"].as_array().unwrap().len(), 1);
}

/// R26: the titles for a whole page come from one `titles_for` lookup, not one
/// `TaskRepository::get` per row — and the answer is the same one the per-row
/// join gave: every run's own title, `null` for a loose artifact and for a run
/// whose task row is gone.
#[tokio::test]
async fn a_page_spanning_three_tasks_carries_every_title() {
    let f = Fixture::new();
    for (id, title) in [("task-a", "Run A"), ("task-b", "Run B"), ("task-c", "Run C")] {
        f.task(id, title);
    }
    f.put(OWNER, "From A", "a\n", Some("task-a"));
    f.put(OWNER, "From B", "b\n", Some("task-b"));
    f.put(OWNER, "Also B", "b2\n", Some("task-b"));
    f.put(OWNER, "From C", "c\n", Some("task-c"));
    f.put(OWNER, "Loose", "d\n", None);

    let (status, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert_eq!(status, StatusCode::OK);

    let rows = body["artifacts"].as_array().expect("an artifacts array");
    assert_eq!(rows.len(), 5);
    for row in rows {
        let expected = match row["task_id"].as_str() {
            Some("task-a") => Some("Run A"),
            Some("task-b") => Some("Run B"),
            Some("task-c") => Some("Run C"),
            Some(other) => panic!("unexpected task id {other}"),
            None => None,
        };
        assert_eq!(
            row["task_title"].as_str(),
            expected,
            "row {} carries its own run's title",
            row["name"]
        );
    }
}

// ============================================================================
// Bounds on the page (R26)
// ============================================================================

#[test]
fn the_requested_limit_is_clamped_not_refused() {
    assert_eq!(page_limit(None), None, "the store's default page");
    assert_eq!(page_limit(Some(0)), None, "never SQLite's 'no limit'");
    assert_eq!(page_limit(Some(-1)), None);
    assert_eq!(page_limit(Some(2)), Some(2));
    assert_eq!(page_limit(Some(MAX_LIST_LIMIT)), Some(MAX_LIST_LIMIT));
    assert_eq!(page_limit(Some(100_000)), Some(MAX_LIST_LIMIT));
}

/// The clamp end to end: a caller asking for a hundred thousand rows gets
/// `MAX_LIST_LIMIT` of them, and `total` still says exactly how many exist.
#[tokio::test]
async fn a_huge_limit_returns_at_most_max_list_limit_rows() {
    let f = Fixture::new();
    let count = (MAX_LIST_LIMIT + 1) as usize;
    f.null_kind_rows(OWNER, "text/plain", count);

    let params = ListArtifactsParams {
        limit: Some(100_000),
        ..Default::default()
    };
    let (status, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["artifacts"].as_array().unwrap().len() as i64,
        MAX_LIST_LIMIT
    );
    assert_eq!(body["total"], count, "the total stays exact and unpaged");
}

#[tokio::test]
async fn a_negative_offset_is_a_400() {
    let f = Fixture::new();
    f.put(OWNER, "Notes", "hello\n", None);

    let params = ListArtifactsParams {
        offset: Some(-1),
        ..Default::default()
    };
    let (status, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "INVALID_OFFSET");
}

// ============================================================================
// The row's shape (§4.9: a superset of the client's `Artifact`)
// ============================================================================

#[tokio::test]
async fn the_row_is_a_superset_of_the_client_artifact_type() {
    let f = Fixture::new();
    f.task("task-a", "Run A");
    let row = f.put(OWNER, "Notes", "hello\n", Some("task-a"));

    let (status, body) = split(get_artifact(&f.db, OWNER, &row.id)).await;
    assert_eq!(status, StatusCode::OK);

    // `unbacked.ts:39-56`, field for field.
    assert_eq!(body["id"], row.id);
    assert_eq!(body["name"], row.name);
    assert_eq!(body["kind"], "markdown");
    assert_eq!(body["mime_type"], "text/markdown");
    assert_eq!(body["size_bytes"], 6);
    assert_eq!(body["task_id"], "task-a");
    assert_eq!(body["task_title"], "Run A");
    assert!(body["agent_id"].is_null());
    assert!(body["agent_template_id"].is_null());
    assert_eq!(body["version"], 1);
    assert_eq!(body["version_count"], 1);
    assert!(body["summary"].is_null());
    assert!(body["metadata"].is_null(), "no metadata_json is a JSON null");
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());

    // The additive fields §4.9 names.
    assert_eq!(body["origin"], "produced");
    assert_eq!(body["pinned"], false);
    assert_eq!(body["missing"], false);
    assert_eq!(body["path"], row.storage_path);
    assert_eq!(
        body["project_root"],
        f.project.path().canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(body["rel_path"], row.rel_path.clone().unwrap());
}

/// R25: an upload is one of the two origins `/v1/artifacts` lists by default,
/// and its `kind` comes from the MIME type the writer classified it by — not
/// the `null` the client's `Artifact` type forbids.
#[tokio::test]
async fn an_uploads_row_carries_the_kind_its_mime_projects_to() {
    let f = Fixture::new();
    let id = f.upload(OWNER, "rows.csv", "text/csv");

    let (status, body) = split(get_artifact(&f.db, OWNER, &id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["origin"], "upload");
    assert_eq!(body["kind"], "table");

    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert_eq!(body["artifacts"][0]["kind"], "table");
}

/// R25's read half: a row whose stored `kind` is NULL — one that predates
/// migration 036, or an upload written before the writer classified them — is
/// projected from its `mime_type` at the route, so no page can hand the client
/// a `kind` of `null`.
#[tokio::test]
async fn a_row_with_no_stored_kind_still_serialises_one_from_its_mime() {
    let f = Fixture::new();
    let id = f.null_kind_rows(OWNER, "text/html", 1).remove(0);
    assert!(
        f.record(&id, OWNER).kind.is_none(),
        "the column really is NULL — the projection is the route's, not the store's"
    );

    let (status, body) = split(get_artifact(&f.db, OWNER, &id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kind"], "html");

    let (_, body) = split(list_artifacts(&f.db, OWNER, ListArtifactsParams::default())).await;
    assert!(
        !body["artifacts"][0]["kind"].is_null(),
        "`kind` is never null on the wire"
    );
    assert_eq!(body["artifacts"][0]["kind"], "html");
}

#[tokio::test]
async fn metadata_is_parsed_json_not_a_string() {
    let f = Fixture::new();
    let scope = f.scope();
    let mut new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Markdown,
        "Notes",
        b"hello\n" as &[u8],
    );
    new.metadata_json = Some(r#"{"language":"rust"}"#);
    let row = ArtifactStore::new(&f.db).put(new).expect("put").0;

    let (_, body) = split(get_artifact(&f.db, OWNER, &row.id)).await;
    assert_eq!(body["metadata"]["language"], "rust");
}

#[tokio::test]
async fn an_unknown_artifact_is_a_404() {
    let f = Fixture::new();
    let (status, body) = split(get_artifact(&f.db, OWNER, "no-such-artifact")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "ARTIFACT_NOT_FOUND");
}

// ============================================================================
// Versions
// ============================================================================

#[tokio::test]
async fn versions_are_newest_first_and_coalesce_a_null_note() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "one\n", None);
    f.put_again(OWNER, "Notes", "one\ntwo\n", Some("added a line"));

    let (status, body) = split(list_artifact_versions(&f.db, OWNER, &row.id)).await;
    assert_eq!(status, StatusCode::OK);
    let versions = body["versions"].as_array().expect("a versions array");
    assert_eq!(versions.len(), 2);

    assert_eq!(versions[0]["version"], 2);
    assert_eq!(versions[0]["note"], "added a line");
    assert_eq!(versions[0]["added_lines"], 1);
    assert_eq!(versions[0]["removed_lines"], 0);
    assert!(versions[0]["author_agent_id"].is_null());
    assert!(versions[0]["created_at"].is_string());
    assert_eq!(versions[0]["size_bytes"], 8);

    // v1 had no note: the route coalesces `NULL` to `""`, never to `null`.
    assert_eq!(versions[1]["version"], 1);
    assert_eq!(versions[1]["note"], "");
    assert!(versions[1]["added_lines"].is_null());
}

/// `?version=N` and `/versions/{n}/content` are the *same* body — the two
/// handlers differ only in where the number came from — so one call proves
/// both, and the head is what `version: None` resolves to.
#[tokio::test]
async fn a_version_selects_the_superseded_bytes() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "one\n", None);
    f.put_again(OWNER, "Notes", "one\ntwo\n", None);

    let (_, _, head) =
        split_bytes(artifact_content(&f.db, OWNER, TOKEN, &bearer(TOKEN), None, &row.id, None).await)
            .await;
    assert_eq!(head, b"one\ntwo\n");

    let (status, referrer, bytes) = split_bytes(
        artifact_content(&f.db, OWNER, TOKEN, &bearer(TOKEN), None, &row.id, Some(1)).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"one\n");
    assert_eq!(referrer.as_deref(), Some("no-referrer"));
}

#[tokio::test]
async fn an_unknown_version_is_a_404() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "one\n", None);

    let (status, body) = split(
        artifact_content(&f.db, OWNER, TOKEN, &bearer(TOKEN), None, &row.id, Some(9)).await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "ARTIFACT_VERSION_NOT_FOUND");
}

// ============================================================================
// Diff — 409 until T29 lands the patch
// ============================================================================

#[tokio::test]
async fn a_text_diff_is_a_409_until_the_patch_lands() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "one\n", None);
    f.put_again(OWNER, "Notes", "one\ntwo\n", None);

    let (status, body) = split(artifact_diff(&f.db, OWNER, &row.id, 1, 2)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "DIFF_UNAVAILABLE");
}

#[tokio::test]
async fn a_binary_diff_is_a_409_not_diffable() {
    let f = Fixture::new();
    let scope = f.scope();
    let new = NewArtifact::new(
        OWNER,
        &scope,
        ArtifactKind::Binary,
        "Blob",
        &[0u8, 1, 2] as &[u8],
    );
    let row = ArtifactStore::new(&f.db).put(new).expect("put").0;

    let (status, body) = split(artifact_diff(&f.db, OWNER, &row.id, 1, 1)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "NOT_DIFFABLE");
}

// ============================================================================
// Pin (R23 — the route lands here, T30 wires the GUI)
// ============================================================================

#[tokio::test]
async fn the_pin_round_trips() {
    let f = Fixture::new();
    let row = f.put(OWNER, "Notes", "hello\n", None);

    let (status, body) = split(pin_artifact(&f.db, OWNER, &row.id, true)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], row.id);
    assert_eq!(body["pinned"], true);
    assert!(f.record(&row.id, OWNER).pinned);

    let (_, body) = split(get_artifact(&f.db, OWNER, &row.id)).await;
    assert_eq!(body["pinned"], true);

    let (status, body) = split(pin_artifact(&f.db, OWNER, &row.id, false)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["pinned"], false);
    assert!(!f.record(&row.id, OWNER).pinned);

    // …and `?pinned=` selects on it.
    let params = ListArtifactsParams {
        pinned: Some(true),
        ..Default::default()
    };
    let (_, body) = split(list_artifacts(&f.db, OWNER, params)).await;
    assert_eq!(body["total"], 0);
}
