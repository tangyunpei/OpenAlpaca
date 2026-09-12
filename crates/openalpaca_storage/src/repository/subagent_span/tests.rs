use super::*;
use crate::repository::TaskRepository;
use crate::{Task, TaskStatus};
use chrono::{DateTime, Utc};

fn setup_db() -> Database {
    let dir = tempfile::tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

fn make_task(db: &Database, id: &str, status: TaskStatus) {
    let now = Utc::now();
    let task = Task {
        id: id.to_string(),
        title: format!("task {id}"),
        description: None,
        status,
        priority: 0,
        progress_current: None,
        progress_total: None,
        result_summary: None,
        created_by: "tester".to_string(),
        source_lane: "user:cli".to_string(),
        created_at: now,
        updated_at: now,
        completed_at: None,
        state_json: None,
        state_version: 0,
        outcome_json: None,
        outcome_kind: None,
        artifact_count: 0,
        workspace_id: None,
        source_task_id: None,
        session_id: None,
    };
    TaskRepository::new(db).create(&task).unwrap();
    if status.is_terminal() {
        TaskRepository::new(db).update_status(id, status).unwrap();
    }
}

fn open(db: &Database, task_id: &str, span_id: &str, template_id: &str) -> SubagentSpanRecord {
    SubagentSpanRepository::new(db)
        .open(NewSubagentSpan {
            id: span_id,
            task_id,
            template_id,
            agent_instance_id: &format!("{template_id}::{span_id}"),
            objective: Some("do the thing"),
        })
        .unwrap()
}

#[test]
fn an_open_span_is_running_with_no_end() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);

    let span = open(&db, "t1", "n1", "research_agent");

    assert_eq!(span.id, "n1");
    assert_eq!(span.task_id, "t1");
    assert_eq!(span.template_id, "research_agent");
    assert_eq!(span.agent_instance_id, "research_agent::n1");
    assert_eq!(span.state, SpanState::Running.as_str());
    assert_eq!(span.objective.as_deref(), Some("do the thing"));
    assert!(span.ended_at.is_none());
    assert!(span.duration_ms.is_none());
    assert!(span.detail.is_none());
    assert!(!span.started_at.is_empty());
}

#[test]
fn labels_are_short_template_and_ordinal_unique_per_task() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    make_task(&db, "t2", TaskStatus::Running);

    assert_eq!(open(&db, "t1", "a", "review_agent").label, "review·1");
    assert_eq!(open(&db, "t1", "b", "review_agent").label, "review·2");
    // A different template gets its own ordinal series.
    assert_eq!(open(&db, "t1", "c", "writing_agent").label, "writing·1");
    // Ordinals are per task, so a second run starts over at 1.
    assert_eq!(open(&db, "t2", "d", "review_agent").label, "review·1");
}

#[test]
fn the_label_short_form_drops_the_agent_suffix_and_extra_segments() {
    assert_eq!(short_template("review_agent"), "review");
    assert_eq!(short_template("lead_agent"), "lead");
    assert_eq!(short_template("code_reviewer"), "code");
    assert_eq!(short_template("research"), "research");
    assert_eq!(short_template("Data-Analyst"), "data");
    assert_eq!(short_template("_agent"), "agent");
    assert_eq!(short_template(""), "agent");
    assert_eq!(
        short_template("averyveryverylongtemplatename"),
        "averyveryver"
    );
}

#[test]
fn closing_a_span_stamps_state_end_and_duration() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    let repo = SubagentSpanRepository::new(&db);
    open(&db, "t1", "n1", "review_agent");

    let closed = repo
        .close("n1", SpanState::Done, None, Some("the answer"))
        .unwrap()
        .expect("span exists");

    assert_eq!(closed.state, "done");
    assert_eq!(closed.output_preview.as_deref(), Some("the answer"));
    assert!(closed.ended_at.is_some());
    assert!(closed.duration_ms.is_some_and(|ms| ms >= 0));
    assert_eq!(closed.label, "review·1");
}

#[test]
fn closing_a_cancelled_span_keeps_its_detail() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    let repo = SubagentSpanRepository::new(&db);
    open(&db, "t1", "n1", "review_agent");

    let closed = repo
        .close(
            "n1",
            SpanState::Cancelled,
            Some("Cancelled before starting"),
            None,
        )
        .unwrap()
        .unwrap();

    assert_eq!(closed.state, "cancelled");
    assert_eq!(closed.detail.as_deref(), Some("Cancelled before starting"));
}

#[test]
fn closing_an_unknown_span_is_none_not_an_error() {
    let db = setup_db();
    let repo = SubagentSpanRepository::new(&db);
    assert!(
        repo.close("nope", SpanState::Done, None, None)
            .unwrap()
            .is_none()
    );
}

#[test]
fn closing_twice_keeps_the_first_close() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    let repo = SubagentSpanRepository::new(&db);
    open(&db, "t1", "n1", "review_agent");

    let first = repo
        .close("n1", SpanState::Done, None, None)
        .unwrap()
        .unwrap();
    let second = repo
        .close("n1", SpanState::Failed, Some("late"), None)
        .unwrap();

    assert!(second.is_none(), "a closed span is not re-closed");
    let listed = repo.list_for_task("t1").unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, "done");
    assert_eq!(listed[0].ended_at, first.ended_at);
}

