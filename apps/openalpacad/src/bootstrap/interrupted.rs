//! The boot pass that turns a crash into an honest record (§5.6b).
//!
//! Two things happen to a run the previous incarnation was driving:
//!
//! 1. its undelivered interjections are read back out of the session log and
//!    filed as `lane_followups(kind='unprocessed_steering')` — the exact rows
//!    the graceful path writes (`dispatcher/lead_agent.rs`), so a `kill -9` no
//!    longer silently eats what the user typed while the workflow ran;
//! 2. its row is marked `interrupted`
//!    ([`TaskRepository::interrupt_all_non_terminal`]), which is terminal and
//!    carries `rerun` as its restart.
//!
//! **In that order, and the order is load-bearing twice.**
//!
//! *Recovery before the flip.* The scan is driven off
//! [`TaskRepository::list_non_terminal`], so a crash between the two leaves
//! the runs still non-terminal and the next boot scans them again — and the
//! recovery's own guard (the follow-up row's presence, an atomic
//! count-then-insert) means that second scan adds nothing. The other order
//! would lose an interjection outright in the same window, and the codebase's
//! standing trade for this queue is "better a duplicate than a silently lost
//! instruction" (`query_handler/unprocessed_steering.rs`).
//!
//! *Recovery before the byte-cap eviction.* T42's `sweep::enforce_total_cap`
//! runs in `services::initialize_services` and may evict an archived session's
//! whole log, including the live segment (R54). The interjection exists only
//! in that file, so the scan has to read it while it is still there. This pass
//! runs in `main.rs` step 3, before `initialize_services` and — the standing
//! call-order guarantee it inherits from the sweep it replaced — before any
//! ingress can create work this incarnation owns.
//!
//! Non-fatal throughout: a daemon that cannot read a log still boots, and says
//! what it could not recover rather than pretending there was nothing.

use openalpaca_core::session_log;
use openalpaca_storage::Database;
use openalpaca_storage::repository::{
    FollowupRepository, NonTerminalRun, RecoveredSteering, TaskRepository,
};
use std::path::Path;

/// One run this boot found in flight and marked `interrupted`.
///
/// Returned so `main.rs` can announce it once the event bridge exists —
/// publishing at step 3 would broadcast into a bus nothing is subscribed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptedRun {
    pub task_id: String,
    /// The conversation the run was started from. `None` for a run dispatched
    /// on a lane with no session row (and for every pre-039 row) — nothing to
    /// scan and nothing to announce.
    pub session_id: Option<String>,
    pub lane_key: String,
    /// How many interjections this run had that the loop never delivered, and
    /// that were filed as follow-ups by this pass.
    pub recovered_steering: usize,
}

/// Recover, then mark. Returns the runs that were marked, newest information
/// first for the caller's log line.
///
/// `sessions_root` is the home store's `sessions/`; `None` when the store
/// could not be resolved, in which case the rows are still marked (a crash is
/// still a crash) and the recovery is skipped with a warning.
pub fn sweep_interrupted_runs(
    db: &Database,
    sessions_root: Option<&Path>,
    instance_id: &str,
) -> Vec<InterruptedRun> {
    let tasks = TaskRepository::new(db);
    let live = match tasks.list_non_terminal() {
        Ok(live) => live,
        Err(e) => {
            tracing::warn!("Startup interruption sweep failed (non-fatal): {e}");
            return Vec::new();
        }
    };
    if live.is_empty() {
        // The ordinary boot: nothing was in flight, so nothing is read.
        return Vec::new();
    }

    let mut runs: Vec<InterruptedRun> = live
        .iter()
        .map(|run| InterruptedRun {
            task_id: run.id.clone(),
            session_id: run.session_id.clone(),
            lane_key: run.lane_key.clone(),
            recovered_steering: 0,
        })
        .collect();

    match sessions_root {
        Some(root) => {
            for (run, out) in live.iter().zip(runs.iter_mut()) {
                out.recovered_steering = recover_one(db, root, run);
            }
        }
        None => tracing::warn!(
            runs = live.len(),
            "Steering recovery skipped — no sessions directory; \
             an interjection left undelivered by the crash cannot be read back"
        ),
    }

    let detail = format!("interrupted — the daemon restarted (instance {instance_id})");
    match tasks.interrupt_all_non_terminal(&detail) {
        Ok(count) => {
            let recovered: usize = runs.iter().map(|r| r.recovered_steering).sum();
            tracing::info!(
                interrupted = count,
                recovered_steering = recovered,
                "Startup interruption sweep"
            );
        }
        // The rows stay non-terminal, so the next boot tries again — and the
        // recovery it re-runs is idempotent, which is why this order is safe.
        Err(e) => tracing::warn!("Startup interruption sweep failed (non-fatal): {e}"),
    }
    runs
}

