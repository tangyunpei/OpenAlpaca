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
    /// Where `GET …/events` reads the JSONL from — the home store's
    /// `sessions/` in production, a tempdir here.
    sessions_root: std::path::PathBuf,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open db");
        let sessions_root = dir.path().join("sessions");
        Self {
            _dir: dir,
            db,
            bus: EventBus::default(),
            ctx: SharedContext::new(),
            sessions_root,
        }
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            db: &self.db,
            bus: &self.bus,
            ctx: &self.ctx,
            owner: OWNER,
            sessions_root: Some(self.sessions_root.clone()),
        }
    }

    fn repo(&self) -> ConversationRepository<'_> {
        ConversationRepository::new(&self.db)
    }
}

/// Split a `Response` into its status and its JSON body.
async fn split(response: Response) -> (StatusCode, serde_json::Value) {
    split_within(response, 1 << 20).await
}

/// The same, for a response deliberately bigger than `split`'s 1 MiB guard —
/// the event log's byte budget is 4 MiB, and the test that pins it has to be
/// able to read a page that reaches it.
async fn split_within(response: Response, cap: usize) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), cap)
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

/// T39 left `interrupted_task_count` structurally 0 because nothing wrote the
/// status. §5.6b's boot sweep does, and the grouped query counts it — so the
/// sidebar's badge stops being decoration.
#[tokio::test]
async fn the_interrupted_count_counts_the_runs_the_boot_sweep_marked() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let other = h
        .repo()
        .create_session(LANE, "gui", None, Some("Untouched"))
        .expect("session");

    // Two runs the previous incarnation left in flight, plus one that really
    // finished and one that really failed — neither is an interruption.
    for (id, status, in_session) in [
        ("t-1", "running", &session.id),
        ("t-2", "queued", &session.id),
        ("t-3", "completed", &session.id),
        ("t-4", "failed", &session.id),
        ("t-5", "running", &other.id),
    ] {
        h.db.with_connection(|conn| {
            conn.execute(
                "INSERT INTO task (id, title, status, priority, created_by, source_lane, session_id)
                 VALUES (?1, 'a run', ?2, 0, 'junpei', 'junpei:gui', ?3)",
                (id, status, in_session.as_str()),
            )?;
            Ok(())
        })
        .expect("seed task");
    }
    openalpaca_storage::repository::TaskRepository::new(&h.db)
        .interrupt_all_non_terminal("interrupted — the daemon restarted (instance i-1)")
        .expect("sweep");

    let (status, body) = split(list_sessions(&h.deps(), ListSessionsQuery::default())).await;
    assert_eq!(status, StatusCode::OK);
    let counts: std::collections::HashMap<&str, i64> = body["sessions"]
        .as_array()
        .expect("array")
        .iter()
        .map(|s| {
            (
                s["id"].as_str().expect("id"),
                s["interrupted_task_count"].as_i64().expect("count"),
            )
        })
        .collect();
    assert_eq!(counts[session.id.as_str()], 2, "the running and the queued one");
    assert_eq!(counts[other.id.as_str()], 1);
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
async fn patching_renames_and_binds_a_workspace_the_session_does_not_have() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
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
    assert!(
        body["workspace_id"].is_null(),
        "an absent workspace_path leaves the binding alone"
    );

    // An unbound session accepts a project, resolved to its root (R22).
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(project.path().join(".git")).expect("marker");
    let path = project.path().to_string_lossy().to_string();
    let (status, body) = split(patch_session(
        &h.deps(),
        &session.id,
        PatchSessionRequest {
            title: None,
            workspace_path: Some(path.clone()),
        },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["workspace_id"].as_str(),
        request_project_root(Some(&path)).as_deref()
    );
    assert_eq!(body["title"], "Renamed", "and leaves the title alone");
}

