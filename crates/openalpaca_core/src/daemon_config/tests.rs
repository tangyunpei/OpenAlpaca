use super::*;

#[test]
fn test_default_config() {
    let config = DaemonConfig::default();
    assert_eq!(config.orchestrator.memory.prompt_recent_messages, 25);
    assert_eq!(config.orchestrator.memory.summary_max_chars, 4000);
    assert_eq!(config.orchestrator.costs.summary_max_daily_cost_usd, 0.50);
    assert_eq!(config.execution.agent_defaults.max_rounds, 15);
    assert_eq!(config.execution.lead_agent_defaults.max_rounds, 18);
    assert_eq!(config.security.max_input_length, 32768);
    assert_eq!(config.server.heartbeat_interval_secs, 5);
}

#[test]
fn test_empty_toml_gives_defaults() {
    let config: DaemonConfig = toml::from_str("").unwrap();
    assert_eq!(config.orchestrator.memory.prompt_recent_messages, 25);
    assert_eq!(config.execution.lead_agent_defaults.max_cost, 5.0);
}

#[test]
fn test_partial_override() {
    let toml_str = r#"
[orchestrator.memory]
prompt_recent_messages = 60

[execution.agent_defaults]
max_rounds = 20
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.orchestrator.memory.prompt_recent_messages, 60);
    assert_eq!(config.orchestrator.memory.summary_max_chars, 4000); // still default
    assert_eq!(config.execution.agent_defaults.max_rounds, 20);
    assert_eq!(config.execution.agent_defaults.max_tools_per_round, 5); // still default
}

#[test]
fn test_validate_clamps_out_of_range_values() {
    let mut config = DaemonConfig::default();
    // Set some values out of range
    config.orchestrator.memory.prompt_recent_messages = 0; // min is 1
    config.orchestrator.memory.summary_max_chars = 999_999; // max is 32000
    config.orchestrator.memory.memory_confidence_threshold = 2.5; // max is 1.0
    config.orchestrator.learning.turns_per_flush = 0; // min is 1
    config.security.max_input_length = 0; // min is 1024
    config.server.event_bus_capacity = 1; // min is 64

    config.validate();

    assert_eq!(config.orchestrator.memory.prompt_recent_messages, 1);
    assert_eq!(config.orchestrator.memory.summary_max_chars, 32000);
    assert_eq!(config.orchestrator.memory.memory_confidence_threshold, 1.0);
    assert_eq!(config.orchestrator.learning.turns_per_flush, 1);
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
        config.security.max_input_length,
        original.security.max_input_length
    );
    assert_eq!(
        config.server.event_bus_capacity,
        original.server.event_bus_capacity
    );
}

#[test]
fn test_batch_spawn_defaults_true_when_field_omitted_in_present_section() {
    // Routing V2: batch_spawn_enabled uses a serde default fn returning true,
    // so a present [execution.lead_agent_defaults] section that omits the
    // field still gets true (previously field-level #[serde(default)] → false).
    let toml_str = r#"
[execution.lead_agent_defaults]
max_rounds = 20
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.execution.lead_agent_defaults.max_rounds, 20);
    assert!(config.execution.lead_agent_defaults.batch_spawn_enabled);

    // An explicit false is still honored.
    let toml_str = r#"
[execution.lead_agent_defaults]
batch_spawn_enabled = false
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert!(!config.execution.lead_agent_defaults.batch_spawn_enabled);
}

#[test]
fn test_skill_defaults_new_fields_default() {
    let config = DaemonConfig::default();
    let sd = &config.execution.skill_defaults;
    assert_eq!(sd.max_rounds, 6);
    assert_eq!(sd.max_tools_per_round, 3);
    assert!((sd.router_auto_select_threshold - 0.65).abs() < f64::EPSILON);
    assert!((sd.router_suggest_threshold - 0.45).abs() < f64::EPSILON);
}

#[test]
fn test_skill_defaults_from_toml() {
    let toml_str = r#"
[execution.skill_defaults]
max_rounds = 10
router_auto_select_threshold = 0.8
router_suggest_threshold = 0.5
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    let sd = &config.execution.skill_defaults;
    assert_eq!(sd.max_rounds, 10);
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
}

