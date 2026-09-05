//! Generic `invoke_skill` builtin (tool/skill wiring, Chunk 1).
//!
//! A per-request tool letting the main conversational loop and the lead agent
//! invoke any catalog skill by id or slash-command name. Execution is a thin
//! adapter over the existing nested-skill machinery
//! ([`SkillInvocationToolExecutor`]) — same sandbox policy, global tool deny,
//! depth/cycle guards, and budget threading as `invoke_skill:*` dependency
//! calls — wrapped with the chat tier's lifecycle events
//! (`SkillInvocationStarted` / `SkillCompleted` / `SkillFailed`) and
//! skill-execution telemetry (mirroring `orchestrator/skill/handler.rs`).
//!
//! Like the other per-request builtins in this module, it is constructed fresh
//! per request and injected into a per-request registry — never registered
//! globally.

use crate::bus::EventBus;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::orchestrator::skill::handler::finish_reason_to_string;
use crate::orchestrator::skill::invoke_executor::SkillInvocationToolExecutor;
use crate::orchestrator::skill_catalog::SkillCatalog;
use crate::runner::LoopFinishReason;
use crate::tools::registry::{BuiltInTool, ToolContext, ToolRegistry};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::{LlmRouter, ToolDefinition};
use openalpaca_storage::SkillExecutionEntry;
use openalpaca_storage::repository::SkillExecutionRepository;
use std::sync::Arc;
use uuid::Uuid;

/// Maximum skill nesting depth for chains started through this tool.
/// Matches the bound the chat tier passes to `SkillInvocationToolExecutor`
/// in `orchestrator/skill/invocation.rs`.
const MAX_NESTING_DEPTH: usize = 3;

/// Tool definition for `invoke_skill`.
pub fn invoke_skill_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "invoke_skill".to_string(),
        description: "Invoke a skill from the skill catalog. The skill runs in its own \
            sandboxed loop with only its declared tools and returns its output text. \
            Use this when a task matches an available skill's purpose."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Skill id or slash-command name (leading '/' optional, e.g. 'review' or '/review')"
                },
                "query": {
                    "type": "string",
                    "description": "The input/query to pass to the skill"
                }
            },
            "required": ["skill", "query"]
        }),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "skill": "review",
            "query": "Review the error handling in src/routes/chat.rs"
        })]),
    }
}

/// Per-request builtin executing catalog skills via the nested-skill executor.
pub struct InvokeSkillTool {
    catalog: Arc<SkillCatalog>,
    tool_registry: Arc<ToolRegistry>,
    router: Arc<LlmRouter>,
    bus: EventBus,
    /// For skill-execution telemetry; `None` skips persistence (events still emitted).
    db: Option<openalpaca_storage::Database>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    /// Budget ceiling (USD) for one skill invocation — callers pass their
    /// loop config's `max_cost`.
    max_cost: f64,
}

impl InvokeSkillTool {
    pub fn new(
        catalog: Arc<SkillCatalog>,
        tool_registry: Arc<ToolRegistry>,
        router: Arc<LlmRouter>,
        bus: EventBus,
        db: Option<openalpaca_storage::Database>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        max_cost: f64,
    ) -> Self {
        Self {
            catalog,
            tool_registry,
            router,
            bus,
            db,
            daemon_config,
            max_cost,
        }
    }

    /// The miss answer.
    ///
    /// Two changes from the bare "unknown skill" dump (design §6.2 #12,
    /// §10 case 5(a)): the **tombstone** is consulted first, so a skill a
    /// disabled plugin used to provide is attributed to that plugin instead of
    /// reading as a typo; and the listing is **availability-filtered**, so it
    /// never names a skill this tool would refuse.
    fn unknown_skill_error(&self, requested: &str) -> String {
        if let Some(tomb) = self.catalog.tombstone(requested) {
            return self.tool_registry.withdrawn_contribution_refusal(
                "Skill",
                &tomb.skill_id,
                &tomb.plugin_id,
            );
        }
        let mut names = self.catalog.available_names();
        names.sort();
        if names.is_empty() {
            format!("Unknown skill '{}'. No skills are available.", requested)
        } else {
            format!(
                "Unknown skill '{}'. Available skills: {}",
                requested,
                names.join(", ")
            )
        }
    }
}