/// R48: a session's project is bound once. Re-pointing it — or unbinding it,
/// which is a re-point in two calls — would leave the runs it already started
/// disagreeing with it about which project they belong to. §5.1's answer to a
/// project change is a new session, and the gateway opens one on the turn that
/// changes project; `PATCH` says so instead of quietly re-pointing.
#[tokio::test]
async fn patching_the_workspace_of_a_bound_session_is_409() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", Some("/repo/one"))
        .expect("session");

    for workspace_path in [Some("/repo/two".to_string()), Some(String::new())] {
        let (status, body) = split(patch_session(
            &h.deps(),
            &session.id,
            PatchSessionRequest {
                title: Some("Renamed".to_string()),
                workspace_path,
            },
        ))
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "SESSION_WORKSPACE_BOUND");
    }
    let unchanged = h.repo().get_session(&session.id).unwrap().expect("session");
    assert_eq!(unchanged.workspace_id.as_deref(), Some("/repo/one"));
    assert_eq!(unchanged.title, "", "a refused PATCH changes nothing at all");

    // Renaming a bound session is untouched — only the binding is frozen.
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
    assert_eq!(body["workspace_id"], "/repo/one");
}

// ── GET /v1/sessions/{id}/events ─────────────────────────────────────

/// Write `lines` verbatim into a session's live segment, the way the writer
/// would have. The route reads files, not a service, so this is the whole
/// fixture it needs.
fn write_log(h: &Harness, session_id: &str, lines: &[&str]) {
    let dir = h
        .sessions_root
        .join(openalpaca_core::session_log::session_dir_name(session_id));
    std::fs::create_dir_all(&dir).expect("session dir");
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join(openalpaca_core::session_log::LIVE_SEGMENT), body).expect("write log");
}

fn record(seq: u64, kind: &str, extra: &str) -> String {
    format!(
        r#"{{"v":1,"seq":{seq},"ts":"2026-09-05T10:22:03.114Z","type":"{kind}"{extra},"data":{{"n":{seq}}}}}"#
    )
}

/// §5.7: `GET /v1/sessions/{id}/events?after_seq=&types=&limit=` answers
/// `{events, next_after_seq}` — the cursor form §5.4 names, over the JSONL.
#[tokio::test]
async fn the_event_log_pages_by_seq_and_answers_a_cursor() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let lines: Vec<String> = (1..=10).map(|i| record(i, "round", "")).collect();
    write_log(&h, &session.id, &lines.iter().map(String::as_str).collect::<Vec<_>>());

    let (status, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { limit: Some(4), ..Default::default() },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[0]["type"], "round");
    assert_eq!(events[0]["data"]["n"], 1);
    assert_eq!(body["next_after_seq"], 4, "the cursor is the last seq returned");

    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { after_seq: Some(4), limit: Some(4), ..Default::default() },
    ))
    .await;
    assert_eq!(body["events"][0]["seq"], 5);
    assert_eq!(body["next_after_seq"], 8);

    // Drained: the cursor holds where it was, so a poller does not rewind.
    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { after_seq: Some(10), ..Default::default() },
    ))
    .await;
    assert!(body["events"].as_array().expect("events").is_empty());
    assert_eq!(body["next_after_seq"], 10);
}

/// `types` is a pure filter over the same paged scan — so is `agent`. The
/// cursor advances by what was **scanned**, never by what matched, or a filter
/// that matches nothing would stall a poller forever.
#[tokio::test]
async fn the_event_log_filters_without_stalling_the_cursor() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let lines = [
        record(1, "round", r#","agent":"lead_agent::a1""#),
        record(2, "tool_call", r#","agent":"lead_agent::a1""#),
        record(3, "tool_result", r#","agent":"research_agent::b2""#),
        record(4, "round", r#","agent":"research_agent::b2""#),
    ];
    write_log(&h, &session.id, &lines.iter().map(String::as_str).collect::<Vec<_>>());

    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { types: Some("tool_call,tool_result".into()), ..Default::default() },
    ))
    .await;
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "tool_call");
    assert_eq!(events[1]["type"], "tool_result");
    assert_eq!(body["next_after_seq"], 4, "the cursor is what was scanned");

    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { agent: Some("research_agent::b2".into()), ..Default::default() },
    ))
    .await;
    assert_eq!(body["events"].as_array().expect("events").len(), 2);

    // A filter that matches nothing still moves the cursor past what it read.
    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { types: Some("compaction".into()), ..Default::default() },
    ))
    .await;
    assert!(body["events"].as_array().expect("events").is_empty());
    assert_eq!(body["next_after_seq"], 4);
}

