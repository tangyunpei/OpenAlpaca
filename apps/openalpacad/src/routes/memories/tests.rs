//! `/v1/memories*` — what the routes own (design §7a; plan 3.8): which
//! refusal becomes which status and code, that the revision travels in
//! `If-Match` and nowhere else, the cursor rule, recently-learned's edit
//! rule, undo's origin rule, and that every write is one `memory_changed`.
//! Driven over [`MemoryApi`] directly: an `AppState` is not constructible in
//! a unit test, and each handler is one field read over these functions.

use super::*;

use axum::body::to_bytes;
use openalpaca_core::bus::EventBus;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::memory::service::ExtractionProvenance;
use openalpaca_storage::IdentityRepository;
use openalpaca_storage::memory_index::MemoryIndex;
use openalpaca_storage::{Database, store};
use tempfile::TempDir;

use crate::test_util::HomeStoreGuard;

const OWNER: &str = "owner-1";

struct Fixture {
    _home: TempDir,
    _env: HomeStoreGuard,
    _db_dir: TempDir,
    home_store: std::path::PathBuf,
    api: MemoryApi,
    bus: EventBus,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let home_store = home
            .path()
            .canonicalize()
            .unwrap()
            .join(store::STORE_DIR_NAME);
        std::fs::create_dir_all(&home_store).unwrap();
        let env = HomeStoreGuard::set(&home_store);
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        IdentityRepository::new(&db)
            .create_global_user(OWNER, None)
            .unwrap();
        let bus = EventBus::new(64);
        let service = MemoryService::new(Some(db), MemoryIndex::open_in_memory().unwrap())
            .with_bus(bus.clone());
        service.rebuild_index().unwrap();
        Self {
            _home: home,
            _env: env,
            _db_dir: db_dir,
            home_store,
            api: MemoryApi {
                service: Arc::new(service),
                owner_id: OWNER.to_string(),
            },
            bus,
        }
    }

    fn headers() -> HeaderMap {
        HeaderMap::new()
    }

    fn with_if_match(revision: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MATCH, format!("\"{revision}\"").parse().unwrap());
        headers
    }

    async fn create(&self, title: &str, body: &str, kind: &str) -> (NoteId, String) {
        let response = self
            .api
            .create(&Self::headers(), create_body(title, body, kind))
            .await;
        let (status, json) = split(response).await;
        assert_eq!(status, StatusCode::CREATED, "{json}");
        (
            NoteId::parse(json["id"].as_str().unwrap()).unwrap(),
            json["revision"].as_str().unwrap().to_string(),
        )
    }

    /// What the background extractor writes for the user's own words: an
    /// active `extracted_user` note — the recently-learned list's content.
    fn extracted(&self, title: &str, body: &str) -> (NoteId, String) {
        let ctx = self
            .api
            .service
            .context(
                OWNER,
                WorkspaceHint::None,
                Surface::Extraction,
                Capabilities::EXTRACTION,
                Attribution::default(),
            )
            .unwrap();
        let out = self
            .api
            .service
            .store(
                &ctx,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(NoteKind::Preference),
                    provenance: Some(ExtractionProvenance::UserWords),
                    ..Default::default()
                },
            )
            .unwrap();
        (out.id, out.revision)
    }

    /// What the extractor writes for assistant or tool output: an
    /// `extracted_other` candidate, outside recall until confirmed.
    fn candidate(&self, title: &str, body: &str) -> (NoteId, String) {
        let ctx = self
            .api
            .service
            .context(
                OWNER,
                WorkspaceHint::None,
                Surface::Extraction,
                Capabilities::EXTRACTION,
                Attribution::default(),
            )
            .unwrap();
        let out = self
            .api
            .service
            .store(
                &ctx,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(NoteKind::Fact),
                    provenance: Some(ExtractionProvenance::OtherOutput),
                    ..Default::default()
                },
            )
            .unwrap();
        (out.id, out.revision)
    }

    fn note_file(&self, id: NoteId) -> std::path::PathBuf {
        let owner_key = openalpaca_storage::memory_files::owner_key(OWNER);
        let notes = self
            .home_store
            .join("memory")
            .join("owners")
            .join(owner_key)
            .join("notes");
        std::fs::read_dir(notes)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&id.to_string())
            })
            .expect("the note file exists")
    }
}