#[test]
fn test_skill_defaults_validate_clamps() {
    let mut config = DaemonConfig::default();
    config.execution.skill_defaults.router_auto_select_threshold = 1.5; // max is 1.0
    config.execution.skill_defaults.router_suggest_threshold = -0.1; // min is 0.0

    config.validate();

    assert!(
        (config.execution.skill_defaults.router_auto_select_threshold - 1.0).abs() < f64::EPSILON
    );
    assert!((config.execution.skill_defaults.router_suggest_threshold - 0.0).abs() < f64::EPSILON);
}

#[test]
fn test_upload_defaults_include_office_mimes() {
    let config = DaemonConfig::default();
    let prefixes = &config.upload.allowed_mime_prefixes;

    // Core MIME prefixes
    assert!(
        prefixes.iter().any(|p| p == "image/"),
        "missing image/ prefix"
    );
    assert!(
        prefixes.iter().any(|p| p == "application/pdf"),
        "missing application/pdf"
    );
    assert!(
        prefixes.iter().any(|p| p == "text/"),
        "missing text/ prefix"
    );
    assert!(
        prefixes.iter().any(|p| p == "audio/"),
        "missing audio/ prefix"
    );

    // Office MIME prefixes
    assert!(
        prefixes.iter().any(|p| p == "application/msword"),
        "missing application/msword"
    );
    assert!(
        prefixes
            .iter()
            .any(|p| p == "application/vnd.openxmlformats-officedocument."),
        "missing OOXML prefix"
    );
    assert!(
        prefixes.iter().any(|p| p == "application/vnd.ms-excel"),
        "missing application/vnd.ms-excel"
    );
    assert!(
        prefixes
            .iter()
            .any(|p| p == "application/vnd.ms-powerpoint"),
        "missing application/vnd.ms-powerpoint"
    );
    assert!(
        prefixes.iter().any(|p| p == "application/vnd.apple."),
        "missing iWork prefix"
    );
    assert_eq!(config.upload.governance.attachment_ready_wait_ms, 8_000);
    assert_eq!(
        config.upload.governance.attachment_ready_poll_interval_ms,
        200
    );
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
    assert_eq!(
        config.upload.governance.attachment_ready_poll_interval_ms,
        500
    );
}

#[test]
fn test_upload_governance_wait_settings_validate_clamps() {
    let mut config = DaemonConfig::default();
    config.upload.governance.attachment_ready_wait_ms = 99_999;
    config.upload.governance.attachment_ready_poll_interval_ms = 1;

    config.validate();

    assert_eq!(config.upload.governance.attachment_ready_wait_ms, 30_000);
    assert_eq!(
        config.upload.governance.attachment_ready_poll_interval_ms,
        50
    );
}

// ── Routing V2: [orchestrator.routing] ──

fn assert_routing_is_default(routing: &crate::daemon_config::RoutingConfig) {
    assert!(routing.steering_enabled);
    assert_eq!(routing.steering_inbox_cap, 16);
    assert_eq!(routing.max_workflows_per_lane, 3);
    assert!(routing.followup_autostart);
    assert_eq!(routing.main_loop_max_rounds, 8);
    assert_eq!(routing.main_loop_max_tools_per_round, 4);
    assert_eq!(routing.tool_selection, "core_union");
    assert!(routing.scheduled_skills_enabled);
    // §5.6c's S2 replay resume is the plan's one speculative piece and ships
    // off. A default that flipped this on would adopt a pending decision.
    assert!(!routing.resume_enabled);
}

#[test]
fn test_routing_defaults() {
    let config = DaemonConfig::default();
    assert_routing_is_default(&config.orchestrator.routing);
}

#[test]
fn test_routing_absent_table_parses_to_defaults() {
    // No [orchestrator] at all.
    let config: DaemonConfig = toml::from_str("").unwrap();
    assert_routing_is_default(&config.orchestrator.routing);

    // [orchestrator] present but [orchestrator.routing] absent.
    let toml_str = r#"
[orchestrator.memory]
prompt_recent_messages = 60
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert_routing_is_default(&config.orchestrator.routing);
}

