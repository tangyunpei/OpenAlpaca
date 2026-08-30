use super::DaemonConfig;

pub(super) fn clamp_val<T: PartialOrd + std::fmt::Display>(val: &mut T, min: T, max: T, name: &str) {
    if *val < min {
        tracing::warn!("Config '{name}': {val} below minimum {min}, clamping");
        *val = min;
    } else if *val > max {
        tracing::warn!("Config '{name}': {val} above maximum {max}, clamping");
        *val = max;
    }
}

impl DaemonConfig {
    /// Validate and clamp all fields to their allowed ranges.
    ///
    /// Ranges are sourced from `config_schema.rs` `ConfigKeyDef` entries.
    /// Invalid values are clamped to the nearest valid boundary with a warning log.
    pub fn validate(&mut self) {
        // ── Orchestrator > Memory ──
        clamp_val(
            &mut self.orchestrator.memory.prompt_recent_messages,
            1,
            200,
            "prompt_recent_messages",
        );
        clamp_val(
            &mut self.orchestrator.memory.summary_min_new_older_messages,
            1,
            200,
            "summary_min_new_older_messages",
        );
        clamp_val(
            &mut self.orchestrator.memory.summary_max_chars,
            100,
            32000,
            "summary_max_chars",
        );
        clamp_val(
            &mut self.orchestrator.memory.msg_trunc_chars,
            100,
            32000,
            "msg_trunc_chars",
        );
        clamp_val(
            &mut self.orchestrator.memory.supersession_distance_threshold,
            0.0,
            10.0,
            "supersession_distance_threshold",
        );
        clamp_val(
            &mut self.orchestrator.memory.fts_jaccard_threshold,
            0.0,
            1.0,
            "fts_jaccard_threshold",
        );
        clamp_val(
            &mut self.orchestrator.memory.profile_confidence_threshold,
            0.0,
            1.0,
            "profile_confidence_threshold",
        );
        clamp_val(
            &mut self.orchestrator.memory.profile_update_confidence_threshold,
            0.0,
            1.0,
            "profile_update_confidence_threshold",
        );
        clamp_val(
            &mut self.orchestrator.memory.memory_confidence_threshold,
            0.0,
            1.0,
            "memory_confidence_threshold",
        );
        // ── Orchestrator > Memory > Decay ──
        clamp_val(
            &mut self.orchestrator.memory.decay.poll_interval_secs,
            60,
            86400,
            "decay.poll_interval_secs",
        );
        clamp_val(
            &mut self.orchestrator.memory.decay.half_life_days,
            1.0,
            365.0,
            "decay.half_life_days",
        );
        clamp_val(
            &mut self.orchestrator.memory.decay.min_importance,
            0.0,
            1.0,
            "decay.min_importance",
        );
        clamp_val(
            &mut self.orchestrator.memory.decay.soft_cap,
            10,
            100_000,
            "decay.soft_cap",
        );
        clamp_val(
            &mut self.orchestrator.memory.decay.access_boost,
            0.0,
            1.0,
            "decay.access_boost",
        );
        // ── Orchestrator > Costs ──
        clamp_val(
            &mut self.orchestrator.costs.summary_max_daily_cost_usd,
            0.0,
            100.0,
            "summary_max_daily_cost_usd",
        );
        clamp_val(
            &mut self.orchestrator.costs.extract_max_daily_cost_usd,
            0.0,
            100.0,
            "extract_max_daily_cost_usd",
        );
        clamp_val(
            &mut self.orchestrator.costs.extract_every_n_turns,
            1,
            100,
            "extract_every_n_turns",
        );
        clamp_val(
            &mut self.orchestrator.costs.task_extract_max_daily_cost_usd,
            0.0,
            100.0,
            "task_extract_max_daily_cost_usd",
        );
        // ── Orchestrator > Prompt Budgets ──
        clamp_val(
            &mut self.orchestrator.prompt_budgets.identity_budget,
            50,
            5000,
            "identity_budget",
        );
        clamp_val(
            &mut self.orchestrator.prompt_budgets.user_profile_budget,
            100,
            10000,
            "user_profile_budget",
        );
        // ── Orchestrator > Routing ──
        clamp_val(
            &mut self.orchestrator.routing.steering_inbox_cap,
            1,
            256,
            "routing.steering_inbox_cap",
        );
        clamp_val(
            &mut self.orchestrator.routing.max_workflows_per_lane,
            1,
            16,
            "routing.max_workflows_per_lane",
        );
        clamp_val(
            &mut self.orchestrator.routing.main_loop_max_rounds,
            1,
            100,
            "routing.main_loop_max_rounds",
        );
        clamp_val(
            &mut self.orchestrator.routing.main_loop_max_tools_per_round,
            1,
            50,
            "routing.main_loop_max_tools_per_round",
        );
        if self.orchestrator.routing.tool_selection != "core_union"
            && self.orchestrator.routing.tool_selection != "full"
        {
            tracing::warn!(
                "Config 'routing.tool_selection': unknown value '{}', resetting to 'core_union'",
                self.orchestrator.routing.tool_selection
            );
            self.orchestrator.routing.tool_selection = "core_union".to_string();
        }
        // ── Execution > Agent Defaults ──
        clamp_val(
            &mut self.execution.agent_defaults.max_rounds,
            1,
            100,
            "agent_defaults.max_rounds",
        );
        clamp_val(
            &mut self.execution.agent_defaults.max_tools_per_round,
            1,
            50,
            "agent_defaults.max_tools_per_round",
        );
        clamp_val(
            &mut self.execution.agent_defaults.max_tool_runtime_secs,
            1,
            600,
            "agent_defaults.max_tool_runtime_secs",
        );
        clamp_val(
            &mut self.execution.agent_defaults.max_cost,
            0.0,
            1000.0,
            "agent_defaults.max_cost",
        );
        // ── Execution > Lead Agent Defaults ──
        clamp_val(
            &mut self.execution.lead_agent_defaults.max_rounds,
            1,
            200,
            "lead_agent_defaults.max_rounds",
        );
        clamp_val(
            &mut self.execution.lead_agent_defaults.max_tools_per_round,
            1,
            50,
            "lead_agent_defaults.max_tools_per_round",
        );
        clamp_val(
            &mut self.execution.lead_agent_defaults.max_tool_runtime_secs,
            1,
            3600,
            "lead_agent_defaults.max_tool_runtime_secs",
        );
        clamp_val(
            &mut self.execution.lead_agent_defaults.max_cost,
            0.0,
            1000.0,
            "lead_agent_defaults.max_cost",
        );
        clamp_val(
            &mut self.execution.lead_agent_defaults.max_concurrent_subagents,
            1,
            32,
            "lead_agent_defaults.max_concurrent_subagents",
        );
        // ── Execution > Skill Defaults ──
        clamp_val(
            &mut self.execution.skill_defaults.max_rounds,
            1,
            100,
            "skill_defaults.max_rounds",
        );
        clamp_val(
            &mut self.execution.skill_defaults.max_tools_per_round,
            1,
            50,
            "skill_defaults.max_tools_per_round",
        );
        clamp_val(
            &mut self.execution.skill_defaults.default_tool_rate_limit,
            1,
            1000,
            "skill_defaults.default_tool_rate_limit",
        );
        clamp_val(
            &mut self.execution.skill_defaults.router_auto_select_threshold,
            0.0,
            1.0,
            "skill_defaults.router_auto_select_threshold",
        );
        clamp_val(
            &mut self.execution.skill_defaults.router_suggest_threshold,
            0.0,
            1.0,
            "skill_defaults.router_suggest_threshold",
        );
        // ── Security ──
        clamp_val(
            &mut self.security.max_input_length,
            1024,
            1_048_576,
            "security.max_input_length",
        );
        clamp_val(
            &mut self.security.circuit_breaker.failure_threshold,
            1,
            100,
            "circuit_breaker.failure_threshold",
        );
        clamp_val(
            &mut self.security.circuit_breaker.reset_timeout_secs,
            10,
            3600,
            "circuit_breaker.reset_timeout_secs",
        );
        // ── Server ──
        clamp_val(
            &mut self.server.event_bus_capacity,
            64,
            65536,
            "server.event_bus_capacity",
        );
        clamp_val(
            &mut self.server.event_broadcaster_capacity,
            8,
            4096,
            "server.event_broadcaster_capacity",
        );
        clamp_val(
            &mut self.server.wake_channel_capacity,
            8,
            4096,
            "server.wake_channel_capacity",
        );
        clamp_val(
            &mut self.server.heartbeat_interval_secs,
            1,
            300,
            "server.heartbeat_interval_secs",
        );
        clamp_val(
            &mut self.server.sse_keep_alive_secs,
            1,
            300,
            "server.sse_keep_alive_secs",
        );
        clamp_val(
            &mut self.server.chat_streams.stream_chunk_delay_ms,
            0,
            500,
            "server.chat_streams.stream_chunk_delay_ms",
        );
        clamp_val(
            &mut self.server.chat_streams.stream_chunk_words,
            1,
            50,
            "server.chat_streams.stream_chunk_words",
        );
        // ── Upload ──
        clamp_val(
            &mut self.upload.max_file_size_bytes,
            1024,
            500 * 1024 * 1024,
            "upload.max_file_size_bytes",
        );
        clamp_val(
            &mut self.upload.max_total_storage_bytes,
            1024 * 1024,
            10 * 1024 * 1024 * 1024,
            "upload.max_total_storage_bytes",
        );
        clamp_val(
            &mut self.upload.max_files_per_message,
            1,
            50,
            "upload.max_files_per_message",
        );
        clamp_val(
            &mut self.upload.retention_days,
            1,
            365,
            "upload.retention_days",
        );
        // ── Upload > Governance ──
        clamp_val(
            &mut self.upload.governance.processing_poll_interval_secs,
            5,
            3600,
            "upload.governance.processing_poll_interval_secs",
        );
        clamp_val(
            &mut self.upload.governance.processing_batch_size,
            1,
            50,
            "upload.governance.processing_batch_size",
        );
        clamp_val(
            &mut self.upload.governance.max_extracted_text_chars,
            1000,
            500_000,
            "upload.governance.max_extracted_text_chars",
        );
        clamp_val(
            &mut self.upload.governance.cleanup_interval_hours,
            1,
            168,
            "upload.governance.cleanup_interval_hours",
        );
        clamp_val(
            &mut self.upload.governance.orphan_grace_period_hours,
            1,
            720,
            "upload.governance.orphan_grace_period_hours",
        );
        clamp_val(
            &mut self.upload.governance.max_concurrent_extractions,
            1,
            16,
            "upload.governance.max_concurrent_extractions",
        );
        clamp_val(
            &mut self.upload.governance.extraction_retry_count,
            0,
            5,
            "upload.governance.extraction_retry_count",
        );
        clamp_val(
            &mut self.upload.governance.attachment_ready_wait_ms,
            0,
            30_000,
            "upload.governance.attachment_ready_wait_ms",
        );
        clamp_val(
            &mut self.upload.governance.attachment_ready_poll_interval_ms,
            50,
            2_000,
            "upload.governance.attachment_ready_poll_interval_ms",
        );
        clamp_val(
            &mut self.upload.governance.max_image_dimension,
            256,
            65536,
            "upload.governance.max_image_dimension",
        );
    }
}