fn create_body(title: &str, body: &str, kind: &str) -> CreateMemoryBody {
    CreateMemoryBody {
        title: title.to_string(),
        body: body.to_string(),
        kind: kind.to_string(),
        scope: None,
        aliases: Vec::new(),
        tags: Vec::new(),
        fact_key: None,
        sources: Vec::new(),
        valid_from: None,
        valid_to: None,
        supersedes: None,
        expected_revision: None,
    }
}

fn update_body(action: &str, body: Option<&str>) -> UpdateMemoryBody {
    UpdateMemoryBody {
        action: action.to_string(),
        patch: body.map(|b| PatchBody {
            body: Some(b.to_string()),
            ..PatchBody::default()
        }),
        reason: None,
        expected_revision: None,
    }
}

async fn split(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

fn etag_of(response: &Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .map(|v| v.to_str().unwrap().trim_matches('"').to_string())
}

fn code(json: &serde_json::Value) -> &str {
    json["error"]["code"].as_str().unwrap_or("")
}

// ── If-Match: the one place a revision travels ─────────────────────────

/// Every single-note mutation without `If-Match` is `428`, and nothing is
/// written.
#[tokio::test]
async fn a_single_note_mutation_without_if_match_is_428() {
    let f = Fixture::new();
    let (id, revision) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let sid = id.to_string();

    let (status, json) = split(
        f.api
            .update(&Fixture::headers(), &sid, update_body("archive", None))
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{json}");
    assert_eq!(code(&json), "MEMORY_IF_MATCH_REQUIRED");
    let (status, json) = split(f.api.undo(&Fixture::headers(), &sid).await).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{json}");
    assert_eq!(code(&json), "MEMORY_IF_MATCH_REQUIRED");
    let (status, json) = split(f.api.delete(&Fixture::headers(), &sid).await).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{json}");
    assert_eq!(code(&json), "MEMORY_IF_MATCH_REQUIRED");

    let (status, json) = split(
        f.api
            .read(&Fixture::headers(), &sid, ReadMemoryQuery::default())
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["revision"], revision, "nothing was written");
    assert_eq!(json["status"], "active");
}

/// A stale `If-Match` is `409 MEMORY_REVISION_CONFLICT`: the message names
/// the current revision, the response carries it as `ETag`, and the file is
/// unchanged. `If-Match: *` names no revision and is refused.
#[tokio::test]
async fn a_stale_if_match_is_409_carrying_the_current_revision_as_etag() {
    let f = Fixture::new();
    let (id, revision) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let sid = id.to_string();
    let stale = "0".repeat(64);

    let response = f
        .api
        .update(
            &Fixture::with_if_match(&stale),
            &sid,
            update_body("edit", Some("# Editor\n\nneovim.\n")),
        )
        .await;
    assert_eq!(etag_of(&response).as_deref(), Some(revision.as_str()));
    let (status, json) = split(response).await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(code(&json), "MEMORY_REVISION_CONFLICT");
    // The message is the service's own sentence, current revision included
    // — the daemon cannot know what a client kept, so it says nothing
    // about it; each client words its own conflict copy.
    assert_eq!(
        json["error"]["message"],
        MemoryError::RevisionConflict {
            id,
            current_revision: revision.clone(),
        }
        .to_string(),
        "{json}"
    );

    let response = f.api.delete(&Fixture::with_if_match(&stale), &sid).await;
    assert_eq!(etag_of(&response).as_deref(), Some(revision.as_str()));
    let (status, json) = split(response).await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(code(&json), "MEMORY_REVISION_CONFLICT");

    let (status, json) = split(
        f.api
            .update(
                &Fixture::with_if_match("*"),
                &sid,
                update_body("archive", None),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");

    let (_, json) = split(
        f.api
            .read(&Fixture::headers(), &sid, ReadMemoryQuery::default())
            .await,
    )
    .await;
    assert_eq!(json["body"], "# Editor\n\nvim.\n");
    assert_eq!(json["status"], "active");
}

/// A body `expected_revision` is `400 MEMORY_INVALID_INPUT` on an update
/// even beside a correct `If-Match`, and on a create — the header is the
/// one place, so two revisions can never disagree silently.
#[tokio::test]
async fn a_body_expected_revision_is_400() {
    let f = Fixture::new();
    let (id, revision) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let sid = id.to_string();

    let mut body = update_body("archive", None);
    body.expected_revision = Some(serde_json::Value::String(revision.clone()));
    let (status, json) = split(
        f.api
            .update(&Fixture::with_if_match(&revision), &sid, body)
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("If-Match"),
        "{json}"
    );

    let mut body = create_body("Shell", "# Shell\n\nzsh.\n", "preference");
    body.expected_revision = Some(serde_json::Value::String(revision.clone()));
    let (status, json) = split(f.api.create(&Fixture::headers(), body).await).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");

    // The archive with the header alone goes through, and the create too.
    let (status, json) = split(
        f.api
            .update(
                &Fixture::with_if_match(&revision),
                &sid,
                update_body("archive", None),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "archived");
    let (status, json) = split(
        f.api
            .list(
                &Fixture::headers(),
                ListMemoriesQuery {
                    status: Some("archived".into()),
                    ..Default::default()
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["items"].as_array().unwrap().len(), 1);
}

// ── Creating and reading ───────────────────────────────────────────────

/// A create is `201` with the new revision as `ETag` and origin `user` —
/// the API is a user-directed surface (design §4); an exact duplicate is
/// `200` with the existing id and `dedup: existing`; a body `owner_id` is
/// not a field this API has.
#[tokio::test]
async fn a_create_is_201_with_origin_user_and_an_etag() {
    let f = Fixture::new();
    let response = f
        .api
        .create(
            &Fixture::headers(),
            create_body("Editor", "# Editor\n\nvim.\n", "preference"),
        )
        .await;
    let etag = etag_of(&response).expect("a create answers ETag");
    let (status, json) = split(response).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["revision"], etag);
    assert_eq!(json["origin"], "user");
    assert_eq!(json["scope"], "global");
    assert_eq!(json["dedup"], "created");

    let (status, again) = split(
        f.api
            .create(
                &Fixture::headers(),
                create_body("Editor", "# Editor\n\nvim.\n", "preference"),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["id"], json["id"]);
    assert_eq!(again["dedup"], "existing");

    let (status, json) = split(
        f.api
            .create(
                &Fixture::headers(),
                create_body("Bad", "# Bad\n", "opinion"),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");

    // The closed body shape: a JSON `owner_id` never reaches the service.
    let rejected = serde_json::from_str::<CreateMemoryBody>(
        r#"{"title":"x","body":"y","kind":"fact","owner_id":"someone-else"}"#,
    );
    assert!(rejected.is_err(), "unknown fields are refused");
}

/// `GET /v1/memories/{id}` answers the note with `ETag`; an unknown id is
/// `404 MEMORY_NOT_FOUND`; a value that is not a UUID is `400`.
#[tokio::test]
async fn a_read_carries_its_etag_and_an_unknown_id_is_404() {
    let f = Fixture::new();
    let (id, revision) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let response = f
        .api
        .read(
            &Fixture::headers(),
            &id.to_string(),
            ReadMemoryQuery::default(),
        )
        .await;
    assert_eq!(etag_of(&response).as_deref(), Some(revision.as_str()));
    let (status, json) = split(response).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["title"], "Editor");
    assert_eq!(json["frontmatter"]["kind"], "preference");
    assert_eq!(json["sources_status"], "intact");

    let (status, json) = split(
        f.api
            .read(
                &Fixture::headers(),
                &NoteId::new().to_string(),
                ReadMemoryQuery::default(),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(code(&json), "MEMORY_NOT_FOUND");

    let (status, json) = split(
        f.api
            .read(
                &Fixture::headers(),
                "not-a-uuid",
                ReadMemoryQuery::default(),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");
}

/// `/history` lists the kept revisions newest first, and `?revision=` reads
/// one of them — the old body at the old revision, `ETag` to match.
#[tokio::test]
async fn history_lists_kept_revisions_and_one_reads_back() {
    let f = Fixture::new();
    let (id, first) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let sid = id.to_string();
    let (status, edited) = split(
        f.api
            .update(
                &Fixture::with_if_match(&first),
                &sid,
                update_body("edit", Some("# Editor\n\nneovim.\n")),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let second = edited["revision"].as_str().unwrap().to_string();

    let (status, json) = split(f.api.history(&Fixture::headers(), &sid).await).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let revisions: Vec<&str> = json["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["revision"].as_str().unwrap())
        .collect();
    assert_eq!(revisions, [second.as_str(), first.as_str()]);

    let response = f
        .api
        .read(
            &Fixture::headers(),
            &sid,
            ReadMemoryQuery {
                revision: Some(first.clone()),
                section: None,
            },
        )
        .await;
    assert_eq!(etag_of(&response).as_deref(), Some(first.as_str()));
    let (status, json) = split(response).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["body"], "# Editor\n\nvim.\n");
    assert_eq!(json["freshness"], "verified");

    let (status, json) = split(
        f.api
            .read(
                &Fixture::headers(),
                &sid,
                ReadMemoryQuery {
                    revision: Some("1".repeat(64)),
                    section: None,
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{json}");
    assert_eq!(code(&json), "MEMORY_NOT_FOUND");
}

// ── The cursor rule ────────────────────────────────────────────────────

/// A `next_cursor` is valid for the filter set that issued it: the next
/// page continues where the first stopped, and the same cursor under
/// another filter set is `400 MEMORY_INVALID_INPUT`.
#[tokio::test]
async fn a_cursor_from_another_filter_set_is_refused() {
    let f = Fixture::new();
    for i in 0..3 {
        f.create(&format!("Note {i}"), &format!("# Note {i}\n"), "fact")
            .await;
    }
    let (status, page) = split(
        f.api
            .list(
                &Fixture::headers(),
                ListMemoriesQuery {
                    limit: Some(2),
                    ..Default::default()
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let cursor = page["next_cursor"]
        .as_str()
        .expect("a third item remains")
        .to_string();

    let (status, next) = split(
        f.api
            .list(
                &Fixture::headers(),
                ListMemoriesQuery {
                    limit: Some(2),
                    cursor: Some(cursor.clone()),
                    ..Default::default()
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert!(next["next_cursor"].is_null());

    let (status, json) = split(
        f.api
            .list(
                &Fixture::headers(),
                ListMemoriesQuery {
                    limit: Some(2),
                    cursor: Some(cursor),
                    kind: Some("fact".into()),
                    ..Default::default()
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("different filters"),
        "{json}"
    );
}

// ── Recently learned and undo ──────────────────────────────────────────

/// `recently-learned` lists the extractor's active `extracted_user` notes
/// and drops one the moment the user makes it theirs — an `edit` through
/// this surface re-records it as `user` (design §6; `confirm` is the same
/// act for a candidate, which was never listed: a confirmed candidate is
/// `user` too and stays out).
#[tokio::test]
async fn recently_learned_excludes_a_note_re_recorded_as_user() {
    let f = Fixture::new();
    let (learned, revision) = f.extracted("Learned", "# Learned\n\nFrom what you said.\n");
    let (kept, _) = f.extracted("Kept", "# Kept\n\nStill learned.\n");
    let (candidate, candidate_revision) =
        f.candidate("Inferred", "# Inferred\n\nFrom the assistant.\n");
    let (status, json) = split(
        f.api
            .recently_learned(&Fixture::headers(), RecentlyLearnedQuery::default())
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let ids = |json: &serde_json::Value| -> Vec<String> {
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect()
    };
    let listed = ids(&json);
    assert!(listed.contains(&learned.to_string()) && listed.contains(&kept.to_string()));
    assert!(
        !listed.contains(&candidate.to_string()),
        "a candidate is not recently learned"
    );
    assert!(json["items"][0]["origin"] == "extracted_user");

    let (status, json) = split(
        f.api
            .update(
                &Fixture::with_if_match(&revision),
                &learned.to_string(),
                update_body("edit", Some("# Learned\n\nNow in my words.\n")),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let (status, json) = split(
        f.api
            .update(
                &Fixture::with_if_match(&candidate_revision),
                &candidate.to_string(),
                update_body("confirm", None),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "active");
    let (_, confirmed) = split(
        f.api
            .read(
                &Fixture::headers(),
                &candidate.to_string(),
                ReadMemoryQuery::default(),
            )
            .await,
    )
    .await;
    assert_eq!(confirmed["origin"], "user");

    let (_, json) = split(
        f.api
            .recently_learned(&Fixture::headers(), RecentlyLearnedQuery::default())
            .await,
    )
    .await;
    assert_eq!(ids(&json), vec![kept.to_string()]);

    // A window that excludes everything: nothing learned since one second
    // into the future… expressed as an RFC 3339 instant.
    let future = (Utc::now() + Duration::days(1)).to_rfc3339();
    let (_, json) = split(
        f.api
            .recently_learned(
                &Fixture::headers(),
                RecentlyLearnedQuery {
                    since: Some(future),
                    limit: None,
                },
            )
            .await,
    )
    .await;
    assert!(ids(&json).is_empty());
    let (status, json) = split(
        f.api
            .recently_learned(
                &Fixture::headers(),
                RecentlyLearnedQuery {
                    since: Some("yesterday".into()),
                    limit: None,
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
}

/// `since` takes a window (`7d`, `24h`, `30m`) as well as an instant, and a
/// window too large to subtract from now is `400 MEMORY_INVALID_INPUT` —
/// a query string under the bearer token must never drop the connection.
#[tokio::test]
async fn a_since_window_is_honoured_and_an_oversized_one_is_400() {
    let f = Fixture::new();
    let (learned, _) = f.extracted("Learned", "# Learned\n\nThis week.\n");
    for window in ["7d", "24h", "30m"] {
        let (status, json) = split(
            f.api
                .recently_learned(
                    &Fixture::headers(),
                    RecentlyLearnedQuery {
                        since: Some(window.into()),
                        limit: None,
                    },
                )
                .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "since={window}: {json}");
        assert_eq!(
            json["items"][0]["id"],
            learned.to_string(),
            "since={window}"
        );
    }

    let (status, json) = split(
        f.api
            .recently_learned(
                &Fixture::headers(),
                RecentlyLearnedQuery {
                    since: Some("100000000d".into()),
                    limit: None,
                },
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(code(&json), "MEMORY_INVALID_INPUT");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("too large"),
        "{json}"
    );
}

/// `undo` forgets a recently learned note, and is `409 MEMORY_NOT_UNDOABLE`
/// once its origin moved — the user has since made it theirs, and a plain
/// forget is the verb for that.
#[tokio::test]
async fn undo_is_refused_once_the_origin_moved() {
    let f = Fixture::new();
    let (learned, revision) = f.extracted("Learned", "# Learned\n\nFrom what you said.\n");
    let (mine, mine_revision) = f.create("Mine", "# Mine\n\nSaved by hand.\n", "fact").await;

    let (status, json) = split(
        f.api
            .undo(&Fixture::with_if_match(&mine_revision), &mine.to_string())
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(code(&json), "MEMORY_NOT_UNDOABLE");

    let (status, edited) = split(
        f.api
            .update(
                &Fixture::with_if_match(&revision),
                &learned.to_string(),
                update_body("edit", Some("# Learned\n\nNow mine.\n")),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let (status, json) = split(
        f.api
            .undo(
                &Fixture::with_if_match(edited["revision"].as_str().unwrap()),
                &learned.to_string(),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(code(&json), "MEMORY_NOT_UNDOABLE");

    let (fresh, fresh_revision) = f.extracted("Fresh", "# Fresh\n\nUndo me.\n");
    let (status, json) = split(
        f.api
            .undo(&Fixture::with_if_match(&fresh_revision), &fresh.to_string())
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["status"], "forgotten");
    let (status, _) = split(
        f.api
            .read(
                &Fixture::headers(),
                &fresh.to_string(),
                ReadMemoryQuery::default(),
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── memory_changed ─────────────────────────────────────────────────────

/// Every write through the routes is one `memory_changed` on the bus with
/// `surface: user`, a `sync` that finds a hand edit publishes one with no
/// surface, and a read publishes nothing.
#[tokio::test]
async fn every_write_and_a_detected_external_edit_publish_one_memory_changed() {
    let f = Fixture::new();
    let mut rx = f.bus.subscribe();
    let (id, revision) = f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let sid = id.to_string();
    let (_, edited) = split(
        f.api
            .update(
                &Fixture::with_if_match(&revision),
                &sid,
                update_body("edit", Some("# Editor\n\nneovim.\n")),
            )
            .await,
    )
    .await;
    let _ = split(
        f.api
            .read(&Fixture::headers(), &sid, ReadMemoryQuery::default())
            .await,
    )
    .await;
    let (other, _) = f.create("Shell", "# Shell\n\nzsh.\n", "preference").await;
    let (status, _) = split(
        f.api
            .delete(
                &Fixture::with_if_match(edited["revision"].as_str().unwrap()),
                &sid,
            )
            .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let file = f.note_file(other);
    let mut text = std::fs::read_to_string(&file).unwrap();
    text.push_str("\nHand-edited.\n");
    std::fs::write(&file, text).unwrap();
    let (status, json) = split(f.api.sync(&Fixture::headers()).await).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["changed"], 1);
    assert_eq!(json["changes"][0]["note_id"], other.to_string());

    let mut seen = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::MemoryChanged {
            change, surface, ..
        } = event
        {
            seen.push((change, surface));
        }
    }
    assert_eq!(
        seen,
        [
            ("created".to_string(), Some("user".to_string())),
            ("updated".to_string(), Some("user".to_string())),
            ("created".to_string(), Some("user".to_string())),
            ("forgotten".to_string(), Some("user".to_string())),
            ("external".to_string(), None),
        ]
    );
}

// ── The management reads ───────────────────────────────────────────────

/// `doctor` reports the owner's partitions and `recently-used` the ring.
#[tokio::test]
async fn the_doctor_and_recently_used_answer_their_shapes() {
    let f = Fixture::new();
    f.create("Editor", "# Editor\n\nvim.\n", "preference").await;
    let (status, json) = split(f.api.doctor(&Fixture::headers()).await).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let partitions = json["partitions"].as_array().unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0]["scope"], "global");
    assert_eq!(partitions[0]["health"], "ready");
    assert_eq!(partitions[0]["notes"], 1);
    assert_eq!(partitions[0]["orphan"], false);
    assert_eq!(json["move_snapshots"].as_array().unwrap().len(), 0);

    let (status, json) = split(f.api.recently_used(RecentlyUsedQuery::default())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["items"].as_array().unwrap().len(),
        0,
        "nothing delivered to a model yet"
    );
}