#[test]
fn test_routing_partial_table_keeps_field_defaults() {
    // The partial-table footgun: a present-but-partial table must not
    // flip default-true fields to false via bool::default(). Every field
    // has a named serde default matching the Default impl.
    let toml_str = r#"
[orchestrator.routing]
steering_inbox_cap = 8
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    let routing = &config.orchestrator.routing;
    assert_eq!(routing.steering_inbox_cap, 8);
    assert_eq!(routing.max_workflows_per_lane, 3);
    // These would be false under bare #[serde(default)]:
    assert!(routing.steering_enabled);
    assert!(routing.followup_autostart);
    assert!(routing.scheduled_skills_enabled);
    assert!(!routing.resume_enabled);
    assert_eq!(routing.main_loop_max_rounds, 8);
    assert_eq!(routing.main_loop_max_tools_per_round, 4);
    assert_eq!(routing.tool_selection, "core_union");
}

#[test]
fn test_routing_serde_round_trip() {
    let serialized = toml::to_string(&DaemonConfig::default()).unwrap();
    let config: DaemonConfig = toml::from_str(&serialized).unwrap();
    assert_routing_is_default(&config.orchestrator.routing);
}

#[test]
fn test_routing_from_toml() {
    let toml_str = r#"
[orchestrator.routing]
steering_enabled = true
steering_inbox_cap = 8
max_workflows_per_lane = 2
followup_autostart = false
main_loop_max_rounds = 12
main_loop_max_tools_per_round = 6
tool_selection = "full"
resume_enabled = true
"#;
    let config: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert!(config.orchestrator.routing.steering_enabled);
    assert!(config.orchestrator.routing.resume_enabled, "and it can be turned on");
    assert_eq!(config.orchestrator.routing.steering_inbox_cap, 8);
    assert_eq!(config.orchestrator.routing.max_workflows_per_lane, 2);
    assert!(!config.orchestrator.routing.followup_autostart);
    assert_eq!(config.orchestrator.routing.main_loop_max_rounds, 12);
    assert_eq!(config.orchestrator.routing.main_loop_max_tools_per_round, 6);
    assert_eq!(config.orchestrator.routing.tool_selection, "full");
}

#[test]
fn test_routing_validate_clamps() {
    let mut config = DaemonConfig::default();
    config.orchestrator.routing.steering_inbox_cap = 0;
    config.orchestrator.routing.max_workflows_per_lane = 999;
    config.orchestrator.routing.main_loop_max_rounds = 0;
    config.orchestrator.routing.main_loop_max_tools_per_round = 999;
    config.orchestrator.routing.tool_selection = "everything".to_string();
    config.validate();
    assert_eq!(config.orchestrator.routing.steering_inbox_cap, 1);
    assert_eq!(config.orchestrator.routing.max_workflows_per_lane, 16);
    assert_eq!(config.orchestrator.routing.main_loop_max_rounds, 1);
    assert_eq!(config.orchestrator.routing.main_loop_max_tools_per_round, 50);
    assert_eq!(config.orchestrator.routing.tool_selection, "core_union");
}

