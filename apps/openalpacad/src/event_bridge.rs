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
                    ..
                } => {
                    tracing::warn!(
                        "Security violation: agent={agent_id}, tool={tool_name}, reason={reason}"
                    );
                    eb.security_violation(&agent_id, &tool_name, &reason);
                }
                openalpaca_core::events::SystemEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    success,
                    duration_ms,
                    ..
                } => {
                    tracing::debug!(
                        "Tool executed: agent={agent_id}, tool={tool_name}, success={success}, duration={duration_ms}ms"
                    );
                    eb.tool_executed(&agent_id, &tool_name, success, duration_ms);
                }
                openalpaca_core::events::SystemEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    ..
                } => {
                    tracing::info!(
                        "LLM call: agent={agent_id}, model={model}, tokens={input_tokens}/{output_tokens}, cost=${cost_usd:.6}"
                    );
                    eb.llm_call_completed(&agent_id, &model, input_tokens, output_tokens, cost_usd);
                }
                openalpaca_core::events::SystemEvent::CircuitBreakerTripped {
                    agent_id,
                    tool_name,
                    consecutive_failures,
                    reset_after_secs,
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
