//! TIMEOUT-ONE-CLOCK (items 19 to 21 of the owner's answers of 2026-11-14):
//! the sandbox is the one clock for every tool call, and a call it cuts is a
//! failed tool run with the reason `timeout`.
//!
//! A `Command`-backend tool, a plugin's tool and an `Http`-backend tool are
//! each cut here at the sandbox's figure, where a clock of their own used to
//! come first (the Command and HTTP tools) or the cut was filed as a
//! `security_violation` (all of them). An MCP tool's case runs against a stub
//! server in the daemon's MCP tests, where the stub lives.

use super::*;
use crate::tools::registry::{RegisteredTool, ToolBackend};
use std::collections::HashMap;

/// A policy with `bound` as the per-tool bound and `ceiling` as the most a
/// call may ask for; nothing else restricts it.
fn policy(bound: u64, ceiling: u64) -> SandboxPolicy {
    SandboxPolicy {
        agent_id: "agent1".to_string(),
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: bound,
        max_tool_runtime_ceiling_secs: ceiling,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
        unattended: false,
    }
}

fn tool(name: &str, parameters: serde_json::Value, backend: ToolBackend) -> RegisteredTool {
    RegisteredTool {
        definition: openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: name.to_string(),
            parameters,
            strict: None,
            input_examples: None,
        },
        backend,
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "tc_clock".to_string(),
        name: name.to_string(),
        arguments,
    }
}

/// `ctx` as agent `agent1`'s, the agent the breaker is checked for.
fn as_agent(ctx: ToolContext) -> ToolContext {
    ToolContext {
        agent_id: Some("agent1".to_string()),
        ..ctx
    }
}

/// A sandbox over `registry` that writes its audit rows to a database of its
/// own and opens a tool's breaker at its first counted failure.
struct Rig {
    _dir: tempfile::TempDir,
    db: openalpaca_storage::Database,
    sandbox: SandboxManager,
    events: tokio::sync::broadcast::Receiver<SystemEvent>,
}

impl Rig {
    fn new(registry: ToolRegistry) -> Self {
        let dir = tempfile::tempdir().expect("audit dir");
        let db = openalpaca_storage::Database::open(&dir.path().join("audit.db")).expect("db");
        let bus = EventBus::default();
        let events = bus.subscribe();
        let breaker = CircuitBreakerConfig {
            failure_threshold: 1,
            ..CircuitBreakerConfig::default()
        };
        let sandbox = SandboxManager::with_db(Arc::new(registry), bus, &breaker, db.clone());
        Self {
            _dir: dir,
            db,
            sandbox,
            events,
        }
    }

    /// What the bus carried for `tool`: each `ToolExecuted`'s success and
    /// reason, and the number of `SecurityViolation`s.
    fn heard(&mut self, tool: &str) -> (Vec<(bool, Option<ToolFailureReason>)>, usize) {
        let (mut executed, mut violations) = (Vec::new(), 0);
        while let Ok(event) = self.events.try_recv() {
            match event {
                SystemEvent::ToolExecuted {
                    tool_name,
                    success,
                    reason,
                    ..
                } if tool_name == tool => executed.push((success, reason)),
                SystemEvent::SecurityViolation { tool_name, .. } if tool_name == tool => {
                    violations += 1;
                }
                _ => {}
            }
        }
        (executed, violations)
    }

    /// The audit rows of `event_type` the sandbox wrote.
    fn audit_rows(&self, event_type: &str) -> usize {
        openalpaca_storage::repository::EventLogRepository::new(&self.db)
            .query(&openalpaca_storage::repository::EventLogQuery {
                event_type: Some(event_type),
                limit: 10,
                ..Default::default()
            })
            .expect("read the audit log")
            .len()
    }

    /// The call was a failed run cut for time: one `tool_executed` with
    /// `success` false and the reason `timeout`, no `security_violation` on
    /// the bus or in the audit log, and the breaker's record as before (one
    /// counted failure opens it at this rig's threshold of one).
    fn assert_a_timed_out_run(&mut self, tool: &str) {
        let (executed, violations) = self.heard(tool);
        assert_eq!(
            executed,
            vec![(false, Some(ToolFailureReason::Timeout))],
            "a cut call is a failed tool run with the reason timeout"
        );
        assert_eq!(violations, 0, "a timeout is no security violation");
        assert_eq!(self.audit_rows("security_violation"), 0);
        assert!(
            self.sandbox.circuit_breaker.check("agent1", tool).is_err(),
            "the cut is counted by the breaker, as it was"
        );
    }
}