/// Every `[orchestrator.sessions]` and `[extensions]` knob is clamped like the
/// rest of the table — they shipped without ranges, so a hand-edited
/// `daemon.toml` could set a 0-byte log cap (the sweep evicts everything it may
/// touch on every boot), a 1-byte inline threshold (every result spills), or a
/// 0-second drain (a disable kills in-flight calls outright).
#[test]
fn test_sessions_and_extensions_validate_clamps() {
    // Below the floor.
    let mut config = DaemonConfig::default();
    config.orchestrator.sessions.log_max_session_bytes = 0;
    config.orchestrator.sessions.log_max_total_bytes = 1;
    config.orchestrator.sessions.tool_result_inline_bytes = 1;
    config.orchestrator.sessions.snapshot_max_bytes = 0;
    config.extensions.drain_timeout_secs = 0;
    config.validate();
    assert_eq!(
        config.orchestrator.sessions.log_max_session_bytes,
        1024 * 1024
    );
    assert_eq!(config.orchestrator.sessions.log_max_total_bytes, 1024 * 1024);
    assert_eq!(config.orchestrator.sessions.tool_result_inline_bytes, 4096);
    assert_eq!(config.orchestrator.sessions.snapshot_max_bytes, 1024 * 1024);
    assert_eq!(config.extensions.drain_timeout_secs, 1);
    // `0` is the documented "off" for the age sweep, and must survive.
    assert_eq!(config.orchestrator.sessions.log_retention_days, 0);

    // Above the ceiling.
    let mut config = DaemonConfig::default();
    config.orchestrator.sessions.log_max_session_bytes = u64::MAX;
    config.orchestrator.sessions.log_max_total_bytes = u64::MAX;
    config.orchestrator.sessions.log_retention_days = u32::MAX;
    config.orchestrator.sessions.tool_result_inline_bytes = 8 * 1024 * 1024;
    config.orchestrator.sessions.snapshot_max_bytes = u64::MAX;
    config.extensions.drain_timeout_secs = 86_400;
    config.validate();
    assert_eq!(
        config.orchestrator.sessions.log_max_session_bytes,
        64 * 1024 * 1024 * 1024
    );
    assert_eq!(
        config.orchestrator.sessions.log_max_total_bytes,
        64 * 1024 * 1024 * 1024
    );
    assert_eq!(config.orchestrator.sessions.log_retention_days, 3650);
    assert_eq!(
        config.orchestrator.sessions.tool_result_inline_bytes,
        crate::session_log::ENVELOPE_DATA_CAP_BYTES,
        "the inline threshold may not exceed the record envelope it must fit"
    );
    assert_eq!(
        config.orchestrator.sessions.snapshot_max_bytes,
        1024 * 1024 * 1024
    );
    assert_eq!(config.extensions.drain_timeout_secs, 120);

    // A global cap below one session's own is raised to match, not left to
    // fight the writer.
    let mut config = DaemonConfig::default();
    config.orchestrator.sessions.log_max_session_bytes = 512 * 1024 * 1024;
    config.orchestrator.sessions.log_max_total_bytes = 2 * 1024 * 1024;
    config.validate();
    assert_eq!(
        config.orchestrator.sessions.log_max_total_bytes,
        512 * 1024 * 1024
    );
}

/// A hand-edited `daemon.toml` still carrying the purged
/// `execution.skill_defaults.global_tool_deny` key loads clean — the key is
/// ignored (and the loader warns once), and every other key in the file is
/// honoured (extension design §11.1).
#[test]
fn test_removed_deny_key_is_ignored_and_file_still_loads() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("daemon.toml");
    std::fs::write(
        &path,
        r#"
[execution.skill_defaults]
max_rounds = 9
global_tool_deny = ["srv__blocked"]

[extensions]
drain_timeout_secs = 7
"#,
    )
    .unwrap();

    let config = load_daemon_config(&path);
    assert_eq!(config.execution.skill_defaults.max_rounds, 9);
    assert_eq!(config.extensions.drain_timeout_secs, 7);
}

/// A `log_retention_days` the owner set to something other than its inert
/// default warns at boot (R85): the key is parsed, clamped and served by
/// `GET /v1/status`, and no age sweep reads it — so a silent load would let the
/// number pass for a bound.
#[tracing_test::traced_test]
#[test]
fn a_configured_log_retention_warns_that_no_age_sweep_exists() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("daemon.toml");
    std::fs::write(
        &path,
        r#"
[orchestrator.sessions]
log_retention_days = 90
"#,
    )
    .unwrap();

    let config = load_daemon_config(&path);
    // Served as set — the value is honest about itself, not ignored.
    assert_eq!(config.orchestrator.sessions.log_retention_days, 90);
    assert!(
        logs_contain("no age sweep exists yet"),
        "a configured retention loaded silently"
    );
    assert!(logs_contain("owner decision T12"));
}

