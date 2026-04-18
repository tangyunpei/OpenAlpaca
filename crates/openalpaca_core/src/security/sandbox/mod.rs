//! Layer 3: Tool execution sandbox.
//!
//! Wraps tool execution with capability checks, input sanitization,
//! circuit breaker protection, timeout enforcement, and event emission.

use crate::agent::subagent::AgentConstraints;
use crate::bus::EventBus;
use crate::daemon_config::CircuitBreakerConfig;
use crate::events::SystemEvent;
use crate::security::capabilities::CapabilityManager;
use crate::security::circuit_breaker::{ToolCircuitBreaker, is_transient_tool_error};
use crate::security::confirmation::{ConfirmationBroker, ConfirmationRequest};
use crate::security::sanitizer::InputSanitizer;
use crate::tools::registry::ToolContext;
use crate::tools::ToolRegistry;
use chrono::Utc;
use openalpaca_llm::ToolCall;
use std::sync::Arc;
use std::time::Duration;

/// Policy governing what a sandboxed agent can do.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub agent_id: String,
    pub allowed_capabilities: Vec<String>,
    pub denied_capabilities: Vec<String>,
    pub require_confirmation_for: Vec<String>,
    pub max_tool_calls: Option<u32>,
    pub max_tool_runtime_secs: u64,
    /// SSE stream ID for routing confirmation prompts to the active chat stream.
    pub stream_id: Option<String>,
    /// Lane key for routing confirmation prompts to connectors (e.g. Telegram).
    pub lane_key: Option<String>,
    /// Seconds to wait for user confirmation before timing out (default: 300).
    pub confirmation_timeout_secs: Option<u64>,
    /// When true, skip interactive confirmations (from global config or per-agent).
    pub auto_approve: bool,
}

impl SandboxPolicy {
    /// Build a SandboxPolicy from agent constraints.
    pub fn from_constraints(agent_id: &str, constraints: &AgentConstraints) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            allowed_capabilities: constraints.allowed_capabilities.clone(),
            denied_capabilities: constraints.denied_capabilities.clone(),
            require_confirmation_for: constraints.require_confirmation_for.clone(),
            max_tool_calls: constraints.max_tool_calls,
            max_tool_runtime_secs: constraints.timeout_seconds.unwrap_or(60),
            stream_id: None,
            lane_key: None,
            confirmation_timeout_secs: None,
            auto_approve: constraints.auto_approve,
        }
    }
}

/// Manages sandboxed tool execution with security checks.
pub struct SandboxManager {
    registry: Arc<ToolRegistry>,
    bus: EventBus,
    circuit_breaker: ToolCircuitBreaker,
    /// Optional database for persisting security violation audit logs.
    db: Option<openalpaca_storage::Database>,
    /// Optional broker for interactive tool confirmation.
    /// When present, tools in `require_confirmation_for` pause and await user approval.
    /// When absent, those tools are fail-closed (blocked immediately).
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
    /// Session-scoped approval cache. Invocations pre-approved by the user
    /// (scoped to args or to the whole tool) bypass the confirmation prompt.
    /// Cleared on daemon restart.
    approval_cache: crate::security::confirmation::ApprovalCache,
}

impl SandboxManager {
    /// Create a new SandboxManager with a specific circuit breaker configuration.
    pub fn new(
        registry: Arc<ToolRegistry>,
        bus: EventBus,
        circuit_breaker_config: &CircuitBreakerConfig,
    ) -> Self {
        let circuit_breaker = ToolCircuitBreaker::new(circuit_breaker_config, bus.clone());
        Self {
            registry,
            bus,
            circuit_breaker,
            db: None,
            confirmation_broker: None,
            approval_cache: crate::security::confirmation::ApprovalCache::new(),
        }
    }

    /// Create a new SandboxManager with a database for audit logging.
    pub fn with_db(
        registry: Arc<ToolRegistry>,
        bus: EventBus,
        circuit_breaker_config: &CircuitBreakerConfig,
        db: openalpaca_storage::Database,
    ) -> Self {
        let circuit_breaker = ToolCircuitBreaker::new(circuit_breaker_config, bus.clone());
        Self {
            registry,
            bus,
            circuit_breaker,
            db: Some(db),
            confirmation_broker: None,
            approval_cache: crate::security::confirmation::ApprovalCache::new(),
        }
    }

    /// Create a new SandboxManager with default circuit breaker settings.
    ///
    /// Used by internal per-request sandbox instances (query handler, skill handler,
    /// DAG executor, lead agent) where no custom config is needed.
    pub fn with_defaults(registry: Arc<ToolRegistry>, bus: EventBus) -> Self {
        Self::new(registry, bus, &CircuitBreakerConfig::default())
    }

    /// Set the confirmation broker for interactive tool approval.
    pub fn set_confirmation_broker(&mut self, broker: Arc<ConfirmationBroker>) {
        self.confirmation_broker = Some(broker);
    }

