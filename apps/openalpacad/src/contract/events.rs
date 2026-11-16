//! One sample per `ServerEvent` variant, serialized as the events socket
//! serializes a frame (`routes/events.rs`, `serde_json::to_string`).
//!
//! [`index`] is an exhaustive match with no catch-all, so a new variant does
//! not compile until it has an index, and the check below fails until
//! [`examples`] carries one of it. Every `Option` is `Some` and every
//! skipped-when-empty field set, so each sample carries every key the
//! variant can.

use chrono::{TimeZone, Utc};
use openalpaca_api::events::{ConfirmationOutcome, ServerEvent, ToolFailureReason, WakeEvent};

use super::Samples;

/// How many variants `ServerEvent` has: [`index`]'s range.
const VARIANTS: usize = 36;

fn index(event: &ServerEvent) -> usize {
    match event {
        ServerEvent::Heartbeat { .. } => 0,
        ServerEvent::DaemonShuttingDown { .. } => 1,
        ServerEvent::CommandReceived { .. } => 2,
        ServerEvent::Wake { .. } => 3,
        ServerEvent::ConnectorStatus { .. } => 4,
        ServerEvent::TaskStatus { .. } => 5,
        ServerEvent::AgentStatus { .. } => 6,
        ServerEvent::KeyStatusChanged { .. } => 7,
        ServerEvent::ChatStreamStarted { .. } => 8,
        ServerEvent::ChatStreamEnded { .. } => 9,
        ServerEvent::AgentConfigChanged { .. } => 10,
        ServerEvent::OrchestratorConfigChanged { .. } => 11,
        ServerEvent::DaemonConfigChanged { .. } => 12,
        ServerEvent::SecurityViolation { .. } => 13,
        ServerEvent::CircuitBreakerTripped { .. } => 14,
        ServerEvent::ToolExecuted { .. } => 15,
        ServerEvent::LlmCallCompleted { .. } => 16,
        ServerEvent::SkillCatalogUpdated { .. } => 17,
        ServerEvent::SkillInvocationStarted { .. } => 18,
        ServerEvent::SkillCompleted { .. } => 19,
        ServerEvent::SkillFailed { .. } => 20,
        ServerEvent::ToolConfirmationRequested { .. } => 21,
        ServerEvent::ToolConfirmationResolved { .. } => 22,
        ServerEvent::SoulUpdated { .. } => 23,
        ServerEvent::WorkflowStarted { .. } => 24,
        ServerEvent::WorkflowSteered { .. } => 25,
        ServerEvent::WorkflowProgress { .. } => 26,
        ServerEvent::FollowupQueued { .. } => 27,
        ServerEvent::SessionChanged { .. } => 28,
        ServerEvent::FollowupCancelled { .. } => 29,
        ServerEvent::ArtifactWritten { .. } => 30,
        ServerEvent::SubagentSpan { .. } => 31,
        ServerEvent::ExtensionStateChanged { .. } => 32,
        ServerEvent::ExtensionCapabilityWithheld { .. } => 33,
        ServerEvent::ExtensionCapabilityWithdrawn { .. } => 34,
        ServerEvent::MemoryChanged { .. } => 35,
    }
}