/// The inert default does not warn, and neither does a file that never mentions
/// the key — a warn on every boot would be noise, not information.
#[tracing_test::traced_test]
#[test]
fn the_default_log_retention_is_silent() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("daemon.toml");
    std::fs::write(
        &path,
        r#"
[orchestrator.sessions]
log_retention_days = 0
log_max_total_bytes = 1073741824
"#,
    )
    .unwrap();

    let config = load_daemon_config(&path);
    assert_eq!(config.orchestrator.sessions.log_retention_days, 0);
    assert_eq!(
        config.orchestrator.sessions.log_max_total_bytes,
        1024 * 1024 * 1024
    );
    assert!(!logs_contain("no age sweep exists yet"));
}

// ── Artifact store: [execution.artifacts] (plan §4.6) ──

fn assert_artifacts_is_default(artifacts: &crate::daemon_config::ArtifactsConfig) {
    // 10 MB — the `file_write` cap, not the 50 MB upload cap: this content
    // comes out of a context window.
    assert_eq!(artifacts.max_artifact_bytes, 10 * 1024 * 1024);
    assert_eq!(artifacts.max_versions_per_artifact, 20);
}

#[test]
fn test_artifacts_defaults() {
    assert_artifacts_is_default(&DaemonConfig::default().execution.artifacts);
}

#[test]
fn test_artifacts_absent_table_parses_to_defaults() {
    let config: DaemonConfig = toml::from_str("").unwrap();
    assert_artifacts_is_default(&config.execution.artifacts);

    // [execution] present but [execution.artifacts] absent.
    let config: DaemonConfig = toml::from_str(
        r#"
[execution.agent_defaults]
max_rounds = 9
"#,
    )
    .unwrap();
    assert_artifacts_is_default(&config.execution.artifacts);
}

#[test]
fn test_artifacts_partial_table_keeps_field_defaults() {
    let config: DaemonConfig = toml::from_str(
        r#"
[execution.artifacts]
max_versions_per_artifact = 5
"#,
    )
    .unwrap();
    assert_eq!(config.execution.artifacts.max_versions_per_artifact, 5);
    assert_eq!(config.execution.artifacts.max_artifact_bytes, 10 * 1024 * 1024);
}

#[test]
fn test_artifacts_from_toml() {
    let config: DaemonConfig = toml::from_str(
        r#"
[execution.artifacts]
max_artifact_bytes = 4096
max_versions_per_artifact = 3
"#,
    )
    .unwrap();
    assert_eq!(config.execution.artifacts.max_artifact_bytes, 4096);
    assert_eq!(config.execution.artifacts.max_versions_per_artifact, 3);
}

#[test]
fn test_artifacts_serde_round_trip() {
    let serialized = toml::to_string(&DaemonConfig::default()).unwrap();
    let config: DaemonConfig = toml::from_str(&serialized).unwrap();
    assert_artifacts_is_default(&config.execution.artifacts);
}

#[test]
fn test_artifacts_validate_clamps() {
    let mut config = DaemonConfig::default();
    config.execution.artifacts.max_artifact_bytes = 1;
    config.execution.artifacts.max_versions_per_artifact = 0;
    config.validate();
    assert_eq!(config.execution.artifacts.max_artifact_bytes, 1024);
    assert_eq!(config.execution.artifacts.max_versions_per_artifact, 1);

    let mut config = DaemonConfig::default();
    config.execution.artifacts.max_artifact_bytes = u64::MAX;
    config.execution.artifacts.max_versions_per_artifact = 10_000;
    config.validate();
    assert_eq!(config.execution.artifacts.max_artifact_bytes, 100 * 1024 * 1024);
    assert_eq!(config.execution.artifacts.max_versions_per_artifact, 200);
}

// ── The two extraction count keys (memory redesign, plan 3.4; Learning v2) ──

/// Design 09 §1.10: a learning call writes at most 12 notes, a task run 10,
/// under `[orchestrator.costs]`.
#[test]
fn extraction_count_keys_default_to_twelve_per_call_and_ten_per_run() {
    let config = DaemonConfig::default();
    assert_eq!(config.orchestrator.costs.extract_max_items_per_call, 12);
    assert_eq!(config.orchestrator.costs.task_extract_max_items_per_run, 10);

    let parsed: DaemonConfig = toml::from_str("").unwrap();
    assert_eq!(parsed.orchestrator.costs.extract_max_items_per_call, 12);
    assert_eq!(parsed.orchestrator.costs.task_extract_max_items_per_run, 10);
}

