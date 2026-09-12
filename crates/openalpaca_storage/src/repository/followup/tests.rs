use super::*;

fn setup_db() -> Database {
    let dir = tempfile::tempdir().unwrap();
    Database::open(&dir.path().join("test.db")).unwrap()
}

fn queue_item(repo: &FollowupRepository<'_>, lane: &str, kind: &str, content: &str) -> i64 {
    repo.queue(lane, kind, content, "\"System\"", None, Some("task-1"))
        .unwrap()
}

#[test]
fn test_queue_and_list_queued_by_lane() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id1 = repo
        .queue(
            "user:cli",
            FOLLOWUP_KIND_FOLLOWUP,
            "run the benchmarks",
            "{\"User\":{\"global_id\":\"user\"}}",
            Some("/tmp/project"),
            Some("task-abc"),
        )
        .unwrap();
    assert!(id1 > 0);
    let id2 = queue_item(&repo, "user:cli", FOLLOWUP_KIND_UNPROCESSED_STEERING, "also check X");
    queue_item(&repo, "other:cli", FOLLOWUP_KIND_FOLLOWUP, "different lane");

    let rows = repo.list_queued_by_lane("user:cli").unwrap();
    assert_eq!(rows.len(), 2);
    // Oldest first, both kinds listed.
    assert_eq!(rows[0].id, id1);
    assert_eq!(rows[0].kind, FOLLOWUP_KIND_FOLLOWUP);
    assert_eq!(rows[0].content, "run the benchmarks");
    assert_eq!(rows[0].principal_json, "{\"User\":{\"global_id\":\"user\"}}");
    assert_eq!(rows[0].workspace_path.as_deref(), Some("/tmp/project"));
    assert_eq!(rows[0].source_task_id.as_deref(), Some("task-abc"));
    assert_eq!(rows[0].status, "queued");
    assert_eq!(rows[1].id, id2);
    assert_eq!(rows[1].kind, FOLLOWUP_KIND_UNPROCESSED_STEERING);
}

#[test]
fn test_claim_next_cas_queued_to_running() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id1 = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "first");
    let id2 = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "second");

    // First claim gets the oldest row, now running.
    let claimed = repo.claim_next("user:cli").unwrap().unwrap();
    assert_eq!(claimed.id, id1);
    assert_eq!(claimed.status, "running");

    // A claimed row is no longer listed as queued.
    let queued = repo.list_queued_by_lane("user:cli").unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].id, id2);

    // Second claim gets the next row; third finds nothing.
    assert_eq!(repo.claim_next("user:cli").unwrap().unwrap().id, id2);
    assert!(repo.claim_next("user:cli").unwrap().is_none());
}

#[test]
fn test_claim_next_never_claims_unprocessed_steering() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    queue_item(&repo, "user:cli", FOLLOWUP_KIND_UNPROCESSED_STEERING, "leftover");
    assert!(repo.claim_next("user:cli").unwrap().is_none());

    // Still visible on the queued list for lazy next-turn injection.
    let queued = repo.list_queued_by_lane("user:cli").unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].status, "queued");
}

#[test]
fn test_claim_next_is_lane_scoped() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    queue_item(&repo, "other:telegram", FOLLOWUP_KIND_FOLLOWUP, "other lane item");
    assert!(repo.claim_next("user:cli").unwrap().is_none());
    assert!(repo.claim_next("other:telegram").unwrap().is_some());
}

/// The CAS the `DELETE` route needs: cancel wins only while the row is still
/// queued *and* still belongs to the lane the caller addressed. Unconditional
/// `mark_cancelled` would race the autostart claim and mark a row that is
/// already running.
#[test]
fn test_cancel_if_queued_wins_only_on_a_queued_row() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "to cancel");

    assert!(repo.cancel_if_queued(id, "user:cli").unwrap());
    assert_eq!(repo.get(id).unwrap().unwrap().status, "cancelled");

    // Idempotence is not a property of a CAS: the second attempt loses,
    // because the row is no longer queued.
    assert!(!repo.cancel_if_queued(id, "user:cli").unwrap());
    assert_eq!(repo.get(id).unwrap().unwrap().status, "cancelled");
}

/// The race the route reports as `409`: the autostart claimed the row between
/// the caller reading it and pressing cancel. Exactly one of the two CASes
/// wins, and it is the claim — the turn is already running.
#[test]
fn test_cancel_if_queued_loses_to_the_autostart_claim() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "already claimed");
    let claimed = repo.claim_next("user:cli").unwrap().unwrap();
    assert_eq!(claimed.id, id);

    assert!(!repo.cancel_if_queued(id, "user:cli").unwrap());
    assert_eq!(
        repo.get(id).unwrap().unwrap().status,
        "running",
        "a claimed row keeps running — cancel must not overwrite it"
    );
}