/// §5.4: "readers treat an unparseable final line as end-of-log". A `kill -9`
/// mid-write must not make the route fail — it answers what parsed.
#[tokio::test]
async fn a_torn_final_line_ends_the_log_instead_of_failing_the_route() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    write_log(
        &h,
        &session.id,
        &[
            &record(1, "round", ""),
            &record(2, "round", ""),
            r#"{"v":1,"seq":3,"ts":"2026-09-05T10:22:03.11"#,
        ],
    );

    let (status, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery::default(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"].as_array().expect("events").len(), 2);
    assert_eq!(body["next_after_seq"], 2);
}

/// A record count is not a response size: 500 records of 64 KB envelopes is a
/// ~32 MB response. The page carries a byte budget too, and answers a cursor
/// so the client comes back for the rest.
#[tokio::test]
async fn the_event_log_page_stops_at_its_byte_budget() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    // 120 envelopes at ~64 KB — 7.5 MB, comfortably over the 4 MiB budget.
    let fat: Vec<String> = (1..=120)
        .map(|seq| {
            format!(
                r#"{{"v":1,"seq":{seq},"ts":"2026-09-05T10:22:03.114Z","type":"round","data":{{"text":"{}"}}}}"#,
                "x".repeat(64 * 1024)
            )
        })
        .collect();
    write_log(&h, &session.id, &fat.iter().map(String::as_str).collect::<Vec<_>>());

    let (status, body) = split_within(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { limit: Some(500), ..Default::default() },
    ), 16 << 20)
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("events");
    assert!(
        (1..120).contains(&events.len()),
        "the byte budget stopped the page short of the 120 records asked for: {}",
        events.len()
    );
    assert_eq!(
        body["next_after_seq"],
        events.len() as u64,
        "and the cursor is the last record actually returned"
    );

    // The client comes back with it and continues.
    let (_, body) = split_within(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery {
            after_seq: body["next_after_seq"].as_u64(),
            limit: Some(500),
            ..Default::default()
        },
    ), 16 << 20)
    .await;
    assert_eq!(body["events"][0]["seq"], events.len() as u64 + 1);
}

/// R55: paging never skips a record whose *payload* carries a `seq`.
///
/// A `tool_call`'s `data.input` is the model's own JSON for whatever tool it
/// called, so `{"seq": 3}` is an ordinary argument (a cursor, a page, a
/// message id) — and the cursor's cheap pre-scan used to take the first
/// `"seq":` in the line. Records at seq > 3 then vanished from their page and
/// from every later page: a silent, permanent hole in the stream a GUI polls.
/// Both line shapes are exercised — the writer's order and the lexicographic
/// order the log already holds from before the envelope became a struct.
#[tokio::test]
async fn the_event_log_pages_past_a_payload_seq_without_skipping_a_record() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");

    let payload = serde_json::json!({"tool_use_id": "toolu_01", "input": {"seq": 3}});
    let lines: Vec<String> = (1..=25)
        .map(|seq| {
            if seq % 2 == 1 {
                // The writer's order: `v`, `seq`, then the rest.
                format!(
                    r#"{{"v":1,"seq":{seq},"ts":"2026-09-05T10:22:03.114Z","type":"tool_call","data":{payload}}}"#
                )
            } else {
                // The lexicographic order a `serde_json::Map` produced, where
                // `data` precedes the envelope's own `seq`.
                serde_json::to_string(&serde_json::json!({
                    "v": 1,
                    "seq": seq,
                    "ts": "2026-09-05T10:22:03.114Z",
                    "type": "tool_call",
                    "data": payload,
                }))
                .expect("an object of owned values cannot fail to serialise")
            }
        })
        .collect();
    write_log(&h, &session.id, &lines.iter().map(String::as_str).collect::<Vec<_>>());

    let mut seen: Vec<u64> = Vec::new();
    let mut cursor: Option<u64> = None;
    for _ in 0..10 {
        let (status, body) = split(get_session_events(
            &h.deps(),
            &session.id,
            SessionEventsQuery { after_seq: cursor, limit: Some(6), ..Default::default() },
        ))
        .await;
        assert_eq!(status, StatusCode::OK);
        let events = body["events"].as_array().expect("events").clone();
        if events.is_empty() {
            break;
        }
        seen.extend(events.iter().map(|e| e["seq"].as_u64().expect("seq")));
        cursor = body["next_after_seq"].as_u64();
    }
    assert_eq!(
        seen,
        (1..=25).collect::<Vec<u64>>(),
        "every record must appear exactly once, in order"
    );
}

