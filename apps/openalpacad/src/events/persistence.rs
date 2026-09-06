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
                    repo.log_for_task("task_status", None, Some(task_id), Some(&detail), None)
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
                    task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "reason": reason,
                        "task_id": task_id
                    });
                    repo.log_for_task(
                        "security_violation",
                        Some(agent_id),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
                }
                ServerEvent::CircuitBreakerTripped {
                    agent_id,
                    tool_name,
                    consecutive_failures,
                    reset_after_secs,
                    task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "consecutive_failures": consecutive_failures,
                        "reset_after_secs": reset_after_secs,
                        "task_id": task_id
                    });
                    repo.log_for_task(
                        "circuit_breaker_tripped",
                        Some(agent_id),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
                }
                ServerEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    success,
                    duration_ms,
                    task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "success": success,
                        "duration_ms": duration_ms,
                        "task_id": task_id
                    });
                    repo.log_for_task(
                        "tool_executed",
                        Some(agent_id),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
                }
                ServerEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "agent_id": agent_id,
                        "model": model,
                        "input_tokens": input_tokens,
                        "output_tokens": output_tokens,
                        "cost_usd": cost_usd,
                        "task_id": task_id
                    });
                    repo.log_for_task(
                        "llm_call_completed",
                        Some(agent_id),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
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
                    repo.log_for_task(
                        "dag_node_status",
                        Some(agent_id),
                        Some(task_id),
                        Some(&detail),
                        None,
                    )
                }
                ServerEvent::ToolConfirmationRequested {
                    request_id,
                    agent_id,
                    tool_name,
                    task_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "request_id": request_id,
                        "agent_id": agent_id,
                        "tool_name": tool_name,
                        "task_id": task_id
                    });
                    repo.log_for_task(
                        "tool_confirmation_requested",
                        Some(agent_id),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
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
                    repo.log_for_task("workflow_started", None, Some(task_id), Some(&detail), None)
                }
                ServerEvent::WorkflowSteered {
                    task_id, lane_key, ..
                } => {
                    let detail = serde_json::json!({
                        "task_id": task_id,
                        "lane_key": lane_key,
                    });
                    repo.log_for_task("workflow_steered", None, Some(task_id), Some(&detail), None)
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
                    repo.log_for_task(
                        "workflow_progress",
                        None,
                        Some(task_id),
                        Some(&detail),
                        None,
                    )
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
                ServerEvent::FollowupCancelled {
                    lane_key,
                    followup_id,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "lane_key": lane_key,
                        "followup_id": followup_id,
                    });
                    repo.log("followup_cancelled", None, Some(&detail), None)
                }
                // A produced artifact (plan §4.9). Persisted like `task_status`
                // so it appears in `GET /v1/events/history` and feeds GAP-10's
                // per-run log; the agent goes in the indexed column so
                // `?agent_id=` finds it.
                ServerEvent::ArtifactWritten {
                    artifact_id,
                    task_id,
                    agent_id,
                    name,
                    kind,
                    version,
                    path,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "artifact_id": artifact_id,
                        "task_id": task_id,
                        "agent_id": agent_id,
                        "name": name,
                        "kind": kind,
                        "version": version,
                        "path": path,
                    });
                    repo.log_for_task(
                        "artifact_written",
                        agent_id.as_deref(),
                        task_id.as_deref(),
                        Some(&detail),
                        None,
                    )
                }
                // One subagent lane opening or closing (GAP-09). Persisted
                // like the rest so the run's history survives a restart; the
                // agent *instance* goes in the indexed `agent_id` column,
                // because that is what identifies the lane.
                ServerEvent::SubagentSpan {
                    task_id,
                    span_id,
                    label,
                    template_id,
                    agent_instance_id,
                    state,
                    detail,
                    started_at,
                    ended_at,
                    duration_ms,
                    output_preview,
                    ..
                } => {
                    let payload = serde_json::json!({
                        "task_id": task_id,
                        "span_id": span_id,
                        "label": label,
                        "template_id": template_id,
                        "agent_instance_id": agent_instance_id,
                        "state": state,
                        "detail": detail,
                        "started_at": started_at,
                        "ended_at": ended_at,
                        "duration_ms": duration_ms,
                        "output_preview": output_preview,
                    });
                    repo.log_for_task(
                        "subagent_span",
                        Some(agent_instance_id.as_str()),
                        Some(task_id.as_str()),
                        Some(&payload),
                        None,
                    )
                }
                // Extension (MCP server / plugin) state transitions
                ServerEvent::ExtensionStateChanged {
                    kind,
                    id,
                    state,
                    generation,
                    tools_changed,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "kind": kind,
                        "id": id,
                        "state": state,
                        "generation": generation,
                        "tools_changed": tools_changed,
                    });
                    repo.log("extension_state_changed", None, Some(&detail), None)
                }
                // S4 moment 1/2 — a withheld capability (already deduped)
                ServerEvent::ExtensionCapabilityWithheld {
                    kind,
                    id,
                    subject,
                    moment,
                    state,
                    scope,
                    stale,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "kind": kind,
                        "id": id,
                        "subject": subject,
                        "moment": moment,
                        "state": state,
                        "scope": scope,
                        "stale": stale,
                    });
                    repo.log("extension_capability_withheld", None, Some(&detail), None)
                }
                // S4 moment 3 — T1 step 3's dependent scan, one per transition
                ServerEvent::ExtensionCapabilityWithdrawn {
                    kind,
                    id,
                    state,
                    cause,
                    capabilities,
                    tools,
                    affected_templates,
                    affected_skills,
                    affected_cron_skills,
                    notice_lane,
                    ..
                } => {
                    let detail = serde_json::json!({
                        "kind": kind,
                        "id": id,
                        "state": state,
                        "cause": cause,
                        "capabilities": capabilities,
                        "tools": tools,
                        "affected_templates": affected_templates,
                        "affected_skills": affected_skills,
                        "affected_cron_skills": affected_cron_skills,
                        "notice_lane": notice_lane,
                    });
                    repo.log("extension_capability_withdrawn", None, Some(&detail), None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::Database;

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    /// T28 — `artifact_written` is persisted like `task_status`, so it shows up
    /// in `GET /v1/events/history` and feeds GAP-10's per-run log. The agent id
    /// goes in the indexed column, so `?agent_id=` finds the row.
    #[test]
    fn artifact_written_is_persisted_to_the_event_log() {
        let (_dir, db) = test_db();
        let eb = EventBroadcaster::new(16, "inst-1".to_string(), Some(db.clone()));

        eb.artifact_written(
            "a-1",
            Some("t-1"),
            Some("writing_agent"),
            "01-quarterly-report.md",
            "markdown",
            2,
            "/p/.openalpaca/artifacts/run/01-quarterly-report.md",
        );

        let rows = EventLogRepository::new(&db).recent(10).unwrap();
        let row = rows
            .iter()
            .find(|r| r.event_type == "artifact_written")
            .expect("the artifact write must reach the event log");
        assert_eq!(row.agent_id.as_deref(), Some("writing_agent"));
        let detail = row.detail.as_ref().expect("the row carries a detail blob");
        assert_eq!(detail["artifact_id"], "a-1");
        assert_eq!(detail["task_id"], "t-1");
        assert_eq!(detail["agent_id"], "writing_agent");
        assert_eq!(detail["name"], "01-quarterly-report.md");
        assert_eq!(detail["kind"], "markdown");
        assert_eq!(detail["version"], 2);
        assert_eq!(
            detail["path"],
            "/p/.openalpaca/artifacts/run/01-quarterly-report.md"
        );
    }

    /// A loose artifact still logs; it simply has no agent column to fill.
    #[test]
    fn a_loose_artifact_logs_without_an_agent() {
        let (_dir, db) = test_db();
        let eb = EventBroadcaster::new(16, "inst-1".to_string(), Some(db.clone()));

        eb.artifact_written(
            "a-2",
            None,
            None,
            "01-notes.md",
            "markdown",
            1,
            "/h/.openalpaca/artifacts/loose/01-notes.md",
        );

        let rows = EventLogRepository::new(&db).recent(10).unwrap();
        let row = rows
            .iter()
            .find(|r| r.event_type == "artifact_written")
            .expect("a loose artifact must still be logged");
        assert_eq!(row.agent_id, None);
        let detail = row.detail.as_ref().expect("the row carries a detail blob");
        assert!(detail["task_id"].is_null());
        assert!(detail["agent_id"].is_null());
    }

    // ── GAP-10: the run goes in the indexed column ──────────────────────

    /// Every arm that knows its run writes the id to `event_log.task_id`, not
    /// only into `detail` — that column is what `?task_id=` reads.
    #[test]
    fn the_run_scoped_arms_fill_the_task_column() {
        let (_dir, db) = test_db();
        let eb = EventBroadcaster::new(16, "inst-1".to_string(), Some(db.clone()));

        eb.task_status("t-1", "A run", "running", None, None, None, None, None, None);
        eb.tool_executed("research_agent::a1", "web_search", true, 12, Some("t-1"));
        eb.security_violation("research_agent::a1", "shell", "denied", Some("t-1"));
        eb.circuit_breaker_tripped("research_agent::a1", "web_search", 3, 300, Some("t-1"));
        eb.llm_call_completed("research_agent::a1", "m", 1, 2, 0.1, Some("t-1"));
        eb.tool_confirmation_requested(
            "req-1",
            "research_agent::a1",
            "shell",
            &serde_json::json!({}),
            None,
            None,
            Some("t-1"),
        );
        eb.workflow_steered("t-1", "junpei:cli");
        eb.workflow_started("t-1", "junpei:cli", "A run");
        eb.workflow_progress("t-1", "junpei:cli", "read 12 files");
        eb.dag_node_status("t-1", "node-1", "review", "review_agent", "started", None, None);
        eb.artifact_written(
            "a-1",
            Some("t-1"),
            Some("writing_agent"),
            "01-report.md",
            "markdown",
            1,
            "/p/.openalpaca/artifacts/run/01-report.md",
        );
        eb.subagent_span(
            "t-1",
            "node-1",
            "review·1",
            "review_agent",
            "review_agent::a1b2",
            "running",
            None,
            "2026-09-05T10:00:00.000Z",
            None,
            None,
            None,
        );

        let rows = EventLogRepository::new(&db).recent(50).unwrap();
        for event_type in [
            "task_status",
            "tool_executed",
            "security_violation",
            "circuit_breaker_tripped",
            "llm_call_completed",
            "tool_confirmation_requested",
            "workflow_steered",
            "workflow_started",
            "workflow_progress",
            "dag_node_status",
            "artifact_written",
            "subagent_span",
        ] {
            let row = rows
                .iter()
                .find(|r| r.event_type == event_type)
                .unwrap_or_else(|| panic!("{event_type} must reach the event log"));
            assert_eq!(
                row.task_id.as_deref(),
                Some("t-1"),
                "{event_type} must carry its run in the indexed column"
            );
            // Still in `detail` too — readers written before the column exists
            // keep working.
            assert_eq!(
                row.detail.as_ref().and_then(|d| d["task_id"].as_str()),
                Some("t-1"),
                "{event_type} keeps the id in its detail blob"
            );
        }
    }

    /// An event with no run leaves the column NULL rather than borrowing one.
    #[test]
    fn a_task_less_event_leaves_the_column_null() {
        let (_dir, db) = test_db();
        let eb = EventBroadcaster::new(16, "inst-1".to_string(), Some(db.clone()));

        eb.tool_executed("orchestrator", "web_search", true, 5, None);
        eb.connector_status("telegram", "connected");

        let rows = EventLogRepository::new(&db).recent(10).unwrap();
        for event_type in ["tool_executed", "connector_status"] {
            let row = rows.iter().find(|r| r.event_type == event_type).unwrap();
            assert_eq!(row.task_id, None);
        }
    }

    /// A lane's open and its close are two rows, both carrying the whole span
    /// payload, with the agent *instance* in the indexed column.
    #[test]
    fn a_subagent_span_logs_its_open_and_its_close() {
        let (_dir, db) = test_db();
        let eb = EventBroadcaster::new(16, "inst-1".to_string(), Some(db.clone()));

        eb.subagent_span(
            "t-1",
            "node-1",
            "review\u{b7}1",
            "review_agent",
            "review_agent::a1b2",
            "running",
            None,
            "2026-09-05T10:00:00.000Z",
            None,
            None,
            None,
        );
        eb.subagent_span(
            "t-1",
            "node-1",
            "review\u{b7}1",
            "review_agent",
            "review_agent::a1b2",
            "cancelled",
            Some("cancelled before starting"),
            "2026-09-05T10:00:00.000Z",
            Some("2026-09-05T10:00:04.500Z"),
            Some(4_500),
            None,
        );

        let rows = EventLogRepository::new(&db).recent(10).unwrap();
        let spans: Vec<_> = rows
            .iter()
            .filter(|r| r.event_type == "subagent_span")
            .collect();
        assert_eq!(spans.len(), 2, "the open and the close are both logged");
        assert!(
            spans
                .iter()
                .all(|r| r.agent_id.as_deref() == Some("review_agent::a1b2"))
        );

        let close = spans
            .iter()
            .find(|r| {
                r.detail
                    .as_ref()
                    .is_some_and(|d| d["state"] == "cancelled")
            })
            .expect("the close row exists");
        let detail = close.detail.as_ref().unwrap();
        assert_eq!(detail["task_id"], "t-1");
        assert_eq!(detail["span_id"], "node-1");
        assert_eq!(detail["label"], "review\u{b7}1");
        assert_eq!(detail["template_id"], "review_agent");
        assert_eq!(detail["agent_instance_id"], "review_agent::a1b2");
        assert_eq!(detail["detail"], "cancelled before starting");
        assert_eq!(detail["started_at"], "2026-09-05T10:00:00.000Z");
        assert_eq!(detail["ended_at"], "2026-09-05T10:00:04.500Z");
        assert_eq!(detail["duration_ms"], 4_500);
        assert!(detail["output_preview"].is_null());
    }
}