#[async_trait]
impl BuiltInTool for InvokeSkillTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("invoke_skill requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let skill_arg = arguments
            .get("skill")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "invoke_skill requires a 'skill' parameter".to_string())?;
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "invoke_skill requires a 'query' parameter".to_string())?;

        // Resolve via slash command / alias, falling back to skill id
        // (get_by_command covers all three). Disabled skills are dropped at
        // catalog-scan time, so they resolve to the unknown-skill error; the
        // explicit mode check below is defense in depth for entries injected
        // by other paths (e.g. plugin skill registration).
        let command = skill_arg.strip_prefix('/').unwrap_or(skill_arg);
        let entry = self
            .catalog
            .get_by_command(command)
            .ok_or_else(|| self.unknown_skill_error(skill_arg))?;
        if entry.frontmatter.invoke.mode == "disabled" {
            return Err(format!(
                "Skill '{}' is disabled (invoke.mode = \"disabled\")",
                entry.frontmatter.name
            ));
        }
        // Use the frontmatter name as the invocation identity — the same
        // string the chat tier passes to handle_skill_invocation, so the
        // executor's catalog lookups and cycle checks stay consistent.
        let skill_name = entry.frontmatter.name.clone();

        let request_id = ctx.request_id.unwrap_or_else(Uuid::new_v4);
        let invocation_start = std::time::Instant::now();

        self.bus.publish(SystemEvent::SkillInvocationStarted {
            request_id,
            skill_id: skill_name.clone(),
            query_preview: query.chars().take(100).collect(),
            timestamp: Utc::now(),
        });

        // Snapshot hot-reloadable config values before the await.
        let (auto_approve, global_tool_deny, circuit_breaker, confirmation_timeout_secs) = {
            let cfg = self.daemon_config.load();
            (
                cfg.security.auto_approve_confirmations,
                cfg.execution.skill_defaults.global_tool_deny.clone(),
                cfg.security.circuit_breaker.clone(),
                cfg.execution.agent_defaults.confirmation_timeout_secs,
            )
        };

        // The executor enforces depth/cycle limits against the call stack and
        // the ToolContext skill stack, so skill -> invoke_skill -> skill
        // chains terminate. Seeding the call stack from ctx.skill_stack makes
        // both guards see the full chain even when this tool is reached from
        // inside a running skill.
        let executor = SkillInvocationToolExecutor::new(
            self.catalog.clone(),
            self.tool_registry.clone(),
            self.router.clone(),
            self.bus.clone(),
            ctx.skill_stack.clone(),
            MAX_NESTING_DEPTH,
            None, // cancel_token — conversational invocations are not cancellable (matches invocation.rs)
            None, // cost_accumulator — fresh budget per invocation
            Some(ctx.clone()),
            self.max_cost,
            auto_approve,
            global_tool_deny,
            circuit_breaker,
            confirmation_timeout_secs,
        );

        // Map an error finish with no content to Err BEFORE events/telemetry,
        // exactly like the chat tier (invocation.rs) does before handler.rs
        // emits — so this failure mode publishes SkillFailed and records
        // status "error", not a bogus completion. A loop that finished with
        // an error reason but still produced content is treated as a
        // completion, matching the chat tier's pass-through behavior.
        let result = executor
            .execute_detailed(&format!("invoke_skill:{}", skill_name), arguments)
            .await
            .and_then(|r| {
                if let LoopFinishReason::Error(ref err) = r.finish_reason
                    && r.final_content.trim().is_empty()
                {
                    Err(format!("LLM error: {}", err))
                } else {
                    Ok(r)
                }
            });

        let duration_ms = invocation_start.elapsed().as_millis() as u64;

        // Lifecycle events, mirroring handle_skill_invocation.
        match &result {
            Ok(loop_result) => {
                self.bus.publish(SystemEvent::SkillCompleted {
                    request_id,
                    skill_id: skill_name.clone(),
                    duration_ms,
                    output_preview: loop_result.final_content.chars().take(200).collect(),
                    timestamp: Utc::now(),
                });
            }
            Err(error) => {
                self.bus.publish(SystemEvent::SkillFailed {
                    request_id,
                    skill_id: skill_name.clone(),
                    error: error.clone(),
                    timestamp: Utc::now(),
                });
            }
        }

        // Persist skill-execution telemetry, mirroring handle_skill_invocation.
        // Output validation/repair does not run on this path (same as nested
        // dependency invocations), so the repair fields stay false.
        if let Some(ref db) = self.db {
            let store_preview = self.daemon_config.load().telemetry.store_query_preview;
            let telemetry = SkillExecutionEntry {
                id: None,
                request_id: request_id.to_string(),
                skill_id: skill_name.clone(),
                agent_id: ctx
                    .agent_id
                    .clone()
                    .unwrap_or_else(|| "orchestrator".to_string()),
                status: match &result {
                    Ok(_) => "success".to_string(),
                    Err(_) => "error".to_string(),
                },
                finish_reason: result
                    .as_ref()
                    .ok()
                    .map(|r| finish_reason_to_string(&r.finish_reason).to_string()),
                error_message: result.as_ref().err().cloned(),
                validation_failures: None,
                duration_ms: duration_ms as i64,
                rounds_used: result.as_ref().ok().map(|r| r.rounds_used as i32),
                tool_calls_made: result.as_ref().ok().map(|r| r.tool_calls_made as i32),
                input_tokens: result
                    .as_ref()
                    .ok()
                    .map(|r| r.total_input_tokens as i32)
                    .unwrap_or(0),
                output_tokens: result
                    .as_ref()
                    .ok()
                    .map(|r| r.total_output_tokens as i32)
                    .unwrap_or(0),
                cost_usd: result.as_ref().ok().map(|r| r.estimated_cost).unwrap_or(0.0),
                model_used: result.as_ref().ok().and_then(|r| r.model_used.clone()),
                query_preview: if store_preview {
                    Some(query.chars().take(200).collect())
                } else {
                    None
                },
                route_score: None,
                was_auto_selected: false,
                repair_attempted: false,
                repair_succeeded: false,
                timestamp: None,
            };
            if let Err(e) = SkillExecutionRepository::new(db).record(&telemetry) {
                tracing::warn!("Failed to persist skill telemetry: {e}");
            }
        }

        result.map(|r| r.final_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::skill::SkillScope;
    use openalpaca_llm::{
        ChatMessage, ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, ProviderType,
        Usage,
    };
    use std::io::Write;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    /// Mock provider that always finishes in one round with fixed content.
    struct MockProvider {
        call_count: AtomicUsize,
        requests: Mutex<Vec<Vec<ChatMessage>>>,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.requests
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(request.messages.as_ref().clone());
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ChatResponse {
                content: "skill output".to_string(),
                tool_calls: vec![],
                model: "claude-sonnet-4-20250514".to_string(),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
    }

    fn write_skill(parent: &std::path::Path, dir_name: &str, skill_md: &str) {
        let dir = parent.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join("SKILL.md")).unwrap();
        f.write_all(skill_md.as_bytes()).unwrap();
    }

    fn fixture_catalog(tmp: &TempDir) -> Arc<SkillCatalog> {
        write_skill(
            tmp.path(),
            "echo-skill",
            r#"---
name: "Echo Skill"
description: "Echoes the query back"
---
Echo the user's query back to them.
"#,
        );
        let catalog = Arc::new(SkillCatalog::new());
        catalog.scan_directory(tmp.path(), SkillScope::Project);
        catalog
    }

    fn tool_with(catalog: Arc<SkillCatalog>, provider: Arc<MockProvider>) -> InvokeSkillTool {
        let registry = Arc::new(ToolRegistry::new().unwrap());
        let router = Arc::new(LlmRouter::single_provider(
            provider,
            ProviderType::Anthropic,
            "claude-sonnet-4-20250514".to_string(),
        ));
        InvokeSkillTool::new(
            catalog,
            registry,
            router,
            EventBus::new(64),
            None,
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            1.0,
        )
    }

    fn mock_provider() -> Arc<MockProvider> {
        Arc::new(MockProvider {
            call_count: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn drain_events(rx: &mut tokio::sync::broadcast::Receiver<SystemEvent>) -> Vec<SystemEvent> {
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        events
    }

    #[tokio::test]
    async fn test_invoke_skill_returns_output_and_emits_events() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with(fixture_catalog(&tmp), mock_provider());
        let mut rx = tool.bus.subscribe();

        let out = tool
            .execute_with_context(
                &serde_json::json!({"skill": "echo-skill", "query": "say hi"}),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out, "skill output");

        let events = drain_events(&mut rx);
        assert!(
            events.iter().any(|e| matches!(
                e,
                SystemEvent::SkillInvocationStarted { skill_id, .. } if skill_id == "Echo Skill"
            )),
            "missing SkillInvocationStarted: {:?}",
            events
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                SystemEvent::SkillCompleted { skill_id, output_preview, .. }
                    if skill_id == "Echo Skill" && output_preview == "skill output"
            )),
            "missing SkillCompleted: {:?}",
            events
        );
    }

    #[tokio::test]
    async fn test_invoke_skill_resolves_slash_prefix() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with(fixture_catalog(&tmp), mock_provider());

        let out = tool
            .execute_with_context(
                &serde_json::json!({"skill": "/echo-skill", "query": "say hi"}),
                &ToolContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(out, "skill output");
    }

    #[tokio::test]
    async fn test_unknown_skill_error_lists_available_names() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with(fixture_catalog(&tmp), mock_provider());

        let err = tool
            .execute_with_context(
                &serde_json::json!({"skill": "nope", "query": "x"}),
                &ToolContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("Unknown skill 'nope'"), "err: {}", err);
        assert!(err.contains("echo-skill"), "err should list names: {}", err);
    }

    #[tokio::test]
    async fn test_disabled_skill_rejected() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "disabled-skill",
            r#"---
