use crate::events::EventBroadcaster;
use openalpaca_core::bus::EventBus;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Spawn the SystemEvent → ServerEvent bridge task.
///
/// Listens on the core `EventBus` and forwards relevant events to the
/// `EventBroadcaster` for WebSocket/SSE delivery to API clients.
pub fn spawn_event_bridge(
    eb: EventBroadcaster,
    bus: &EventBus,
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
                    eb.task_status(&task_id, &title, "queued", None, None, None);
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
                    );
                }
                openalpaca_core::events::SystemEvent::TaskCompleted {
                    task_id,
                    result_summary,
                    ..
                } => {
                    eb.task_status(&task_id, "", "completed", None, None, result_summary);
                }
                openalpaca_core::events::SystemEvent::TaskFailed { task_id, error, .. } => {
                    eb.task_status(&task_id, "", "failed", None, None, Some(error));
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
                    eb.llm_call_completed(
                        &agent_id,
                        &model,
                        input_tokens,
                        output_tokens,
                        cost_usd,
                    );
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
                } // NO catch-all: compiler will flag any missing SystemEvent variant
            }
        }
    });
}
