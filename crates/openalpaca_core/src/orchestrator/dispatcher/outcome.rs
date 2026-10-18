//! Task outcome construction, terminal consistency checks, and finalization.

use crate::orchestrator::task_state::{TaskOutcome, TaskState};
use openalpaca_storage::OutcomeKind;

/// Maximum length for the summary stored in `result_summary` column.
///
/// **S4 — this is the only cap.** Two others used to sit in front of it, both
/// 500: the lead's step summary (`dispatcher/lead_agent.rs`) and
/// `TaskState::mark_step_completed`, which is what the outcome's summary is
/// built from. The number this constant names was therefore never reached,
/// and a run's report arrived cut mid-sentence — "Done, with one snag:".
pub(crate) const MAX_SUMMARY_LENGTH: usize = 2000;

/// Persist a state update with retry (up to 3 attempts) to handle optimistic locking conflicts.
///
/// Returns `true` if the update was successfully persisted, `false` if all retries were
/// exhausted or a DB error occurred.
pub(super) async fn update_state_with_retry(
    db: &openalpaca_storage::Database,
    task_id: &str,
    mutate: impl Fn(&mut TaskState),
    context: &str,
) -> bool {
    const MAX_RETRIES: usize = 3;
    for attempt in 0..MAX_RETRIES {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let existing = match repo.get(task_id) {
            Ok(Some(t)) => t,
            _ => return false,
        };
        let sj = match existing.state_json.as_deref() {
            Some(s) => s,
            None => return false,
        };
        let mut state: TaskState = match serde_json::from_str(sj) {
            Ok(s) => s,
            Err(_) => return false,
        };
        mutate(&mut state);
        match repo.update_state(task_id, &state.to_json(), existing.state_version) {
            Ok(true) => return true,
            Ok(false) => {
                if attempt < MAX_RETRIES - 1 {
                    tracing::debug!(
                        "State update version conflict ({}) for task '{}' (attempt {}/{}), retrying",
                        context,
                        task_id,
                        attempt + 1,
                        MAX_RETRIES
                    );
                    // Linear backoff with pseudo-jitter (avoids rand dependency).
                    // Jitter from task_id hash reduces contention under DAG parallelism.
                    let pseudo_jitter = task_id.as_bytes().iter().map(|b| *b as u64).sum::<u64>() % 5;
                    tokio::time::sleep(std::time::Duration::from_millis(10 + (attempt as u64 * 5) + pseudo_jitter)).await;
                } else {
                    tracing::warn!(
                        "State update ({}) for task '{}' failed after {} retries — state may be stale",
                        context,
                        task_id,
                        MAX_RETRIES
                    );
                    return false;
                }
            }
            Err(e) => {
                tracing::warn!(
                    "State update ({}) failed for task '{}': {}",
                    context,
                    task_id,
                    e
                );
                return false;
            }
        }
    }
    false
}

/// Add the artifacts the run really produced to an outcome, and reclassify it
/// (M4).
///
/// `TaskState.steps[].artifact_pointers` only ever knew about the pipeline
/// shapes that wrote them; the lead-agent topology writes files through
/// `artifact_write`, which records them in `file_assets` under the run's
/// `task_id` — for the lead itself and for every subagent of the run alike.
/// A run that produced three files therefore reported `artifact_count: 0`,
/// `outcome_kind: "text_only"` and "No artifacts were produced.".
///
/// The store is the authority: anything it holds for the run that the state
/// did not already name is appended, in the order it was written. Kept pure —
/// no DB, no clock — so the classification rules are testable on their own.
fn merge_produced_artifacts(
    outcome: &mut TaskOutcome,
    produced: Vec<openalpaca_storage::ProducedArtifact>,
    has_text_summary: bool,
) {
    for file in produced {
        let already_named = outcome
            .artifacts
            .iter()
            .any(|a| a.file_asset_id.as_deref() == Some(file.id.as_str()));
        if already_named {
            continue;
        }
        outcome.artifacts.push(crate::orchestrator::task_state::ArtifactPointer {
            key: file.filename.clone(),
            label: file.filename,
            agent_id: file.agent_id.unwrap_or_default(),
            // Not a pipeline step: the lead-agent topology has none.
            step_order: -1,
            file_asset_id: Some(file.id),
        });
    }

    if outcome.artifacts.is_empty() {
        return;
    }
    outcome.no_artifact_reason = None;
    // A failed run keeps saying so — it may well have produced something
    // before it failed, and that is not what the reader needs to know first.
    if outcome.outcome_kind != OutcomeKind::Failed {
        outcome.outcome_kind = if has_text_summary {
            OutcomeKind::Mixed
        } else {
            OutcomeKind::ArtifactOnly
        };
    }
}