#[test]
fn spans_list_in_start_order() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    make_task(&db, "t2", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");
    open(&db, "t1", "n2", "writing_agent");
    open(&db, "t2", "n3", "review_agent");

    let rows = SubagentSpanRepository::new(&db)
        .list_for_task("t1")
        .unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["n1", "n2"]);
}

#[test]
fn close_orphans_cancels_running_spans_on_terminal_tasks_only() {
    let db = setup_db();
    make_task(&db, "live", TaskStatus::Running);
    make_task(&db, "dead", TaskStatus::Failed);
    open(&db, "live", "n-live", "review_agent");
    open(&db, "dead", "n-dead", "review_agent");

    let repo = SubagentSpanRepository::new(&db);
    let closed = repo.close_orphans().unwrap();
    assert_eq!(closed, 1);

    let dead = &repo.list_for_task("dead").unwrap()[0];
    assert_eq!(dead.state, "cancelled");
    assert_eq!(dead.detail.as_deref(), Some("interrupted"));
    assert!(dead.ended_at.is_some());
    assert!(dead.duration_ms.is_some());

    let live = &repo.list_for_task("live").unwrap()[0];
    assert_eq!(live.state, "running");

    // Idempotent: a second boot closes nothing.
    assert_eq!(repo.close_orphans().unwrap(), 0);
}

/// §5.6b changed what the task sweep writes — `interrupted`, not `failed` —
/// and this pass runs immediately after it. An `interrupted` run is exactly
/// the crash case whose spans are stranded, so it must be in the terminal set.
#[test]
fn close_orphans_treats_an_interrupted_run_as_terminal() {
    let db = setup_db();
    make_task(&db, "crashed", TaskStatus::Interrupted);
    open(&db, "crashed", "n-crashed", "review_agent");

    let repo = SubagentSpanRepository::new(&db);
    assert_eq!(repo.close_orphans().unwrap(), 1);
    let span = &repo.list_for_task("crashed").unwrap()[0];
    assert_eq!(span.state, "cancelled");
    assert_eq!(span.detail.as_deref(), Some("interrupted"));
}

#[test]
fn deleting_a_task_cascades_to_its_spans() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");

    db.with_connection(|conn| {
        conn.execute("DELETE FROM task WHERE id = 't1'", [])?;
        Ok(())
    })
    .unwrap();

    assert!(
        SubagentSpanRepository::new(&db)
            .list_for_task("t1")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_span_for_an_unknown_task_is_refused() {
    let db = setup_db();
    let err = SubagentSpanRepository::new(&db).open(NewSubagentSpan {
        id: "n1",
        task_id: "ghost",
        template_id: "review_agent",
        agent_instance_id: "review_agent::n1",
        objective: None,
    });
    assert!(err.is_err(), "the FK to task(id) is enforced");
}

// ── Run counts per template (GAP-20, T48) ──────────────────────────

/// Backdates a span's `started_at` directly — the only way to put a row
/// outside a `since` cutoff, since [`open`] always stamps "now".
fn set_started_at(db: &Database, span_id: &str, started_at: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "UPDATE subagent_span SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![started_at, span_id],
        )?;
        Ok(())
    })
    .unwrap();
}

/// One grouped query answers the whole template list: every *completed* span
/// the template opened within the window, and when the newest of those
/// started. A run still in flight is not counted — the plan's spec is
/// "completed runs", not "runs, including the ones still going" (T48; that
/// wording was this repository's own P8 interim, now closed).
#[test]
fn run_counts_group_every_completed_span_by_its_template() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    make_task(&db, "t2", TaskStatus::Completed);
    open(&db, "t1", "n1", "review_agent");
    open(&db, "t1", "n2", "review_agent"); // stays running
    open(&db, "t2", "n3", "review_agent");
    let writing = open(&db, "t2", "n4", "writing_agent");

    let repo = SubagentSpanRepository::new(&db);
    repo.close("n1", SpanState::Done, None, None).unwrap();
    repo.close("n3", SpanState::Failed, None, None).unwrap();
    repo.close("n4", SpanState::Cancelled, None, None).unwrap();

    let counts = repo.run_counts_by_template(None).unwrap();

    assert_eq!(counts.len(), 2, "one entry per template, not per span");
    let review = counts.get("review_agent").expect("review_agent counted");
    // n1 (done) and n3 (failed) count; n2 (still running) does not.
    assert_eq!(review.run_count, 2);
    let writing_count = counts.get("writing_agent").expect("writing_agent counted");
    assert_eq!(writing_count.run_count, 1, "a cancelled run still counts");
    assert_eq!(
        writing_count.last_run_at.as_deref(),
        Some(writing.started_at.as_str()),
        "last_run_at is the newest completed span's start"
    );
    // A template that never ran has no entry at all: the caller reports 0
    // rather than this query inventing a row.
    assert!(counts.get("research_agent").is_none());
}

