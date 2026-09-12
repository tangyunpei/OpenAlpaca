//! Lazy injection of unprocessed steering leftovers (Routing V2).
//!
//! When a workflow detaches with undelivered steering messages, the spawn
//! converts them to `unprocessed_steering` rows in `lane_followups`
//! (`dispatcher/lead_agent.rs`). They are never auto-run; instead the lane's
//! next main-loop turn surfaces them exactly once as a compact context
//! block, injected per-turn at the same assembly point as the
//! workflow-context block (and outside the compose layers for the same
//! cache-staleness reasons — see `workflow_context.rs`).

use openalpaca_storage::Database;
use openalpaca_storage::repository::{
    FOLLOWUP_KIND_UNPROCESSED_STEERING, FollowupRecord, FollowupRepository,
};

/// At most this many leftover rows surface per turn; the rest stay queued
/// for the following turns.
const MAX_ROWS_PER_TURN: usize = 5;

/// Render the lane's queued `unprocessed_steering` rows as a context block
/// and mark them done, so each row surfaces exactly once. Returns `None`
/// when the lane has none (the common case) or there is no database.
/// `followup`-kind rows are never touched — those belong to the follow-up
/// runner (`FollowupRepository::claim_next`).
pub(crate) fn take_unprocessed_steering_block(
    db: Option<&Database>,
    lane_key: &str,
) -> Option<String> {
    let db = db?;
    let repo = FollowupRepository::new(db);
    let rows = match repo.list_queued_by_lane(lane_key) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(lane_key = %lane_key, "Failed to list lane followups: {e}");
            return None;
        }
    };
    let candidates: Vec<_> = rows
        .into_iter()
        .filter(|r| r.kind == FOLLOWUP_KIND_UNPROCESSED_STEERING)
        .take(MAX_ROWS_PER_TURN)
        .collect();
    claim_and_render(&repo, lane_key, candidates)
}

