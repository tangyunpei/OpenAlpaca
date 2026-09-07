//! The three ways a *stored* task row becomes a live run (GAP-06, §5.6c).
//!
//! All three start from a row in `task` rather than from a message, and they
//! are deliberately asymmetric:
//!
//! * [`Orchestrator::rerun_task`] copies a finished run's goal onto a **new
//!   id** and dispatches that (`201`). The original row is untouched — it is
//!   the thing the user is re-running *against* — and the copy records where it
//!   came from in `task.source_task_id`.
//! * [`Orchestrator::start_task`] runs a row **under its own id** (`200`). This
//!   is settled decision D5: a client that queued a task through
//!   `POST /v1/tasks` is already holding that id, and handing it a different
//!   one back would mean every reference it stored is now to the wrong row.
//!   Because it re-launches the row *in place*, it takes only rows that have
//!   not finished (R43) — a finished one would lose its result.
//! * [`Orchestrator::resume_task`] is §5.6c's opt-in **replay resume** (S2).
//!   It takes only an `interrupted` row — the one terminal status that means
//!   "this run did not choose to stop" — re-enters it under its own id like
//!   `start`, and hands the loop the history rebuilt from the run's session
//!   log ([`session_log::replay`](crate::session_log::replay)) plus a
//!   synthetic interjection saying what happened. It is gated on
//!   `[orchestrator.routing] resume_enabled`, which ships **off**: the plan
//!   calls S2 its one speculative piece and `rerun` is the trusted fallback.
//!
//! None of the three is a chat turn: the caller addresses a run, so nothing
//! here goes through the gateway, the lane's history, or the model.

use super::Orchestrator;
use crate::lane::LaneKey;
use crate::memory::scope_context::MemoryScopeContext;
use crate::runner::LoopConfig;
use crate::session_log::replay::{ReplayPlan, ResumeSeed, rebuild};
use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::{Task, TaskStatus};

/// Why a re-run or a start could not happen. Each caller formats its own
/// response — the daemon route maps these one-to-one onto status codes.
#[derive(Debug, PartialEq, Eq)]
pub enum TaskLaunchError {
    /// No such row — or (the route's own check) not this caller's run.
    NotFound,
    /// `rerun` on a run that has not finished. Re-running work that is still
    /// in flight would put two agents on the same goal; steer or cancel it.
    NotTerminal { current: &'static str },
    /// The row carries no description, so there is no goal to dispatch. A
    /// `POST /v1/tasks` row may legitimately be title-only.
    NoDescription,
    /// `start` on a run that has already finished (R43). `start` re-launches a
    /// row **in place**, and a finished row's summary, outcome and artifact
    /// count are the only record that run ever happened; `rerun` is the verb
    /// that runs the goal again without spending them.
    NotStartable { current: &'static str },
    /// `start` on an id that already has a live run.
    AlreadyRunning,
    /// `resume` while `[orchestrator.routing] resume_enabled` is off — which
    /// is the shipped default (§5.6c: S2 is opt-in until it is trusted).
    ResumeDisabled,
    /// `resume` on a run that is not `interrupted`. Every other status either
    /// chose to stop (`rerun` is the verb) or has not stopped at all.
    NotResumable { current: &'static str },
    /// `resume` with nothing to re-prime from: the run recorded no session,
    /// its log was evicted by the byte-cap sweep, or what survives holds no
    /// complete round of this run. §5.6c's "a gutted log is a clean 409
    /// pointing at `rerun`".
    ResumeLogMissing,
    /// Database read failure.
    Db(String),
    /// The dispatcher refused — no agent template was free to lead the run.
    Dispatch(String),
}

/// What a re-run produced: a different run, and the one it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunOutcome {
    /// The **new** run's id.
    pub task_id: String,
    /// The run it was copied from.
    pub source_task_id: String,
    pub title: String,
    /// See [`StartOutcome::status`].
    pub status: String,
}

/// What a `start` produced: the same id, now dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartOutcome {
    pub task_id: String,
    pub title: String,
    /// The run's status as the registry holds it the instant the dispatch
    /// returned — `queued`, or already `running` if the background task got
    /// there first. Both are true answers to "what happened?"; neither is a
    /// promise about the round after this one.
    pub status: String,
}

/// What a `resume` produced: the same id, dispatched over a rebuilt history.
///
/// The replay's own numbers travel back with it so a client (and the report
/// in the log) can say *how much* of the run was recovered — "resumed" with
/// no rounds behind it would be indistinguishable from a fresh start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOutcome {
    pub task_id: String,
    pub title: String,
    /// See [`StartOutcome::status`].
    pub status: String,
    /// The conversation the replayed history came from.
    pub session_id: String,
    pub rounds_replayed: usize,
    /// The slice of the log the history was rebuilt from.
    pub from_seq: Option<u64>,
    pub to_seq: Option<u64>,
}