/// A `Command`-backend tool running `sleep <seconds>` whose author gave it
/// `own_secs`, with `parameters` as its schema.
fn sleeping_command(seconds: &str, own_secs: u64, parameters: serde_json::Value) -> ToolRegistry {
    let registry = ToolRegistry::default();
    registry
        .register(tool(
            "slow_cmd",
            parameters,
            ToolBackend::Command {
                command: "sleep".to_string(),
                args_template: Some(seconds.to_string()),
                timeout_secs: own_secs,
            },
        ))
        .unwrap();
    registry
}

/// What a call cut at `limit` whose schema takes no `timeout_secs` is told
/// below the ceiling: its limit and the ceiling as facts, and what to do —
/// neither advising nor denying a `timeout_secs` (item 19).
fn no_advice(limit: u64, most: u64) -> String {
    let hint = format!(
        "Hint: this tool's limit here is {limit} seconds (the most any call may run is {most} \
         seconds). Split the work into smaller calls that each finish sooner."
    );
    assert!(
        !hint.contains("timeout_secs") && !hint.contains("ask"),
        "{hint}"
    );
    hint
}

/// A `Command`-backend tool whose author's figure (1 s) is below the bound
/// (5 s) is cut by the sandbox at that figure: no clock of its own comes
/// first, the run is a failed one with the reason `timeout`, and its hint
/// does not send it after a `timeout_secs` its schema does not take.
#[tokio::test]
async fn a_command_tool_below_the_bound_is_cut_by_the_sandbox_at_its_own_figure() {
    let gp = crate::test_util::GuardedProject::new();
    let mut rig = Rig::new(sleeping_command(
        "3",
        1,
        serde_json::json!({"type": "object"}),
    ));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow_cmd", serde_json::json!({})),
            &policy(5, 10),
            &as_agent(gp.ctx()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!("Tool 'slow_cmd' timed out after 1s\n{}", no_advice(1, 10))
    );
    rig.assert_a_timed_out_run("slow_cmd");
}

/// One whose author's figure (3 s) is above the bound (1 s) is cut at it,
/// as CMD-TIMEOUT-2 made it, and is now a failed run, not a violation. Its
/// schema declares `timeout_secs`, so it is told to ask for more.
#[tokio::test]
async fn a_command_tool_above_the_bound_is_a_failed_run_when_cut() {
    let gp = crate::test_util::GuardedProject::new();
    let takes_timeout = serde_json::json!({
        "type": "object",
        "properties": {"timeout_secs": {"type": "integer"}},
    });
    let mut rig = Rig::new(sleeping_command("5", 3, takes_timeout));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow_cmd", serde_json::json!({})),
            &policy(1, 10),
            &as_agent(gp.ctx()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Tool 'slow_cmd' timed out after 3s\nHint: this tool may run up to 10 seconds here — \
         call it again with a larger timeout_secs, or split the work into smaller calls that \
         each finish sooner."
    );
    rig.assert_a_timed_out_run("slow_cmd");
}

/// A plugin's tool that answers after 2 s, its schema `parameters`.
fn slow_plugin(parameters: serde_json::Value) -> ToolRegistry {
    struct AnswersLate;
    impl openalpaca_api::plugin_traits::PluginToolExecutor for AnswersLate {
        async fn execute(
            &self,
            _tool_name: &str,
            _arguments: &serde_json::Value,
        ) -> Result<String, String> {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok("late".to_string())
        }
        fn plugin_id(&self) -> &str {
            "slow-plugin"
        }
    }
    let registry = ToolRegistry::default();
    registry
        .register(tool(
            "slow-plugin::slow",
            parameters,
            ToolBackend::Plugin(
                openalpaca_api::plugin_traits::DynPluginToolExecutor::new_arc(AnswersLate),
            ),
        ))
        .unwrap();
    registry
}

