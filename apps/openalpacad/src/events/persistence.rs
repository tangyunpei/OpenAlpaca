//! Event persistence to database (EventLogRepository).

use super::EventBroadcaster;
use openalpaca_api::events::ServerEvent;
use openalpaca_storage::{SkillExecutionRepository, ToolExecutionEntry, repository::EventLogRepository};

impl EventBroadcaster {
    /// Persist important events to the database
    pub(super) fn persist(&self, event: &ServerEvent) {
        if let Some(db) = &self.db {
            let repo = EventLogRepository::new(db);
            let persist_result: Result<i64, _> = match event {
                ServerEvent::Heartbeat { .. } => Ok(0), // Skip heartbeats
                ServerEvent::CommandReceived {
                    request_id,
                    command,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "command": command
                    });
                    repo.log("command_received", None, Some(&detail), None)
                }
                // Wake events are persisted by the same mechanism
                ServerEvent::Wake { wake, .. } => {
                    let detail = serde_json::json!({
                        "wake_event": wake
                    });
                    repo.log("wake", None, Some(&detail), None)
                }
                // Log connector status changes
                ServerEvent::ConnectorStatus { id, status, .. } => {
                    let detail = serde_json::json!({
                        "connector_id": id,
                        "status": status
                    });
                    repo.log("connector_status", None, Some(&detail), None)
                }
                // Log agent status changes
                ServerEvent::AgentStatus {
                    agent_id,
                    name,
                    status,
                    current_task_id,
                    agent_instance_id,
                    template_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "name": name,
                        "status": status,
                        "current_task_id": current_task_id,
                        "agent_instance_id": agent_instance_id,
                        "template_id": template_id
                    });
                    repo.log("agent_status_change", None, Some(&detail), None)
                }
                // Log task status changes
                ServerEvent::TaskStatus {
                    task_id,
                    title,
                    status,
                    progress_current,
                    progress_total,
                    result_summary,
                    outcome_kind,
                    artifact_count,
                    outcome_summary,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "title": title,
                        "status": status,
                        "progress_current": progress_current,
                        "progress_total": progress_total,
                        "result_summary": result_summary,
                        "outcome_kind": outcome_kind,
                        "artifact_count": artifact_count,
                        "outcome_summary": outcome_summary,
                    });
                    repo.log("task_status", None, Some(&detail), None)
                }
                // Log key status changes
                ServerEvent::KeyStatusChanged {
                    provider,
                    key_id,
                    status,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "provider": provider,
                        "key_id": key_id,
                        "status": status
                    });
                    repo.log("key_status_changed", None, Some(&detail), None)
                }
                // Log chat stream events
                ServerEvent::ChatStreamStarted {
                    stream_id,
                    lane_key,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "stream_id": stream_id,
                        "lane_key": lane_key
                    });
                    repo.log("chat_stream_started", None, Some(&detail), None)
                }
                ServerEvent::ChatStreamEnded {
                    stream_id,
                    lane_key,
                    status,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "stream_id": stream_id,
                        "lane_key": lane_key,
                        "status": status
                    });
                    repo.log("chat_stream_ended", None, Some(&detail), None)
                }
                ServerEvent::AgentConfigChanged {
                    agent_id,
                    action,
                    config_version,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "action": action,
                        "config_version": config_version
                    });
                    repo.log("agent_config_changed", None, Some(&detail), None)
                }
                ServerEvent::OrchestratorConfigChanged { model, .. } => {
                    let detail = serde_json::json!({
                        "model": model
                    });
                    repo.log("orchestrator_config_changed", None, Some(&detail), None)
                }
                ServerEvent::DaemonConfigChanged { .. } => {
                    repo.log("daemon_config_changed", None, None, None)
                }
                ServerEvent::SecurityViolation {
                    agent_id,
                    tool_name,
                    reason,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "reason": reason
                    });
                    repo.log("security_violation", Some(agent_id), Some(&detail), None)
                }
                ServerEvent::CircuitBreakerTripped {
                    agent_id,
                    tool_name,
                    consecutive_failures,
                    reset_after_secs,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "consecutive_failures": consecutive_failures,
                        "reset_after_secs": reset_after_secs
                    });
                    repo.log(
                        "circuit_breaker_tripped",
                        Some(agent_id),
                        Some(&detail),
                        None,
                    )
                }
                ServerEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    success,
                    duration_ms,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "success": success,
                        "duration_ms": duration_ms
                    });
                    repo.log("tool_executed", Some(agent_id), Some(&detail), None)
                }
                ServerEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "model": model,
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "cost_usd": cost_usd
                    });
                    repo.log("llm_call_completed", Some(agent_id), Some(&detail), None)
                }
                ServerEvent::SkillCatalogUpdated {
                    skill_name, action, ..
                } => {
                    let detail = serde_json::json!({
                        "skill_name": skill_name,
                        "action": action
                    });
                    repo.log("skill_catalog_updated", None, Some(&detail), None)
                }
                ServerEvent::SkillInvocationStarted {
                    request_id,
                    skill_id,
                    query_preview,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "skill_id": skill_id,
                        "query_preview": query_preview,
                    });
                    repo.log("skill_invocation_started", None, Some(&detail), None)
                }
                ServerEvent::SkillCompleted {
                    request_id,
                    skill_id,
                    duration_ms,
                    output_preview,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "skill_id": skill_id,
                        "duration_ms": duration_ms,
                        "output_preview": output_preview,
                    });
                    repo.log("skill_completed", None, Some(&detail), None)
                }
                ServerEvent::SkillFailed {
                    request_id,
                    skill_id,
                    error,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "skill_id": skill_id,
                        "error": error,
                    });
                    repo.log("skill_failed", None, Some(&detail), None)
                }
                // Log DAG node status changes
                ServerEvent::DagNodeStatus {
                    task_id,
                    node_id,
                    agent_id,
                    status,
                    duration_ms,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "node_id": node_id,
                        "agent_id": agent_id,
                        "status": status,
                        "duration_ms": duration_ms,
                    });
                    repo.log("dag_node_status", Some(agent_id), Some(&detail), None)
                }
                ServerEvent::ToolConfirmationRequested {
                    request_id,
                    agent_id,
                    tool_name,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "agent_id": agent_id,
                        "tool_name": tool_name
                    });
                    repo.log("tool_confirmation_requested", Some(agent_id), Some(&detail), None)
                }
                // Log SOUL.md personality updates with actor attribution
                ServerEvent::SoulUpdated {
                    actor,
                    mode,
                    content_sha256,
                    backup_path,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "actor": actor,
                        "mode": mode,
                        "content_sha256": content_sha256,
                        "backup_path": backup_path
                    });
                    repo.log("soul_updated", None, Some(&detail), None)
                }
                // Log workflow lifecycle events (Routing V2)
                ServerEvent::WorkflowStarted {
                    task_id,
                    lane_key,
                    title,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "lane_key": lane_key,
                        "title": title,
                    });
                    repo.log("workflow_started", None, Some(&detail), None)
                }
                ServerEvent::WorkflowSteered {
                    task_id, lane_key, ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "lane_key": lane_key,
                    });
                    repo.log("workflow_steered", None, Some(&detail), None)
                }
                ServerEvent::WorkflowProgress {
                    task_id,
                    lane_key,
                    message,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "lane_key": lane_key,
                        "message": message,
                    });
                    repo.log("workflow_progress", None, Some(&detail), None)
                }
                ServerEvent::FollowupQueued {
                    lane_key,
                    followup_id,
                    kind,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "lane_key": lane_key,
                        "followup_id": followup_id,
                        "kind": kind,
                    });
                    repo.log("followup_queued", None, Some(&detail), None)
                }
                // Plugin lifecycle events
                ServerEvent::PluginLoaded {
                    plugin_id, tools, ..
                } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id,
                        "tools": tools
                    });
                    repo.log("plugin_loaded", None, Some(&detail), None)
                }
                ServerEvent::PluginUnloaded { plugin_id, .. } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id
                    });
                    repo.log("plugin_unloaded", None, Some(&detail), None)
                }
                ServerEvent::PluginCrashed {
                    plugin_id,
                    error,
                    restart_in_secs,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id,
                        "error": error,
                        "restart_in_secs": restart_in_secs
                    });
                    repo.log("plugin_crashed", None, Some(&detail), None)
                }
                ServerEvent::PluginDisabled {
                    plugin_id, reason, ..
                } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id,
                        "reason": reason
                    });
                    repo.log("plugin_disabled", None, Some(&detail), None)
                }
                ServerEvent::PluginPendingApproval {
                    plugin_id,
                    capabilities,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id,
                        "capabilities": capabilities
                    });
                    repo.log("plugin_pending_approval", None, Some(&detail), None)
                }
                ServerEvent::PluginNeedsConfig {
                    plugin_id,
                    missing_keys,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "plugin_id": plugin_id,
                        "missing_keys": missing_keys
                    });
                    repo.log("plugin_needs_config", None, Some(&detail), None)
                }
            };
            if let Err(e) = persist_result {
                tracing::warn!("Failed to persist event to DB: {e}");
            }
        }
    }

    /// Persist a tool execution to the tool_execution_log table (in addition to event_log).
    pub(super) fn persist_tool_execution(
        &self,
        agent_id: &str,
        tool_name: &str,
        success: bool,
        duration_ms: u64,
    ) {
        if let Some(ref db) = self.db {
            let entry = ToolExecutionEntry {
                id: None,
                request_id: None,
                agent_id: agent_id.to_string(),
                tool_name: tool_name.to_string(),
                success,
                duration_ms: duration_ms as i64,
                error_message: None,
                timestamp: None,
            };
            if let Err(e) = SkillExecutionRepository::new(db).record_tool(&entry) {
                tracing::warn!("Failed to persist tool execution log: {e}");
            }
        }
    }
}
