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
use crate::security::sanitizer::InputSanitizer;
use async_trait::async_trait;
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
        }
    }
}

/// Trait for executing tools. Implementations provide the actual tool logic.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool by name with the given arguments. Returns the tool output or an error.
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String>;

    /// List all tools this executor can handle.
    fn registered_tools(&self) -> Vec<String>;
}

/// Manages sandboxed tool execution with security checks.
pub struct SandboxManager {
    executor: Arc<dyn ToolExecutor>,
    bus: EventBus,
    circuit_breaker: ToolCircuitBreaker,
    /// Optional database for persisting security violation audit logs.
    db: Option<openalpaca_storage::Database>,
}

impl SandboxManager {
    /// Create a new SandboxManager with a specific circuit breaker configuration.
    pub fn new(
        executor: Arc<dyn ToolExecutor>,
        bus: EventBus,
        circuit_breaker_config: &CircuitBreakerConfig,
    ) -> Self {
        let circuit_breaker = ToolCircuitBreaker::new(circuit_breaker_config, bus.clone());
        Self {
            executor,
            bus,
            circuit_breaker,
            db: None,
        }
    }

    /// Create a new SandboxManager with a database for audit logging.
    pub fn with_db(
        executor: Arc<dyn ToolExecutor>,
        bus: EventBus,
        circuit_breaker_config: &CircuitBreakerConfig,
        db: openalpaca_storage::Database,
    ) -> Self {
        let circuit_breaker = ToolCircuitBreaker::new(circuit_breaker_config, bus.clone());
        Self {
            executor,
            bus,
            circuit_breaker,
            db: Some(db),
        }
    }

    /// Create a new SandboxManager with default circuit breaker settings.
    ///
    /// Used by internal per-request sandbox instances (query handler, skill handler,
    /// DAG executor, lead agent) where no custom config is needed.
    pub fn with_defaults(executor: Arc<dyn ToolExecutor>, bus: EventBus) -> Self {
        Self::new(executor, bus, &CircuitBreakerConfig::default())
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
        agent_id: &str,
        tool_call: &ToolCall,
        policy: &SandboxPolicy,
    ) -> Result<String, String> {
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
        let registered = self.executor.registered_tools();
        if let Err(violation) =
            InputSanitizer::sanitize_tool_args(&tool_call.name, &tool_call.arguments, &registered)
        {
            self.emit_security_violation(agent_id, &tool_call.name, &violation.to_string());
            return Err(violation.to_string());
        }

        // 3. Confirmation check — fail-closed if tool requires confirmation
        if policy
            .require_confirmation_for
            .iter()
            .any(|t| t == &tool_call.name)
        {
            let reason = format!(
                "Tool '{}' requires confirmation (configured via require_confirmation_for) \
                 but no confirmation mechanism is available in this execution context",
                tool_call.name
            );
            self.emit_security_violation(agent_id, &tool_call.name, &reason);
            return Err(reason);
        }

        // 4. Circuit breaker check
        if let Err(reason) = self.circuit_breaker.check(agent_id, &tool_call.name) {
            self.emit_tool_executed(agent_id, &tool_call.name, false, 0);
            return Err(reason);
        }

        // 5. Timeout-wrapped execution
        let timeout = Duration::from_secs(policy.max_tool_runtime_secs);
        let executor = self.executor.clone();
        let tool_name = tool_call.name.clone();
        let arguments = tool_call.arguments.clone();

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(timeout, async move {
            executor.execute(&tool_name, &arguments).await
        })
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        // 5. Process result and record for circuit breaker
        let final_result = match result {
            Ok(Ok(output)) => {
                self.emit_tool_executed(agent_id, &tool_call.name, true, duration_ms);
                self.circuit_breaker.record_success(agent_id, &tool_call.name);
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
        };

        final_result
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;

    struct MockExecutor;

    #[async_trait]
    impl ToolExecutor for MockExecutor {
        async fn execute(
            &self,
            tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            match tool_name {
                "web_search" => Ok("search results".to_string()),
                "slow_tool" => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok("done".to_string())
                }
                _ => Err(format!("Unknown tool: {}", tool_name)),
            }
        }

        fn registered_tools(&self) -> Vec<String> {
            vec!["web_search".to_string(), "slow_tool".to_string()]
        }
    }

    fn make_sandbox() -> SandboxManager {
        SandboxManager::new(
            Arc::new(MockExecutor),
            EventBus::default(),
            &CircuitBreakerConfig::default(),
        )
    }

    fn make_policy(agent_id: &str) -> SandboxPolicy {
        SandboxPolicy {
            agent_id: agent_id.to_string(),
            allowed_capabilities: vec![],
            denied_capabilities: vec![],
            require_confirmation_for: vec![],
            max_tool_calls: None,
            max_tool_runtime_secs: 60,
        }
    }

    fn make_tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "tc_1".to_string(),
            name: name.to_string(),
            arguments: serde_json::json!({"query": "test"}),
        }
    }

    #[tokio::test]
    async fn test_happy_path() {
        let sandbox = make_sandbox();
        let policy = make_policy("agent1");
        let tc = make_tool_call("web_search");

        let result = sandbox.execute_tool("agent1", &tc, &policy).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "search results");
    }

    #[tokio::test]
    async fn test_denied_capability() {
        let sandbox = make_sandbox();
        let mut policy = make_policy("agent1");
        policy.denied_capabilities = vec!["web_search".to_string()];
        let tc = make_tool_call("web_search");

        let result = sandbox.execute_tool("agent1", &tc, &policy).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("denied"));
    }

    #[tokio::test]
    async fn test_timeout() {
        let sandbox = make_sandbox();
        let mut policy = make_policy("agent1");
        policy.max_tool_runtime_secs = 1; // 1 second timeout
        let tc = make_tool_call("slow_tool");

        let result = sandbox.execute_tool("agent1", &tc, &policy).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[tokio::test]
    async fn test_security_event_emitted() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let sandbox = SandboxManager::new(
            Arc::new(MockExecutor),
            bus,
            &CircuitBreakerConfig::default(),
        );
        let mut policy = make_policy("agent1");
        policy.denied_capabilities = vec!["web_search".to_string()];
        let tc = make_tool_call("web_search");

        let _ = sandbox.execute_tool("agent1", &tc, &policy).await;

        let event = rx.try_recv().unwrap();
        match event {
            SystemEvent::SecurityViolation {
                agent_id,
                tool_name,
                ..
            } => {
                assert_eq!(agent_id, "agent1");
                assert_eq!(tool_name, "web_search");
            }
            other => panic!("Expected SecurityViolation, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_tool_event_emitted() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        let sandbox = SandboxManager::new(
            Arc::new(MockExecutor),
            bus,
            &CircuitBreakerConfig::default(),
        );
        let policy = make_policy("agent1");
        let tc = make_tool_call("web_search");

        let _ = sandbox.execute_tool("agent1", &tc, &policy).await;

        let event = rx.try_recv().unwrap();
        match event {
            SystemEvent::ToolExecuted {
                agent_id,
                tool_name,
                success,
                ..
            } => {
                assert_eq!(agent_id, "agent1");
                assert_eq!(tool_name, "web_search");
                assert!(success);
            }
            other => panic!("Expected ToolExecuted, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_unregistered_tool() {
        let sandbox = make_sandbox();
        let policy = make_policy("agent1");
        let tc = make_tool_call("unknown_tool");

        let result = sandbox.execute_tool("agent1", &tc, &policy).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not in the allowed tools list"));
    }
}