/// Lane-scoped, like `claim_next`: naming the wrong lane cancels nothing, so
/// the route can answer `404` for a row that is not this lane's without a
/// second query deciding it.
#[test]
fn test_cancel_if_queued_is_lane_scoped() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "mine");

    assert!(!repo.cancel_if_queued(id, "other:telegram").unwrap());
    assert_eq!(repo.get(id).unwrap().unwrap().status, "queued");

    // An id that does not exist at all is the same "nothing changed".
    assert!(!repo.cancel_if_queued(id + 999, "user:cli").unwrap());
}

/// The claim the lazy `unprocessed_steering` injection needs: a leftover row is
/// consumed only while it is still queued. The injector lists the lane, renders
/// a block, and marks the rows it used — and a cancel can land in between, so
/// the mark has to be a CAS like every other transition off `queued`.
#[test]
fn test_mark_done_if_queued_wins_only_on_a_queued_row() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_UNPROCESSED_STEERING, "leftover");

    assert!(repo.mark_done_if_queued(id).unwrap());
    assert_eq!(repo.get(id).unwrap().unwrap().status, "done");

    // Not idempotent, for the same reason `cancel_if_queued` is not: the row
    // has left `queued`, so the second claim loses.
    assert!(!repo.mark_done_if_queued(id).unwrap());
    assert_eq!(repo.get(id).unwrap().unwrap().status, "done");
}

/// The race it exists to lose: the row was cancelled between the injector's
/// list and its claim. The claim must not resurrect it as `done` — the
/// cancellation has already been announced to the client.
#[test]
fn test_mark_done_if_queued_loses_to_a_cancel() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_UNPROCESSED_STEERING, "never mind");
    assert!(repo.cancel_if_queued(id, "user:cli").unwrap());

    assert!(!repo.mark_done_if_queued(id).unwrap());
    assert_eq!(
        repo.get(id).unwrap().unwrap().status,
        "cancelled",
        "a cancelled row stays cancelled — `done` would overwrite what the user was told"
    );

    // An id that names no row is the same "nothing changed".
    assert!(!repo.mark_done_if_queued(id + 999).unwrap());
}

#[test]
fn test_mark_done_and_cancelled() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let id1 = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "to finish");
    let id2 = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "to cancel");

    let claimed = repo.claim_next("user:cli").unwrap().unwrap();
    assert_eq!(claimed.id, id1);
    repo.mark_done(id1).unwrap();
    repo.mark_cancelled(id2).unwrap();
    assert_eq!(repo.get(id1).unwrap().unwrap().status, "done");
    assert_eq!(repo.get(id2).unwrap().unwrap().status, "cancelled");
    assert!(repo.get(id2 + 999).unwrap().is_none());

    // Neither terminal row is queued or claimable any more.
    assert!(repo.list_queued_by_lane("user:cli").unwrap().is_empty());
    assert!(repo.claim_next("user:cli").unwrap().is_none());
}

// ── §5.3: a follow-up runs in the conversation it was promised in ────

#[test]
fn a_queued_followup_is_pinned_to_the_lanes_active_session() {
    let db = setup_db();
    let conv = crate::ConversationRepository::new(&db);
    let repo = FollowupRepository::new(&db);

    let origin = conv
        .get_or_create_active_session("user:cli", "cli", None)
        .unwrap();
    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "later");

    let row = repo.get(id).unwrap().unwrap();
    assert_eq!(row.session_id.as_deref(), Some(origin.id.as_str()));
}

#[test]
fn claiming_a_followup_reactivates_the_session_it_was_queued_in() {
    let db = setup_db();
    let conv = crate::ConversationRepository::new(&db);
    let repo = FollowupRepository::new(&db);

    let origin = conv
        .get_or_create_active_session("user:cli", "cli", None)
        .unwrap();
    queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "continue that");

    // The user opens a new chat on the same lane before the follow-up runs.
    let usurper = conv.create_session("user:cli", "cli", None, None).unwrap();
    assert_eq!(
        conv.active_session_id("user:cli").unwrap().as_deref(),
        Some(usurper.id.as_str())
    );

    let claimed = repo.claim_next("user:cli").unwrap().unwrap();
    assert_eq!(claimed.status, "running");
    assert_eq!(claimed.session_id.as_deref(), Some(origin.id.as_str()));

    // The turn about to run resolves the lane → the originating session.
    assert_eq!(
        conv.active_session_id("user:cli").unwrap().as_deref(),
        Some(origin.id.as_str()),
        "the follow-up's own conversation is live again"
    );
    assert_eq!(
        conv.get_session(&usurper.id).unwrap().unwrap().status,
        crate::SESSION_ARCHIVED
    );
}

