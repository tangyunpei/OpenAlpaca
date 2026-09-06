//! Lane follow-up queue endpoints (GAP-03)
//!
//! ```text
//! GET    /v1/lanes/{lane_key}/followups        -> the lane's pending queue
//! POST   /v1/lanes/{lane_key}/followups        -> queue one (201)
//! DELETE /v1/lanes/{lane_key}/followups/{id}   -> cancel one, race-safe
//! ```
//!
//! The queue itself is not new: `lane_followups` has existed since Routing V2,
//! the model writes to it through its own `queue_followup` tool, and the
//! autostart claims from it when a workflow finalizes. What was missing was any
//! way for a *client* to see the queue, add to it, or take something back out —
//! so the GUI's `Queue follow-up` control had nothing to call.
//!
//! Four things this module is deliberate about:
//!
//! * **The two mutating verbs are owner-scoped; `GET` is not.** A queued
//!   follow-up is not a note filed against a lane — the daemon claims it and
//!   runs it as a fresh turn on the lane it names, and posts the answer there.
//!   `POST` and `DELETE` therefore refuse a lane whose `user_id` is not the
//!   local user's, with the same `404` an absent row gets (R42).
//! * **`FollowupRecord` is never serialized.** It carries `principal_json` —
//!   the identity a queued item re-enters the front door with. [`FollowupView`]
//!   is the wire shape, and it has no principal in any form.
//! * **Clients cannot mint `unprocessed_steering`.** That kind is the daemon's
//!   own bookkeeping for a steering message a workflow exited before draining;
//!   `claim_next` never claims one (it is injected as a context block on the
//!   lane's next turn instead), so a client-minted row of that kind would sit
//!   in the queue forever. `POST` accepts `kind` only when it is `"followup"`.
//! * **Cancel is a CAS, not a write.** `FollowupRepository::cancel_if_queued`
//!   and the autostart's `claim_next` race on the same `status = 'queued'`
//!   predicate, so exactly one of them wins. A cancel that lost is a `409`,
//!   not a silently-ignored request — the turn it tried to stop is running.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use openalpaca_core::bus::EventBus;
use openalpaca_core::events::SystemEvent;
use openalpaca_core::lane::LaneKey;
use openalpaca_core::security::policy::Principal;
use openalpaca_storage::{Database, FOLLOWUP_KIND_FOLLOWUP, FollowupRecord, FollowupRepository};

use super::{api_error, request_project_root, workspace_header};
use crate::AppState;

// ── Wire shapes ──────────────────────────────────────────────────────