    /// Execute a tool call within the sandbox.
    ///
    /// Flow:
    /// 1. Capability check (deny/allow lists)
    /// 2. Input sanitization (path traversal, command injection)
    /// 3. Confirmation check (fail-closed if tool requires confirmation)
    /// 4. Circuit breaker check (block if tool has too many recent failures)
    /// 5. Timeout-wrapped execution
    /// 6. Record outcome for circuit breaker
    /// 7. Event emission (ToolExecuted or SecurityViolation)
    pub async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        policy: &SandboxPolicy,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let agent_id = ctx.agent_id.as_deref().unwrap_or("unknown");

        // 1. Capability check
        let constraints = AgentConstraints {
            allowed_capabilities: policy.allowed_capabilities.clone(),
            denied_capabilities: policy.denied_capabilities.clone(),
            ..Default::default()
        };

        if let Err(violation) =
            CapabilityManager::check_agent_capability(agent_id, &tool_call.name, &constraints)
        {
            self.emit_security_violation(agent_id, &tool_call.name, &violation.to_string());
            return Err(violation.to_string());
        }

        // 2. Input sanitization
        let registered = self.registry.registered_tool_names();
        let shell_like = self.registry.command_backend_tool_names();
        if let Err(violation) = InputSanitizer::sanitize_tool_args(
            &tool_call.name,
            &tool_call.arguments,
            &registered,
            &shell_like,
        ) {
            self.emit_security_violation(agent_id, &tool_call.name, &violation.to_string());
            return Err(violation.to_string());
        }

        // 3. Confirmation check — driven by effective set (annotations or policy).
        //    An explicit non-empty `require_confirmation_for` wins over annotation
        //    hints; otherwise we derive the set from `destructive_hint=true` tools.
        let args_hash = crate::security::confirmation::hash_canonical_args(&tool_call.arguments);
        let confirmation_set = effective_confirmation_set(policy, &self.registry);

        if confirmation_set.iter().any(|t| t == &tool_call.name) {
            // Check the approval cache first — a prior user approval (this session)
            // can bypass the prompt when scoped to these args or the whole tool.
            if self.approval_cache.is_approved(&tool_call.name, args_hash) {
                tracing::debug!(
                    agent_id,
                    tool = %tool_call.name,
                    args_hash,
                    "Invocation pre-approved (cache hit)"
                );
                // Fall through to circuit breaker + execution.
            } else if policy.auto_approve {
                tracing::info!(
                    agent_id,
                    tool = %tool_call.name,
                    "Tool auto-approved (policy bypass)"
                );
                // Audit: persist auto-approve decision to event_log
                if let Some(ref db) = self.db {
                    let detail = serde_json::json!({
                        "tool_name": tool_call.name,
                        "reason": "auto_approve policy bypass",
                    });
                    let result = serde_json::json!({ "outcome": "auto_approved" });
                    let repo = openalpaca_storage::repository::EventLogRepository::new(db);
                    if let Err(e) = repo.log(
                        "tool_auto_approved",
                        Some(agent_id),
                        Some(&detail),
                        Some(&result),
                    ) {
                        tracing::warn!("Failed to persist auto-approve audit log: {e}");
                    }
                }
                // Fall through to circuit breaker + execution
            } else if let Some(ref broker) = self.confirmation_broker {
                let request_id = uuid::Uuid::new_v4().to_string();
                let request = ConfirmationRequest {
                    request_id: request_id.clone(),
                    agent_id: agent_id.to_string(),
                    tool_name: tool_call.name.clone(),
                    tool_arguments: tool_call.arguments.clone(),
                    stream_id: policy.stream_id.clone(),
                    lane_key: policy.lane_key.clone(),
                    timestamp: Utc::now(),
                };

                // Register with broker BEFORE publishing event to avoid race
                // where a fast client responds before the oneshot is registered.
                let rx = broker.request(&request);

                // Publish event for WebSocket/SSE clients
                self.bus.publish(SystemEvent::ToolConfirmationRequested {
                    request_id: request_id.clone(),
                    agent_id: agent_id.to_string(),
                    tool_name: tool_call.name.clone(),
                    tool_arguments: tool_call.arguments.clone(),
                    stream_id: policy.stream_id.clone(),
                    lane_key: policy.lane_key.clone(),
                    timestamp: Utc::now(),
                });
                let timeout_secs = policy.confirmation_timeout_secs.unwrap_or(300);
                let timeout = Duration::from_secs(timeout_secs);

                tracing::info!(
                    agent_id,
                    tool = %tool_call.name,
                    "Tool requires confirmation — awaiting user response (timeout: {timeout_secs}s)"
                );

                match tokio::time::timeout(timeout, rx).await {
                    Ok(Ok(resp)) if resp.approved => {
                        tracing::info!(agent_id, tool = %tool_call.name, "Tool approved by user");
                        // Record the approval so subsequent invocations skip the prompt.
                        // Default to TheseArgs (safest) when the caller omits a scope.
                        let scope = resp.approval_scope.unwrap_or(
                            crate::security::confirmation::ApprovalScope::TheseArgs,
                        );
                        self.approval_cache
                            .record(&tool_call.name, args_hash, scope);
                        // Fall through to circuit breaker + execution
                    }
                    Ok(Ok(_)) => {
                        let reason = format!("Tool '{}' denied by user", tool_call.name);
                        tracing::info!(agent_id, tool = %tool_call.name, "Tool denied by user");
                        return Err(reason);
                    }
                    Ok(Err(_)) => {
                        return Err("Confirmation request cancelled".to_string());
                    }
                    Err(_) => {
                        broker.cancel(&request_id);
                        return Err(format!(
                            "Tool '{}' confirmation timed out after {timeout_secs}s",
                            tool_call.name
                        ));
                    }
                }
            } else {
                // No broker — preserve original fail-closed behavior
                let reason = format!(
                    "Tool '{}' requires human confirmation but no interactive \
                     confirmation mechanism is available. Fail-closed safety default.",
                    tool_call.name
                );
                tracing::info!(
                    agent_id,
                    tool = %tool_call.name,
                    "Tool blocked: fail-closed"
                );
                self.emit_security_violation(agent_id, &tool_call.name, &reason);
                return Err(reason);
            }
        }