/// A zero would switch extraction off by the back door and an absurd count
/// would let one call flood a partition: both keys are clamped to 1..=50 and
/// 1..=100, as the registry rows say.
#[test]
fn extraction_count_keys_are_clamped() {
    let mut config: DaemonConfig = toml::from_str(
        r#"
[orchestrator.costs]
extract_max_items_per_call = 0
task_extract_max_items_per_run = 5000
"#,
    )
    .unwrap();
    config.validate();
    assert_eq!(config.orchestrator.costs.extract_max_items_per_call, 1);
    assert_eq!(
        config.orchestrator.costs.task_extract_max_items_per_run,
        100
    );
}

// ── Learning v2's keys (design 09 §1.10) ─────────────────────────────

/// MD-10's two numbers, the off switch and the daily call cap: 120 s of
/// quiet, 10 new turns, learning on, and 200 extraction calls a UTC day —
/// the same from the struct default and from an empty file.
#[test]
fn learning_keys_default_to_on_120s_10_turns_and_200_calls() {
    for config in [DaemonConfig::default(), toml::from_str("").unwrap()] {
        assert!(config.orchestrator.learning.enabled);
        assert_eq!(config.orchestrator.learning.idle_secs, 120);
        assert_eq!(config.orchestrator.learning.turns_per_flush, 10);
        assert_eq!(config.orchestrator.costs.extract_max_calls_per_day, 200);
    }
}

/// A partial `[orchestrator.learning]` table keeps the defaults of the keys
/// it does not name (named serde defaults, never a bool's `false`).
#[test]
fn a_partial_learning_table_keeps_the_other_defaults() {
    let config: DaemonConfig = toml::from_str(
        r#"
[orchestrator.learning]
idle_secs = 0
"#,
    )
    .unwrap();
    assert!(config.orchestrator.learning.enabled);
    assert_eq!(
        config.orchestrator.learning.idle_secs, 0,
        "0 switches the idle trigger off"
    );
    assert_eq!(config.orchestrator.learning.turns_per_flush, 10);
}

/// The registry's ranges: `idle_secs` 0..=3600, `turns_per_flush` 1..=100,
/// `extract_max_calls_per_day` 0..=10000 (0 = no cap).
#[test]
fn learning_keys_are_clamped() {
    let mut config: DaemonConfig = toml::from_str(
        r#"
[orchestrator.learning]
idle_secs = 99999
turns_per_flush = 0

[orchestrator.costs]
extract_max_calls_per_day = 20000
"#,
    )
    .unwrap();
    config.validate();
    assert_eq!(config.orchestrator.learning.idle_secs, 3600);
    assert_eq!(config.orchestrator.learning.turns_per_flush, 1);
    assert_eq!(config.orchestrator.costs.extract_max_calls_per_day, 10000);

    let mut high: DaemonConfig = toml::from_str(
        r#"
[orchestrator.learning]
turns_per_flush = 1000
"#,
    )
    .unwrap();
    high.validate();
    assert_eq!(high.orchestrator.learning.turns_per_flush, 100);
}

// ── Index-first recall (design 09 §13.2, MD-15) ──────────────────────

/// `[orchestrator.memory] recall_mode` is `index` unless the file says
/// otherwise — from the struct default, an empty file and a memory table
/// that does not name it — and `excerpts` is the other value. Both survive a
/// round trip; a value the key does not have fails the file like any other
/// mistyped key.
#[test]
fn recall_mode_defaults_to_index_and_reads_excerpts() {
    for config in [
        DaemonConfig::default(),
        toml::from_str("").unwrap(),
        toml::from_str("[orchestrator.memory]\nprompt_recent_messages = 40\n").unwrap(),
    ] {
        assert_eq!(config.orchestrator.memory.recall_mode, RecallMode::Index);
    }
    let config: DaemonConfig =
        toml::from_str("[orchestrator.memory]\nrecall_mode = \"excerpts\"\n").unwrap();
    assert_eq!(config.orchestrator.memory.recall_mode, RecallMode::Excerpts);
    let round: DaemonConfig = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(round.orchestrator.memory.recall_mode, RecallMode::Excerpts);
    assert!(
        toml::to_string(&DaemonConfig::default())
            .unwrap()
            .contains("recall_mode = \"index\"")
    );
    assert!(
        toml::from_str::<DaemonConfig>("[orchestrator.memory]\nrecall_mode = \"snippets\"\n")
            .is_err()
    );
}