/// One follow-up row, as a client sees it.
///
/// Two stored columns are absent by design. `principal_json` is the identity
/// the item re-enters with and is nobody's business on the wire; `workspace_path`
/// is where the re-entered turn will be scoped, which the client did not ask
/// about and cannot change. Everything else is the row.
#[derive(Debug, Clone, Serialize)]
pub struct FollowupView {
    pub id: i64,
    pub lane_key: String,
    /// `"followup"` | `"unprocessed_steering"`.
    pub kind: String,
    pub content: String,
    /// The run the item was queued from, if any.
    pub source_task_id: Option<String>,
    /// `"queued"` | `"running"` | `"done"` | `"cancelled"`.
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<FollowupRecord> for FollowupView {
    fn from(record: FollowupRecord) -> Self {
        Self {
            id: record.id,
            lane_key: record.lane_key,
            kind: record.kind,
            content: record.content,
            source_task_id: record.source_task_id,
            status: record.status,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct QueueFollowupRequest {
    pub content: String,
    /// Optional, and only ever `"followup"` — see the module note.
    #[serde(default)]
    pub kind: Option<String>,
    /// The run this follow-up came out of, when the client knows it.
    #[serde(default)]
    pub source_task_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct CancelFollowupResponse {
    id: i64,
    status: &'static str,
}

// ── The lane key ─────────────────────────────────────────────────────

/// Whether a path segment is a lane key at all.
///
/// [`LaneKey::from_str`] is the daemon's own parser for the canonical
/// `"{user_id}:{source}"` form — reused rather than re-spelled, so a route
/// cannot disagree with the lane manager about what a lane is. A path segment
/// that is not one names no lane, and a `queue` on it would insert a row
/// nothing will ever claim.
fn lane_key_ok(lane_key: &str) -> bool {
    LaneKey::from_str(lane_key).is_some()
}

fn invalid_lane_key() -> Response {
    api_error(
        StatusCode::BAD_REQUEST,
        "INVALID_LANE_KEY",
        "lane_key must be \"{user_id}:{source}\" — e.g. \"junpei:gui\".",
    )
}

/// The refusal a mutating verb owes this lane key, if any: `None` means the
/// key parses **and** names a lane the local user owns.
///
/// The two mutating verbs are owner-scoped; `GET` is not (R42). A follow-up is
/// not a note filed against a lane — the daemon later claims it and **runs it
/// as a fresh turn** on the lane it names (`dispatcher/lead_agent.rs` →
/// `followup.rs`, with `lane_override`), and its answer is posted there. That
/// puts `POST` on the injecting side of R40's own line, where the steer route
/// already refuses a run the caller does not own; `DELETE` drops pending work
/// that would otherwise run. Neither is this caller's to do on
/// `4242:telegram`.
///
/// The refusal is `404`, never `403`, and it is the same `404` an absent row
/// gets: a lane key must not be probeable for existence by a caller who does
/// not hold it. Byte for byte what `routes/tasks.rs`'s steer route answers for
/// a run belonging to someone else.
fn lane_refusal(lane_key: &str, owner_id: &str) -> Option<Response> {
    let Some(lane) = LaneKey::from_str(lane_key) else {
        return Some(invalid_lane_key());
    };
    if lane.user_id != owner_id {
        return Some(api_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "No such lane",
        ));
    }
    None
}

fn db_error(e: impl std::fmt::Display) -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string())
}

// ── GET /v1/lanes/{lane_key}/followups ───────────────────────────────

/// The lane's **pending** queue, oldest first — the claim order.
///
/// Queued rows only, of either kind: those are the items that still have a
/// future, and the only ones `DELETE` can act on. A finished or cancelled row
/// is history and lives in the event log (`followup_queued` /
/// `followup_cancelled`), not in a queue read-back that would grow for the
/// lifetime of the lane.
///
/// A bare array, per plan §7: the list is per-lane and unpaginated.
fn list_followups(db: &Database, lane_key: &str) -> Response {
    if !lane_key_ok(lane_key) {
        return invalid_lane_key();
    }
    match FollowupRepository::new(db).list_queued_by_lane(lane_key) {
        Ok(rows) => {
            let views: Vec<FollowupView> = rows.into_iter().map(FollowupView::from).collect();
            Json(views).into_response()
        }
        Err(e) => db_error(e),
    }
}

// ── POST /v1/lanes/{lane_key}/followups ──────────────────────────────

/// Queue one follow-up on the local user's own lane.
///
/// The principal is the daemon's own user, exactly as the model's
/// `queue_followup` tool records it, because that is who the re-entered turn
/// will run as. `workspace_path` comes from the request's `x-workspace-path`
/// header through the one resolver every route shares — never from the body,
/// which would let a client scope a future turn anywhere. And the lane must be
/// one this user owns ([`lane_refusal`]), because that is where the turn will run
/// and where its answer will be posted.
fn queue_followup(
    db: &Database,
    bus: &EventBus,
    owner_id: &str,
    lane_key: &str,
    workspace_path: Option<&str>,
    request: QueueFollowupRequest,
) -> Response {
    if let Some(refusal) = lane_refusal(lane_key, owner_id) {
        return refusal;
    }

    let content = request.content.trim();
    if content.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "EMPTY_CONTENT",
            "content must not be empty",
        );
    }