/// The artifacts `file_assets` holds for this run, or none when it cannot be
/// read — a failed read costs the count, never the finalisation.
fn produced_artifacts(
    db: &openalpaca_storage::Database,
    task_id: &str,
) -> Vec<openalpaca_storage::ProducedArtifact> {
    openalpaca_storage::FileAssetRepository::new(db)
        .produced_for_task(task_id)
        .unwrap_or_else(|e| {
            tracing::warn!(task_id, "Failed to read the run's produced artifacts: {e}");
            Vec::new()
        })
}

/// The tools this run had to refuse because nobody could approve them (S4),
/// oldest first, each named once.
///
/// Read from the run's own audit rows rather than tracked in memory: the
/// refusals happen in the sandbox — the lead's *and* every subagent's, each
/// with its own `SandboxManager` — and they all write to one place already.
/// A read failure costs the note, never the finalisation.
pub(super) fn unapprovable_tools(
    db: &openalpaca_storage::Database,
    task_id: &str,
) -> Vec<String> {
    let rows = openalpaca_storage::repository::EventLogRepository::new(db)
        .query(&openalpaca_storage::repository::EventLogQuery {
            task_id: Some(task_id),
            event_type: Some(crate::security::sandbox::UNAPPROVABLE_EVENT_TYPE),
            limit: 100,
            ..Default::default()
        })
        .unwrap_or_else(|e| {
            tracing::warn!(task_id, "Failed to read the run's unapproved tools: {e}");
            Vec::new()
        });
    let mut names = Vec::new();
    // The query answers newest first; the report reads in the order the run
    // hit them.
    for row in rows.into_iter().rev() {
        let Some(name) = row
            .detail
            .as_ref()
            .and_then(|d| d.get("tool_name"))
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if !names.iter().any(|n: &String| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The one runtime-authored line that says what the run could not do (S4).
///
/// `None` when nothing was refused. Deliberately not model prose: the
/// evidence for this ruling is a report that ended mid-sentence at
/// "Done, with one snag:" because the summary was cut at 500 characters, and
/// a sentence the model wrote is exactly what a cut can take away. This line
/// is appended by the runtime and [`truncate_summary`] keeps it whole.
///
/// An extension tool's name is any UTF-8 its server chose, up to 256 bytes,
/// so each is shown through `utils::text::escape_hidden_line` and the names
/// share [`REFUSED_TOOLS_LIST_CHARS`] as [`project_files_note`]'s paths share
/// theirs, the rest counted (residual X21; no count line, as there).
pub(super) fn unapprovable_note(tools: &[String]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }
    let list = bounded_list(tools, REFUSED_TOOLS_SHOWN, REFUSED_TOOLS_LIST_CHARS);
    let (subject, verb) = if tools.len() == 1 {
        ("tool", "was")
    } else {
        ("tools", "were")
    };
    Some(format!(
        "Not run — this run could not ask anyone for approval, so the following \
         {subject} {verb} refused: {list}. Run it from the GUI, or from an \
         interactive `openalpaca chat`, and approve it there."
    ))
}

/// How many tools [`unapprovable_note`] names before it counts the rest.
const REFUSED_TOOLS_SHOWN: usize = 20;

/// How many characters the tool names [`unapprovable_note`] lists may take
/// together, `, ` between them included. With [`PROJECT_FILES_LIST_CHARS`]
/// and the two lines' own words the runtime's lines stay under about 1,550
/// characters, so the summary's [`MAX_SUMMARY_LENGTH`] cut always keeps them
/// whole with some of the report's prose before them.
const REFUSED_TOOLS_LIST_CHARS: usize = 400;

/// How many paths [`project_files_note`] names before it counts the rest.
const PROJECT_FILES_SHOWN: usize = 20;

/// How many characters the paths [`project_files_note`] names may take
/// together, `, ` between them included: the line stays under 1,000
/// characters, so the summary's [`MAX_SUMMARY_LENGTH`] cut
/// ([`truncate_summary`]) keeps it and the refusal line whole with the
/// report's own prose before them (S4a's deferred (2): twenty paths a run can
/// write ran to thousands of characters).
const PROJECT_FILES_LIST_CHARS: usize = 900;

/// **R38.** The runtime-authored line naming what a run wrote into its
/// project — `None` when it wrote nothing there. Result files are the user's
/// files, not the run's artifacts: the run's count, its chips and connector
/// delivery read by-products only, and this line is how the report lists the
/// rest. At most [`PROJECT_FILES_SHOWN`] paths, and only as many as fit in
/// [`PROJECT_FILES_LIST_CHARS`], then "and N more"; a first path longer than
/// that alone is named cut, `…` where the cut is.
///
/// A path is the agent's choice, so each is shown through
/// `utils::text::escape_hidden_line`: every hidden character, a line break
/// among them, as `<U+XXXX>` (S4a's deferred (1)). No count line follows:
/// the report is not an approval surface (the lead's ruling on S6a's M4 for
/// the event log's written-file row, the same text).
pub(super) fn project_files_note(rel_paths: &[String]) -> Option<String> {
    if rel_paths.is_empty() {
        return None;
    }
    let list = bounded_list(rel_paths, PROJECT_FILES_SHOWN, PROJECT_FILES_LIST_CHARS);
    Some(format!("Written in the project: {list}."))
}

/// `items` as a runtime line lists them: each escaped on one line
/// (`utils::text::escape_hidden_line`), joined by `, `, at most `shown_max`
/// of them and only as many as fit in `budget` characters together, then
/// ", and N more" for the rest. A first item longer than the whole budget is
/// named cut on a whole piece ([`escaped_within`]), so the list is never
/// empty.
fn bounded_list(items: &[String], shown_max: usize, budget: usize) -> String {
    use crate::utils::text::escape_hidden_line;

    let mut shown: Vec<String> = Vec::new();
    let mut used = 0;
    for item in items.iter().take(shown_max) {
        let escaped = escape_hidden_line(item).text;
        let cost = escaped.chars().count() + if shown.is_empty() { 0 } else { 2 };
        if used + cost > budget {
            if shown.is_empty() {
                shown.push(escaped_within(item, budget));
            }
            break;
        }
        used += cost;
        shown.push(escaped);
    }
    let more = items.len() - shown.len();
    let shown = shown.join(", ");
    if more == 0 {
        shown
    } else {
        format!("{shown}, and {more} more")
    }
}

/// `text` as [`bounded_list`] shows it, cut to at most `max` characters on a
/// whole piece — a character, or its whole `<U+XXXX>` escape, never part of
/// one — with `…` where the cut is.
fn escaped_within(text: &str, max: usize) -> String {
    use crate::utils::text::escape_hidden_line;

    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let piece = escape_hidden_line(c.encode_utf8(&mut [0; 4])).text;
        let len = piece.chars().count();
        if used + len + 1 > max {
            out.push('…');
            break;
        }
        used += len;
        out.push_str(&piece);
    }
    out
}

/// The files a run wrote into its project, oldest first — none when they
/// cannot be read: a failed read costs the line, never the finalisation.
fn project_files(db: &openalpaca_storage::Database, task_id: &str) -> Vec<String> {
    openalpaca_storage::FileAssetRepository::new(db)
        .project_files_for_task(task_id)
        .map(|files| files.into_iter().map(|file| file.filename).collect())
        .unwrap_or_else(|e| {
            tracing::warn!(task_id, "Failed to read the run's result files: {e}");
            Vec::new()
        })
}

/// How many subagents [`stopped_subagents_note`] names before it counts the
/// rest.
const STOPPED_SUBAGENTS_SHOWN: usize = 8;

/// How many characters the lane labels [`stopped_subagents_note`] names may
/// take together. With the two lines above it the runtime's lines stay under
/// about 1,800 characters, inside the summary's [`MAX_SUMMARY_LENGTH`] cut
/// with room for the report's own prose.
const STOPPED_SUBAGENTS_LIST_CHARS: usize = 160;

/// **N-LEAD-1.** The runtime-authored line naming the subagents the run's end
/// stopped — `None` when it stopped none. Each is named by its lane's label,
/// as the run's timeline names it; a label carries its template's id, which
/// a plugin chose, so it is shown escaped like the other lines' names.
pub(super) fn stopped_subagents_note(labels: &[String]) -> Option<String> {
    if labels.is_empty() {
        return None;
    }
    let list = bounded_list(
        labels,
        STOPPED_SUBAGENTS_SHOWN,
        STOPPED_SUBAGENTS_LIST_CHARS,
    );
    let (subject, verb) = if labels.len() == 1 {
        ("this subagent", "it was")
    } else {
        ("these subagents", "they were")
    };
    Some(format!(
        "Stopped when the run ended — {subject} had not finished, so {verb} \
         cancelled: {list}."
    ))
}

/// The lanes of this run its end stopped (N-LEAD-1), oldest first: closed as
/// cancelled with the run-ended detail, which only the run's end writes. The
/// lead's run waits for every lane to close before it returns, so they are
/// all closed by the time the report is written. A read failure costs the
/// line, never the finalisation.
fn stopped_subagents(db: &openalpaca_storage::Database, task_id: &str) -> Vec<String> {
    openalpaca_storage::SubagentSpanRepository::new(db)
        .list_for_task(task_id)
        .map(|spans| {
            spans
                .into_iter()
                .filter(|span| {
                    span.state == openalpaca_storage::SpanState::Cancelled.as_str()
                        && span.detail.as_deref()
                            == Some(crate::runner::lead_agent::SUBAGENT_RUN_ENDED_DETAIL)
                })
                .map(|span| span.label)
                .collect()
        })
        .unwrap_or_else(|e| {
            tracing::warn!(task_id, "Failed to read the run's stopped subagents: {e}");
            Vec::new()
        })
}

/// The runtime's own lines a run's report and its summary end with — what
/// nobody could approve (S4), what it wrote into the project (R38), then the
/// subagents its end stopped (N-LEAD-1) — joined by a blank line, or `None`
/// when there is none. One function for the report message and the
/// summary's cut, so both keep the same lines.
pub(super) fn runtime_notes(db: &openalpaca_storage::Database, task_id: &str) -> Option<String> {
    let notes: Vec<String> = [
        unapprovable_note(&unapprovable_tools(db, task_id)),
        project_files_note(&project_files(db, task_id)),
        stopped_subagents_note(&stopped_subagents(db, task_id)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!notes.is_empty()).then(|| notes.join("\n\n"))
}

/// Cut `summary` to `max` characters, keeping a runtime-authored `note` at
/// the end whole (S4).
///
/// The note is the point of the cut: a summary long enough to be truncated is
/// exactly the one whose last line would otherwise be lost, and the last line
/// is the only part the runtime wrote. When the note alone is longer than
/// `max` the note wins and the prose goes. At [`MAX_SUMMARY_LENGTH`] that
/// never happens: both runtime lines are bounded ([`unapprovable_note`],
/// [`project_files_note`]; residual X21). The function must still be total.
pub(super) fn truncate_summary(summary: &str, note: Option<&str>, max: usize) -> String {
    if summary.chars().count() <= max {
        return summary.to_string();
    }
    let Some(note) = note else {
        return summary.chars().take(max).collect();
    };
    let tail = format!("\n\n{note}");
    let tail_len = tail.chars().count();
    if tail_len >= max {
        return note.chars().take(max).collect();
    }
    // The prose is everything before the note the caller already appended.
    let prose: String = summary
        .strip_suffix(&tail)
        .unwrap_or(summary)
        .chars()
        .take(max - tail_len)
        .collect();
    format!("{prose}{tail}")
}

/// Build a structured TaskOutcome from the current task state.
///
/// Reads the task's state_json from the DB (if available), uses it to collect
/// step summaries and artifact pointers, then classifies the outcome.
///
/// If state_json is unavailable (lead agent with no state, legacy tasks),
/// falls back to constructing a minimal outcome from the provided content.
///
/// Either way the files the run recorded in `file_assets` are merged in
/// (M4) — the state's pointers are what the *pipeline* shapes wrote down, and
/// on the lead-agent topology they are empty however many artifacts the run
/// produced.
pub(super) fn build_task_outcome(
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    final_content: &str,
    success: bool,
) -> TaskOutcome {
    // Try to read state_json for rich outcome data
    if let Some(db) = db {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        if let Ok(Some(task)) = repo.get(task_id) {
            if let Some(ref sj) = task.state_json {
                if let Ok(state) = serde_json::from_str::<TaskState>(sj) {
                    let fallback = if final_content.is_empty() {
                        if success { "Task completed." } else { "Task failed." }
                    } else {
                        final_content
                    };
                    let mut outcome = state.build_outcome(fallback, None);
                    if !success {
                        outcome.outcome_kind = OutcomeKind::Failed;
                        // Prepend error reason if it's not already in the summary
                        if !final_content.is_empty() && !outcome.summary.contains(final_content) {
                            outcome.summary =
                                format!("{}\n\n{}", final_content, outcome.summary);
                        }
                    }
                    let has_summary = !outcome.summary.is_empty()
                        && outcome.summary != "Task completed."
                        && outcome.summary != "Task failed.";
                    merge_produced_artifacts(
                        &mut outcome,
                        produced_artifacts(db, task_id),
                        has_summary,
                    );
                    return outcome;
                }
            }
        }
    }

    // Fallback: no state_json available, build minimal outcome from content
    let summary = if final_content.is_empty() {
        if success { "Task completed.".to_string() } else { "Task failed.".to_string() }
    } else {
        final_content.to_string()
    };

    let mut outcome = if success {
        TaskOutcome {
            summary,
            outcome_kind: OutcomeKind::TextOnly,
            artifacts: Vec::new(),
            no_artifact_reason: Some("No artifacts were produced.".to_string()),
        }
    } else {
        TaskOutcome {
            summary,
            outcome_kind: OutcomeKind::Failed,
            artifacts: Vec::new(),
            no_artifact_reason: None,
        }
    };
    if let Some(db) = db {
        // The lead agent's ordinary path: no `state_json`, a report as the
        // summary, and every artifact of the run in the store.
        let has_summary = !final_content.trim().is_empty();
        merge_produced_artifacts(&mut outcome, produced_artifacts(db, task_id), has_summary);
    }
    outcome
}

/// Log warnings for inconsistent terminal task states.
///
/// This is observability-only (never blocks or fails). It catches cases like:
/// - Empty outcome summary at terminal time
/// - success=true but outcome_kind=Failed (or vice versa)
fn check_terminal_consistency(task_id: &str, success: bool, outcome: &TaskOutcome) {
    if outcome.summary.is_empty() {
        tracing::warn!(
            task_id,
            success,
            outcome_kind = %outcome.outcome_kind.as_str(),
            "Terminal task has empty outcome summary"
        );
    }
    if success && outcome.outcome_kind == OutcomeKind::Failed {
        tracing::warn!(
            task_id,
            "Task marked successful but outcome_kind=Failed — inconsistent state"
        );
    }
    if !success && outcome.outcome_kind != OutcomeKind::Failed {
        tracing::warn!(
            task_id,
            outcome_kind = %outcome.outcome_kind.as_str(),
            "Task marked failed but outcome_kind is not Failed — inconsistent state"
        );
    }
}

/// Finalize a task with a structured outcome.
///
/// This is the unified replacement for the ad-hoc assembly in each execution mode.
/// It:
/// 1. Builds the TaskOutcome (via `build_task_outcome`)
/// 2. Checks terminal consistency (log-only warnings)
/// 3. Persists the outcome to DB (outcome_json, outcome_kind, artifact_count)
/// 4. Delegates to `finalize_task` for status update, `result_summary`, and event emission
pub(super) fn finalize_task_with_outcome(
    ctx: &crate::context::SharedContext,
    bus: &crate::bus::EventBus,
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    final_content: &str,
    success: bool,
) -> TaskOutcome {
    let mut outcome = build_task_outcome(db, task_id, final_content, success);

    // S4 and R38: what nobody could approve and what the run wrote into the
    // project, in the runtime's own words, appended to the summary so they
    // reach `outcome_json` as well as `result_summary`.
    let note = db.and_then(|db| runtime_notes(db, task_id));
    if let Some(ref note) = note {
        outcome.summary = if outcome.summary.trim().is_empty() {
            note.clone()
        } else {
            format!("{}\n\n{note}", outcome.summary)
        };
    }

    // Observability: log warnings for inconsistent terminal states
    check_terminal_consistency(task_id, success, &outcome);

    // Persist structured outcome fields to DB (outcome_json, outcome_kind, artifact_count)
    if let Some(db) = db {
        let repo = openalpaca_storage::repository::TaskRepository::new(db);
        let outcome_json = match serde_json::to_string(&outcome) {
            Ok(json) => json,
            Err(e) => {
                tracing::warn!(
                    "finalize_task_with_outcome: failed to serialize outcome for task '{}': {e}",
                    task_id
                );
                // Skip DB write — don't persist invalid/empty JSON
                String::new()
            }
        };
        if outcome_json.is_empty() {
            tracing::warn!(
                "finalize_task_with_outcome: skipping set_outcome for task '{}' due to empty outcome_json",
                task_id
            );
        } else if let Err(e) = repo.set_outcome(
            task_id,
            &outcome_json,
            outcome.outcome_kind,
            outcome.artifacts.len() as i32,
        ) {
            tracing::warn!(
                "finalize_task_with_outcome: failed to set outcome for task '{}': {e}",
                task_id
            );
        }
    }

    // Delegate status update + result_summary + event emission to existing
    // finalize_task. S4: the cut keeps the runtime's line — it is the one
    // part of the summary no rewording can restore.
    let truncated_summary = truncate_summary(&outcome.summary, note.as_deref(), MAX_SUMMARY_LENGTH);
    finalize_task(
        ctx,
        bus,
        db,
        task_id,
        &truncated_summary,
        success,
        Some(outcome.outcome_kind),
        Some(outcome.artifacts.len() as i32),
        Some(&outcome.summary),
    );

    outcome
}

/// Update task status in registry + DB + emit event for a completed or failed task.
#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_task(
    ctx: &crate::context::SharedContext,
    bus: &crate::bus::EventBus,
    db: Option<&openalpaca_storage::Database>,
    task_id: &str,
    summary: &str,
    success: bool,
    outcome_kind: Option<OutcomeKind>,
    artifact_count: Option<i32>,
    outcome_summary: Option<&str>,
) {
    let now = chrono::Utc::now();
    // Title comes from the in-memory registry; a DB-only task resurrected
    // after a restart (no registry entry) falls back to "" (GAP-07).
    let title = ctx
        .task_registry
        .get(task_id)
        .map(|e| e.title)
        .unwrap_or_default();
    if success {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Completed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Completed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskCompleted {
            task_id: task_id.to_string(),
            title,
            result_summary: Some(summary.to_string()),
            outcome_kind: outcome_kind.map(|k| k.as_str().to_string()),
            artifact_count,
            outcome_summary: outcome_summary
                .map(|s| s.chars().take(MAX_SUMMARY_LENGTH).collect()),
            timestamp: now,
        });
    } else {
        ctx.task_registry
            .update_status(task_id, crate::context::TaskEntryStatus::Failed);
        if let Some(db) = db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            if let Err(e) = repo.update_status(task_id, openalpaca_storage::TaskStatus::Failed) {
                tracing::warn!(
                    "finalize_task: failed to update status for task '{}': {e}",
                    task_id
                );
            }
            if let Err(e) = repo.set_result(task_id, summary) {
                tracing::warn!(
                    "finalize_task: failed to set result for task '{}': {e}",
                    task_id
                );
            }
        }
        bus.publish(crate::events::SystemEvent::TaskFailed {
            task_id: task_id.to_string(),
            title,
            error: summary.to_string(),
            outcome_kind: outcome_kind.map(|k| k.as_str().to_string()),
            timestamp: now,
        });
    }
}

