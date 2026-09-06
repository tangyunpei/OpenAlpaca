//! `/v1/sessions` route tests — the status codes and the ownership rules.

use super::*;
use axum::body::to_bytes;
use openalpaca_core::events::SystemEvent;
use openalpaca_storage::{ConversationMessage, Database};

const OWNER: &str = "junpei";
const LANE: &str = "junpei:gui";

struct Harness {
    _dir: tempfile::TempDir,
    db: Database,
    bus: EventBus,
    ctx: SharedContext,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        Self {
            _dir: dir,
            db,
            bus: EventBus::default(),
            ctx: SharedContext::new(),
        }
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            db: &self.db,
            bus: &self.bus,
            ctx: &self.ctx,
            owner: OWNER,
        }
    }

    fn repo(&self) -> ConversationRepository<'_> {
        ConversationRepository::new(&self.db)
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

// ── GET /v1/sessions ─────────────────────────────────────────────────

#[tokio::test]
async fn listing_answers_the_envelope_with_its_total() {
    let h = Harness::new();
    h.repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    h.repo()
        .create_session(LANE, "gui", None, Some("Second"))
        .expect("session");

    let (status, body) = split(list_sessions(&h.deps(), ListSessionsQuery::default())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 2);
    assert_eq!(body["sessions"].as_array().expect("array").len(), 2);
    // The derived counts are present and honest — nothing is running.
    assert_eq!(body["sessions"][0]["active_task_count"], 0);
    assert_eq!(body["sessions"][0]["interrupted_task_count"], 0);
    // The compactor's bookkeeping stays off the wire.
    assert!(body["sessions"][0].get("summary").is_none());
}