    // The one kind a client may name. `unprocessed_steering` is refused rather
    // than accepted-and-rewritten: a caller that asked for it wanted the other
    // behaviour, and would never learn it did not get it.
    match request.kind.as_deref() {
        None => {}
        Some(FOLLOWUP_KIND_FOLLOWUP) => {}
        Some(other) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "INVALID_KIND",
                format!(
                    "kind must be \"{FOLLOWUP_KIND_FOLLOWUP}\" (got \"{other}\"). \
                     \"unprocessed_steering\" is the daemon's own bookkeeping — it is \
                     never auto-claimed, so a client-queued one would never run."
                ),
            );
        }
    }

    let principal = Principal::User {
        global_id: owner_id.to_string(),
    };
    let principal_json = match serde_json::to_string(&principal) {
        Ok(json) => json,
        Err(e) => return db_error(e),
    };

    let repo = FollowupRepository::new(db);
    let id = match repo.queue(
        lane_key,
        FOLLOWUP_KIND_FOLLOWUP,
        content,
        &principal_json,
        workspace_path,
        request.source_task_id.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) => return db_error(e),
    };

    // Read the row back rather than assembling one: `created_at`/`updated_at`
    // are SQLite defaults, so the daemon's own strings are the only honest
    // ones to serve.
    let row = match repo.get(id) {
        Ok(Some(row)) => row,
        Ok(None) => return db_error("queued follow-up disappeared before it could be read"),
        Err(e) => return db_error(e),
    };

    bus.publish(SystemEvent::FollowupQueued {
        lane_key: lane_key.to_string(),
        followup_id: id,
        kind: FOLLOWUP_KIND_FOLLOWUP.to_string(),
        timestamp: Utc::now(),
    });

    (StatusCode::CREATED, Json(FollowupView::from(row))).into_response()
}

// ── DELETE /v1/lanes/{lane_key}/followups/{id} ───────────────────────

/// Cancel a queued follow-up, or say why it could not be.
///
/// The read decides `404`; the CAS decides `409`. They are not the same
/// question and cannot be collapsed: a row this lane does not own must be
/// indistinguishable from one that never existed, while a row that *is* this
/// lane's but has already been claimed has to say so — the turn is running,
/// and reporting "cancelled" would be a lie the client acts on.
///
/// A lane the local user does not own answers the same `404` ([`lane_refusal`]):
/// another user's pending work is not this caller's to drop.
fn cancel_followup(
    db: &Database,
    bus: &EventBus,
    owner_id: &str,
    lane_key: &str,
    id: &str,
) -> Response {
    if let Some(refusal) = lane_refusal(lane_key, owner_id) {
        return refusal;
    }

    let not_found = || {
        api_error(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "No such follow-up in this lane",
        )
    };

    // Parsed here rather than by the extractor so a malformed id answers with
    // this route's envelope: an id that is not a number names no row.
    let Ok(id) = id.parse::<i64>() else {
        return not_found();
    };

    let repo = FollowupRepository::new(db);
    match repo.get(id) {
        Ok(Some(row)) if row.lane_key == lane_key => {}
        // Absent, or another lane's — the same answer, so an id cannot be
        // probed for existence from a lane that does not hold it.
        Ok(_) => return not_found(),
        Err(e) => return db_error(e),
    }

    match repo.cancel_if_queued(id, lane_key) {
        Ok(true) => {
            bus.publish(SystemEvent::FollowupCancelled {
                lane_key: lane_key.to_string(),
                followup_id: id,
                timestamp: Utc::now(),
            });
            Json(CancelFollowupResponse {
                id,
                status: "cancelled",
            })
            .into_response()
        }
        // The row was read as queued a moment ago and is not any more: the
        // autostart claimed it, or another client cancelled it first.
        Ok(false) => api_error(
            StatusCode::CONFLICT,
            "FOLLOWUP_NOT_QUEUED",
            "That follow-up is no longer queued — it has already started, finished, or been \
             cancelled.",
        ),
        Err(e) => db_error(e),
    }
}