/// One-line status prefix for the completion conversation message when the
/// lead-agent loop did not finish cleanly (Routing V2 §2b). `Complete`
/// finishes carry no prefix — the report speaks for itself.
pub(super) fn completion_status_line(
    finish_reason: &crate::runner::LoopFinishReason,
) -> Option<&'static str> {
    use crate::runner::LoopFinishReason;
    match finish_reason {
        LoopFinishReason::Complete => None,
        LoopFinishReason::MaxRounds => Some("Task ended early (round budget exhausted):"),
        LoopFinishReason::CostExceeded => Some("Task ended early (cost budget exhausted):"),
        LoopFinishReason::Truncated => Some("Task ended early (output truncated):"),
        LoopFinishReason::Cancelled => Some("Task was cancelled before finishing:"),
        LoopFinishReason::Error(_) => Some("Task ended with an error:"),
    }
}

/// Persist a task result as a conversation message.
///
/// `pub` — re-exported from `orchestrator::dispatcher` — because the lead
/// agent's `post_update` tool and the daemon's `NotificationDispatcher` both
/// reuse it: T1 step 3's cron notice is written to the default lane through
/// exactly this call (extension design §7.3 step 2).
///
/// Two things about the arguments, stated so nobody re-derives them: the
/// `source` argument is the **column**, not the role — `role` is hardcoded
/// `"assistant"` below, which is why the row renders (the GUI transcript skips
/// only `role === "system"`) — and it should be the lane's own source, because
/// `get_or_create_conversation` creates a missing conversation with whatever
/// `source` it is handed.
///
/// `task_id` is GAP-23's run link, and only the two messages the design names
/// carry one: the turn that *started* a workflow (written at the gateway) and
/// the completion report that closed it ([`persist_completion_report`]). Every
/// other caller — the lead's `post_update` progress note, the daemon's
/// extension notices — passes `None`: those are lane chatter, not the run's
/// own record. Returns the new message's id, or `None` if nothing was written.
///
/// `session_id` is §5.3's pin. `Some` writes into that conversation whatever
/// the lane is currently showing — the caller has a reason to name it, and the
/// only reason that exists is that this message belongs to a run started
/// there. `None` means "wherever this lane is talking now", which is what lane
/// chatter wants, and it creates the lane's session if it has none.
#[allow(clippy::too_many_arguments)]
pub fn persist_conversation(
    db: &openalpaca_storage::Database,
    lane_key: &str,
    source: &str,
    session_id: Option<&str>,
    content: String,
    model: Option<String>,
    tokens_in: i64,
    tokens_out: i64,
    runtime_secs: i64,
    task_id: Option<&str>,
) -> Option<i64> {
    let conv_repo = openalpaca_storage::ConversationRepository::new(db);
    if session_id.is_none()
        && let Err(e) = conv_repo.get_or_create_active_session(lane_key, source, None)
    {
        tracing::warn!(
            "persist_conversation: failed to get/create session for lane '{}': {e}",
            lane_key
        );
        return None;
    }

    let msg = openalpaca_storage::ConversationMessage {
        lane_key: lane_key.to_string(),
        role: "assistant".to_string(),
        content,
        source: Some(source.to_string()),
        model,
        tokens_in: Some(tokens_in),
        tokens_out: Some(tokens_out),
        duration_ms: Some(runtime_secs * 1000),
        task_id: task_id.map(str::to_string),
        session_id: session_id.map(str::to_string),
        ..Default::default()
    };

    match conv_repo.insert(&msg) {
        Ok(message_id) => {
            let counted = match session_id {
                Some(id) => conv_repo.increment_message_count_for_session(id),
                None => conv_repo.increment_message_count(lane_key),
            };
            if let Err(e) = counted {
                tracing::warn!(
                    "persist_conversation: failed to increment message count for lane '{}': {e}",
                    lane_key
                );
            }
            Some(message_id)
        }
        Err(e) => {
            tracing::warn!(
                "persist_conversation: failed to insert assistant message for lane '{}': {e}",
                lane_key
            );
            None
        }
    }
}

