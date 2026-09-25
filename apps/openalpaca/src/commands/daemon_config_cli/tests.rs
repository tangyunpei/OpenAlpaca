use super::*;

#[test]
fn test_find_mapping_known_key() {
    let m = find_mapping("daemon.execution.max_rounds").unwrap();
    assert_eq!(m.section, &["execution", "agent_defaults"]);
    assert_eq!(m.field, "max_rounds");
}

#[test]
fn test_find_mapping_retired_dag_keys_gone() {
    // Routing V2 Phase 5: DAG executor deleted; its config keys are retired.
    assert!(find_mapping("daemon.dag.max_concurrent_agents").is_none());
    assert!(find_mapping("system.max_agents").is_none());
}

#[test]
fn test_find_mapping_unknown() {
    assert!(find_mapping("unknown.key").is_none());
}

#[test]
fn test_string_to_toml_value_int() {
    assert_eq!(string_to_toml_value("42"), toml::Value::Integer(42));
}

#[test]
fn test_string_to_toml_value_float() {
    assert_eq!(string_to_toml_value("2.5"), toml::Value::Float(2.5));
}

#[test]
fn test_string_to_toml_value_bool() {
    assert_eq!(string_to_toml_value("true"), toml::Value::Boolean(true));
    assert_eq!(string_to_toml_value("false"), toml::Value::Boolean(false));
}

#[test]
fn test_string_to_toml_value_string() {
    assert_eq!(
        string_to_toml_value("hello"),
        toml::Value::String("hello".to_string())
    );
}

#[test]
fn test_toml_value_to_string() {
    assert_eq!(toml_value_to_string(&toml::Value::Integer(42)), "42");
    assert_eq!(toml_value_to_string(&toml::Value::Float(0.5)), "0.5");
    assert_eq!(
        toml_value_to_string(&toml::Value::String("hi".to_string())),
        "hi"
    );
}

#[test]
fn test_navigate_to_section() {
    let toml_str = r#"
[execution.agent_defaults]
max_rounds = 5
"#;
    let root: toml::Value = toml::from_str(toml_str).unwrap();
    let section = navigate_to_section(&root, &["execution", "agent_defaults"]).unwrap();
    assert_eq!(
        section.get("max_rounds").unwrap(),
        &toml::Value::Integer(5)
    );
}

#[test]
fn test_navigate_to_section_missing() {
    let root: toml::Value = toml::from_str("").unwrap();
    assert!(navigate_to_section(&root, &["execution", "agent_defaults"]).is_none());
}

/// Plan 3.4: the two extraction count keys live under `[orchestrator.costs]`
/// beside the cost caps they bound with.
#[test]
fn test_find_mapping_extraction_count_keys() {
    let per_call = find_mapping("daemon.orchestrator.extract_max_items_per_call").unwrap();
    assert_eq!(per_call.section, &["orchestrator", "costs"]);
    assert_eq!(per_call.field, "extract_max_items_per_call");
    assert!(find_mapping("daemon.orchestrator.extract_every_n_turns").is_none());
    let per_run = find_mapping("daemon.orchestrator.task_extract_max_items_per_run").unwrap();
    assert_eq!(per_run.section, &["orchestrator", "costs"]);
    assert_eq!(per_run.field, "task_extract_max_items_per_run");
}

/// Learning v2 (design 09 §1.10): the three `[orchestrator.learning]` keys,
/// the daily call cap beside the cost caps, and the confidence floor under
/// `[orchestrator.memory]`, where `DaemonConfig` reads it.
#[test]
fn test_find_mapping_learning_keys() {
    for (key, section, field) in [
        (
            "daemon.orchestrator.learning.enabled",
            &["orchestrator", "learning"],
            "enabled",
        ),
        (
            "daemon.orchestrator.learning.idle_secs",
            &["orchestrator", "learning"],
            "idle_secs",
        ),
        (
            "daemon.orchestrator.learning.turns_per_flush",
            &["orchestrator", "learning"],
            "turns_per_flush",
        ),
        (
            "daemon.orchestrator.extract_max_calls_per_day",
            &["orchestrator", "costs"],
            "extract_max_calls_per_day",
        ),
        (
            "daemon.orchestrator.memory_confidence_threshold",
            &["orchestrator", "memory"],
            "memory_confidence_threshold",
        ),
    ] {
        let mapping = find_mapping(key).unwrap_or_else(|| panic!("{key} has no mapping"));
        assert_eq!(mapping.section, section, "{key}");
        assert_eq!(mapping.field, field, "{key}");
    }
}