// ── Handlers ─────────────────────────────────────────────────────────

/// GET /v1/lanes/{lane_key}/followups
pub async fn list_followups_handler(
    State(state): State<Arc<AppState>>,
    Path(lane_key): Path<String>,
) -> Response {
    list_followups(&state.db, &lane_key)
}

/// POST /v1/lanes/{lane_key}/followups
pub async fn queue_followup_handler(
    State(state): State<Arc<AppState>>,
    Path(lane_key): Path<String>,
    headers: HeaderMap,
    Json(request): Json<QueueFollowupRequest>,
) -> Response {
    let workspace = request_project_root(workspace_header(&headers).as_deref());
    queue_followup(
        &state.db,
        &state.gateway.bus,
        &state.local_user_id,
        &lane_key,
        workspace.as_deref(),
        request,
    )
}

/// DELETE /v1/lanes/{lane_key}/followups/{id}
pub async fn cancel_followup_handler(
    State(state): State<Arc<AppState>>,
    Path((lane_key, id)): Path<(String, String)>,
) -> Response {
    cancel_followup(
        &state.db,
        &state.gateway.bus,
        &state.local_user_id,
        &lane_key,
        &id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use openalpaca_storage::FOLLOWUP_KIND_UNPROCESSED_STEERING;

    const OWNER: &str = "junpei";
    const LANE: &str = "junpei:gui";

    fn db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        (dir, db)
    }

    fn request(content: &str) -> QueueFollowupRequest {
        QueueFollowupRequest {
            content: content.to_string(),
            kind: None,
            source_task_id: None,
        }
    }

    /// Split a `Response` into its status and its JSON body.
    async fn split(response: Response) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("read the response body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    /// Queue one row straight through the repository (what the model's own
    /// `queue_followup` tool does), returning its id.
    fn seed(db: &Database, lane: &str, kind: &str, content: &str) -> i64 {
        FollowupRepository::new(db)
            .queue(lane, kind, content, "\"System\"", None, Some("task-1"))
            .expect("queue")
    }

    /// The stored status of one row.
    fn status_of(db: &Database, id: i64) -> String {
        FollowupRepository::new(db)
            .get(id)
            .expect("read")
            .expect("the row exists")
            .status
    }

    // ── POST ──────────────────────────────────────────────────────

    /// The happy path: `201` with the stored row, and the rail's own
    /// `FollowupQueued` event so a client that is not the caller sees it too.
    #[tokio::test]
    async fn queueing_a_followup_answers_201_with_the_stored_row() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        let (status, body) = split(queue_followup(
            &db,
            &bus,
            OWNER,
            LANE,
            Some("/Users/dev/openalpaca"),
            QueueFollowupRequest {
                content: "  audit the connectors  ".to_string(),
                kind: Some(FOLLOWUP_KIND_FOLLOWUP.to_string()),
                source_task_id: Some("task-7".to_string()),
            },
        ))
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["lane_key"], LANE);
        assert_eq!(body["kind"], "followup");
        assert_eq!(
            body["content"], "audit the connectors",
            "content is trimmed"
        );
        assert_eq!(body["source_task_id"], "task-7");
        assert_eq!(body["status"], "queued");
        assert!(body["created_at"].is_string());
        assert!(body["updated_at"].is_string());

        // Stored with the local user's identity and the request's project, so
        // the re-entered turn runs as this user, scoped where they were.
        let id = body["id"].as_i64().expect("an id");
        let row = FollowupRepository::new(&db).get(id).unwrap().unwrap();
        assert_eq!(row.principal_json, "{\"User\":{\"global_id\":\"junpei\"}}");
        assert_eq!(row.workspace_path.as_deref(), Some("/Users/dev/openalpaca"));

        let mut queued = 0;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::FollowupQueued {
                lane_key,
                followup_id,
                kind,
                ..
            } = event
            {
                assert_eq!(lane_key, LANE);
                assert_eq!(followup_id, id);
                assert_eq!(kind, "followup");
                queued += 1;
            }
        }
        assert_eq!(queued, 1, "exactly one FollowupQueued");
    }

    /// The wire shape carries no identity, in any spelling. `FollowupRecord`
    /// holds `principal_json`, and serializing it would put the queueing user
    /// on the wire for every client on the socket.
    #[test]
    fn the_wire_shape_never_carries_the_principal() {
        let view = FollowupView::from(FollowupRecord {
            id: 1,
            lane_key: LANE.to_string(),
            kind: FOLLOWUP_KIND_FOLLOWUP.to_string(),
            content: "do the thing".to_string(),
            principal_json: "{\"User\":{\"global_id\":\"who-queued-it\"}}".to_string(),
            workspace_path: Some("/Users/dev/openalpaca".to_string()),
            source_task_id: Some("task-1".to_string()),
            status: "queued".to_string(),
            created_at: "2026-09-05 10:00:00".to_string(),
            updated_at: "2026-09-05 10:00:00".to_string(),
        });
        let value = serde_json::to_value(&view).unwrap();

        for key in ["principal", "principal_json", "workspace_path"] {
            assert!(
                value.get(key).is_none(),
                "FollowupView still serves `{key}`"
            );
        }
        let text = serde_json::to_string(&view).unwrap();
        assert!(
            !text.contains("who-queued-it"),
            "the principal leaked into the body: {text}"
        );

        // …and it is the client's shape, field for field
        // (`lib/api/unbacked.ts`'s `FollowupRecord`).
        let object = value.as_object().expect("an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "content",
                "created_at",
                "id",
                "kind",
                "lane_key",
                "source_task_id",
                "status",
                "updated_at",
            ]
        );
    }

    /// A client cannot mint `unprocessed_steering`: `claim_next` never claims
    /// one, so the row would sit in the queue forever.
    #[tokio::test]
    async fn a_client_cannot_queue_unprocessed_steering() {
        let (_dir, db) = db();
        let bus = EventBus::default();

        let (status, body) = split(queue_followup(
            &db,
            &bus,
            OWNER,
            LANE,
            None,
            QueueFollowupRequest {
                content: "leftover".to_string(),
                kind: Some(FOLLOWUP_KIND_UNPROCESSED_STEERING.to_string()),
                source_task_id: None,
            },
        ))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"]["code"], "INVALID_KIND");
        assert!(
            FollowupRepository::new(&db)
                .list_queued_by_lane(LANE)
                .unwrap()
                .is_empty(),
            "nothing was written"
        );
    }

    /// An empty (or whitespace-only) follow-up is a `400`: it would re-enter as
    /// a turn with nothing to do.
    #[tokio::test]
    async fn an_empty_followup_is_a_400() {
        let (_dir, db) = db();
        let bus = EventBus::default();

        for content in ["", "   \n\t "] {
            let (status, body) = split(queue_followup(
                &db,
                &bus,
                OWNER,
                LANE,
                None,
                request(content),
            ))
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body["error"]["code"], "EMPTY_CONTENT");
        }
        assert!(
            FollowupRepository::new(&db)
                .list_queued_by_lane(LANE)
                .unwrap()
                .is_empty()
        );
    }

    /// Every verb validates the lane key, and a segment that is not one is a
    /// `400` — not an empty list or a row nothing will claim.
    #[tokio::test]
    async fn a_malformed_lane_key_is_a_400_on_every_verb() {
        let (_dir, db) = db();
        let bus = EventBus::default();

        for lane in ["no-colon", ":empty-user", "empty-source:"] {
            let (status, body) = split(list_followups(&db, lane)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "GET {lane}");
            assert_eq!(body["error"]["code"], "INVALID_LANE_KEY");

            let (status, _) =
                split(queue_followup(&db, &bus, OWNER, lane, None, request("go"))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "POST {lane}");

            let (status, _) = split(cancel_followup(&db, &bus, OWNER, lane, "1")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "DELETE {lane}");
        }
    }

    /// A `POST` may only name a lane the local user owns. A follow-up is text
    /// the daemon will later **run as a fresh turn** on the lane it names, so
    /// this verb is on the injecting side of R40's line — the same side the
    /// steer route is on, and it refuses a run the caller does not own with
    /// `404`. Without the check, a caller holding the bearer token could park a
    /// turn on a connector lane such as `4242:telegram` and have its answer
    /// land in someone else's chat.
    #[tokio::test]
    async fn queueing_on_another_users_lane_is_a_404() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();

        let (status, body) = split(queue_followup(
            &db,
            &bus,
            OWNER,
            "4242:telegram",
            None,
            request("post this to their chat"),
        ))
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body["error"]["code"], "NOT_FOUND",
            "a lane this caller does not own is indistinguishable from one that is not there — \
             never a 403"
        );
        assert!(
            FollowupRepository::new(&db)
                .list_queued_by_lane("4242:telegram")
                .unwrap()
                .is_empty(),
            "nothing was written to the other lane"
        );
        assert!(rx.try_recv().is_err(), "and nothing was announced");
    }

    /// `DELETE` likewise: another user's pending work is not this caller's to
    /// drop, and the refusal must not confirm the row exists.
    #[tokio::test]
    async fn cancelling_on_another_users_lane_is_a_404() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let foreign = seed(&db, "4242:telegram", FOLLOWUP_KIND_FOLLOWUP, "theirs");

        let (status, body) = split(cancel_followup(
            &db,
            &bus,
            OWNER,
            "4242:telegram",
            &foreign.to_string(),
        ))
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "NOT_FOUND");
        assert_eq!(status_of(&db, foreign), "queued", "their row is untouched");
        assert!(rx.try_recv().is_err());
    }

    // ── GET ───────────────────────────────────────────────────────

    /// The lane's pending queue, oldest first, both kinds — and nothing from
    /// another lane. Terminal rows are history, not queue.
    #[tokio::test]
    async fn the_list_is_this_lanes_queue_in_claim_order() {
        let (_dir, db) = db();
        let first = seed(&db, LANE, FOLLOWUP_KIND_FOLLOWUP, "first");
        let leftover = seed(&db, LANE, FOLLOWUP_KIND_UNPROCESSED_STEERING, "leftover");
        let done = seed(&db, LANE, FOLLOWUP_KIND_FOLLOWUP, "already run");
        seed(&db, "someone:cli", FOLLOWUP_KIND_FOLLOWUP, "another lane");
        FollowupRepository::new(&db).mark_done(done).unwrap();

        let (status, body) = split(list_followups(&db, LANE)).await;
        assert_eq!(status, StatusCode::OK);

        let rows = body.as_array().expect("a bare array, not an envelope");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], first);
        assert_eq!(rows[0]["kind"], "followup");
        assert_eq!(rows[1]["id"], leftover);
        assert_eq!(
            rows[1]["kind"], "unprocessed_steering",
            "a steering leftover is queued too, and the client renders it"
        );
    }

    /// `GET` stays unscoped where `POST`/`DELETE` are not (R42): reading a
    /// queue changes nothing, and it is the same read/list line every other
    /// task-surface route already sits on.
    #[tokio::test]
    async fn the_list_is_not_owner_scoped() {
        let (_dir, db) = db();
        let id = seed(&db, "4242:telegram", FOLLOWUP_KIND_FOLLOWUP, "theirs");

        let (status, body) = split(list_followups(&db, "4242:telegram")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().expect("an array").len(), 1);
        assert_eq!(body[0]["id"], id);
    }

    /// A lane with nothing pending is an empty array, not a 404.
    #[tokio::test]
    async fn an_empty_lane_lists_nothing() {
        let (_dir, db) = db();
        let (status, body) = split(list_followups(&db, LANE)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!([]));
    }

    // ── DELETE ────────────────────────────────────────────────────

    /// The happy path: the row is cancelled, the answer names it, and the
    /// cancellation is announced once.
    #[tokio::test]
    async fn cancelling_a_queued_followup_answers_200_and_announces_it() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let id = seed(&db, LANE, FOLLOWUP_KIND_FOLLOWUP, "never mind");

        let (status, body) = split(cancel_followup(&db, &bus, OWNER, LANE, &id.to_string())).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["id"], id);
        assert_eq!(body["status"], "cancelled");

        assert_eq!(status_of(&db, id), "cancelled");
        assert!(
            FollowupRepository::new(&db)
                .list_queued_by_lane(LANE)
                .unwrap()
                .is_empty()
        );

        let mut cancelled = 0;
        while let Ok(event) = rx.try_recv() {
            if let SystemEvent::FollowupCancelled {
                lane_key,
                followup_id,
                ..
            } = event
            {
                assert_eq!(lane_key, LANE);
                assert_eq!(followup_id, id);
                cancelled += 1;
            }
        }
        assert_eq!(cancelled, 1, "exactly one FollowupCancelled");
    }

    /// **The race.** The autostart claimed the row (queued → running) before
    /// the cancel landed: the CAS loses, the answer is `409`, the row keeps
    /// running, and nothing is announced.
    #[tokio::test]
    async fn a_followup_already_claimed_by_the_autostart_is_a_409() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let id = seed(&db, LANE, FOLLOWUP_KIND_FOLLOWUP, "too late");

        // What `finalize` does when the workflow ends — the other CAS.
        let claimed = FollowupRepository::new(&db)
            .claim_next(LANE)
            .unwrap()
            .expect("the autostart claims it");
        assert_eq!(claimed.id, id);

        let (status, body) = split(cancel_followup(&db, &bus, OWNER, LANE, &id.to_string())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "FOLLOWUP_NOT_QUEUED");
        assert_eq!(
            status_of(&db, id),
            "running",
            "the claimed turn keeps running — the cancel must not overwrite it"
        );
        assert!(
            rx.try_recv().is_err(),
            "a cancel that lost the CAS announces nothing"
        );
    }

    /// A second cancel of the same row is the same `409`: cancel is a
    /// transition, not an idempotent assertion.
    #[tokio::test]
    async fn cancelling_twice_is_a_409_the_second_time() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let id = seed(&db, LANE, FOLLOWUP_KIND_FOLLOWUP, "never mind");

        let (status, _) = split(cancel_followup(&db, &bus, OWNER, LANE, &id.to_string())).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = split(cancel_followup(&db, &bus, OWNER, LANE, &id.to_string())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "FOLLOWUP_NOT_QUEUED");
    }

    /// An unknown id, a non-numeric id, and another lane's row are all the same
    /// `404` — an id must not be probeable from a lane that does not hold it.
    #[tokio::test]
    async fn an_unknown_or_foreign_followup_is_a_404() {
        let (_dir, db) = db();
        let bus = EventBus::default();
        let foreign = seed(&db, "someone:cli", FOLLOWUP_KIND_FOLLOWUP, "not yours");

        for id in ["999", "not-a-number", &foreign.to_string()] {
            let (status, body) = split(cancel_followup(&db, &bus, OWNER, LANE, id)).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "id {id}");
            assert_eq!(body["error"]["code"], "NOT_FOUND");
        }

        // The other lane's row is untouched.
        assert_eq!(status_of(&db, foreign), "queued");
    }
}