fn examples() -> Vec<ServerEvent> {
    let ts = Utc.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
    let instance_id = || "0f1e2d3c-4b5a-4968-8776-655443322110".to_string();
    let s = |v: &str| v.to_string();
    vec![
        ServerEvent::Heartbeat {
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::DaemonShuttingDown {
            grace_secs: 10,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::CommandReceived {
            request_id: s("req-1"),
            command: s("shutdown"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::Wake {
            wake: WakeEvent::Timer {
                job_id: s("job-1"),
                tag: s("daily"),
            },
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ConnectorStatus {
            id: s("telegram"),
            status: s("connected"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::TaskStatus {
            task_id: s("task-1"),
            title: s("A run"),
            status: s("completed"),
            progress_current: Some(1),
            progress_total: Some(2),
            result_summary: Some(s("done")),
            outcome_kind: Some(s("answer")),
            artifact_count: Some(1),
            outcome_summary: Some(s("done")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::AgentStatus {
            agent_id: s("agent-1"),
            name: s("general_agent"),
            status: s("running"),
            current_task_id: Some(s("task-1")),
            agent_instance_id: s("inst-1"),
            template_id: s("general_agent"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::KeyStatusChanged {
            provider: s("anthropic"),
            key_id: s("key-1"),
            status: s("ok"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ChatStreamStarted {
            stream_id: s("stream-1"),
            lane_key: s("owner:gui"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ChatStreamEnded {
            stream_id: s("stream-1"),
            lane_key: s("owner:gui"),
            status: s("completed"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::AgentConfigChanged {
            agent_id: s("agent-1"),
            action: s("updated"),
            config_version: 2,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::OrchestratorConfigChanged {
            model: s("claude-haiku-5-5"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::DaemonConfigChanged {
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SecurityViolation {
            agent_id: s("agent-1"),
            tool_name: s("shell"),
            reason: s("not allowed"),
            task_id: Some(s("task-1")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::CircuitBreakerTripped {
            agent_id: s("agent-1"),
            tool_name: s("web_fetch"),
            consecutive_failures: 3,
            reset_after_secs: 60,
            task_id: Some(s("task-1")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ToolExecuted {
            agent_id: s("agent-1"),
            tool_name: s("web_fetch"),
            success: false,
            duration_ms: 5,
            task_id: Some(s("task-1")),
            reason: Some(ToolFailureReason::Timeout),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::LlmCallCompleted {
            agent_id: s("agent-1"),
            model: s("claude-haiku-5-5"),
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.01,
            task_id: Some(s("task-1")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SkillCatalogUpdated {
            skill_name: s("daily"),
            action: s("added"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SkillInvocationStarted {
            request_id: s("req-1"),
            skill_id: s("daily"),
            query_preview: s("what"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SkillCompleted {
            request_id: s("req-1"),
            skill_id: s("daily"),
            duration_ms: 5,
            output_preview: s("that"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SkillFailed {
            request_id: s("req-1"),
            skill_id: s("daily"),
            error: s("failed"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ToolConfirmationRequested {
            request_id: s("req-1"),
            agent_id: s("agent-1"),
            tool_name: s("file_write"),
            tool_arguments: serde_json::json!({ "path": "notes.md" }),
            stream_id: Some(s("stream-1")),
            lane_key: Some(s("owner:gui")),
            task_id: Some(s("task-1")),
            preview: Some(s("+ a line")),
            per_call: true,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ToolConfirmationResolved {
            request_id: s("req-1"),
            agent_id: s("agent-1"),
            tool_name: s("file_write"),
            outcome: ConfirmationOutcome::Approved,
            stream_id: Some(s("stream-1")),
            lane_key: Some(s("owner:gui")),
            task_id: Some(s("task-1")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SoulUpdated {
            actor: s("owner"),
            mode: s("replace"),
            content_sha256: s("00"),
            backup_path: Some(s("backup")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::WorkflowStarted {
            task_id: s("task-1"),
            lane_key: s("owner:gui"),
            title: s("A run"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::WorkflowSteered {
            task_id: s("task-1"),
            lane_key: s("owner:gui"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::WorkflowProgress {
            task_id: s("task-1"),
            lane_key: s("owner:gui"),
            message: s("step"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::FollowupQueued {
            lane_key: s("owner:gui"),
            followup_id: 1,
            kind: s("message"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SessionChanged {
            session_id: s("session-1"),
            lane_key: s("owner:gui"),
            status: s("active"),
            task_id: Some(s("task-1")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::FollowupCancelled {
            lane_key: s("owner:gui"),
            followup_id: 1,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ArtifactWritten {
            artifact_id: s("artifact-1"),
            task_id: Some(s("task-1")),
            agent_id: Some(s("agent-1")),
            name: s("report.md"),
            kind: s("document"),
            version: 1,
            path: s("artifacts/report.md"),
            in_project: false,
            openable: true,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::SubagentSpan {
            task_id: s("task-1"),
            span_id: s("span-1"),
            label: s("research"),
            template_id: s("general_agent"),
            agent_instance_id: s("inst-1"),
            state: s("completed"),
            detail: Some(s("done")),
            started_at: s("2026-11-16T12:00:00.000Z"),
            ended_at: Some(s("2026-11-16T12:00:01.000Z")),
            duration_ms: Some(1000),
            output_preview: Some(s("found")),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ExtensionStateChanged {
            kind: s("mcp"),
            id: s("files"),
            state: s("enabled"),
            generation: 1,
            tools_changed: true,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ExtensionCapabilityWithheld {
            kind: s("mcp"),
            id: s("files"),
            subject: s("files__read"),
            moment: s("call"),
            state: s("disabled"),
            scope: s("tool"),
            stale: false,
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::ExtensionCapabilityWithdrawn {
            kind: s("mcp"),
            id: s("files"),
            state: s("disabled"),
            cause: s("disabled"),
            capabilities: vec![s("files__read")],
            tools: vec![s("files__read")],
            affected_templates: vec![s("general_agent")],
            affected_skills: vec![s("daily")],
            affected_cron_skills: vec![s("daily")],
            notice_lane: s("owner:gui"),
            ts,
            instance_id: instance_id(),
        },
        ServerEvent::MemoryChanged {
            partition_key: s("p"),
            note_id: s("n"),
            change: s("created"),
            revision: Some(s("r1")),
            origin: s("user"),
            surface: Some(s("gui")),
            ts,
            instance_id: instance_id(),
        },
    ]
}

/// One sample per variant, as the socket sends it.
pub(super) fn samples(out: &mut Samples) {
    let examples = examples();
    let mut seen = [false; VARIANTS];
    for event in &examples {
        let i = index(event);
        assert!(i < VARIANTS, "index {i} is past VARIANTS; raise VARIANTS");
        assert!(!seen[i], "two examples of variant {i}");
        seen[i] = true;
        let frame = serde_json::to_string(event).expect("a frame serializes");
        out.event(serde_json::from_str(&frame).expect("a frame is JSON"));
    }
    let missing: Vec<usize> = (0..VARIANTS).filter(|i| !seen[*i]).collect();
    assert!(
        missing.is_empty(),
        "ServerEvent variants with no example: {missing:?}"
    );
}