/// Persist a workflow's completion report — GAP-23's second link.
///
/// The report is the one message that can name the run's *output*: by the time
/// it is written, `file_assets` has a row for everything the run produced,
/// which is why the delegating turn (written before the work happened) carries
/// only the run id. Each produced file gets a `role='artifact'` row beside the
/// message, so a reloaded transcript draws the report card and its chips from
/// history alone instead of from the frames one client happened to watch.
///
/// A failure to link is logged, never fatal: the report itself is the record of
/// the run, and losing a chip must not lose the message.
#[allow(clippy::too_many_arguments)]
pub fn persist_completion_report(
    db: &openalpaca_storage::Database,
    lane_key: &str,
    source: &str,
    session_id: Option<&str>,
    content: String,
    model: Option<String>,
    tokens_in: i64,
    tokens_out: i64,
    runtime_secs: i64,
    task_id: &str,
) {
    let Some(message_id) = persist_conversation(
        db,
        lane_key,
        source,
        session_id,
        content,
        model,
        tokens_in,
        tokens_out,
        runtime_secs,
        Some(task_id),
    ) else {
        return;
    };

    let file_repo = openalpaca_storage::FileAssetRepository::new(db);
    let produced = match file_repo.produced_ids_for_task(task_id) {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(
                task_id = %task_id,
                "Failed to read the run's produced artifacts for its report: {e}"
            );
            return;
        }
    };
    for (i, file_id) in produced.iter().enumerate() {
        if let Err(e) = file_repo.link_to_message_with_role(
            message_id,
            file_id,
            i as i32,
            None,
            openalpaca_storage::ARTIFACT_ROLE,
        ) {
            tracing::warn!(
                task_id = %task_id,
                "Failed to link artifact {file_id} to completion report {message_id}: {e}"
            );
        }
    }
}