#[test]
fn a_pre_039_followup_row_falls_back_to_the_lanes_active_session() {
    let db = setup_db();
    let conv = crate::ConversationRepository::new(&db);
    let repo = FollowupRepository::new(&db);

    // A row queued before the column existed carries no session.
    let id = queue_item(&repo, "user:cli", FOLLOWUP_KIND_FOLLOWUP, "legacy");
    db.with_connection(|c| {
        c.execute(
            "UPDATE lane_followups SET session_id = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    })
    .unwrap();
    let current = conv
        .get_or_create_active_session("user:cli", "cli", None)
        .unwrap();

    let claimed = repo.claim_next("user:cli").unwrap().unwrap();
    assert!(claimed.session_id.is_none());
    assert_eq!(
        conv.active_session_id("user:cli").unwrap().as_deref(),
        Some(current.id.as_str()),
        "nothing to re-home; the lane's active session is untouched"
    );
}

// ── §5.6b: recovering a crashed run's interjections ──────────────────

fn recovered(content: &str) -> RecoveredSteering {
    RecoveredSteering {
        content: content.to_string(),
        principal_json: "{\"User\":{\"global_id\":\"u-42\"}}".to_string(),
        workspace_path: Some("/repo".to_string()),
    }
}

/// The recovered row is the row the graceful path writes: same kind, same
/// content, same principal, same lane — plus the run's own session, which is
/// the conversation the interjection was made in (§5.3).
#[test]
fn a_recovered_interjection_is_the_row_the_graceful_path_would_have_written() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let ids = repo
        .recover_unprocessed_steering(
            "user:cli",
            "task-1",
            Some("sess-1"),
            &[recovered("check the migration first")],
        )
        .unwrap();
    assert_eq!(ids.len(), 1);

    let row = repo.get(ids[0]).unwrap().unwrap();
    assert_eq!(row.kind, FOLLOWUP_KIND_UNPROCESSED_STEERING);
    assert_eq!(row.content, "check the migration first");
    assert_eq!(row.principal_json, "{\"User\":{\"global_id\":\"u-42\"}}");
    assert_eq!(row.workspace_path.as_deref(), Some("/repo"));
    assert_eq!(row.source_task_id.as_deref(), Some("task-1"));
    assert_eq!(row.session_id.as_deref(), Some("sess-1"));
    assert_eq!(row.status, "queued");
    // It surfaces on the lane's next turn like any other leftover, and is
    // never auto-claimed.
    assert_eq!(repo.list_queued_by_lane("user:cli").unwrap().len(), 1);
    assert!(repo.claim_next("user:cli").unwrap().is_none());
}

/// The idempotence marker is the row's own presence: a second pass over the
/// same log — a crash between the recovery and the status flip, or simply a
/// second boot — adds nothing.
#[test]
fn a_second_recovery_pass_over_the_same_log_adds_nothing() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);
    let items = [recovered("one"), recovered("two")];

    assert_eq!(
        repo.recover_unprocessed_steering("user:cli", "task-1", None, &items)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        repo.recover_unprocessed_steering("user:cli", "task-1", None, &items)
            .unwrap(),
        Vec::<i64>::new()
    );
    assert_eq!(repo.list_queued_by_lane("user:cli").unwrap().len(), 2);
}

/// R56 narrowed this guard from a multiset to a set (see
/// `queue_unprocessed_steering_once`'s doc comment): two interjections with
/// identical text now collapse to one row, even within the same recovery
/// call, because the schema carries no column that would let the guard tell
/// a repeat from a coincidence. That is a deliberate trade against the
/// harder guarantee the narrowing exists to close elsewhere — the very same
/// interjection must never be filed twice by two different call sites —
/// never a *double delivery of one* interjection. Content that actually
/// differs is still one row per message; see `the_guard_does_not_reach_across_runs`
/// and `a_recovered_interjection_is_the_row_the_graceful_path_would_have_written`.
#[test]
fn two_interjections_with_the_same_words_collapse_to_one_row() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);
    let items = [recovered("hurry up"), recovered("hurry up")];

    assert_eq!(
        repo.recover_unprocessed_steering("user:cli", "task-1", None, &items)
            .unwrap()
            .len(),
        1
    );
    // And a re-run of the same scan still adds nothing.
    assert!(
        repo.recover_unprocessed_steering("user:cli", "task-1", None, &items)
            .unwrap()
            .is_empty()
    );
    assert_eq!(repo.list_queued_by_lane("user:cli").unwrap().len(), 1);
}

