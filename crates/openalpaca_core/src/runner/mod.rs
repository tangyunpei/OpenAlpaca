mod agentic_loop;
pub mod lead_agent;
pub mod model_choice;
pub mod model_window;
pub mod plugin_agent;
pub mod span;
pub mod steering;

pub use agentic_loop::{
    AnswerGuard, Correction, LoopConfig, LoopCostAccumulator, LoopFinishReason, LoopResult,
    StreamCallback, run_agentic_loop, run_agentic_loop_routed,
};
pub(crate) use agentic_loop::{
    MAX_TOOL_RESULT_SIZE, compress_context, estimate_messages_tokens, estimate_tools_tokens,
};
pub use lead_agent::{LeadAgentResult, run_lead_agent};
pub use model_window::{routed_context_window, routed_model};
pub use span::{close_span, open_span, span_detail_for, span_state_for};
pub use steering::{SteeringInbox, SteeringMsg, SteeringPushError};

/// `fut` raced against `token` when the run has one, `None` when the token
/// won; without a token, `fut` alone. Through
/// [`CancellationToken::run_until_cancelled`](tokio_util::sync::CancellationToken::run_until_cancelled):
/// one up-front `is_cancelled()` check, then on every wake the future polled
/// before the token, so a future and a cancel ready in the same tick yield
/// the future's output (M60).
pub(crate) async fn unless_cancelled<F: std::future::Future>(
    token: Option<&tokio_util::sync::CancellationToken>,
    fut: F,
) -> Option<F::Output> {
    match token {
        Some(token) => token.run_until_cancelled(fut).await,
        None => Some(fut.await),
    }
}