/// A plugin's tool cut at the bound is a failed run with the reason
/// `timeout`, never a violation; with no `timeout_secs` in its schema it is
/// told its limit and the ceiling, and with one, that it may ask.
#[tokio::test]
async fn a_plugin_tool_cut_by_the_sandbox_is_a_failed_run() {
    let mut rig = Rig::new(slow_plugin(serde_json::json!({"type": "object"})));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow-plugin::slow", serde_json::json!({})),
            &policy(1, 600),
            &as_agent(ToolContext::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Tool 'slow-plugin::slow' timed out after 1s\n{}",
            no_advice(1, 600)
        )
    );
    rig.assert_a_timed_out_run("slow-plugin::slow");

    let rig = Rig::new(slow_plugin(serde_json::json!({
        "type": "object",
        "properties": {"timeout_secs": {"type": "number"}},
    })));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow-plugin::slow", serde_json::json!({})),
            &policy(1, 600),
            &as_agent(ToolContext::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Tool 'slow-plugin::slow' timed out after 1s\nHint: this tool may run up to 600 seconds \
         here — call it again with a larger timeout_secs, or split the work into smaller calls \
         that each finish sooner."
    );
}

/// Item 21, kept: a call cut at the ceiling is told that is the most it may
/// run, whatever its schema takes.
#[tokio::test]
async fn a_call_cut_at_the_ceiling_keeps_that_is_the_most() {
    let mut rig = Rig::new(slow_plugin(serde_json::json!({"type": "object"})));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow-plugin::slow", serde_json::json!({"timeout_secs": 5})),
            &policy(1, 1),
            &as_agent(ToolContext::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Tool 'slow-plugin::slow' timed out after 1s\n{}",
            crate::tools::platform::TIMED_OUT_HINT
        )
    );
    rig.assert_a_timed_out_run("slow-plugin::slow");
}

/// A server on loopback that reads each request and never answers, kept
/// until the returned handle is dropped (aborted), and a client that reaches
/// it as `slow.test` — a name the registry's SSRF check lets through, which
/// a loopback address is not — with no proxy in between.
async fn silent_server() -> (u16, reqwest::Client, AbortOnDrop) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let client = reqwest::Client::builder()
        .resolve("slow.test", addr)
        .no_proxy()
        .build()
        .expect("client");
    (addr.port(), client, AbortOnDrop(task))
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn slow_http(port: u16, client: reqwest::Client, own_secs: u64) -> ToolRegistry {
    let registry = ToolRegistry::default().with_http_client(client);
    registry
        .register(tool(
            "slow_http",
            serde_json::json!({"type": "object"}),
            ToolBackend::Http {
                method: "GET".to_string(),
                url: format!("http://slow.test:{port}/never"),
                headers: HashMap::new(),
                timeout_secs: own_secs,
            },
        ))
        .unwrap();
    registry
}

/// An `Http`-backend tool whose own `timeout_secs` (1 s) is below the bound
/// (5 s) is cut by the sandbox at that figure: the request's own timeout no
/// longer ends it first with "HTTP request failed", and the run is a failed
/// one with the reason `timeout`. Such a call may not ask, so no hint.
#[tokio::test]
async fn an_http_tool_is_cut_by_the_sandbox_at_its_own_figure() {
    let (port, client, _server) = silent_server().await;
    let mut rig = Rig::new(slow_http(port, client, 1));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow_http", serde_json::json!({})),
            &policy(5, 600),
            &as_agent(ToolContext::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "Tool 'slow_http' timed out after 1s");
    rig.assert_a_timed_out_run("slow_http");
}

/// One whose own figure (10 s) is above the bound (1 s) is cut at the bound,
/// as before — and is now a failed run, not a violation.
#[tokio::test]
async fn an_http_tool_above_the_bound_is_cut_at_the_bound_as_a_failed_run() {
    let (port, client, _server) = silent_server().await;
    let mut rig = Rig::new(slow_http(port, client, 10));
    let err = rig
        .sandbox
        .execute_tool(
            &call("slow_http", serde_json::json!({})),
            &policy(1, 600),
            &as_agent(ToolContext::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "Tool 'slow_http' timed out after 1s");
    rig.assert_a_timed_out_run("slow_http");
}

/// A call outside the sandbox keeps the tool's own clock: the HTTP
/// backend's request stops at its own figure, with its own words.
#[tokio::test]
async fn an_http_tool_called_outside_the_sandbox_keeps_its_own_clock() {
    let (port, client, _server) = silent_server().await;
    let registry = slow_http(port, client, 1);
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        registry.execute("slow_http", &serde_json::json!({})),
    )
    .await
    .expect("its own clock ends it")
    .unwrap_err();
    assert!(err.starts_with("HTTP request failed"), "{err}");
}