/// Claim each listed leftover, then render the ones that were still there.
///
/// **Claim before render, never after.** The listing above and this claim are
/// two separate trips through the connection mutex, and
/// `DELETE /v1/lanes/{lane_key}/followups/{id}` CASes the same rows off
/// `queued` in between. Marking them done *after* building the block would put
/// content the user had already cancelled — and seen retired from the queue —
/// in front of the model, and would overwrite the `cancelled` the client was
/// told about with `done`. So each row is claimed first, and only the claims
/// that won are rendered.
///
/// A row that lost is skipped silently: its cancellation was already
/// announced (`FollowupCancelled`), and there is nothing to tell the model
/// about a message that no longer exists. A row whose claim *errored* is also
/// skipped, which leaves it queued for the next turn — better a duplicate than
/// a silently lost instruction, the same trade the previous shape made.
///
/// Split out from [`take_unprocessed_steering_block`] so the gap between the
/// list and the claim is addressable in a test.
fn claim_and_render(
    repo: &FollowupRepository<'_>,
    lane_key: &str,
    candidates: Vec<FollowupRecord>,
) -> Option<String> {
    let mut claimed: Vec<FollowupRecord> = Vec::with_capacity(candidates.len());
    for row in candidates {
        match repo.mark_done_if_queued(row.id) {
            Ok(true) => claimed.push(row),
            // Cancelled (or otherwise moved off `queued`) since the list.
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    lane_key = %lane_key,
                    followup_id = row.id,
                    "Failed to claim unprocessed steering row: {e}"
                );
            }
        }
    }
    if claimed.is_empty() {
        return None;
    }

    let mut block = String::from(
        "<unprocessed_steering>\n\
         Messages the user sent while the last workflow was finishing. They \
         were NOT processed — the workflow ended before they could be \
         delivered, and nothing has acted on them:\n",
    );
    for row in &claimed {
        block.push_str(&format!("- {}\n", row.content));
    }
    block.push_str(
        "Acknowledge them and, where still relevant, act on them now or ask \
         the user how to proceed.\n\
         </unprocessed_steering>",
    );
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::repository::FOLLOWUP_KIND_FOLLOWUP;

    fn setup_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        (dir, db)
    }

    #[test]
    fn test_no_db_and_no_rows_render_nothing() {
        assert_eq!(take_unprocessed_steering_block(None, "user1:cli"), None);

        let (_dir, db) = setup_db();
        assert_eq!(take_unprocessed_steering_block(Some(&db), "user1:cli"), None);
    }

    #[test]
    fn test_leftover_surfaces_once_and_is_marked_done() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        let id = repo
            .queue(
                "user1:cli",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                "focus on unit tests",
                "\"System\"",
                None,
                Some("task-1"),
            )
            .unwrap();

        let block = take_unprocessed_steering_block(Some(&db), "user1:cli")
            .expect("leftover row must render a block");
        assert!(block.starts_with("<unprocessed_steering>"), "{block}");
        assert!(block.ends_with("</unprocessed_steering>"), "{block}");
        assert!(block.contains("NOT processed"), "{block}");
        assert!(block.contains("- focus on unit tests"), "{block}");

        // Row marked done, so the next turn is clean.
        assert_eq!(repo.get(id).unwrap().unwrap().status, "done");
        assert_eq!(take_unprocessed_steering_block(Some(&db), "user1:cli"), None);
    }

    #[test]
    fn test_followup_kind_rows_are_not_injected_or_consumed() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        let followup_id = repo
            .queue(
                "user1:cli",
                FOLLOWUP_KIND_FOLLOWUP,
                "run the benchmarks after",
                "\"System\"",
                None,
                None,
            )
            .unwrap();
        repo.queue(
            "user1:cli",
            FOLLOWUP_KIND_UNPROCESSED_STEERING,
            "also check X",
            "\"System\"",
            None,
            None,
        )
        .unwrap();

        let block = take_unprocessed_steering_block(Some(&db), "user1:cli").unwrap();
        assert!(block.contains("also check X"), "{block}");
        assert!(!block.contains("run the benchmarks after"), "{block}");
        // The followup row stays queued for the follow-up runner.
        assert_eq!(repo.get(followup_id).unwrap().unwrap().status, "queued");
    }

    #[test]
    fn test_cap_five_rows_per_turn_rest_stay_queued() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        for i in 0..7 {
            repo.queue(
                "user1:cli",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                &format!("msg-{i}"),
                "\"System\"",
                None,
                None,
            )
            .unwrap();
        }

        let block = take_unprocessed_steering_block(Some(&db), "user1:cli").unwrap();
        for i in 0..5 {
            assert!(block.contains(&format!("msg-{i}")), "{block}");
        }
        assert!(!block.contains("msg-5"), "{block}");
        assert!(!block.contains("msg-6"), "{block}");

        // The overflow surfaces on the following turn.
        let block2 = take_unprocessed_steering_block(Some(&db), "user1:cli").unwrap();
        assert!(block2.contains("msg-5"), "{block2}");
        assert!(block2.contains("msg-6"), "{block2}");
        assert_eq!(take_unprocessed_steering_block(Some(&db), "user1:cli"), None);
    }

    /// **The race.** `DELETE /v1/lanes/{lane_key}/followups/{id}` cancels a
    /// leftover in the gap between this injector's list and its claim — every
    /// repository call takes and releases the connection separately, so the gap
    /// is real. The cancelled row must not reach the model (the user was told
    /// it was dropped, and the client has already retired the chip) and must
    /// not end up `done` (which would overwrite what was announced).
    #[test]
    fn test_a_row_cancelled_between_list_and_claim_never_reaches_the_model() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        let cancelled = repo
            .queue(
                "user1:cli",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                "drop this one",
                "\"System\"",
                None,
                None,
            )
            .unwrap();
        let survivor = repo
            .queue(
                "user1:cli",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                "keep this one",
                "\"System\"",
                None,
                None,
            )
            .unwrap();

        // What the injector read when it listed the lane…
        let candidates = repo.list_queued_by_lane("user1:cli").unwrap();
        assert_eq!(candidates.len(), 2);
        // …and what the DELETE route did before the injector could claim.
        assert!(repo.cancel_if_queued(cancelled, "user1:cli").unwrap());

        let block =
            claim_and_render(&repo, "user1:cli", candidates).expect("the survivor still renders");
        assert!(!block.contains("drop this one"), "{block}");
        assert!(block.contains("- keep this one"), "{block}");
        assert_eq!(
            repo.get(cancelled).unwrap().unwrap().status,
            "cancelled",
            "the cancelled row stays cancelled"
        );
        assert_eq!(repo.get(survivor).unwrap().unwrap().status, "done");
    }

    /// When every listed row lost its claim there is nothing to say, and the
    /// turn gets no block at all — not an empty one.
    #[test]
    fn test_all_candidates_cancelled_renders_nothing() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        let id = repo
            .queue(
                "user1:cli",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                "the only one",
                "\"System\"",
                None,
                None,
            )
            .unwrap();

        let candidates = repo.list_queued_by_lane("user1:cli").unwrap();
        assert!(repo.cancel_if_queued(id, "user1:cli").unwrap());

        assert_eq!(claim_and_render(&repo, "user1:cli", candidates), None);
        assert_eq!(repo.get(id).unwrap().unwrap().status, "cancelled");
    }

    #[test]
    fn test_other_lanes_untouched() {
        let (_dir, db) = setup_db();
        let repo = FollowupRepository::new(&db);
        let id = repo
            .queue(
                "user2:telegram",
                FOLLOWUP_KIND_UNPROCESSED_STEERING,
                "for another lane",
                "\"System\"",
                None,
                None,
            )
            .unwrap();

        assert_eq!(take_unprocessed_steering_block(Some(&db), "user1:cli"), None);
        assert_eq!(repo.get(id).unwrap().unwrap().status, "queued");
    }
}