/// Design 11 §8 (CQ-5): `[orchestrator.memory] carry_over_turns` is 3
/// unless the file says otherwise, the shipped file names 3, `0` (off) is
/// kept, and a value past the registry's 20 is clamped.
#[test]
fn carry_over_turns_defaults_to_three_and_zero_turns_it_off() {
    for config in [
        DaemonConfig::default(),
        toml::from_str("").unwrap(),
        toml::from_str(include_str!("../../../../config/daemon.toml")).unwrap(),
    ] {
        assert_eq!(config.orchestrator.memory.carry_over_turns, 3);
    }
    let mut off: DaemonConfig =
        toml::from_str("[orchestrator.memory]\ncarry_over_turns = 0\n").unwrap();
    off.validate();
    assert_eq!(off.orchestrator.memory.carry_over_turns, 0);
    let mut high: DaemonConfig =
        toml::from_str("[orchestrator.memory]\ncarry_over_turns = 500\n").unwrap();
    high.validate();
    assert_eq!(high.orchestrator.memory.carry_over_turns, 20);
}

/// N-SKILL-1: `[orchestrator.prompt_budgets] skills_catalog_budget` is the
/// 500 characters `<available_skills>` always had, unless the file says
/// otherwise; the shipped file names 500; a value outside 100..=20000 is
/// clamped.
#[test]
fn skills_catalog_budget_defaults_to_five_hundred() {
    for config in [
        DaemonConfig::default(),
        toml::from_str("").unwrap(),
        toml::from_str("[orchestrator.prompt_budgets]\nidentity_budget = 400\n").unwrap(),
        toml::from_str(include_str!("../../../../config/daemon.toml")).unwrap(),
    ] {
        assert_eq!(
            config.orchestrator.prompt_budgets.skills_catalog_budget,
            500
        );
    }
    for (written, kept) in [(0, 100), (2_000, 2_000), (1_000_000, 20_000)] {
        let mut config: DaemonConfig = toml::from_str(&format!(
            "[orchestrator.prompt_budgets]\nskills_catalog_budget = {written}\n"
        ))
        .unwrap();
        config.validate();
        assert_eq!(
            config.orchestrator.prompt_budgets.skills_catalog_budget,
            kept
        );
    }
}

/// The shipped `config/daemon.toml` names the key with its default, and the
/// whole file still parses.
#[test]
fn the_shipped_daemon_toml_names_recall_mode_index() {
    let shipped = include_str!("../../../../config/daemon.toml");
    assert!(shipped.contains("\nrecall_mode = \"index\"\n"), "{shipped}");
    let config: DaemonConfig = toml::from_str(shipped).unwrap();
    assert_eq!(config.orchestrator.memory.recall_mode, RecallMode::Index);
}

// ── Conversation summaries (design 09 §13.3, MD-16) ─────────────────────

/// `[orchestrator.learning] episode_max_per_day` is gone (design 11 §8,
/// owner decision CQ-2; kept gone by CQ-11): a summary's own call counts
/// under the summariser's daily ceiling, and no daily count bounds it. A hand-edited file that still names
/// it is read as every stale key is — ignored — and the shipped
/// `config/daemon.toml` no longer names it.
#[test]
fn episode_max_per_day_is_gone_and_a_stale_key_is_ignored() {
    let stale: DaemonConfig =
        toml::from_str("[orchestrator.learning]\nidle_secs = 60\nepisode_max_per_day = 0\n")
            .unwrap();
    assert_eq!(stale.orchestrator.learning.idle_secs, 60);
    let round = toml::to_string(&stale).unwrap();
    assert!(!round.contains("episode_max_per_day"), "{round}");

    let shipped = include_str!("../../../../config/daemon.toml");
    assert!(!shipped.contains("episode_max_per_day"), "{shipped}");
}
