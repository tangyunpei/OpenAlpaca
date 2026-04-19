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
                    status,
                    progress_current,
                    progress_total,
                    ..
                } => {
                    eb.task_status(
                        &task_id,
                        "",
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
                    result_summary,
                    outcome_kind,
                    artifact_count,
                    outcome_summary,
                    ..
                } => {
                    eb.task_status(
                        &task_id, "", "completed", None, None, result_summary,
                        outcome_kind, artifact_count, outcome_summary,
                    );
                }
                openalpaca_core::events::SystemEvent::TaskFailed {
                    task_id, error, outcome_kind, ..
                } => {
                    eb.task_status(
                        &task_id, "", "failed", None, None, Some(error),
                        outcome_kind, None, None,
                    );
                }
                openalpaca_core::events::SystemEvent::AgentStatusChanged {
                    agent_id,
                    instance_id,
                    template_id,
                    status,
                    current_task_id,
                    ..
                } => {
                    eb.agent_status(
                        &agent_id,
                        "",
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
                openalpaca_core::events::SystemEvent::TaskReplanned {
                    task_id,
                    replan_number,
                    decision,
                    nodes_added,
                    nodes_removed,
                    ..
                } => {
                    eb.task_replanned(
                        &task_id,
                        replan_number,
                        &decision,
                        nodes_added,
                        nodes_removed,
                    );
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

                // ── Infrastructure events (handled via separate channels) ─
                openalpaca_core::events::SystemEvent::Heartbeat { .. } => {
                    // Heartbeat managed by dedicated timer, not the bridge
                }
                openalpaca_core::events::SystemEvent::Wake(_) => {
                    // Wake events forwarded via dedicated wake_rx channel
                }
                openalpaca_core::events::SystemEvent::Log { .. } => {
                    // Log events generated by EventBroadcaster directly
                }
                openalpaca_core::events::SystemEvent::UserRequest { .. } => {
                    // User requests flow through chat SSE, not event bus bridge
                }
                openalpaca_core::events::SystemEvent::AgentResponse { .. } => {
                    // Agent responses flow through chat SSE
                }
                openalpaca_core::events::SystemEvent::Error { code, message, .. } => {
                    tracing::error!("System error: code={code}, message={message}");
                }
                openalpaca_core::events::SystemEvent::PlannerBypassed {
                    request_id,
                    reason,
                    ..
                } => {
                    tracing::debug!("Planner bypassed: request={request_id}, reason={reason}");
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
                    planner_ms,
                    dispatch_ms,
                    ack_ms,
                    ..
                } => {
                    tracing::debug!(
                        "Orchestration: request={request_id}, mode={mode}, planner={planner_ms}ms, dispatch={dispatch_ms}ms, ack={ack_ms}ms"
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
                openalpaca_core::events::SystemEvent::CompactionPhaseCompleted {
                    request_id, ref phase, duration_ms, items_processed, ..
                } => {
                    tracing::debug!(
                        %request_id, %phase, duration_ms, items_processed,
                        "Compaction phase completed"
                    );
                }
                openalpaca_core::events::SystemEvent::ContextPackageBuilt {
                    request_id, ref agent_id, ref sections, total_tokens, budget, sub_agent_window, ..
                } => {
                    tracing::debug!(
                        %request_id, %agent_id, ?sections, total_tokens, budget, sub_agent_window,
                        "Context package built for sub-agent"
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
                } // NO catch-all: compiler will flag any missing SystemEvent variant
            }
        }
    });
}
