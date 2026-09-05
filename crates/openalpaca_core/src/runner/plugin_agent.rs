//! Plugin-backed subagent execution loop.
//!
//! Agent templates registered by plugins (`AgentSource::Plugin`) run an
//! external reasoning loop inside the plugin process instead of the
//! built-in agentic loop. This module drives that loop from the subagent
//! spawn path: spawn the plugin instance with the task instructions, poll
//! `step()` until completion (capped at [`MAX_PLUGIN_ITERATIONS`]), and
//! proxy any requested tool calls through the sandboxed execute path so
//! plugin agents get the same capability checks, input sanitization,
//! confirmation gating, and timeouts as internal agents.
//!
//! Ported from the deleted sequential-pipeline dispatcher
//! (`dispatcher/pipeline_step.rs`, removed in Routing V2 Phase 5), adapted
//! to the lead-agent subagent context: the outcome maps directly onto the
//! `SubagentTracker` completion shape (content string + success flag).

use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::extensions::{ExtensionId, ExtensionLedger, ScopedRun};
use crate::tools::registry::ToolContext;
use openalpaca_api::plugin_traits::PluginAgentExecutor;
use openalpaca_llm::ToolCall;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Maximum `step()` polls before the plugin agent is stopped and failed.
pub const MAX_PLUGIN_ITERATIONS: usize = 50;

/// Which plugin, and which *load* of it, an agent run belongs to.
///
/// The loop consults the ledger at every step boundary — exactly where it
/// already checks the cancellation token — so a plugin disabled mid-run stops
/// deliberately at its next step instead of waiting for T4 to close the channel
/// under it (design §3.2 T3(b)).
#[derive(Clone)]
pub struct PluginRunScope {
    pub ledger: Arc<ExtensionLedger>,
    pub extension: ExtensionId,
    pub generation: u64,
}

impl PluginRunScope {
    /// The S4 refusal for this plugin's current state, or `None` while it reads
    /// `Enabled` at this run's generation.
    fn blocked(&self) -> Option<String> {
        if let Some(refusal) = self.ledger.refusal_if_not_enabled(&self.extension, None) {
            return Some(refusal);
        }
        match self.ledger.generation(&self.extension) {
            Some(current) if current != self.generation => Some(
                crate::tools::extensions::Described::stale_run(
                    &self.extension,
                    crate::tools::extensions::Audience::Model,
                )
                .render_model(None),
            ),
            _ => None,
        }
    }
}

/// Delay between polls while the plugin reports `"working"`.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Outcome of a plugin agent loop, shaped for the subagent tracker.
#[derive(Debug)]
pub enum PluginLoopOutcome {
    /// The plugin reported `"complete"`.
    Completed {
        content: String,
        tool_calls_made: usize,
        iterations: usize,
    },
    /// Spawn was rejected, the plugin reported `"failed"`, an RPC errored,
    /// the iteration cap was hit, or the task was cancelled.
    Failed {
        error: String,
        tool_calls_made: usize,
    },
}

impl PluginLoopOutcome {
    pub fn success(&self) -> bool {
        matches!(self, Self::Completed { .. })
    }
}

/// `run_scoped` has to see this outcome's failure the way it sees an `Err`
/// (design §3.2 T3(b)): the loop reports a killed plugin as
/// `Failed { error: "plugin agent step failed: …process crashed" }`, which a
/// `Result`-only wrapper would pass through as a raw channel string.
impl ScopedRun for PluginLoopOutcome {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    fn rewrite_failure(self, refusal: String) -> Self {
        match self {
            Self::Failed {
                tool_calls_made, ..
            } => Self::Failed {
                error: refusal,
                tool_calls_made,
            },
            other => other,
        }
    }
}