        // 4. Circuit breaker check
        if let Err(reason) = self.circuit_breaker.check(agent_id, &tool_call.name) {
            self.emit_tool_executed(agent_id, &tool_call.name, false, 0);
            return Err(reason);
        }

        // 5. Timeout-wrapped execution
        //
        // Tools flagged as exempt_from_timeout (e.g., coordination tools like
        // wait_for_subagents) manage their own timeouts and must not be subject
        // to the per-tool sandbox timeout.
        let is_exempt = self.registry.is_exempt_from_timeout(&tool_call.name);

        let registry = self.registry.clone();
        let tool_name = tool_call.name.clone();
        let arguments = tool_call.arguments.clone();
        let ctx_owned = ctx.clone();

        let start = std::time::Instant::now();
        let result = if is_exempt {
            Ok(registry.execute_with_context(&tool_name, &arguments, &ctx_owned).await)
        } else {
            let timeout = Duration::from_secs(policy.max_tool_runtime_secs);
            tokio::time::timeout(timeout, async move {
                registry.execute_with_context(&tool_name, &arguments, &ctx_owned).await
            })
            .await
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        // 6. Process result and record for circuit breaker

        match result {
            Ok(Ok(output)) => {
                self.emit_tool_executed(agent_id, &tool_call.name, true, duration_ms);
                self.circuit_breaker
                    .record_success(agent_id, &tool_call.name);
                Ok(output)
            }
            Ok(Err(err)) => {
                self.emit_tool_executed(agent_id, &tool_call.name, false, duration_ms);
                if is_transient_tool_error(&err) {
                    self.circuit_breaker
                        .record_failure(agent_id, &tool_call.name);
                }
                Err(err)
            }
            Err(_timeout) => {
                let reason = format!(
                    "Tool '{}' timed out after {}s",
                    tool_call.name, policy.max_tool_runtime_secs
                );
                self.emit_security_violation(agent_id, &tool_call.name, &reason);
                // Timeouts are transient — record for circuit breaker
                self.circuit_breaker
                    .record_failure(agent_id, &tool_call.name);
                Err(reason)
            }
        }
    }

    fn emit_security_violation(&self, agent_id: &str, tool_name: &str, reason: &str) {
        self.bus.publish(SystemEvent::SecurityViolation {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            reason: reason.to_string(),
            timestamp: Utc::now(),
        });

        // Best-effort persistence to event_log for audit trail
        if let Some(ref db) = self.db {
            let detail = serde_json::json!({
                "tool_name": tool_name,
                "reason": reason,
            });
            let result = serde_json::json!({ "outcome": "denied" });
            let repo = openalpaca_storage::repository::EventLogRepository::new(db);
            if let Err(e) = repo.log(
                "security_violation",
                Some(agent_id),
                Some(&detail),
                Some(&result),
            ) {
                tracing::warn!("Failed to persist security violation to event_log: {e}");
            }
        }
    }

    fn emit_tool_executed(&self, agent_id: &str, tool_name: &str, success: bool, duration_ms: u64) {
        self.bus.publish(SystemEvent::ToolExecuted {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            success,
            duration_ms,
            timestamp: Utc::now(),
        });
    }

    /// Access the session-scoped approval cache (tests only).
    #[cfg(test)]
    pub(crate) fn approval_cache(&self) -> &crate::security::confirmation::ApprovalCache {
        &self.approval_cache
    }
}

/// Compute the effective confirmation set for a policy.
///
/// - If `policy.require_confirmation_for` is non-empty, return it verbatim
///   (explicit list wins per design spec Q1(C)).
/// - Otherwise, derive from `destructive_hint=true` annotations on registered tools.
pub(crate) fn effective_confirmation_set(
    policy: &SandboxPolicy,
    registry: &crate::tools::ToolRegistry,
) -> Vec<String> {
    if !policy.require_confirmation_for.is_empty() {
        return policy.require_confirmation_for.clone();
    }
    registry
        .iter_registered_tools()
        .filter_map(|(name, reg)| {
            let destructive = reg
                .annotations
                .as_ref()
                .and_then(|a| a.destructive_hint)
                .unwrap_or(false);
            destructive.then(|| name.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests;