/// The goal, the lane and the project a stored row is re-launched with.
struct RunPlan {
    description: String,
    title: String,
    created_by: String,
    lane_key: String,
    source: String,
    workspace: MemoryScopeContext,
}

impl RunPlan {
    /// Reconstruct a dispatch from the row that recorded one.
    ///
    /// `source` comes out of the lane key (`"{user_id}:{source}"`) rather than
    /// being invented: a run re-launched from the GUI still belongs to the lane
    /// it was started on, and its completion report is posted there.
    fn from_row(task: &Task) -> Result<Self, TaskLaunchError> {
        let description = task
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .ok_or(TaskLaunchError::NoDescription)?
            .to_string();
        let source = LaneKey::from_str(&task.source_lane)
            .map(|key| key.source)
            .unwrap_or_else(|| "internal".to_string());
        Ok(Self {
            description,
            title: task.title.clone(),
            created_by: task.created_by.clone(),
            lane_key: task.source_lane.clone(),
            source,
            // The row's `workspace_id` is the workspace the *request* supplied
            // (R22), which is exactly what `for_request` takes — so a re-launch
            // resolves its project the same way the original turn did, home
            // fold included, and a row with no project falls back to memory
            // scoping alone just as that turn did.
            workspace: MemoryScopeContext::for_request(task.workspace_id.as_deref()),
        })
    }
}

impl Orchestrator {
    /// Read a row, or say why it cannot be read.
    fn launchable_row(&self, task_id: &str) -> Result<Task, TaskLaunchError> {
        let db = self.db.as_ref().ok_or(TaskLaunchError::NotFound)?;
        TaskRepository::new(db)
            .get(task_id)
            .map_err(|e| TaskLaunchError::Db(e.to_string()))?
            .ok_or(TaskLaunchError::NotFound)
    }

    /// GAP-06's `rerun` — dispatch a **new** run from a finished one's goal.
    ///
    /// Refuses a run that has not finished (`NotTerminal`) and one with nothing
    /// to re-dispatch (`NoDescription`). On success the new row carries
    /// `source_task_id = task_id`, so the link survives a restart the way the
    /// runs themselves do.
    pub fn rerun_task(&self, task_id: &str) -> Result<RerunOutcome, TaskLaunchError> {
        let row = self.launchable_row(task_id)?;
        if !row.status.is_terminal() {
            return Err(TaskLaunchError::NotTerminal {
                current: row.status.as_str(),
            });
        }
        let plan = RunPlan::from_row(&row)?;

        let outcome = self
            .task_dispatcher
            .dispatch_lead_agent_rerun(
                &plan.description,
                plan.title,
                &plan.created_by,
                &plan.lane_key,
                &plan.source,
                plan.workspace,
                &row.id,
            )
            .map_err(TaskLaunchError::Dispatch)?;

        Ok(RerunOutcome {
            status: self.dispatched_status(&outcome.task_id),
            task_id: outcome.task_id,
            source_task_id: row.id,
            title: outcome.title,
        })
    }

    /// D5's `start` — dispatch a stored row under its own id.
    ///
    /// Refuses a run that has already finished (`NotStartable`, R43) and one
    /// with nothing to dispatch (`NoDescription`). The run slot is then claimed
    /// before anything else happens, so two simultaneous starts cannot both
    /// dispatch; a claim that fails is the `AlreadyRunning` answer, and a
    /// dispatch that fails releases it.
    ///
    /// The terminal check comes first for the same reason `rerun`'s does: a
    /// re-launch under this id goes through `TaskRepository::upsert_queued`,
    /// which resets the row to a fresh queued run — summary, outcome, artifact
    /// count, `completed_at` and `state_version` all cleared. On a finished run
    /// that is the destruction of the only record it left, with nothing (not
    /// even `source_task_id`, which a `start` never sets) to say a first run
    /// happened. `rerun` exists so this is never the way to run a goal twice.
    pub fn start_task(&self, task_id: &str) -> Result<StartOutcome, TaskLaunchError> {
        let row = self.launchable_row(task_id)?;
        if row.status.is_terminal() {
            return Err(TaskLaunchError::NotStartable {
                current: row.status.as_str(),
            });
        }
        let plan = RunPlan::from_row(&row)?;

        if !self.shared_context.claim_run_slot(&row.id) {
            return Err(TaskLaunchError::AlreadyRunning);
        }

        let outcome = match self.task_dispatcher.dispatch_lead_agent_with_id(
            &row.id,
            &plan.description,
            plan.title,
            &plan.created_by,
            &plan.lane_key,
            &plan.source,
            plan.workspace,
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                // Nothing is running under this id after all.
                self.shared_context.remove_cancellation_token(&row.id);
                return Err(TaskLaunchError::Dispatch(e));
            }
        };

