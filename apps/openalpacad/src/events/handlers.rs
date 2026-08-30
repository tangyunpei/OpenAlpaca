//! Event broadcast handler methods for EventBroadcaster.

use super::EventBroadcaster;
use chrono::Utc;
use openalpaca_api::events::ServerEvent;

impl EventBroadcaster {
    /// Broadcast a task status event and persist it
    #[allow(clippy::too_many_arguments)]
    pub fn task_status(
        &self,
        task_id: &str,
        title: &str,
        status: &str,
        progress_current: Option<i32>,
        progress_total: Option<i32>,
        result_summary: Option<String>,
        outcome_kind: Option<String>,
        artifact_count: Option<i32>,
        outcome_summary: Option<String>,
    ) {
        let event = ServerEvent::TaskStatus {
            task_id: task_id.to_string(),
            title: title.to_string(),
            status: status.to_string(),
            progress_current,
            progress_total,
            result_summary,
            outcome_kind,
            artifact_count,
            outcome_summary,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an agent status event and persist it
    pub fn agent_status(
        &self,
        agent_id: &str,
        name: &str,
        status: &str,
        current_task_id: Option<String>,
        agent_instance_id: &str,
        template_id: &str,
    ) {
        let event = ServerEvent::AgentStatus {
            agent_id: agent_id.to_string(),
            name: name.to_string(),
            status: status.to_string(),
            current_task_id,
            agent_instance_id: agent_instance_id.to_string(),
            template_id: template_id.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a key status change event and persist it
    pub fn key_status_changed(&self, provider: &str, key_id: &str, status: &str) {
        let event = ServerEvent::KeyStatusChanged {
            provider: provider.to_string(),
            key_id: key_id.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a connector status change and persist it
    pub fn connector_status(&self, id: &str, status: &str) {
        let event = ServerEvent::ConnectorStatus {
            id: id.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a chat stream started event and persist it
    pub fn chat_stream_started(&self, stream_id: &str, lane_key: &str) {
        let event = ServerEvent::ChatStreamStarted {
            stream_id: stream_id.to_string(),
            lane_key: lane_key.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an agent config changed event and persist it
    pub fn agent_config_changed(&self, agent_id: &str, action: &str, config_version: u64) {
        let event = ServerEvent::AgentConfigChanged {
            agent_id: agent_id.to_string(),
            action: action.to_string(),
            config_version,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast an orchestrator config changed event and persist it
    pub fn orchestrator_config_changed(&self, model: &str) {
        let event = ServerEvent::OrchestratorConfigChanged {
            model: model.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a daemon config changed event and persist it
    pub fn daemon_config_changed(&self) {
        let event = ServerEvent::DaemonConfigChanged {
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a chat stream ended event and persist it
    pub fn chat_stream_ended(&self, stream_id: &str, lane_key: &str, status: &str) {
        let event = ServerEvent::ChatStreamEnded {
            stream_id: stream_id.to_string(),
            lane_key: lane_key.to_string(),
            status: status.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a DAG node status event and persist it
    #[allow(clippy::too_many_arguments)]
    pub fn dag_node_status(
        &self,
        task_id: &str,
        node_id: &str,
        node_title: &str,
        agent_id: &str,
        status: &str,
        duration_ms: Option<u64>,
        output_preview: Option<String>,
    ) {
        let event = ServerEvent::DagNodeStatus {
            task_id: task_id.to_string(),
            node_id: node_id.to_string(),
            node_title: node_title.to_string(),
            agent_id: agent_id.to_string(),
            status: status.to_string(),
            duration_ms,
            output_preview,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a security violation event and persist it
    pub fn security_violation(&self, agent_id: &str, tool_name: &str, reason: &str) {
        let event = ServerEvent::SecurityViolation {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            reason: reason.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a circuit breaker tripped event and persist it
    pub fn circuit_breaker_tripped(
        &self,
        agent_id: &str,
        tool_name: &str,
        consecutive_failures: usize,
        reset_after_secs: u64,
    ) {
        let event = ServerEvent::CircuitBreakerTripped {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            consecutive_failures,
            reset_after_secs,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a tool executed event and persist it
    pub fn tool_executed(&self, agent_id: &str, tool_name: &str, success: bool, duration_ms: u64) {
        let event = ServerEvent::ToolExecuted {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            success,
            duration_ms,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        self.persist_tool_execution(agent_id, tool_name, success, duration_ms);
        let _ = self.tx.send(event);
    }

    /// Broadcast an LLM call completed event and persist it
    pub fn llm_call_completed(
        &self,
        agent_id: &str,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
        cost_usd: f64,
    ) {
        let event = ServerEvent::LlmCallCompleted {
            agent_id: agent_id.to_string(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cost_usd,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a skill catalog updated event and persist it
    pub fn skill_catalog_updated(&self, skill_name: &str, action: &str) {
        let event = ServerEvent::SkillCatalogUpdated {
            skill_name: skill_name.to_string(),
            action: action.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a skill invocation started event and persist it
    pub fn skill_invocation_started(
        &self,
        request_id: &str,
        skill_id: &str,
        query_preview: &str,
    ) {
        let event = ServerEvent::SkillInvocationStarted {
            request_id: request_id.to_string(),
            skill_id: skill_id.to_string(),
            query_preview: query_preview.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a skill completed event and persist it
    pub fn skill_completed(
        &self,
        request_id: &str,
        skill_id: &str,
        duration_ms: u64,
        output_preview: &str,
    ) {
        let event = ServerEvent::SkillCompleted {
            request_id: request_id.to_string(),
            skill_id: skill_id.to_string(),
            duration_ms,
            output_preview: output_preview.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a skill failed event and persist it
    pub fn skill_failed(&self, request_id: &str, skill_id: &str, error: &str) {
        let event = ServerEvent::SkillFailed {
            request_id: request_id.to_string(),
            skill_id: skill_id.to_string(),
            error: error.to_string(),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a tool confirmation requested event and persist it
    #[allow(clippy::too_many_arguments)]
    pub fn tool_confirmation_requested(
        &self,
        request_id: &str,
        agent_id: &str,
        tool_name: &str,
        tool_arguments: &serde_json::Value,
        stream_id: Option<&str>,
        lane_key: Option<&str>,
    ) {
        let event = ServerEvent::ToolConfirmationRequested {
            request_id: request_id.to_string(),
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            tool_arguments: tool_arguments.clone(),
            stream_id: stream_id.map(|s| s.to_string()),
            lane_key: lane_key.map(|s| s.to_string()),
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }

    /// Broadcast a SOUL.md update event and persist it
    pub fn soul_updated(
        &self,
        actor: &str,
        mode: &str,
        content_sha256: &str,
        backup_path: Option<String>,
    ) {
        let event = ServerEvent::SoulUpdated {
            actor: actor.to_string(),
            mode: mode.to_string(),
            content_sha256: content_sha256.to_string(),
            backup_path,
            ts: Utc::now(),
            instance_id: self.instance_id.clone(),
        };

        self.persist(&event);
        let _ = self.tx.send(event);
    }
}