#[tokio::test]
async fn listing_filters_by_status_and_refuses_an_unknown_one() {
    let h = Harness::new();
    let first = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    h.repo()
        .create_session(LANE, "gui", None, None)
        .expect("session");

    let (status, body) = split(list_sessions(
        &h.deps(),
        ListSessionsQuery {
            status: Some(SESSION_ARCHIVED.to_string()),
            ..Default::default()
        },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
    assert_eq!(body["sessions"][0]["id"], first.id);

    let (status, body) = split(list_sessions(
        &h.deps(),
        ListSessionsQuery {
            status: Some("paused".to_string()),
            ..Default::default()
        },
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "INVALID_STATUS");
}

#[tokio::test]
async fn listing_is_unscoped_by_owner() {
    let h = Harness::new();
    h.repo()
        .get_or_create_active_session("someone-else:gui", "gui", None)
        .expect("session");

    // R40: reads see every lane; only writes are scoped.
    let (status, body) = split(list_sessions(&h.deps(), ListSessionsQuery::default())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1);
}

// ── POST /v1/sessions ────────────────────────────────────────────────

#[tokio::test]
async fn creating_answers_201_and_archives_the_previous_active() {
    let h = Harness::new();
    let first = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let mut rx = h.bus.subscribe();

    let (status, body) = split(create_session(
        &h.deps(),
        CreateSessionRequest {
            title: Some("New chat".to_string()),
            ..Default::default()
        },
    ))
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["title"], "New chat");
    assert_eq!(body["status"], SESSION_ACTIVE);
    assert_eq!(body["lane_key"], LANE);
    assert_ne!(body["id"], serde_json::json!(first.id));

    assert_eq!(
        h.repo().get_session(&first.id).unwrap().unwrap().status,
        SESSION_ARCHIVED
    );

    // The second window hears about it.
    let announced = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            SystemEvent::SessionChanged {
                session_id, status, ..
            } => Some((session_id, status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        announced,
        vec![(
            body["id"].as_str().unwrap().to_string(),
            SESSION_ACTIVE.to_string()
        )]
    );
}

#[tokio::test]
async fn creating_lands_on_the_callers_own_lane_whatever_the_source() {
    let h = Harness::new();
    let (status, body) = split(create_session(
        &h.deps(),
        CreateSessionRequest {
            source: Some("cli".to_string()),
            ..Default::default()
        },
    ))
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["lane_key"], "junpei:cli");
    assert_eq!(body["source"], "cli");
}

// ── GET /v1/sessions/{id} and /messages ──────────────────────────────

#[tokio::test]
async fn an_unknown_session_is_404_everywhere() {
    let h = Harness::new();
    let deps = h.deps();

    for response in [
        deps.reread("no-such-session"),
        get_session_messages(&deps, "no-such-session", SessionMessagesQuery::default()),
        activate_session(&deps, "no-such-session"),
        archive_session(&deps, "no-such-session"),
        patch_session(&deps, "no-such-session", PatchSessionRequest::default()),
        delete_session(&deps, "no-such-session"),
    ] {
        let (status, body) = split(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "SESSION_NOT_FOUND");
    }
}

#[tokio::test]
async fn messages_are_the_sessions_own_and_page_backwards_from_a_cursor() {
    let h = Harness::new();
    let first = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    for i in 0..5 {
        h.repo()
            .insert(&ConversationMessage {
                lane_key: LANE.to_string(),
                role: "user".to_string(),
                content: format!("message {i}"),
                ..Default::default()
            })
            .expect("insert");
    }
    let second = h
        .repo()
        .create_session(LANE, "gui", None, None)
        .expect("session");
    h.repo()
        .insert(&ConversationMessage {
            lane_key: LANE.to_string(),
            role: "user".to_string(),
            content: "elsewhere".to_string(),
            ..Default::default()
        })
        .expect("insert");

    let (status, body) =
        split(get_session_messages(&h.deps(), &first.id, SessionMessagesQuery::default())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 5);
    let ids: Vec<i64> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 5);
    // GAP-23's chips ride every message, empty or not.
    assert_eq!(body["messages"][0]["artifacts"], serde_json::json!([]));

    // "Load older": the two messages before the third, chronological.
    let (status, body) = split(get_session_messages(
        &h.deps(),
        &first.id,
        SessionMessagesQuery {
            limit: Some(2),
            before_id: Some(ids[2]),
            ..Default::default()
        },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["message 0", "message 1"]
    );

    // The other conversation on the same lane is untouched.
    let (_, body) =
        split(get_session_messages(&h.deps(), &second.id, SessionMessagesQuery::default())).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["messages"][0]["content"], "elsewhere");
}

// ── activate / archive ───────────────────────────────────────────────

#[tokio::test]
async fn activating_an_archived_session_steps_the_incumbent_down() {
    let h = Harness::new();
    let first = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let second = h
        .repo()
        .create_session(LANE, "gui", None, None)
        .expect("session");
    let mut rx = h.bus.subscribe();

    let (status, body) = split(activate_session(&h.deps(), &first.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], SESSION_ACTIVE);
    assert_eq!(
        h.repo().get_session(&second.id).unwrap().unwrap().status,
        SESSION_ARCHIVED
    );

    // Both halves of the switch are announced, incumbent first.
    let announced = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|e| match e {
            SystemEvent::SessionChanged {
                session_id, status, ..
            } => Some((session_id, status)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        announced,
        vec![
            (second.id.clone(), SESSION_ARCHIVED.to_string()),
            (first.id.clone(), SESSION_ACTIVE.to_string()),
        ]
    );
}

#[tokio::test]
async fn archiving_leaves_the_lane_with_no_active_session() {
    let h = Harness::new();
    let only = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");

    let (status, body) = split(archive_session(&h.deps(), &only.id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], SESSION_ARCHIVED);
    assert!(body["ended_at"].is_string());
    assert!(h.repo().active_session_id(LANE).unwrap().is_none());
}

// ── PATCH ────────────────────────────────────────────────────────────

#[tokio::test]
async fn patching_renames_and_unbinds_the_workspace() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", Some("/repo/one"))
        .expect("session");

    let (status, body) = split(patch_session(
        &h.deps(),
        &session.id,
        PatchSessionRequest {
            title: Some("Renamed".to_string()),
            workspace_path: None,
        },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["title"], "Renamed");
    assert_eq!(
        body["workspace_id"], "/repo/one",
        "an absent workspace_path leaves the binding alone"
    );

    let (status, body) = split(patch_session(
        &h.deps(),
        &session.id,
        PatchSessionRequest {
            title: None,
            workspace_path: Some(String::new()),
        },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["workspace_id"].is_null(), "an empty string unbinds");
    assert_eq!(body["title"], "Renamed", "and leaves the title alone");
}

// ── DELETE ───────────────────────────────────────────────────────────

#[tokio::test]
async fn deleting_answers_204_and_takes_the_transcript_with_it() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    h.repo()
        .insert(&ConversationMessage {
            lane_key: LANE.to_string(),
            role: "user".to_string(),
            content: "hello".to_string(),
            ..Default::default()
        })
        .expect("insert");
    let mut rx = h.bus.subscribe();

    let response = delete_session(&h.deps(), &session.id);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(h.repo().get_session(&session.id).unwrap().is_none());
    assert_eq!(h.repo().count_by_lane(LANE).unwrap(), 0);

    assert!(matches!(
        rx.try_recv(),
        Ok(SystemEvent::SessionChanged { ref status, .. }) if status == "deleted"
    ));
}

#[tokio::test]
async fn deleting_a_session_with_a_run_in_flight_is_409() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    h.db
        .with_connection(|conn| {
            conn.execute(
                "INSERT INTO task (id, title, created_by, source_lane, session_id, status)
                 VALUES ('task-1', 'A run', 'junpei', ?1, ?2, 'running')",
                [LANE, session.id.as_str()],
            )?;
            Ok(())
        })
        .expect("seed the run");
    h.ctx.register_workflow_for_lane(LANE, "task-1");

    let (status, body) = split(delete_session(&h.deps(), &session.id)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "SESSION_HAS_ACTIVE_WORKFLOWS");
    assert!(h.repo().get_session(&session.id).unwrap().is_some());
}

/// A run in flight on the same lane but in *another* conversation does not
/// block this one: sessions are the unit, lanes are not.
#[tokio::test]
async fn a_run_in_another_conversation_on_the_lane_does_not_block_the_delete() {
    let h = Harness::new();
    let busy = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let idle = h
        .repo()
        .create_session(LANE, "gui", None, None)
        .expect("session");
    h.db
        .with_connection(|conn| {
            conn.execute(
                "INSERT INTO task (id, title, created_by, source_lane, session_id, status)
                 VALUES ('task-1', 'A run', 'junpei', ?1, ?2, 'running')",
                [LANE, busy.id.as_str()],
            )?;
            Ok(())
        })
        .expect("seed the run");
    h.ctx.register_workflow_for_lane(LANE, "task-1");

    let response = delete_session(&h.deps(), &idle.id);
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// ── Owner scoping (R40) ──────────────────────────────────────────────

/// Reads see another owner's conversation; every write answers `404` rather
/// than `403`, so the route never confirms that it exists.
#[tokio::test]
async fn writes_on_another_owners_session_answer_404() {
    let h = Harness::new();
    let theirs = h
        .repo()
        .get_or_create_active_session("someone-else:gui", "gui", None)
        .expect("session");

    let deps = h.deps();
    let (status, _) = split(deps.reread(&theirs.id)).await;
    assert_eq!(status, StatusCode::OK, "reads stay unscoped");

    for response in [
        activate_session(&deps, &theirs.id),
        archive_session(&deps, &theirs.id),
        patch_session(
            &deps,
            &theirs.id,
            PatchSessionRequest {
                title: Some("mine now".to_string()),
                workspace_path: None,
            },
        ),
        delete_session(&deps, &theirs.id),
    ] {
        let (status, body) = split(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "SESSION_NOT_FOUND");
    }

    // Nothing moved.
    let after = h.repo().get_session(&theirs.id).unwrap().expect("still there");
    assert_eq!(after.status, SESSION_ACTIVE);
    assert_eq!(after.title, "");
}