/// A session that has never written a record has no directory (P-22) — an
/// empty page, not a 404 and not an error.
#[tokio::test]
async fn a_session_with_no_log_answers_an_empty_page() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");

    let (status, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery::default(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["events"].as_array().expect("events").is_empty());
    assert_eq!(body["next_after_seq"], 0);
}

/// §5.7's `file_snapshot` is storage-only: nothing new was built to serve it,
/// because the events route already does. Written by the real writer rather
/// than hand-typed, so this pins the whole path — record, reader, route.
#[tokio::test]
async fn the_event_log_serves_a_file_snapshot_record() {
    use openalpaca_core::session_log::{SessionLogLimits, SessionLogService, SnapshotSpec};

    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");

    let work = tempfile::tempdir().expect("workspace");
    let source = work.path().join("report.md");
    std::fs::write(&source, "the old draft").expect("source");

    let service = SessionLogService::new(
        h.sessions_root.clone(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for(&session.id);
    let taken = handle
        .snapshot(SnapshotSpec {
            source,
            path: "docs/report.md".to_string(),
            task_id: None,
            span_id: None,
            agent: Some("lead_agent::a1".to_string()),
        })
        .await
        .expect("the image is taken");
    assert!(handle.flush().await);

    let (status, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { types: Some("file_snapshot".into()), ..Default::default() },
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 1, "{body}");
    assert_eq!(events[0]["type"], "file_snapshot");
    assert_eq!(events[0]["seq"].as_u64(), Some(taken.seq));
    assert_eq!(events[0]["agent"], "lead_agent::a1");
    assert_eq!(events[0]["data"]["path"], "docs/report.md");
    assert_eq!(events[0]["data"]["snapshot_ref"], format!("file:{}", taken.rel));
    assert_eq!(events[0]["data"]["size"], 13);
    assert_eq!(events[0]["data"]["sha256"], taken.sha256);
}

/// An unknown id answers `404 SESSION_NOT_FOUND` — the reason the route was
/// registered before it could be served, and still true now that it is.
#[tokio::test]
async fn an_unknown_session_still_answers_404() {
    let h = Harness::new();
    let (status, body) = split(get_session_events(
        &h.deps(),
        "no-such-session",
        SessionEventsQuery::default(),
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "SESSION_NOT_FOUND");
}

/// `limit` is clamped: a client asking for the whole log gets a page.
#[tokio::test]
async fn the_event_log_clamps_its_page_size() {
    let h = Harness::new();
    let session = h
        .repo()
        .get_or_create_active_session(LANE, "gui", None)
        .expect("session");
    let lines: Vec<String> = (1..=600).map(|i| record(i, "round", "")).collect();
    write_log(&h, &session.id, &lines.iter().map(String::as_str).collect::<Vec<_>>());

    let (_, body) = split(get_session_events(
        &h.deps(),
        &session.id,
        SessionEventsQuery { limit: Some(10_000), ..Default::default() },
    ))
    .await;
    assert_eq!(
        body["events"].as_array().expect("events").len(),
        MAX_EVENTS_LIMIT as usize
    );
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
