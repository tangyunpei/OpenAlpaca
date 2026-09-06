use crate::events::EventBroadcaster;
use openalpaca_core::bus::EventBus;
use openalpaca_core::chat::ChatStreamManager;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Spawn the SystemEvent → ServerEvent bridge task.
///
/// Listens on the core `EventBus` and forwards relevant events to the
/// `EventBroadcaster` for WebSocket/SSE delivery to API clients.
/// Optionally forwards confirmation events to the chat stream manager for SSE delivery.
pub fn spawn_event_bridge(
    eb: EventBroadcaster,
    bus: &EventBus,
    chat_streams: Option<Arc<ChatStreamManager>>,
    cancel: CancellationToken,
) {
    let mut system_rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                result = system_rx.recv() => match result {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                },
                _ = cancel.cancelled() => break,
            };
            match event {
                openalpaca_core::events::SystemEvent::ConnectorStatus { id, status, .. } => {
                    eb.connector_status(&id, &status);
                }
                openalpaca_core::events::SystemEvent::TaskCreated {
                    task_id,
                    title,
                    created_by: _,
                    ..
                } => {
                    eb.task_status(&task_id, &title, "queued", None, None, None, None, None, None);
                }
                openalpaca_core::events::SystemEvent::TaskUpdated {
                    task_id,
                    title,
                    status,
                    progress_current,
                    progress_total,
                    ..
                } => {
                    eb.task_status(
                        &task_id,
                        &title,
                        &status,
                        progress_current,
                        progress_total,
                        None,
                        None,
                        None,
                        None,
                    );
                }
                openalpaca_core::events::SystemEvent::TaskCompleted {
                    task_id,
                    title,
                    result_summary,
                    outcome_kind,
                    artifact_count,
                    outcome_summary,
                    ..
                } => {
                    eb.task_status(
                        &task_id, &title, "completed", None, None, result_summary,
                        outcome_kind, artifact_count, outcome_summary,
                    );
                }
                openalpaca_core::events::SystemEvent::TaskFailed {
                    task_id, title, error, outcome_kind, ..
                } => {
                    eb.task_status(
                        &task_id, &title, "failed", None, None, Some(error),
                        outcome_kind, None, None,
                    );
                }
                openalpaca_core::events::SystemEvent::AgentStatusChanged {
                    agent_id,
                    instance_id,
                    template_id,
                    name,
                    status,
                    current_task_id,
                    ..
                } => {
                    eb.agent_status(
                        &agent_id,
                        &name,
                        &status,
                        current_task_id,
                        &instance_id,
                        &template_id,
                    );
                }
                // ── Forwarded to clients: security & observability ─────────
                openalpaca_core::events::SystemEvent::SecurityViolation {
                    agent_id,
                    tool_name,
                    reason,
                    task_id,
                    ..
                } => {
                    tracing::warn!(
                        "Security violation: agent={agent_id}, tool={tool_name}, reason={reason}"
                    );
                    eb.security_violation(&agent_id, &tool_name, &reason, task_id.as_deref());
                }
                openalpaca_core::events::SystemEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    success,
                    duration_ms,
                    task_id,
                    session_id,
                    tool_use_id,
                    ..
                } => {
                    tracing::debug!(
                        "Tool executed: agent={agent_id}, tool={tool_name}, success={success}, duration={duration_ms}ms"
                    );
                    eb.tool_executed(
                        &agent_id,
                        &tool_name,
                        success,
                        duration_ms,
                        task_id.as_deref(),
                        session_id.as_deref(),
                        tool_use_id.as_deref(),
                    );
                }
                openalpaca_core::events::SystemEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    task_id,
                    ..
                } => {
                    tracing::info!(
                        "LLM call: agent={agent_id}, model={model}, tokens={input_tokens}/{output_tokens}, cost=${cost_usd:.6}"
                    );
                    eb.llm_call_completed(
                        &agent_id,
                        &model,
                        input_tokens,
                        output_tokens,
                        cost_usd,
                        task_id.as_deref(),
                    );
                }
                openalpaca_core::events::SystemEvent::CircuitBreakerTripped {
                    agent_id,
                    tool_name,
                    consecutive_failures,
                    reset_after_secs,
                    task_id,
                    ..
                } => {
                    tracing::warn!(
                        "Circuit breaker tripped: agent={agent_id}, tool={tool_name}, failures={consecutive_failures}"
                    );
                    eb.circuit_breaker_tripped(
                        &agent_id,
                        &tool_name,
                        consecutive_failures,
                        reset_after_secs,
                        task_id.as_deref(),
                    );
                }
                openalpaca_core::events::SystemEvent::SkillCatalogUpdated {
                    skill_name,
                    action,
                    ..
                } => {
                    eb.skill_catalog_updated(&skill_name, &action);
                }
                // ── Forwarded to clients: config changes ──────────────────
                openalpaca_core::events::SystemEvent::AgentConfigChanged {
                    agent_id,
                    action,
                    config_version,
                    ..
                } => {
                    eb.agent_config_changed(&agent_id, &action, config_version);
                }
                openalpaca_core::events::SystemEvent::OrchestratorConfigChanged {
                    model, ..
                } => {
                    eb.orchestrator_config_changed(&model);
                }
                openalpaca_core::events::SystemEvent::DaemonConfigChanged { .. } => {
                    eb.daemon_config_changed();
                }
                openalpaca_core::events::SystemEvent::KeyStatusChanged {
                    provider,
                    key_id,
                    status,
                    ..
                } => {
                    eb.key_status_changed(&provider, &key_id, &status);
                }
                openalpaca_core::events::SystemEvent::ChatStreamStarted {
                    stream_id,
                    lane_key,
                    ..
                } => {
                    eb.chat_stream_started(&stream_id, &lane_key);
                }
                openalpaca_core::events::SystemEvent::ChatStreamEnded {
                    stream_id,
                    lane_key,
                    status,
                    ..
                } => {
                    eb.chat_stream_ended(&stream_id, &lane_key, &status);
                }

                // ── Forwarded to clients: existing mappings ───────────────
                openalpaca_core::events::SystemEvent::SoulUpdated {
                    actor,
                    mode,
                    content_sha256,
                    backup_path,
                    ..
                } => {
                    tracing::info!(
                        target: "soul_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        backup_path = ?backup_path,
                        "SOUL.md updated"
                    );
                    eb.soul_updated(&actor, &mode, &content_sha256, backup_path);
                }
                openalpaca_core::events::SystemEvent::DagNodeStarted {
                    task_id,
                    node_id,
                    node_title,
                    agent_id,
                    ..
                } => {
                    eb.dag_node_status(
                        &task_id,
                        &node_id,
                        &node_title,
                        &agent_id,
                        "started",
                        None,
                        None,
                    );
                }
                openalpaca_core::events::SystemEvent::DagNodeCompleted {
                    task_id,
                    node_id,
                    node_title,
                    agent_id,
                    success,
                    duration_ms,
                    output_preview,
                    ..
                } => {
                    let status = if success { "completed" } else { "failed" };
                    eb.dag_node_status(
                        &task_id,
                        &node_id,
                        &node_title,
                        &agent_id,
                        status,
                        Some(duration_ms),
                        output_preview,
                    );
                }

                // ── Log-only (NOT forwarded to clients) ───────────────────
                openalpaca_core::events::SystemEvent::ModelAccessDenied {
                    agent_id,
                    model_id,
                    reason,
                    ..
                } => {
                    tracing::warn!(
                        "Model access denied: agent={agent_id}, model={model_id}, reason={reason}"
                    );
                }
                openalpaca_core::events::SystemEvent::IntentClassified {
                    request_id,
                    intent_type,
                    ..
                } => {
                    tracing::debug!("Intent classified: request={request_id}, type={intent_type}");
                }
                openalpaca_core::events::SystemEvent::UserProfileUpdated {
                    actor,
                    mode,
                    content_sha256,
                    modified_sections,
                    ..
                } => {
                    tracing::info!(
                        target: "user_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        modified_sections = ?modified_sections,
                        "USER.md updated"
                    );
                }
                openalpaca_core::events::SystemEvent::IdentityUpdated {
                    actor,
                    mode,
                    content_sha256,
                    ..
                } => {
                    tracing::info!(
                        target: "identity_audit",
                        actor = %actor,
                        mode = %mode,
                        content_sha256 = %content_sha256,
                        "IDENTITY.md updated"
                    );
                }
                openalpaca_core::events::SystemEvent::BootstrapCompleted {
                    identity_populated,
                    user_populated,
                    ..
                } => {
                    tracing::info!(
                        target: "bootstrap_audit",
                        identity_populated = %identity_populated,
                        user_populated = %user_populated,
                        "Bootstrap onboarding completed"
                    );
                }
                openalpaca_core::events::SystemEvent::DispatchDecision {
                    request_id,
                    task_id,
                    mode,
                    reason,
                    agent_count,
                    error_message,
                    ..
                } => {
                    tracing::debug!(
                        "DispatchDecision: request={request_id}, task={task_id:?}, mode={mode}, reason={reason}, agents={agent_count}, error={error_message:?}"
                    );
                }
                openalpaca_core::events::SystemEvent::OrchestrationStage {
                    request_id,
                    mode,
                    ack_ms,
                    ..
                } => {
                    tracing::debug!(
                        "Orchestration: request={request_id}, mode={mode}, ack={ack_ms}ms"
                    );
                }

                // ── Skill lifecycle events (log-only) ─────────────────────
                openalpaca_core::events::SystemEvent::SkillDiscovered {
                    skill_id,
                    skill_name,
                    scope,
                    ..
                } => {
                    tracing::debug!(
                        "Skill discovered: id={skill_id}, name={skill_name}, scope={scope}"
                    );
                }
                openalpaca_core::events::SystemEvent::SkillSelected {
                    skill_id, score, ..
                } => {
                    tracing::debug!("Skill auto-selected: id={skill_id}, score={score:.3}");
                }
                openalpaca_core::events::SystemEvent::SkillInvocationStarted {
                    request_id,
                    skill_id,
                    query_preview,
                    ..
                } => {
                    tracing::debug!(
                        "Skill invocation started: request={request_id}, skill={skill_id}"
                    );
                    eb.skill_invocation_started(
                        &request_id.to_string(),
                        &skill_id,
                        &query_preview,
                    );
                }
                openalpaca_core::events::SystemEvent::SkillContextInjected {
                    request_id,
                    skill_id,
                    context_bytes,
                    ..
                } => {
                    tracing::debug!(
                        "Skill context injected: request={request_id}, skill={skill_id}, bytes={context_bytes}"
                    );
                }
                openalpaca_core::events::SystemEvent::SkillCompleted {
                    request_id,
                    skill_id,
                    duration_ms,
                    output_preview,
                    ..
                } => {
                    tracing::info!(
                        "Skill completed: request={request_id}, skill={skill_id}, duration={duration_ms}ms"
                    );
                    eb.skill_completed(
                        &request_id.to_string(),
                        &skill_id,
                        duration_ms,
                        &output_preview,
                    );
                }
                openalpaca_core::events::SystemEvent::SkillFailed {
                    request_id,
                    skill_id,
                    error,
                    ..
                } => {
                    tracing::warn!(
                        "Skill failed: request={request_id}, skill={skill_id}, error={error}"
                    );
                    eb.skill_failed(&request_id.to_string(), &skill_id, &error);
                }

                // ── Tool confirmation (interactive approval) ──────────────
                openalpaca_core::events::SystemEvent::ToolConfirmationRequested {
                    request_id,
                    agent_id,
                    tool_name,
                    ref tool_arguments,
                    ref stream_id,
                    ref lane_key,
                    ref task_id,
                    ..
                } => {
                    tracing::info!(
                        "Tool confirmation requested: tool={tool_name}, agent={agent_id}, request={request_id}"
                    );
                    // 1. Forward to WebSocket (GUI + connectors)
                    eb.tool_confirmation_requested(
                        &request_id,
                        &agent_id,
                        &tool_name,
                        tool_arguments,
                        stream_id.as_deref(),
                        lane_key.as_deref(),
                        task_id.as_deref(),
                    );
                    // 2. Forward to SSE chat stream (CLI + GUI active chat)
                    if let (Some(csm), Some(sid)) = (&chat_streams, &stream_id) {
                        let _ = csm.send(
                            sid,
                            openalpaca_core::chat::ChatStreamEvent::ConfirmationRequested {
                                request_id,
                                tool_name,
                                tool_arguments: tool_arguments.clone(),
                            },
                        );
                    }
                }
                openalpaca_core::events::SystemEvent::ContextBudgetComputed {
                    request_id, model, window_size, fixed_zone_tokens, free_zone_tokens, buffer_size, ..
                } => {
                    tracing::debug!(
                        %request_id, %model, window_size, fixed_zone_tokens, free_zone_tokens, buffer_size,
                        "Context budget computed"
                    );
                }
                openalpaca_core::events::SystemEvent::CompactionTriggered {
                    request_id, messages_before, messages_after, memories_extracted, ..
                } => {
                    tracing::info!(
                        %request_id, messages_before, messages_after, memories_extracted,
                        "Context compaction triggered"
                    );
                }
                // Compose-engine cache telemetry (spec section Component 4).
                // Daemon-level side-effect sink is tracing-only for Phase 1.
                openalpaca_core::events::SystemEvent::ComposeLayerCacheHit {
                    ref layer, ref lane_id, ..
                } => {
                    tracing::debug!(?layer, ?lane_id, "Compose layer cache hit");
                }
                openalpaca_core::events::SystemEvent::ComposeLayerCacheMiss {
                    ref layer, ref reason, ref lane_id, ..
                } => {
                    tracing::debug!(?layer, ?reason, ?lane_id, "Compose layer cache miss");
                }
                // ── Routing V2 workflow events (forwarded to clients) ──────
                openalpaca_core::events::SystemEvent::WorkflowStarted {
                    request_id, ref task_id, ref lane_key, ref title, ..
                } => {
                    tracing::info!(
                        %request_id, %task_id, %lane_key, %title, "Workflow started"
                    );
                    eb.workflow_started(task_id, lane_key, title);
                }
                openalpaca_core::events::SystemEvent::WorkflowSteered {
                    ref task_id, ref lane_key, request_id, ..
                } => {
                    tracing::info!(%request_id, %task_id, %lane_key, "Workflow steered");
                    eb.workflow_steered(task_id, lane_key);
                }
                openalpaca_core::events::SystemEvent::WorkflowProgress {
                    ref task_id, ref lane_key, ref message, ..
                } => {
                    tracing::debug!(%task_id, %lane_key, "Workflow progress update");
                    eb.workflow_progress(task_id, lane_key, message);
                }
                openalpaca_core::events::SystemEvent::FollowupQueued {
                    ref lane_key, followup_id, ref kind, ..
                } => {
                    tracing::info!(%lane_key, followup_id, %kind, "Follow-up queued");
                    eb.followup_queued(lane_key, followup_id, kind);
                }
                openalpaca_core::events::SystemEvent::SessionChanged {
                    ref session_id, ref lane_key, ref status, ref task_id, ..
                } => {
                    tracing::info!(%session_id, %lane_key, %status, "Session changed");
                    eb.session_changed(session_id, lane_key, status, task_id.as_deref());
                }
                openalpaca_core::events::SystemEvent::FollowupCancelled {
                    ref lane_key, followup_id, ..
                } => {
                    tracing::info!(%lane_key, followup_id, "Follow-up cancelled");
                    eb.followup_cancelled(lane_key, followup_id);
                }
                openalpaca_core::events::SystemEvent::ArtifactWritten {
                    ref artifact_id,
                    ref task_id,
                    ref agent_id,
                    ref name,
                    ref kind,
                    version,
                    ref path,
                    ..
                } => {
                    tracing::info!(
                        %artifact_id, ?task_id, %name, %kind, version,
                        "Artifact written"
                    );
                    eb.artifact_written(
                        artifact_id,
                        task_id.as_deref(),
                        agent_id.as_deref(),
                        name,
                        kind,
                        version,
                        path,
                    );
                }
                openalpaca_core::events::SystemEvent::SubagentSpan {
                    ref task_id,
                    ref span_id,
                    ref label,
                    ref template_id,
                    ref agent_instance_id,
                    ref state,
                    ref detail,
                    ref started_at,
                    ref ended_at,
                    duration_ms,
                    ref output_preview,
                    ..
                } => {
                    tracing::debug!(
                        %task_id, %span_id, %label, %state,
                        "Subagent span"
                    );
                    eb.subagent_span(
                        task_id,
                        span_id,
                        label,
                        template_id,
                        agent_instance_id,
                        state,
                        detail.as_deref(),
                        started_at,
                        ended_at.as_deref(),
                        duration_ms,
                        output_preview.as_deref(),
                    );
                }
                openalpaca_core::events::SystemEvent::ExtensionStateChanged {
                    ref extension, ref state, generation, tools_changed, ..
                } => {
                    tracing::info!(
                        %extension, %state, generation, tools_changed,
                        "Extension state changed"
                    );
                    eb.extension_state_changed(
                        extension.kind.as_str(),
                        &extension.name,
                        state,
                        generation,
                        tools_changed,
                    );
                }
                openalpaca_core::events::SystemEvent::ExtensionCapabilityWithheld {
                    ref extension, ref subject, moment, ref state, ref scope, stale, ..
                } => {
                    tracing::debug!(
                        %extension, %subject, moment = moment.word(), %state, stale,
                        "Extension capability withheld"
                    );
                    eb.extension_capability_withheld(
                        extension.kind.as_str(),
                        &extension.name,
                        subject,
                        moment.word(),
                        state,
                        scope,
                        stale,
                    );
                }
                openalpaca_core::events::SystemEvent::ExtensionCapabilityWithdrawn {
                    ref extension,
                    ref state,
                    cause,
                    ref capabilities,
                    ref tools,
                    ref affected_templates,
                    ref affected_skills,
                    ref affected_cron_skills,
                    ref notice_lane,
                    ..
                } => {
                    tracing::info!(
                        %extension,
                        cause = cause.word(),
                        templates = affected_templates.len(),
                        skills = affected_skills.len(),
                        cron_skills = affected_cron_skills.len(),
                        "Extension capabilities withdrawn"
                    );
                    eb.extension_capability_withdrawn(
                        extension.kind.as_str(),
                        &extension.name,
                        state.word(),
                        cause.word(),
                        capabilities.clone(),
                        tools.clone(),
                        affected_templates.clone(),
                        affected_skills.clone(),
                        affected_cron_skills.clone(),
                        notice_lane,
                    );
                } // NO catch-all: compiler will flag any missing SystemEvent variant
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_api::events::ServerEvent;
    use openalpaca_core::events::SystemEvent;
    use std::time::Duration;
    use uuid::Uuid;

    /// Spawn a bridge over a fresh bus/broadcaster pair and return
    /// (bus, ServerEvent receiver, cancellation token).
    fn setup_bridge() -> (
        EventBus,
        broadcast::Receiver<ServerEvent>,
        CancellationToken,
    ) {
        let bus = EventBus::new(16);
        let eb = EventBroadcaster::new(16, "test-instance".to_string(), None);
        let rx = eb.subscribe();
        let cancel = CancellationToken::new();
        spawn_event_bridge(eb, &bus, None, cancel.clone());
        (bus, rx, cancel)
    }

    async fn recv_event(rx: &mut broadcast::Receiver<ServerEvent>) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for bridged ServerEvent")
            .expect("broadcast channel closed")
    }

    #[tokio::test]
    async fn test_workflow_started_bridged_to_server_event() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::WorkflowStarted {
            request_id: Uuid::new_v4(),
            task_id: "t-1".into(),
            lane_key: "junpei:cli".into(),
            title: "Research task".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::WorkflowStarted {
                task_id,
                lane_key,
                title,
                instance_id,
                ..
            } => {
                assert_eq!(task_id, "t-1");
                assert_eq!(lane_key, "junpei:cli");
                assert_eq!(title, "Research task");
                assert_eq!(instance_id, "test-instance");
            }
            other => panic!("Expected WorkflowStarted, got {other:?}"),
        }
        cancel.cancel();
    }

    /// The S4 withholding frame carries `ts` **and** `instance_id` — the two
    /// fields the six `plugin_*` variants omit (GAP-22), which is what makes
    /// this family's rows orderable.
    #[tokio::test]
    async fn test_extension_capability_withheld_bridged_with_ts_and_instance_id() {
        use openalpaca_core::tools::extensions::{ExtensionId, Moment};

        let (bus, mut rx, cancel) = setup_bridge();
        let before = chrono::Utc::now();
        bus.publish(SystemEvent::ExtensionCapabilityWithheld {
            extension: ExtensionId::mcp("github"),
            subject: "github__create_issue".into(),
            moment: Moment::AttemptedUse,
            state: "disabled".into(),
            scope: "task-1".into(),
            agent_id: None,
            task_id: Some("task-1".into()),
            stale: false,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ExtensionCapabilityWithheld {
                kind,
                id,
                subject,
                moment,
                state,
                scope,
                stale,
                ts,
                instance_id,
            } => {
                assert_eq!(kind, "mcp");
                assert_eq!(id, "github");
                assert_eq!(subject, "github__create_issue");
                assert_eq!(moment, "attempted_use");
                assert_eq!(state, "disabled");
                assert_eq!(scope, "task-1");
                assert!(!stale);
                assert!(ts >= before);
                assert_eq!(instance_id, "test-instance");
            }
            other => panic!("Expected ExtensionCapabilityWithheld, got {other:?}"),
        }
        cancel.cancel();
    }

    /// T1 step 3's transition frame, with the lists the `NotificationDispatcher`
    /// and the GUI read.
    #[tokio::test]
    async fn test_extension_capability_withdrawn_bridged_with_its_lists() {
        use openalpaca_core::tools::extensions::{ExtensionId, ExtensionState, WithdrawalCause};

        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::ExtensionCapabilityWithdrawn {
            extension: ExtensionId::plugin("acme"),
            state: ExtensionState::Disabling,
            cause: WithdrawalCause::Deny,
            capabilities: vec!["net_read".into()],
            tools: vec!["acme::fetch".into()],
            affected_templates: vec!["reader".into()],
            affected_skills: vec!["nightly".into()],
            affected_cron_skills: vec!["nightly".into()],
            notice_lane: "owner:gui".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
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
                instance_id,
                ..
            } => {
                assert_eq!(kind, "plugin");
                assert_eq!(id, "acme");
                assert_eq!(state, "disabling");
                assert_eq!(cause, "deny", "the wording is keyed on the cause, not the state");
                assert_eq!(capabilities, vec!["net_read".to_string()]);
                assert_eq!(tools, vec!["acme::fetch".to_string()]);
                assert_eq!(affected_templates, vec!["reader".to_string()]);
                assert_eq!(affected_skills, vec!["nightly".to_string()]);
                assert_eq!(affected_cron_skills, vec!["nightly".to_string()]);
                assert_eq!(notice_lane, "owner:gui");
                assert_eq!(instance_id, "test-instance");
            }
            other => panic!("Expected ExtensionCapabilityWithdrawn, got {other:?}"),
        }
        cancel.cancel();
    }

    /// T28 — the artifact announcement crosses the bridge with the bridge's own
    /// `ts`/`instance_id`, exactly as `TaskStatus` does, and keeps `task_id` /
    /// `agent_id` optional.
    #[tokio::test]
    async fn test_artifact_written_bridged_with_ts_and_instance_id() {
        let (bus, mut rx, cancel) = setup_bridge();
        let before = chrono::Utc::now();
        bus.publish(SystemEvent::ArtifactWritten {
            artifact_id: "a-1".into(),
            task_id: Some("t-1".into()),
            agent_id: Some("writing_agent".into()),
            name: "01-quarterly-report.md".into(),
            kind: "markdown".into(),
            version: 2,
            path: "/p/.openalpaca/artifacts/run/01-quarterly-report.md".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ArtifactWritten {
                artifact_id,
                task_id,
                agent_id,
                name,
                kind,
                version,
                path,
                ts,
                instance_id,
            } => {
                assert_eq!(artifact_id, "a-1");
                assert_eq!(task_id.as_deref(), Some("t-1"));
                assert_eq!(agent_id.as_deref(), Some("writing_agent"));
                assert_eq!(name, "01-quarterly-report.md");
                assert_eq!(kind, "markdown");
                assert_eq!(version, 2);
                assert_eq!(path, "/p/.openalpaca/artifacts/run/01-quarterly-report.md");
                assert!(ts >= before);
                assert_eq!(instance_id, "test-instance");
            }
            other => panic!("Expected ArtifactWritten, got {other:?}"),
        }
        cancel.cancel();
    }

    /// A loose artifact — no run, no agent — bridges with both fields absent.
    #[tokio::test]
    async fn test_artifact_written_bridges_without_a_task_or_agent() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::ArtifactWritten {
            artifact_id: "a-2".into(),
            task_id: None,
            agent_id: None,
            name: "01-notes.md".into(),
            kind: "markdown".into(),
            version: 1,
            path: "/h/.openalpaca/artifacts/loose/01-notes.md".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ArtifactWritten {
                task_id, agent_id, ..
            } => {
                assert_eq!(task_id, None);
                assert_eq!(agent_id, None);
            }
            other => panic!("Expected ArtifactWritten, got {other:?}"),
        }
        cancel.cancel();
    }

    /// GAP-09 — a lane's open crosses the bridge with the bridge's own
    /// `ts`/`instance_id`, and the row's own `started_at` string untouched.
    #[tokio::test]
    async fn test_subagent_span_open_bridged_with_ts_and_instance_id() {
        let (bus, mut rx, cancel) = setup_bridge();
        let before = chrono::Utc::now();
        bus.publish(SystemEvent::SubagentSpan {
            task_id: "t-1".into(),
            span_id: "node-1".into(),
            label: "review·1".into(),
            template_id: "review_agent".into(),
            agent_instance_id: "review_agent::a1b2".into(),
            state: "running".into(),
            detail: None,
            started_at: "2026-09-05T10:00:00.000Z".into(),
            ended_at: None,
            duration_ms: None,
            output_preview: None,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
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
                ts,
                instance_id,
            } => {
                assert_eq!(task_id, "t-1");
                assert_eq!(span_id, "node-1");
                assert_eq!(label, "review·1");
                assert_eq!(template_id, "review_agent");
                assert_eq!(agent_instance_id, "review_agent::a1b2");
                assert_eq!(state, "running");
                assert_eq!(detail, None);
                assert_eq!(started_at, "2026-09-05T10:00:00.000Z");
                assert_eq!(ended_at, None);
                assert_eq!(duration_ms, None);
                assert_eq!(output_preview, None);
                assert!(ts >= before);
                assert_eq!(instance_id, "test-instance");
            }
            other => panic!("Expected SubagentSpan, got {other:?}"),
        }
        cancel.cancel();
    }

    /// A close carries every closing field across unchanged — a cancellation
    /// stays a cancellation rather than becoming "not successful".
    #[tokio::test]
    async fn test_subagent_span_close_bridges_its_terminal_fields() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::SubagentSpan {
            task_id: "t-1".into(),
            span_id: "node-1".into(),
            label: "review·1".into(),
            template_id: "review_agent".into(),
            agent_instance_id: "review_agent::a1b2".into(),
            state: "cancelled".into(),
            detail: Some("cancelled before starting".into()),
            started_at: "2026-09-05T10:00:00.000Z".into(),
            ended_at: Some("2026-09-05T10:00:04.500Z".into()),
            duration_ms: Some(4_500),
            output_preview: Some("partial".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::SubagentSpan {
                state,
                detail,
                ended_at,
                duration_ms,
                output_preview,
                ..
            } => {
                assert_eq!(state, "cancelled");
                assert_eq!(detail.as_deref(), Some("cancelled before starting"));
                assert_eq!(ended_at.as_deref(), Some("2026-09-05T10:00:04.500Z"));
                assert_eq!(duration_ms, Some(4_500));
                assert_eq!(output_preview.as_deref(), Some("partial"));
            }
            other => panic!("Expected SubagentSpan, got {other:?}"),
        }
        cancel.cancel();
    }

    // ── GAP-10: the run crosses the bridge with the frame ──────────────

    /// The five security/tool frames carry `task_id` from the core bus to the
    /// client-facing twin, so a socket consumer can scope them to a run and
    /// the persistence arm has an id to index on.
    #[tokio::test]
    async fn test_tool_and_security_events_bridge_their_task_id() {
        let (bus, mut rx, cancel) = setup_bridge();

        bus.publish(SystemEvent::ToolExecuted {
            agent_id: "research_agent::a1".into(),
            tool_name: "web_search".into(),
            success: true,
            duration_ms: 12,
            task_id: Some("t-1".into()),
            session_id: None,
            tool_use_id: None,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ToolExecuted { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("t-1"))
            }
            other => panic!("Expected ToolExecuted, got {other:?}"),
        }

        bus.publish(SystemEvent::SecurityViolation {
            agent_id: "research_agent::a1".into(),
            tool_name: "shell".into(),
            reason: "denied".into(),
            task_id: Some("t-1".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::SecurityViolation { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("t-1"))
            }
            other => panic!("Expected SecurityViolation, got {other:?}"),
        }

        bus.publish(SystemEvent::CircuitBreakerTripped {
            agent_id: "research_agent::a1".into(),
            tool_name: "web_search".into(),
            consecutive_failures: 3,
            reset_after_secs: 300,
            task_id: Some("t-1".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::CircuitBreakerTripped { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("t-1"))
            }
            other => panic!("Expected CircuitBreakerTripped, got {other:?}"),
        }

        bus.publish(SystemEvent::LlmCallCompleted {
            agent_id: "research_agent::a1".into(),
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: 0.1,
            task_id: Some("t-1".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::LlmCallCompleted { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("t-1"))
            }
            other => panic!("Expected LlmCallCompleted, got {other:?}"),
        }

        bus.publish(SystemEvent::ToolConfirmationRequested {
            request_id: "req-1".into(),
            agent_id: "research_agent::a1".into(),
            tool_name: "shell".into(),
            tool_arguments: serde_json::json!({}),
            stream_id: None,
            lane_key: None,
            task_id: Some("t-1".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ToolConfirmationRequested { task_id, .. } => {
                assert_eq!(task_id.as_deref(), Some("t-1"))
            }
            other => panic!("Expected ToolConfirmationRequested, got {other:?}"),
        }

        cancel.cancel();
    }

    /// A frame from outside any run bridges with `task_id: None` — the bridge
    /// never invents attribution.
    #[tokio::test]
    async fn test_a_task_less_tool_event_bridges_without_a_task_id() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::ToolExecuted {
            agent_id: "orchestrator".into(),
            tool_name: "web_search".into(),
            success: true,
            duration_ms: 12,
            task_id: None,
            session_id: None,
            tool_use_id: None,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::ToolExecuted { task_id, .. } => assert_eq!(task_id, None),
            other => panic!("Expected ToolExecuted, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_workflow_steered_bridged_to_server_event() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::WorkflowSteered {
            task_id: "t-2".into(),
            lane_key: "junpei:cli".into(),
            request_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::WorkflowSteered {
                task_id, lane_key, ..
            } => {
                assert_eq!(task_id, "t-2");
                assert_eq!(lane_key, "junpei:cli");
            }
            other => panic!("Expected WorkflowSteered, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_workflow_progress_bridged_to_server_event() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::WorkflowProgress {
            task_id: "t-3".into(),
            lane_key: "junpei:cli".into(),
            message: "Halfway done".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::WorkflowProgress {
                task_id, message, ..
            } => {
                assert_eq!(task_id, "t-3");
                assert_eq!(message, "Halfway done");
            }
            other => panic!("Expected WorkflowProgress, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_followup_queued_bridged_to_server_event() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::FollowupQueued {
            lane_key: "junpei:cli".into(),
            followup_id: 42,
            kind: "followup".into(),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::FollowupQueued {
                lane_key,
                followup_id,
                kind,
                ..
            } => {
                assert_eq!(lane_key, "junpei:cli");
                assert_eq!(followup_id, 42);
                assert_eq!(kind, "followup");
            }
            other => panic!("Expected FollowupQueued, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_followup_cancelled_bridged_to_server_event() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::FollowupCancelled {
            lane_key: "junpei:cli".into(),
            followup_id: 42,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::FollowupCancelled {
                lane_key,
                followup_id,
                ..
            } => {
                assert_eq!(lane_key, "junpei:cli");
                assert_eq!(followup_id, 42);
            }
            other => panic!("Expected FollowupCancelled, got {other:?}"),
        }
        cancel.cancel();
    }

    // ── GAP-07: task/agent events must carry a non-empty title/name ────

    #[tokio::test]
    async fn test_task_updated_bridged_carries_title() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::TaskUpdated {
            task_id: "t-updated".into(),
            title: "Sync the repo".into(),
            status: "running".into(),
            progress_current: Some(1),
            progress_total: Some(4),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::TaskStatus {
                task_id,
                title,
                status,
                ..
            } => {
                assert_eq!(task_id, "t-updated");
                assert_eq!(title, "Sync the repo");
                assert!(!title.is_empty());
                assert_eq!(status, "running");
            }
            other => panic!("Expected TaskStatus, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_task_completed_bridged_carries_title() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::TaskCompleted {
            task_id: "t-completed".into(),
            title: "Generate the report".into(),
            result_summary: Some("Done".into()),
            outcome_kind: None,
            artifact_count: None,
            outcome_summary: None,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::TaskStatus {
                task_id,
                title,
                status,
                ..
            } => {
                assert_eq!(task_id, "t-completed");
                assert_eq!(title, "Generate the report");
                assert!(!title.is_empty());
                assert_eq!(status, "completed");
            }
            other => panic!("Expected TaskStatus, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_task_failed_bridged_carries_title() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::TaskFailed {
            task_id: "t-failed".into(),
            title: "Deploy the release".into(),
            error: "Network timeout".into(),
            outcome_kind: None,
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::TaskStatus {
                task_id,
                title,
                status,
                result_summary,
                ..
            } => {
                assert_eq!(task_id, "t-failed");
                assert_eq!(title, "Deploy the release");
                assert!(!title.is_empty());
                assert_eq!(status, "failed");
                assert_eq!(result_summary, Some("Network timeout".to_string()));
            }
            other => panic!("Expected TaskStatus, got {other:?}"),
        }
        cancel.cancel();
    }

    #[tokio::test]
    async fn test_agent_status_changed_bridged_carries_name() {
        let (bus, mut rx, cancel) = setup_bridge();
        bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: "code_agent::a1b2c3d4".into(),
            instance_id: "code_agent::a1b2c3d4".into(),
            template_id: "code_agent".into(),
            name: "Code Agent".into(),
            status: "spawned".into(),
            current_task_id: Some("t-1".into()),
            timestamp: chrono::Utc::now(),
        });
        match recv_event(&mut rx).await {
            ServerEvent::AgentStatus {
                agent_id,
                name,
                status,
                template_id,
                ..
            } => {
                assert_eq!(agent_id, "code_agent::a1b2c3d4");
                assert_eq!(name, "Code Agent");
                assert!(!name.is_empty());
                assert_eq!(status, "spawned");
                assert_eq!(template_id, "code_agent");
            }
            other => panic!("Expected AgentStatus, got {other:?}"),
        }
        cancel.cancel();
    }
}