/// Read one run's undelivered interjections and file them. Returns how many
/// rows were written.
fn recover_one(db: &Database, sessions_root: &Path, run: &NonTerminalRun) -> usize {
    let Some(session_id) = run.session_id.as_deref() else {
        return 0;
    };
    let dir = sessions_root.join(session_log::session_dir_name(session_id));
    let scan = match session_log::recovery::undrained_steering(&dir, &run.id) {
        Ok(scan) => scan,
        Err(e) => {
            tracing::warn!(
                task_id = %run.id,
                session_id = %session_id,
                "Could not read the run's session log for undelivered steering: {e}"
            );
            return 0;
        }
    };
    if scan.unrecoverable > 0 {
        // Named rather than dropped in silence: these are records from a build
        // that did not carry the principal the follow-up row requires.
        tracing::warn!(
            task_id = %run.id,
            count = scan.unrecoverable,
            "Undelivered steering messages could not be recovered — the log \
             record carries no principal"
        );
    }
    if scan.undrained.is_empty() {
        return 0;
    }

    let items: Vec<RecoveredSteering> = scan
        .undrained
        .iter()
        .map(|u| RecoveredSteering {
            content: u.text.clone(),
            principal_json: u.principal_json.clone(),
            workspace_path: u.workspace_path.clone(),
        })
        .collect();
    match FollowupRepository::new(db).recover_unprocessed_steering(
        &run.lane_key,
        &run.id,
        Some(session_id),
        &items,
    ) {
        Ok(ids) => {
            if !ids.is_empty() {
                tracing::info!(
                    task_id = %run.id,
                    lane_key = %run.lane_key,
                    recovered = ids.len(),
                    "Recovered undelivered steering messages from the session log"
                );
            }
            ids.len()
        }
        Err(e) => {
            tracing::warn!(
                task_id = %run.id,
                "Failed to file recovered steering messages as follow-ups: {e}"
            );
            0
        }
    }
}

/// Announce what the sweep found, once the event bridge exists.
///
/// Split from [`sweep_interrupted_runs`] because that pass runs at step 3 —
/// before the `EventBus` and its bridge are built at step 7 — and a publish
/// into a bus with no subscriber is a frame nobody persists and nobody sees.
///
/// One frame per run, on `SessionChanged` rather than a variant of its own:
/// `interrupted` is a fact about a run that a *conversation* has to show (its
/// `interrupted_task_count` badge just moved), and `task_id` names the run.
/// A run with no session has no conversation to announce it in.
pub fn announce_interrupted(bus: &openalpaca_core::bus::EventBus, runs: &[InterruptedRun]) {
    for run in runs {
        let Some(session_id) = run.session_id.as_deref() else {
            continue;
        };
        bus.publish(openalpaca_core::events::SystemEvent::SessionChanged {
            session_id: session_id.to_string(),
            lane_key: run.lane_key.clone(),
            status: openalpaca_storage::TaskStatus::Interrupted.as_str().to_string(),
            task_id: Some(run.task_id.clone()),
            timestamp: chrono::Utc::now(),
        });
    }
}
