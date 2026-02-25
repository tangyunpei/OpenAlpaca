    use super::*;

    #[test]
    fn test_default_config() {
        let config = DaemonConfig::default();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 40);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 4000);
        assert_eq!(config.orchestrator.costs.summary_max_daily_cost_usd, 0.50);
        assert_eq!(config.execution.agent_defaults.max_rounds, 15);
        assert_eq!(config.execution.lead_agent_defaults.max_rounds, 18);
        assert_eq!(config.execution.planner.planning_timeout_secs, 60);
        assert_eq!(config.execution.planner.max_retries, 2);
        assert_eq!(config.execution.planner.max_tokens, 1024);
        assert_eq!(config.execution.dag.max_concurrent_agents, 4);
        assert_eq!(config.security.max_input_length, 32768);
        assert_eq!(config.server.heartbeat_interval_secs, 5);
    }

    #[test]
    fn test_empty_toml_gives_defaults() {
        let config: DaemonConfig = toml::from_str("").unwrap();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 40);
        assert_eq!(config.execution.dag.total_timeout_secs, 1800);
    }

    #[test]
    fn test_partial_override() {
        let toml_str = r#"
[orchestrator.memory]
prompt_recent_messages = 60

[execution.dag]
max_concurrent_agents = 8
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 60);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 4000); // still default
        assert_eq!(config.execution.dag.max_concurrent_agents, 8);
        assert_eq!(config.execution.dag.node_timeout_secs, 300); // still default
    }

    #[test]
    fn test_validate_clamps_out_of_range_values() {
        let mut config = DaemonConfig::default();
        // Set some values out of range
        config.orchestrator.memory.prompt_recent_messages = 0; // min is 1
        config.orchestrator.memory.summary_max_chars = 999_999; // max is 32000
        config.orchestrator.memory.fts_jaccard_threshold = 2.5; // max is 1.0
        config.orchestrator.memory.decay.half_life_days = 0.0; // min is 1.0
        config.execution.dag.max_concurrent_agents = 100; // max is 32
        config.security.max_input_length = 0; // min is 1024
        config.server.event_bus_capacity = 1; // min is 64

        config.validate();

        assert_eq!(config.orchestrator.memory.prompt_recent_messages, 1);
        assert_eq!(config.orchestrator.memory.summary_max_chars, 32000);
        assert_eq!(config.orchestrator.memory.fts_jaccard_threshold, 1.0);
        assert_eq!(config.orchestrator.memory.decay.half_life_days, 1.0);
        assert_eq!(config.execution.dag.max_concurrent_agents, 32);
        assert_eq!(config.security.max_input_length, 1024);
        assert_eq!(config.server.event_bus_capacity, 64);
    }

    #[test]
    fn test_validate_leaves_valid_values_unchanged() {
        let mut config = DaemonConfig::default();
        let original = config.clone();
        config.validate();

        // All defaults should be within valid ranges
        assert_eq!(
            config.orchestrator.memory.prompt_recent_messages,
            original.orchestrator.memory.prompt_recent_messages
        );
        assert_eq!(
            config.orchestrator.memory.summary_max_chars,
            original.orchestrator.memory.summary_max_chars
        );
        assert_eq!(
            config.execution.dag.max_concurrent_agents,
            original.execution.dag.max_concurrent_agents
        );
        assert_eq!(
            config.security.max_input_length,
            original.security.max_input_length
        );
        assert_eq!(
            config.server.event_bus_capacity,
            original.server.event_bus_capacity
        );
    }

    #[test]
    fn test_phase2_flags_default_to_false() {
        let config = DaemonConfig::default();
        assert!(!config.execution.planner.dispatch_analysis_enabled);
        assert!(!config.execution.planner.plan_protocol_v2_enabled);
        assert!(!config.execution.lead_agent_defaults.batch_spawn_enabled);
        assert!(!config.execution.dag.critical_path_scheduling_enabled);
    }

    #[test]
    fn test_phase2_flags_from_toml() {
        let toml_str = r#"
[execution.planner]
dispatch_analysis_enabled = true
plan_protocol_v2_enabled = true

[execution.lead_agent_defaults]
batch_spawn_enabled = true

[execution.dag]
critical_path_scheduling_enabled = true
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert!(config.execution.planner.dispatch_analysis_enabled);
        assert!(config.execution.planner.plan_protocol_v2_enabled);
        assert!(config.execution.lead_agent_defaults.batch_spawn_enabled);
        assert!(config.execution.dag.critical_path_scheduling_enabled);
    }

    #[test]
    fn test_dag_max_concurrent_default_4() {
        let config = DaemonConfig::default();
        assert_eq!(config.execution.dag.max_concurrent_agents, 4);
    }

    #[test]
    fn test_phase2_flags_backward_compat() {
        // Parsing a TOML without Phase 2 fields should succeed with all flags false
        let toml_str = r#"
[execution.planner]
max_tokens = 1024
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.execution.planner.dispatch_analysis_enabled);
        assert!(!config.execution.planner.plan_protocol_v2_enabled);
        assert!(!config.execution.lead_agent_defaults.batch_spawn_enabled);
        assert!(!config.execution.dag.critical_path_scheduling_enabled);
    }

    #[test]
    fn test_skill_defaults_new_fields_default() {
        let config = DaemonConfig::default();
        let sd = &config.execution.skill_defaults;
        assert_eq!(sd.max_rounds, 6);
        assert_eq!(sd.max_tools_per_round, 3);
        assert_eq!(sd.default_permission_level, "readonly");
        assert!(sd.global_tool_deny.is_empty());
        assert_eq!(sd.default_tool_rate_limit, 60);
        assert!((sd.router_auto_select_threshold - 0.65).abs() < f64::EPSILON);
        assert!((sd.router_suggest_threshold - 0.45).abs() < f64::EPSILON);
    }

    #[test]
    fn test_skill_defaults_from_toml() {
        let toml_str = r#"
[execution.skill_defaults]
max_rounds = 10
default_permission_level = "readwrite"
global_tool_deny = ["shell_command", "file_delete"]
default_tool_rate_limit = 30
router_auto_select_threshold = 0.8
router_suggest_threshold = 0.5
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        let sd = &config.execution.skill_defaults;
        assert_eq!(sd.max_rounds, 10);
        assert_eq!(sd.default_permission_level, "readwrite");
        assert_eq!(sd.global_tool_deny, vec!["shell_command", "file_delete"]);
        assert_eq!(sd.default_tool_rate_limit, 30);
        assert!((sd.router_auto_select_threshold - 0.8).abs() < f64::EPSILON);
        assert!((sd.router_suggest_threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_skill_defaults_backward_compat() {
        // Old TOML with only max_rounds/max_tools_per_round should still parse
        let toml_str = r#"
[execution.skill_defaults]
max_rounds = 8
max_tools_per_round = 5
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        let sd = &config.execution.skill_defaults;
        assert_eq!(sd.max_rounds, 8);
        assert_eq!(sd.max_tools_per_round, 5);
        // New fields have defaults
        assert_eq!(sd.default_permission_level, "readonly");
        assert!(sd.global_tool_deny.is_empty());
        assert_eq!(sd.default_tool_rate_limit, 60);
    }

    #[test]
    fn test_skill_defaults_validate_clamps() {
        let mut config = DaemonConfig::default();
        config.execution.skill_defaults.router_auto_select_threshold = 1.5; // max is 1.0
        config.execution.skill_defaults.router_suggest_threshold = -0.1; // min is 0.0
        config.execution.skill_defaults.default_tool_rate_limit = 0; // min is 1

        config.validate();

        assert!((config.execution.skill_defaults.router_auto_select_threshold - 1.0).abs() < f64::EPSILON);
        assert!((config.execution.skill_defaults.router_suggest_threshold - 0.0).abs() < f64::EPSILON);
        assert_eq!(config.execution.skill_defaults.default_tool_rate_limit, 1);
    }

    #[test]
    fn test_upload_defaults_include_office_mimes() {
        let config = DaemonConfig::default();
        let prefixes = &config.upload.allowed_mime_prefixes;

        // Core MIME prefixes
        assert!(prefixes.iter().any(|p| p == "image/"), "missing image/ prefix");
        assert!(prefixes.iter().any(|p| p == "application/pdf"), "missing application/pdf");
        assert!(prefixes.iter().any(|p| p == "text/"), "missing text/ prefix");
        assert!(prefixes.iter().any(|p| p == "audio/"), "missing audio/ prefix");

        // Office MIME prefixes
        assert!(
            prefixes.iter().any(|p| p == "application/msword"),
            "missing application/msword"
        );
        assert!(
            prefixes.iter().any(|p| p == "application/vnd.openxmlformats-officedocument."),
            "missing OOXML prefix"
        );
        assert!(
            prefixes.iter().any(|p| p == "application/vnd.ms-excel"),
            "missing application/vnd.ms-excel"
        );
        assert!(
            prefixes.iter().any(|p| p == "application/vnd.ms-powerpoint"),
            "missing application/vnd.ms-powerpoint"
        );
        assert!(
            prefixes.iter().any(|p| p == "application/vnd.apple."),
            "missing iWork prefix"
        );
        assert_eq!(config.upload.governance.attachment_ready_wait_ms, 8_000);
        assert_eq!(config.upload.governance.attachment_ready_poll_interval_ms, 200);
    }

    #[test]
    fn test_upload_governance_wait_settings_from_toml() {
        let toml_str = r#"
[upload.governance]
attachment_ready_wait_ms = 12000
attachment_ready_poll_interval_ms = 500
"#;
        let config: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.upload.governance.attachment_ready_wait_ms, 12_000);
        assert_eq!(config.upload.governance.attachment_ready_poll_interval_ms, 500);
    }

    #[test]
    fn test_upload_governance_wait_settings_validate_clamps() {
        let mut config = DaemonConfig::default();
        config.upload.governance.attachment_ready_wait_ms = 99_999;
        config.upload.governance.attachment_ready_poll_interval_ms = 1;

        config.validate();

        assert_eq!(config.upload.governance.attachment_ready_wait_ms, 30_000);
        assert_eq!(config.upload.governance.attachment_ready_poll_interval_ms, 50);
    }