        Ok(StartOutcome {
            status: self.dispatched_status(&outcome.task_id),
            task_id: outcome.task_id,
            title: outcome.title,
        })
    }

    /// §5.6c's `resume` (S2) — re-enter an **interrupted** run under its own
    /// id, over the history rebuilt from its session log.
    ///
    /// The order of the refusals is the order of the questions a caller can
    /// act on, most specific first:
    ///
    /// 1. **Is there such a run?** (`NotFound`, owner-scoped at the route.)
    /// 2. **Was it interrupted?** (`NotResumable`.) Checked *before* the
    ///    flag, deliberately: `resume` is also the word for un-pausing a
    ///    paused run, and a caller who reached this verb by accident is owed
    ///    the truth about their row rather than a lecture about a feature
    ///    flag they were not asking for.
    /// 3. **Is S2 on?** (`ResumeDisabled`.) Only a genuinely interrupted run
    ///    hears this, which is exactly the case the flag exists to gate.
    /// 4. **Is there a goal, and a transcript?** (`NoDescription`,
    ///    `ResumeLogMissing`.) The replay runs here, before anything is
    ///    claimed or dispatched, so a refusal costs the row nothing — the row
    ///    still says `interrupted` and `rerun` is still available.
    /// 5. **Is the id free?** (`AlreadyRunning`.) The same compare-and-set
    ///    `start` uses, for the same reason.
    ///
    /// The rebuild is file I/O over a log that may be hundreds of megabytes,
    /// so it runs on a blocking thread rather than on the runtime this was
    /// awaited from (the same rule T41's writer follows, R52).
    pub async fn resume_task(&self, task_id: &str) -> Result<ResumeOutcome, TaskLaunchError> {
        let row = self.launchable_row(task_id)?;
        if row.status != TaskStatus::Interrupted {
            return Err(TaskLaunchError::NotResumable {
                current: row.status.as_str(),
            });
        }
        if !self
            .daemon_config
            .load()
            .orchestrator
            .routing
            .resume_enabled
        {
            return Err(TaskLaunchError::ResumeDisabled);
        }
        let plan = RunPlan::from_row(&row)?;

        // The run's **own** session, never the lane's current one: the
        // transcript to replay is the conversation this run happened in, and
        // by the time a crash is noticed the lane may be showing another
        // (T43's own reasoning for `recover_unprocessed_steering`).
        let session_id = row.session_id.clone().ok_or(TaskLaunchError::ResumeLogMissing)?;
        let dir = self
            .shared_context
            .session_log()
            .ok_or(TaskLaunchError::ResumeLogMissing)?
            .session_dir(&session_id);
        // `context_tail_keep` is what a compaction leaves in place, and the
        // replay adds that tail back ahead of the boundary. Every loop in the
        // tree uses the default; reading it from `LoopConfig` keeps the two
        // numbers one number.
        let tail_keep = LoopConfig::default().context_tail_keep;
        let owned_task_id = row.id.clone();
        let replay: ReplayPlan = tokio::task::spawn_blocking(move || {
            rebuild(&dir, &owned_task_id, tail_keep)
        })
        .await
        .map_err(|e| TaskLaunchError::Db(format!("replay task failed: {e}")))?
        .map_err(|e| TaskLaunchError::Db(format!("could not read the session log: {e}")))?;
        if replay.is_empty() {
            return Err(TaskLaunchError::ResumeLogMissing);
        }
        let rounds_replayed = replay.rounds;
        let from_seq = replay.from_seq;
        let to_seq = replay.to_seq;

        if !self.shared_context.claim_run_slot(&row.id) {
            return Err(TaskLaunchError::AlreadyRunning);
        }

        let seed = ResumeSeed {
            session_id: session_id.clone(),
            replay,
        };
        let outcome = match self.task_dispatcher.dispatch_lead_agent_resume(
            &row.id,
            &plan.description,
            plan.title,
            &plan.created_by,
            &plan.lane_key,
            &plan.source,
            plan.workspace,
            seed,
        ) {
            Ok(outcome) => outcome,
            Err(e) => {
                self.shared_context.remove_cancellation_token(&row.id);
                return Err(TaskLaunchError::Dispatch(e));
            }
        };

        Ok(ResumeOutcome {
            status: self.dispatched_status(&outcome.task_id),
            task_id: outcome.task_id,
            title: outcome.title,
            session_id,
            rounds_replayed,
            from_seq,
            to_seq,
        })
    }

    /// A just-dispatched run's status, read rather than assumed.
    ///
    /// The dispatch persists `queued` and its background half flips the row to
    /// `running`; which of the two a caller sees depends on scheduling, and
    /// both are true. Guessing "queued" would be a lie half the time.
    fn dispatched_status(&self, task_id: &str) -> String {
        self.shared_context
            .task_registry
            .get(task_id)
            .map(|entry| entry.status.as_str().to_string())
            .unwrap_or_else(|| TaskStatus::Queued.as_str().to_string())
    }
}

#[cfg(test)]
mod tests;
