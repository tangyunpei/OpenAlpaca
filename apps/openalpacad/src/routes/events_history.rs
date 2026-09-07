//! History endpoint for querying persisted events (GAP-10).
//!
//! ```text
//! GET /v1/events/history?task_id=&agent_id=&event_type=&before=&limit=
//!     -> { events: [EventLog], next_before: <id | null> }
//! ```
//!
//! **The envelope is unconditional** (P20). The rev-1 plan had this route
//! answer a bare array when called without filters and an envelope with them;
//! that dual shape was never built, and it is not being built now — one shape
//! means a client never has to sniff the response to know how to read it. The
//! one CLI consumer (`openalpaca task log`) reads the envelope.
//!
//! **Paging is on the autoincrement `id`, never on `timestamp`.** The column
//! holds two spellings — RFC 3339 for rows the repository wrote, SQLite's
//! space-separated `datetime('now')` for older ones — so it neither sorts nor
//! slices reliably, and rows written inside the same second are not orderable
//! by it at all. `?before=` is an *exclusive* upper bound on `id`, so the
//! boundary row is never served twice.

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use openalpaca_storage::{
    Database, EventLog,
    repository::{EventLogQuery, EventLogRepository},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::api_error;
use crate::AppState;

/// Page size when the caller names none.
pub const DEFAULT_LIMIT: usize = 100;
/// Hard ceiling, applied to whatever the caller asked for.
pub const MAX_LIMIT: usize = 1000;

/// Query parameters for history endpoint
#[derive(Debug, Default, Deserialize)]
pub struct HistoryParams {
    /// The run whose log this is. Matches `event_log.task_id`, the column the
    /// persistence layer fills — never a `detail` blob that happens to mention
    /// a run.
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    /// Exact match; `tool_` does not select `tool_executed`.
    pub event_type: Option<String>,
    /// Exclusive upper bound on `id` — the `next_before` of the page before.
    pub before: Option<i64>,
    pub limit: Option<usize>,
}

/// `GET /v1/events/history` — the only shape this route returns.
#[derive(Debug, Serialize)]
pub struct EventHistoryPage {
    pub events: Vec<EventLog>,
    /// Pass back as `?before=` for the next (older) page; `null` when this
    /// page did not fill, which is as close to "that was the last page" as a
    /// keyset cursor gets.
    pub next_before: Option<i64>,
}

/// `?limit=`, defaulted and clamped. Zero is not a page — it would return
/// nothing forever — so it reads as "unspecified".
pub(crate) fn effective_limit(limit: Option<usize>) -> usize {
    match limit {
        None | Some(0) => DEFAULT_LIMIT,
        Some(n) => n.min(MAX_LIMIT),
    }
}

/// The cursor for the next page: the oldest id on a full page, `None` on a
/// short one.
pub(crate) fn next_before(events: &[EventLog], limit: usize) -> Option<i64> {
    if events.len() < limit {
        return None;
    }
    events.last().map(|event| event.id)
}

/// Assemble one page. Split from the handler so it can be tested without an
/// `AppState`.
pub(crate) fn history_page(
    db: &Database,
    params: &HistoryParams,
) -> anyhow::Result<EventHistoryPage> {
    let limit = effective_limit(params.limit);
    let events = EventLogRepository::new(db).query(&EventLogQuery {
        task_id: params.task_id.as_deref(),
        agent_id: params.agent_id.as_deref(),
        event_type: params.event_type.as_deref(),
        before: params.before,
        limit,
    })?;
    let next_before = next_before(&events, limit);
    Ok(EventHistoryPage {
        events,
        next_before,
    })
}

/// Handle GET /v1/events/history
pub async fn events_history_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> Response {
    match history_page(&state.db, &params) {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(e) => {
            tracing::error!("Failed to query event history: {}", e);
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DB_ERROR",
                format!("Failed to query event history: {e}"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::repository::EventLogRepository;

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    fn params(task_id: Option<&str>, before: Option<i64>, limit: Option<usize>) -> HistoryParams {
        HistoryParams {
            task_id: task_id.map(|t| t.to_string()),
            before,
            limit,
            ..Default::default()
        }
    }

    #[test]
    fn the_limit_defaults_and_clamps() {
        assert_eq!(effective_limit(None), DEFAULT_LIMIT);
        assert_eq!(effective_limit(Some(0)), DEFAULT_LIMIT);
        assert_eq!(effective_limit(Some(7)), 7);
        assert_eq!(effective_limit(Some(10_000)), MAX_LIMIT);
    }

    /// The envelope is returned with **and** without filters (P20) — there is
    /// no bare-array branch to fall into.
    #[test]
    fn the_envelope_is_unconditional() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
            .unwrap();

        let unfiltered = history_page(&db, &params(None, None, None)).unwrap();
        assert_eq!(unfiltered.events.len(), 1);
        assert_eq!(unfiltered.next_before, None);

        let filtered = history_page(&db, &params(Some("t-1"), None, None)).unwrap();
        assert_eq!(filtered.events.len(), 1);

        // Both serialize to the same two keys.
        for page in [&unfiltered, &filtered] {
            let json = serde_json::to_value(page).unwrap();
            assert!(json.get("events").is_some_and(|v| v.is_array()));
            assert!(json.as_object().unwrap().contains_key("next_before"));
        }
    }

    /// `?task_id=` is what makes this a *run's* log: another run's rows, and a
    /// row that only mentions the run inside `detail`, stay out.
    #[test]
    fn the_task_filter_selects_one_run() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
            .unwrap();
        repo.log_for_task("tool_executed", None, Some("t-2"), None, None)
            .unwrap();
        repo.log(
            "workflow_progress",
            None,
            Some(&serde_json::json!({"task_id": "t-1"})),
            None,
        )
        .unwrap();

        let page = history_page(&db, &params(Some("t-1"), None, None)).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].task_id.as_deref(), Some("t-1"));
    }

    /// `?event_type=` composes with the run filter and matches exactly.
    #[test]
    fn the_event_type_filter_is_exact_and_composes() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
            .unwrap();
        repo.log_for_task("task_status", None, Some("t-1"), None, None)
            .unwrap();

        let mut p = params(Some("t-1"), None, None);
        p.event_type = Some("tool_executed".to_string());
        let page = history_page(&db, &p).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event_type, "tool_executed");

        p.event_type = Some("tool_".to_string());
        assert!(history_page(&db, &p).unwrap().events.is_empty());
    }

    /// A full page hands back a cursor; feeding it to `?before=` yields the
    /// next rows with no overlap, and the short final page closes the walk.
    #[test]
    fn the_cursor_walks_the_pages_without_repeating_a_row() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        let ids: Vec<i64> = (0..5)
            .map(|_| {
                repo.log_for_task("tool_executed", None, Some("t-1"), None, None)
                    .unwrap()
            })
            .collect();

        let first = history_page(&db, &params(Some("t-1"), None, Some(2))).unwrap();
        assert_eq!(
            first.events.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[4], ids[3]]
        );
        assert_eq!(first.next_before, Some(ids[3]));

        let second =
            history_page(&db, &params(Some("t-1"), first.next_before, Some(2))).unwrap();
        assert_eq!(
            second.events.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[2], ids[1]]
        );

        let third =
            history_page(&db, &params(Some("t-1"), second.next_before, Some(2))).unwrap();
        assert_eq!(
            third.events.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![ids[0]]
        );
        assert_eq!(third.next_before, None, "a short page ends the walk");
    }

    /// P9 (Phase 8): `DagNodeStatus`/`ServerEvent::DagNodeStatus` were deleted,
    /// but `event_type` is a plain string column, never a deserialized
    /// `ServerEvent` — so a row a pre-deletion daemon wrote with
    /// `event_type = "dag_node_status"` stays perfectly readable history: it
    /// lists, pages, and filters exactly like any other row, without needing
    /// to know the daemon no longer emits that string.
    #[test]
    fn a_legacy_dag_node_status_row_still_lists() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        repo.log_for_task(
            "dag_node_status",
            Some("review_agent"),
            Some("t-1"),
            Some(&serde_json::json!({
                "task_id": "t-1",
                "node_id": "node-1",
                "agent_id": "review_agent",
                "status": "completed",
                "duration_ms": 4200,
            })),
            None,
        )
        .unwrap();

        let page = history_page(&db, &params(Some("t-1"), None, None)).unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event_type, "dag_node_status");

        // The route serializes `event_type` as a bare string (never the
        // `ServerEvent` enum), so an old row round-trips through JSON with no
        // panic and no special-casing.
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["events"][0]["event_type"], "dag_node_status");

        // `?event_type=` composes with the legacy string exactly like any
        // current one.
        let mut p = params(Some("t-1"), None, None);
        p.event_type = Some("dag_node_status".to_string());
        assert_eq!(history_page(&db, &p).unwrap().events.len(), 1);
    }

    /// A run with nothing logged is an empty page, not an error and not a
    /// cursor that would loop.
    #[test]
    fn an_unknown_run_is_an_empty_page() {
        let (_dir, db) = test_db();
        let page = history_page(&db, &params(Some("nope"), None, None)).unwrap();
        assert!(page.events.is_empty());
        assert_eq!(page.next_before, None);
    }

    /// `next_before` is a property of the page, not of the store: exactly
    /// `limit` rows means "there may be more".
    #[test]
    fn a_page_that_exactly_fills_still_offers_a_cursor() {
        let (_dir, db) = test_db();
        let repo = EventLogRepository::new(&db);
        let a = repo
            .log_for_task("tool_executed", None, Some("t-1"), None, None)
            .unwrap();
        let b = repo
            .log_for_task("tool_executed", None, Some("t-1"), None, None)
            .unwrap();

        let page = history_page(&db, &params(Some("t-1"), None, Some(2))).unwrap();
        assert_eq!(
            page.events.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![b, a]
        );
        assert_eq!(page.next_before, Some(a));

        // …and the follow-up page is simply empty.
        let next = history_page(&db, &params(Some("t-1"), page.next_before, Some(2))).unwrap();
        assert!(next.events.is_empty());
        assert_eq!(next.next_before, None);
    }
}