name: "Disabled Skill"
description: "Should never run"
invoke:
  mode: disabled
---
Never runs.
"#,
        );
        let tool = tool_with(fixture_catalog(&tmp), mock_provider());

        // Disabled skills are dropped at catalog-scan time, so the invocation
        // is rejected as unavailable (and never reaches the provider).
        let err = tool
            .execute_with_context(
                &serde_json::json!({"skill": "disabled-skill", "query": "x"}),
                &ToolContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("Unknown skill 'disabled-skill'"), "err: {}", err);
    }

    #[tokio::test]
    async fn test_depth_limit_terminates_recursion() {
        let tmp = TempDir::new().unwrap();
        let provider = mock_provider();
        let tool = tool_with(fixture_catalog(&tmp), provider.clone());
        let mut rx = tool.bus.subscribe();

        // Simulate being MAX_NESTING_DEPTH skills deep already: the executor's
        // depth guard must reject before any LLM call.
        let ctx = ToolContext {
            skill_stack: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            ..ToolContext::default()
        };
        let err = tool
            .execute_with_context(
                &serde_json::json!({"skill": "echo-skill", "query": "x"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("Max skill nesting depth"), "err: {}", err);
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 0);

        let events = drain_events(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SystemEvent::SkillFailed { .. })),
            "missing SkillFailed: {:?}",
            events
        );
    }

    #[tokio::test]
    async fn test_cycle_detection_terminates_recursion() {
        let tmp = TempDir::new().unwrap();
        let provider = mock_provider();
        let tool = tool_with(fixture_catalog(&tmp), provider.clone());

        // The invoked skill is already on the invocation chain — the
        // executor's cycle guard must reject it.
        let ctx = ToolContext {
            skill_stack: vec!["Echo Skill".to_string()],
            ..ToolContext::default()
        };
        let err = tool
            .execute_with_context(
                &serde_json::json!({"skill": "echo-skill", "query": "x"}),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("Circular skill invocation"), "err: {}", err);
        assert_eq!(provider.call_count.load(Ordering::SeqCst), 0);
    }

    /// Provider that always fails, driving the nested loop to an error finish
    /// with empty content.
    struct FailingProvider;

    #[async_trait]
    impl LlmProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }

        fn supports_tools(&self) -> bool {
            true
        }

        async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
            Err(LlmError::Http("provider down".to_string()))
        }
    }

    #[tokio::test]
    async fn test_error_finish_with_no_content_fails_like_chat_tier() {
        let tmp = TempDir::new().unwrap();
        let catalog = fixture_catalog(&tmp);
        let registry = Arc::new(ToolRegistry::new().unwrap());
        let router = Arc::new(LlmRouter::single_provider(
            Arc::new(FailingProvider),
            ProviderType::Anthropic,
            "claude-sonnet-4-20250514".to_string(),
        ));
        let tool = InvokeSkillTool::new(
            catalog,
            registry,
            router,
            EventBus::new(64),
            None,
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            1.0,
        );
        let mut rx = tool.bus.subscribe();

        let err = tool
            .execute_with_context(
                &serde_json::json!({"skill": "echo-skill", "query": "x"}),
                &ToolContext::default(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("LLM error"), "err: {}", err);

        // Parity with the chat tier: this failure mode publishes SkillFailed,
        // never SkillCompleted.
        let events = drain_events(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SystemEvent::SkillFailed { .. })),
            "missing SkillFailed: {:?}",
            events
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SystemEvent::SkillCompleted { .. })),
            "SkillCompleted must not fire on an empty error finish: {:?}",
            events
        );
    }

    #[test]
    fn test_tool_definition_shape() {
        let def = invoke_skill_tool_definition();
        assert_eq!(def.name, "invoke_skill");
        let required: Vec<&str> = def.parameters["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(required, vec!["skill", "query"]);
    }
}