/// A template whose only spans are still running has no *completed* run at
/// all, so it is absent from the map the same way a template with zero spans
/// is — not present with `run_count: 0` baked into a row, which would be this
/// query inventing an entry for it.
#[test]
fn a_template_with_only_running_spans_is_absent_from_the_map() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");

    let counts = SubagentSpanRepository::new(&db)
        .run_counts_by_template(None)
        .unwrap();

    assert!(counts.get("review_agent").is_none());
}

/// `since` is the cutoff `?window=7d|30d` compiles down to: a completed span
/// started before it does not count, and does not win `last_run_at` either.
#[test]
fn run_counts_respect_a_since_cutoff() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    let old = open(&db, "t1", "n-old", "review_agent");
    let recent = open(&db, "t1", "n-recent", "review_agent");
    let repo = SubagentSpanRepository::new(&db);
    repo.close("n-old", SpanState::Done, None, None).unwrap();
    repo.close("n-recent", SpanState::Done, None, None).unwrap();
    set_started_at(&db, "n-old", "2000-01-01T00:00:00.000Z");
    let _ = old; // the span's return value is not needed once backdated

    let cutoff = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let counts = repo.run_counts_by_template(Some(cutoff)).unwrap();

    let review = counts.get("review_agent").expect("counted");
    assert_eq!(
        review.run_count, 1,
        "the span before the cutoff is excluded"
    );
    assert_eq!(
        review.last_run_at.as_deref(),
        Some(recent.started_at.as_str())
    );
}

/// `since: None` is `?window=all` — no time filter, only the completed-only
/// one. Same table as the cutoff test, opposite pole.
#[test]
fn run_counts_with_no_cutoff_include_every_completed_span_ever() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    open(&db, "t1", "n-old", "review_agent");
    open(&db, "t1", "n-recent", "review_agent");
    let repo = SubagentSpanRepository::new(&db);
    repo.close("n-old", SpanState::Done, None, None).unwrap();
    repo.close("n-recent", SpanState::Done, None, None).unwrap();
    set_started_at(&db, "n-old", "2000-01-01T00:00:00.000Z");

    let counts = repo.run_counts_by_template(None).unwrap();
    assert_eq!(counts.get("review_agent").unwrap().run_count, 2);
}

/// An empty table is an empty map, not an error — a fresh install's Agents
/// panel shows every template with `0 runs`.
#[test]
fn run_counts_on_an_empty_table_are_empty() {
    let db = setup_db();
    assert!(
        SubagentSpanRepository::new(&db)
            .run_counts_by_template(None)
            .unwrap()
            .is_empty()
    );
}

// ── Span counts per task (the list route's AGENTS signal) ─────────

/// One grouped query over a whole page of task ids, the same shape
/// `LlmUsageRepository::cost_for_tasks` uses for the page's costs — never one
/// query per row, which is what the deleted `agent_task_history` summary cost.
#[test]
fn span_counts_group_every_span_by_its_task() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    make_task(&db, "t2", TaskStatus::Completed);
    make_task(&db, "t3", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");
    open(&db, "t1", "n2", "writing_agent");
    open(&db, "t2", "n3", "review_agent");

    // A closed span still counts, and so does one still in flight — the
    // number is "agents this run spawned", not "agents still working".
    SubagentSpanRepository::new(&db)
        .close("n1", SpanState::Done, None, None)
        .unwrap();

    let counts = SubagentSpanRepository::new(&db)
        .counts_for_tasks(&[
            "t1".to_string(),
            "t2".to_string(),
            "t3".to_string(),
            "never-existed".to_string(),
        ])
        .unwrap();

    assert_eq!(counts.get("t1").copied(), Some(2));
    assert_eq!(counts.get("t2").copied(), Some(1));
    // A run that spawned nothing is absent, not zero-valued: the caller
    // defaults to 0 rather than this query inventing rows.
    assert!(counts.get("t3").is_none(), "a run with no spans has no entry");
    assert!(counts.get("never-existed").is_none());
}

/// Ids the caller did not ask about are not counted, so a page's numbers
/// cannot borrow another page's spans.
#[test]
fn span_counts_are_scoped_to_the_ids_asked_for() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    make_task(&db, "t2", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");
    open(&db, "t2", "n2", "review_agent");

    let counts = SubagentSpanRepository::new(&db)
        .counts_for_tasks(&["t1".to_string()])
        .unwrap();

    assert_eq!(counts.len(), 1);
    assert_eq!(counts.get("t1").copied(), Some(1));
}

/// An empty page asks no question, so it runs no query.
#[test]
fn span_counts_for_no_tasks_are_empty() {
    let db = setup_db();
    make_task(&db, "t1", TaskStatus::Running);
    open(&db, "t1", "n1", "review_agent");

    assert!(
        SubagentSpanRepository::new(&db)
            .counts_for_tasks(&[])
            .unwrap()
            .is_empty()
    );
}
