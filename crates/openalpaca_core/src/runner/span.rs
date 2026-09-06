//! Subagent span instrumentation (plan Phase 4, GAP-09).
//!
//! Two calls — [`open_span`] when a lane starts and [`close_span`] when it
//! ends — write the `subagent_span` row and announce it on the bus. Both are
//! deliberately infallible from the caller's side: a run must never fail
//! because its observability row could not be written, so a storage error is a
//! `warn!` and nothing more.
//!
//! Without a database there is no span and no event. That is the same rule
//! `record_agent_history` already follows, and it keeps the socket honest —
//! a frame is only ever emitted for a row that exists.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::runner::LoopFinishReason;
use chrono::Utc;
use openalpaca_storage::{
    Database, NewSubagentSpan, SpanState, SubagentSpanRecord, SubagentSpanRepository,
};

/// How much of an agent's final message a lane carries.
const OUTPUT_PREVIEW_CHARS: usize = 200;
/// How much of an objective a span stores.
const OBJECTIVE_CHARS: usize = 500;
/// How much of a failure reason a lane's trailing detail carries.
const DETAIL_CHARS: usize = 200;

/// Announce a span row — its open, or one of its closes.
fn publish(bus: &EventBus, record: &SubagentSpanRecord) {
    bus.publish(SystemEvent::SubagentSpan {
        task_id: record.task_id.clone(),
        span_id: record.id.clone(),
        label: record.label.clone(),
        template_id: record.template_id.clone(),
        agent_instance_id: record.agent_instance_id.clone(),
        state: record.state.clone(),
        detail: record.detail.clone(),
        started_at: record.started_at.clone(),
        ended_at: record.ended_at.clone(),
        duration_ms: record.duration_ms,
        output_preview: record.output_preview.clone(),
        timestamp: Utc::now(),
    });
}

/// Open a lane for a freshly spawned agent. `span_id` is the spawn's
/// `node_id`, so the span and its `DagNodeStarted`/`DagNodeCompleted` pair
/// name the same thing.
///
/// Returns the assigned label, purely so a caller can log it; the close side
/// reads its own label back out of the row.
pub fn open_span(
    db: Option<&Database>,
    bus: &EventBus,
    task_id: &str,
    span_id: &str,
    template_id: &str,
    agent_instance_id: &str,
    objective: &str,
) -> Option<String> {
    let db = db?;
    let objective: String = objective.chars().take(OBJECTIVE_CHARS).collect();
    match SubagentSpanRepository::new(db).open(NewSubagentSpan {
        id: span_id,
        task_id,
        template_id,
        agent_instance_id,
        objective: Some(&objective),
    }) {
        Ok(record) => {
            publish(bus, &record);
            Some(record.label)
        }
        Err(e) => {
            tracing::warn!(
                task_id,
                span_id,
                template_id,
                "Failed to open subagent span: {e}"
            );
            None
        }
    }
}

/// Close a lane. A span that is already closed is left alone (first close
/// wins), and closing an unknown span is a no-op — both silently, because a
/// double close is a legitimate race between a completion path and the
/// boot-time orphan sweep, not an error.
pub fn close_span(
    db: Option<&Database>,
    bus: &EventBus,
    span_id: &str,
    state: SpanState,
    detail: Option<&str>,
    output_preview: Option<&str>,
) {
    let Some(db) = db else { return };
    let detail = detail.map(|d| d.chars().take(DETAIL_CHARS).collect::<String>());
    let preview = output_preview.and_then(|p| {
        let trimmed: String = p.chars().take(OUTPUT_PREVIEW_CHARS).collect();
        (!trimmed.is_empty()).then_some(trimmed)
    });

    match SubagentSpanRepository::new(db).close(
        span_id,
        state,
        detail.as_deref(),
        preview.as_deref(),
    ) {
        Ok(Some(record)) => publish(bus, &record),
        Ok(None) => tracing::debug!(span_id, "Subagent span already closed or unknown"),
        Err(e) => tracing::warn!(span_id, "Failed to close subagent span: {e}"),
    }
}

/// The lane state an agentic loop's exit means.
///
/// `Cancelled` is mapped explicitly rather than folded into "not successful":
/// `agent_success` treats it as a failure because the lead agent needs a
/// boolean, but a cancelled lane and a failed lane say different things to a
/// reader, and the UI has separate copy for each.
pub fn span_state_for(finish: &LoopFinishReason) -> SpanState {
    match finish {
        LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated => {
            SpanState::Done
        }
        LoopFinishReason::Cancelled => SpanState::Cancelled,
        LoopFinishReason::CostExceeded | LoopFinishReason::Error(_) => SpanState::Failed,
    }
}

/// The lane state a **plugin-backed** agent's exit means.
///
/// `PluginLoopOutcome` has no cancelled variant — a cancellation between steps
/// comes back as `Failed { error: "Cancelled" }` — so the run's own cancel
/// token is what tells the two apart. Reading the token rather than matching
/// the error string keeps the distinction from resting on a message someone
/// may reword, and it makes a plugin lane say the same word an LLM lane says
/// for the same event.
pub fn plugin_span_state(succeeded: bool, cancelled: bool) -> SpanState {
    match (succeeded, cancelled) {
        (true, _) => SpanState::Done,
        (false, true) => SpanState::Cancelled,
        (false, false) => SpanState::Failed,
    }
}

/// The trailing detail a lane shows for a non-`done` exit, or `None` when the
/// lane finished normally and the state word is the whole story.
pub fn span_detail_for(finish: &LoopFinishReason) -> Option<String> {
    match finish {
        LoopFinishReason::Complete => None,
        LoopFinishReason::MaxRounds => Some("hit the round limit".to_string()),
        LoopFinishReason::Truncated => Some("output truncated".to_string()),
        LoopFinishReason::Cancelled => Some("cancelled".to_string()),
        LoopFinishReason::CostExceeded => Some("cost cap reached".to_string()),
        LoopFinishReason::Error(e) => Some(e.clone()),
    }
}

#[cfg(test)]
mod tests;