/// Drive a plugin-backed agent to completion.
///
/// Mechanics (per the plugin agent protocol in
/// `openalpaca_api::plugin_traits::PluginAgentExecutor`):
/// 1. `spawn()` with the task instructions; a `false` return means the
///    plugin rejected the task.
/// 2. Poll `step()` up to [`MAX_PLUGIN_ITERATIONS`] times. Statuses:
///    - `"complete"` → done, output is the final content.
///    - `"failed"` → failed, output is the error.
///    - `"tool_request"` → execute each requested tool through the
///      sandbox and feed the results into the next `step()` call.
///      Sandbox denials/errors are returned to the plugin as the tool
///      result rather than aborting the loop.
///    - anything else (`"working"`) → wait [`POLL_INTERVAL`] and re-poll.
/// 3. The cancellation token is checked between steps and raced against
///    the poll sleep; `stop()` is sent to the plugin on every abnormal
///    exit (cancel, RPC error, iteration cap).
#[allow(clippy::too_many_arguments)]
pub async fn run_plugin_agent_loop(
    executor: &Arc<dyn PluginAgentExecutor>,
    instance_id: &str,
    task_id: &str,
    instructions: &str,
    context: &serde_json::Value,
    sandbox: &SandboxManager,
    sandbox_policy: &SandboxPolicy,
    tool_ctx: &ToolContext,
    cancel_token: Option<&CancellationToken>,
    scope: Option<&PluginRunScope>,
) -> PluginLoopOutcome {
    let accepted = match executor
        .spawn(instance_id, task_id, instructions, context)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            return PluginLoopOutcome::Failed {
                error: format!("plugin agent spawn failed: {e}"),
                tool_calls_made: 0,
            };
        }
    };
    if !accepted {
        return PluginLoopOutcome::Failed {
            error: "Plugin agent rejected the task".to_string(),
            tool_calls_made: 0,
        };
    }

    let mut tool_results: Option<serde_json::Value> = None;
    let mut total_tool_calls: usize = 0;

    for iteration in 0..MAX_PLUGIN_ITERATIONS {
        // Cancellation is checked between steps, mirroring the internal
        // agentic loop's per-round cancellation check.
        if let Some(token) = cancel_token
            && token.is_cancelled()
        {
            tracing::info!(instance_id, "plugin agent cancelled between steps");
            let _ = executor.stop(instance_id).await;
            return PluginLoopOutcome::Failed {
                error: "Cancelled".to_string(),
                tool_calls_made: total_tool_calls,
            };
        }

        // The step-boundary ledger check (design §3.2 T3(b)): a plugin that
        // left `Enabled` — disabled, denied, crashed — stops the loop here,
        // deliberately and with the S4 wording, rather than running until T4
        // closes the channel and the next RPC fails with a transport string.
        if let Some(scope) = scope
            && let Some(refusal) = scope.blocked()
        {
            tracing::info!(
                instance_id,
                extension = %scope.extension,
                "plugin agent stopped at a step boundary: the extension is no longer enabled"
            );
            let _ = executor.stop(instance_id).await;
            return PluginLoopOutcome::Failed {
                error: refusal,
                tool_calls_made: total_tool_calls,
            };
        }

        let (status, output, tool_calls) =
            match executor.step(instance_id, tool_results.as_ref()).await {
                Ok(v) => v,
                Err(e) => {
                    let _ = executor.stop(instance_id).await;
                    return PluginLoopOutcome::Failed {
                        error: format!("plugin agent step failed: {e}"),
                        tool_calls_made: total_tool_calls,
                    };
                }
            };

        match status.as_str() {
            "complete" => {
                tracing::info!(
                    instance_id,
                    iterations = iteration + 1,
                    tool_calls = total_tool_calls,
                    "plugin agent completed"
                );
                return PluginLoopOutcome::Completed {
                    content: output,
                    tool_calls_made: total_tool_calls,
                    iterations: iteration + 1,
                };
            }
            "failed" => {
                return PluginLoopOutcome::Failed {
                    error: output,
                    tool_calls_made: total_tool_calls,
                };
            }
            "tool_request" => {
                let mut results = Vec::with_capacity(tool_calls.len());
                for call in &tool_calls {
                    let name = call.get("tool").and_then(|t| t.as_str()).unwrap_or("");
                    let args = call.get("arguments").cloned().unwrap_or_default();
                    let tool_call = ToolCall {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: name.to_string(),
                        arguments: args,
                    };
                    // Sandboxed execute path: capability checks, input
                    // sanitization, confirmation gating, circuit breaker,
                    // and timeout all apply. Denials are fed back to the
                    // plugin as the tool result.
                    let result = sandbox
                        .execute_tool(&tool_call, sandbox_policy, tool_ctx)
                        .await;
                    results.push(serde_json::json!({
                        "tool": name,
                        "result": result.unwrap_or_else(|e| e),
                    }));
                    total_tool_calls += 1;
                }
                tool_results = Some(serde_json::json!(results));
            }
            _ => {
                // "working" — wait briefly then poll again, respecting
                // cancellation during the sleep.
                tool_results = None;
                if let Some(token) = cancel_token {
                    tokio::select! {
                        _ = tokio::time::sleep(POLL_INTERVAL) => {}
                        _ = token.cancelled() => {
                            tracing::info!(instance_id, "plugin agent cancelled during poll");
                            let _ = executor.stop(instance_id).await;
                            return PluginLoopOutcome::Failed {
                                error: "Cancelled".to_string(),
                                tool_calls_made: total_tool_calls,
                            };
                        }
                    }
                } else {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }

    let error = format!("Plugin agent exceeded max iterations ({MAX_PLUGIN_ITERATIONS})");
    tracing::error!(instance_id, "{}", error);
    let _ = executor.stop(instance_id).await;
    PluginLoopOutcome::Failed {
        error,
        tool_calls_made: total_tool_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
    use crate::tools::ToolRegistry;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// One scripted `step()` response: (status, output, tool_calls).
    type StepScript = (String, String, Vec<serde_json::Value>);

    /// Stub executor driven by a script of step responses; records what
    /// the daemon sends back.
    struct StubExecutor {
        accept: bool,
        steps: Mutex<VecDeque<StepScript>>,
        /// tool_results payloads received by `step()` (None entries kept).
        received_results: Mutex<Vec<Option<serde_json::Value>>>,
        stopped: Mutex<bool>,
    }

    impl StubExecutor {
        fn new(accept: bool, steps: Vec<StepScript>) -> Arc<Self> {
            Arc::new(Self {
                accept,
                steps: Mutex::new(steps.into()),
                received_results: Mutex::new(Vec::new()),
                stopped: Mutex::new(false),
            })
        }

        fn was_stopped(&self) -> bool {
            *self.stopped.lock().unwrap()
        }

        fn results_received(&self) -> Vec<Option<serde_json::Value>> {
            self.received_results.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PluginAgentExecutor for StubExecutor {
        async fn spawn(
            &self,
            _instance_id: &str,
            _task_id: &str,
            _instructions: &str,
            _context: &serde_json::Value,
        ) -> Result<bool, String> {
            Ok(self.accept)
        }

        async fn step(
            &self,
            _instance_id: &str,
            tool_results: Option<&serde_json::Value>,
        ) -> Result<(String, String, Vec<serde_json::Value>), String> {
            self.received_results
                .lock()
                .unwrap()
                .push(tool_results.cloned());
            let mut steps = self.steps.lock().unwrap();
            // Repeat the last scripted step when the script runs out
            // (lets the iteration-cap test script a single "working").
            let front = steps.pop_front();
            match front {
                Some(step) => {
                    if steps.is_empty() {
                        steps.push_back(step.clone());
                    }
                    Ok(step)
                }
                None => Err("script exhausted".to_string()),
            }
        }

        async fn stop(&self, _instance_id: &str) -> Result<(), String> {
            *self.stopped.lock().unwrap() = true;
            Ok(())
        }

        fn plugin_id(&self) -> &str {
            "stub_plugin"
        }

        fn agent_id(&self) -> &str {
            "stub_agent"
        }
    }

    struct EchoTool;

    #[async_trait]
    impl BuiltInTool for EchoTool {
        async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
            Ok(format!("echo:{}", arguments.get("q").and_then(|v| v.as_str()).unwrap_or("")))
        }
    }

    fn make_sandbox() -> (SandboxManager, Arc<ToolRegistry>) {
        let registry = ToolRegistry::default();
        registry
            .register(RegisteredTool {
                definition: openalpaca_llm::ToolDefinition {
                    name: "echo".to_string(),
                    description: "Echo tool".to_string(),
                    parameters: serde_json::json!({"type": "object"}),
                    strict: None,
                    input_examples: None,
                },
                backend: ToolBackend::BuiltIn(Arc::new(EchoTool)),
                provides_capabilities: vec![],
                exempt_from_timeout: false,
                annotations: None,
                version: "test-0.0.0".into(),
                author: "test".into(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        let registry = Arc::new(registry);
        let sandbox = SandboxManager::with_defaults(registry.clone(), EventBus::default());
        (sandbox, registry)
    }

    /// A run scope over a ledger the test drives directly.
    fn scope(ledger: &Arc<ExtensionLedger>, generation: u64) -> PluginRunScope {
        PluginRunScope {
            ledger: Arc::clone(ledger),
            extension: ExtensionId::plugin("notion"),
            generation,
        }
    }

    /// **The S4 refusal, not a channel string** (design §3.2 T3(b), §3.6 item 2).
    ///
    /// A plugin agent whose child is killed mid-`step` while the plugin is
    /// `Disabling` reports `PluginLoopOutcome::Failed { error: "plugin agent
    /// step failed: …process crashed" }` — **not** an `Err`, which is why
    /// `run_scoped` is generic over [`ScopedRun`] and not over `Result`. The
    /// caller must see *"is being turned off right now"*, never the transport.
    #[tokio::test(start_paused = true)]
    async fn a_plugin_agent_killed_mid_step_while_disabling_gets_the_s4_refusal() {
        let ledger = Arc::new(ExtensionLedger::new());
        let ext = ExtensionId::plugin("notion");
        ledger.upsert(&ext, true, crate::tools::extensions::ExtensionState::Enabled);
        let generation = ledger.generation(&ext).unwrap();

        // The child dies on the first `step`, exactly as a T4 kill leaves it.
        let executor = StubExecutor::new(true, vec![]);
        let exec: Arc<dyn PluginAgentExecutor> = executor.clone();
        let (sandbox, _registry) = make_sandbox();

        // The toggle flips while the run is in flight.
        ledger.begin(
            &ext,
            crate::tools::extensions::ExtensionState::Disabling,
            Some(crate::tools::extensions::WithdrawalCause::Disable),
        );

        let outcome = ledger
            .run_scoped(
                &ext,
                run_plugin_agent_loop(
                    &exec,
                    "plugin-instance",
                    "task-1",
                    "do the thing",
                    &serde_json::json!({}),
                    &sandbox,
                    &policy(),
                    &ToolContext::default(),
                    None,
                    Some(&scope(&ledger, generation)),
                ),
            )
            .await;

        let PluginLoopOutcome::Failed { error, .. } = outcome else {
            panic!("a run against a disabling plugin must not succeed");
        };
        assert!(
            error.contains("is being turned off right now"),
            "expected the S4 refusal, got: {error}"
        );
        assert!(!error.contains("process crashed"), "{error}");
        assert!(!error.contains("script exhausted"), "{error}");
        assert!(
            *executor.stopped.lock().unwrap(),
            "the loop did not stop the plugin instance deliberately"
        );
    }

    /// The step-boundary check is what makes the stop *deliberate*: the loop
    /// terminates at its next step rather than waiting for T4 to close the
    /// channel under it.
    #[tokio::test(start_paused = true)]
    async fn a_plugin_agent_stops_at_the_next_step_boundary_when_the_plugin_is_disabled() {
        let ledger = Arc::new(ExtensionLedger::new());
        let ext = ExtensionId::plugin("notion");
        ledger.upsert(&ext, true, crate::tools::extensions::ExtensionState::Enabled);
        let generation = ledger.generation(&ext).unwrap();

        // A script that would otherwise run to completion.
        let executor = StubExecutor::new(
            true,
            vec![("working".into(), String::new(), vec![])],
        );
        let exec: Arc<dyn PluginAgentExecutor> = executor.clone();
        let (sandbox, _registry) = make_sandbox();

        ledger.upsert(&ext, false, crate::tools::extensions::ExtensionState::Disabled);

        let outcome = run_plugin_agent_loop(
            &exec,
            "plugin-instance",
            "task-1",
            "do the thing",
            &serde_json::json!({}),
            &sandbox,
            &policy(),
            &ToolContext::default(),
            None,
            Some(&scope(&ledger, generation)),
        )
        .await;

        let PluginLoopOutcome::Failed { error, .. } = outcome else {
            panic!("the loop ran on against a disabled plugin");
        };
        assert!(error.contains("disabled by the owner"), "{error}");
        assert!(*executor.stopped.lock().unwrap());
        assert!(
            executor.results_received().is_empty(),
            "the loop took a step after the plugin was disabled"
        );
    }

    fn policy() -> SandboxPolicy {
        // Template-scoped, like any spawned agent: the plugin agent may call
        // exactly the tool its template declared.
        SandboxPolicy::from_constraints(
            "plugin-instance",
            &crate::agent::subagent::AgentConstraints {
                allowed_capabilities: vec!["echo".to_string()],
                ..Default::default()
            },
        )
    }

    async fn run(
        executor: &Arc<StubExecutor>,
        cancel_token: Option<&CancellationToken>,
    ) -> PluginLoopOutcome {
        let (sandbox, _registry) = make_sandbox();
        let exec: Arc<dyn PluginAgentExecutor> = executor.clone();
        run_plugin_agent_loop(
            &exec,
            "plugin-instance",
            "task-1",
            "do the thing",
            &serde_json::json!({}),
            &sandbox,
            &policy(),
            &ToolContext::default(),
            cancel_token,
            // No run scope: these stubs are not backed by a supervisor, so the
            // step-boundary ledger check has nothing to consult.
            None,
        )
        .await
    }

    #[tokio::test(start_paused = true)]
    async fn test_spawn_steps_finish() {
        let executor = StubExecutor::new(
            true,
            vec![
                ("working".into(), String::new(), vec![]),
                ("complete".into(), "final answer".into(), vec![]),
            ],
        );
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Completed {
                content,
                tool_calls_made,
                iterations,
            } => {
                assert_eq!(content, "final answer");
                assert_eq!(tool_calls_made, 0);
                assert_eq!(iterations, 2);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert!(!executor.was_stopped());
    }

    #[tokio::test]
    async fn test_rejected_spawn_fails() {
        let executor = StubExecutor::new(false, vec![]);
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => {
                assert!(error.contains("rejected"), "got: {error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_failed_status_maps_to_failure() {
        let executor = StubExecutor::new(
            true,
            vec![("failed".into(), "plugin blew up".into(), vec![])],
        );
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => assert_eq!(error, "plugin blew up"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_tool_proxy_roundtrip() {
        // The plugin requests "echo"; the sandboxed result must flow back
        // into the next step() call's tool_results.
        let executor = StubExecutor::new(
            true,
            vec![
                (
                    "tool_request".into(),
                    String::new(),
                    vec![serde_json::json!({"tool": "echo", "arguments": {"q": "hi"}})],
                ),
                ("complete".into(), "used the tool".into(), vec![]),
            ],
        );
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Completed {
                content,
                tool_calls_made,
                ..
            } => {
                assert_eq!(content, "used the tool");
                assert_eq!(tool_calls_made, 1);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        let received = executor.results_received();
        assert_eq!(received.len(), 2);
        assert!(received[0].is_none(), "first step carries no results");
        let results = received[1].as_ref().expect("second step carries results");
        assert_eq!(results[0]["tool"], "echo");
        assert_eq!(results[0]["result"], "echo:hi");
    }

    #[tokio::test]
    async fn test_denied_tool_result_is_fed_back_not_fatal() {
        // A capability-denied tool must surface the violation to the
        // plugin as the tool result, not abort the loop.
        let executor = StubExecutor::new(
            true,
            vec![
                (
                    "tool_request".into(),
                    String::new(),
                    vec![serde_json::json!({"tool": "echo", "arguments": {"q": "hi"}})],
                ),
                ("complete".into(), "done".into(), vec![]),
            ],
        );
        let (sandbox, _registry) = make_sandbox();
        let mut denying_policy = policy();
        denying_policy.denied_capabilities = vec!["echo".to_string()];
        let exec: Arc<dyn PluginAgentExecutor> = executor.clone();
        let outcome = run_plugin_agent_loop(
            &exec,
            "plugin-instance",
            "task-1",
            "do the thing",
            &serde_json::json!({}),
            &sandbox,
            &denying_policy,
            &ToolContext::default(),
            None,
            None,
        )
        .await;
        assert!(outcome.success());
        let received = executor.results_received();
        let result = received[1].as_ref().unwrap()[0]["result"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            result.to_lowercase().contains("denied") || result.to_lowercase().contains("echo"),
            "denial should be visible in the proxied result, got: {result}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_iteration_cap_stops_plugin() {
        // Always "working" — the loop must give up at the cap and stop
        // the plugin instance. start_paused auto-advances the poll sleeps.
        let executor = StubExecutor::new(true, vec![("working".into(), String::new(), vec![])]);
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => {
                assert!(error.contains("exceeded max iterations"), "got: {error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(executor.was_stopped());
        assert_eq!(executor.results_received().len(), MAX_PLUGIN_ITERATIONS);
    }

    #[tokio::test(start_paused = true)]
    async fn test_cancellation_mid_loop_stops_plugin() {
        let executor = StubExecutor::new(true, vec![("working".into(), String::new(), vec![])]);
        let token = CancellationToken::new();
        let cancel = token.clone();
        // Cancel shortly after the loop starts polling.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            cancel.cancel();
        });
        let outcome = run(&executor, Some(&token)).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => assert_eq!(error, "Cancelled"),
            other => panic!("expected Failed(Cancelled), got {other:?}"),
        }
        assert!(executor.was_stopped());
    }

    #[tokio::test]
    async fn test_pre_cancelled_token_short_circuits() {
        let executor = StubExecutor::new(true, vec![("working".into(), String::new(), vec![])]);
        let token = CancellationToken::new();
        token.cancel();
        let outcome = run(&executor, Some(&token)).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => assert_eq!(error, "Cancelled"),
            other => panic!("expected Failed(Cancelled), got {other:?}"),
        }
        // Cancelled before the first step() — nothing polled.
        assert!(executor.results_received().is_empty());
        assert!(executor.was_stopped());
    }

    #[tokio::test]
    async fn test_step_rpc_error_stops_plugin() {
        // Empty script: the stub's step() errors immediately.
        let executor = StubExecutor::new(true, vec![]);
        let outcome = run(&executor, None).await;
        match outcome {
            PluginLoopOutcome::Failed { error, .. } => {
                assert!(error.contains("step failed"), "got: {error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(executor.was_stopped());
    }
}
