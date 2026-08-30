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