// ── R56: the shared guarded insert every unprocessed_steering writer uses ──

/// The core of R56's fix: the dropped-record fallback (`runner/steering.rs`)
/// and the graceful-exit leftover conversion (`dispatcher/lead_agent.rs`)
/// both file the same interjection through this one guarded insert. A second
/// call naming the same `(source_task_id, kind, content)` must not produce a
/// second row — that was the bug (one steered instruction shown to the model
/// twice via `<unprocessed_steering>`).
#[test]
fn queue_unprocessed_steering_once_files_an_interjection_only_once() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let first = repo
        .queue_unprocessed_steering_once(
            "user:cli",
            "focus on the tests",
            "\"System\"",
            Some("/repo"),
            "task-1",
            None,
        )
        .unwrap();
    assert!(first.is_some(), "the first call must insert a row");

    let second = repo
        .queue_unprocessed_steering_once(
            "user:cli",
            "focus on the tests",
            "\"System\"",
            Some("/repo"),
            "task-1",
            None,
        )
        .unwrap();
    assert_eq!(
        second, None,
        "a matching row already exists — nothing should be inserted again"
    );

    let rows = repo.list_queued_by_lane("user:cli").unwrap();
    assert_eq!(rows.len(), 1, "exactly one row, not a duplicate: {rows:?}");
    assert_eq!(rows[0].content, "focus on the tests");
}

/// The guard's key is `(source_task_id, kind, content)` — content that
/// actually differs is a different interjection, and both must be filed.
#[test]
fn queue_unprocessed_steering_once_still_inserts_distinct_content() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);

    let first = repo
        .queue_unprocessed_steering_once(
            "user:cli",
            "first message",
            "\"System\"",
            None,
            "task-1",
            None,
        )
        .unwrap();
    let second = repo
        .queue_unprocessed_steering_once(
            "user:cli",
            "second message",
            "\"System\"",
            None,
            "task-1",
            None,
        )
        .unwrap();

    assert!(first.is_some());
    assert!(second.is_some());
    assert_ne!(first, second);
    assert_eq!(repo.list_queued_by_lane("user:cli").unwrap().len(), 2);
}

/// A crash *after* the graceful path filed some of the leftovers: the ones it
/// wrote are not written twice, and the ones it never reached are.
#[test]
fn the_guard_counts_rows_the_graceful_path_already_wrote() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);
    repo.queue(
        "user:cli",
        FOLLOWUP_KIND_UNPROCESSED_STEERING,
        "already filed",
        "\"System\"",
        None,
        Some("task-1"),
    )
    .unwrap();

    let ids = repo
        .recover_unprocessed_steering(
            "user:cli",
            "task-1",
            None,
            &[recovered("already filed"), recovered("never filed")],
        )
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(repo.get(ids[0]).unwrap().unwrap().content, "never filed");
}

/// The guard is scoped to the run. Another run's identical interjection is
/// another run's, and must not suppress this one.
#[test]
fn the_guard_does_not_reach_across_runs() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);
    repo.recover_unprocessed_steering("user:cli", "task-1", None, &[recovered("same words")])
        .unwrap();
    let ids = repo
        .recover_unprocessed_steering("user:cli", "task-2", None, &[recovered("same words")])
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(repo.list_queued_by_lane("user:cli").unwrap().len(), 2);
}

/// A row the user cancelled, or one already surfaced (`done`), still counts:
/// the guard is "was this interjection ever filed", not "is it still queued".
/// Re-queueing a cancelled message would put back what the user retired.
#[test]
fn a_cancelled_or_surfaced_row_is_not_recovered_again() {
    let db = setup_db();
    let repo = FollowupRepository::new(&db);
    let ids = repo
        .recover_unprocessed_steering(
            "user:cli",
            "task-1",
            None,
            &[recovered("cancelled one"), recovered("surfaced one")],
        )
        .unwrap();
    repo.mark_cancelled(ids[0]).unwrap();
    repo.mark_done(ids[1]).unwrap();

    assert!(
        repo.recover_unprocessed_steering(
            "user:cli",
            "task-1",
            None,
            &[recovered("cancelled one"), recovered("surfaced one")],
        )
        .unwrap()
        .is_empty()
    );
}
