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
    assert_eq!(section.get("max_rounds").unwrap(), &toml::Value::Integer(5));
}

#[test]
fn test_navigate_to_section_missing() {
    let root: toml::Value = toml::from_str("").unwrap();
    assert!(navigate_to_section(&root, &["execution", "agent_defaults"]).is_none());
}

#[test]
fn batch_updates_multiple_sections_and_preserves_unrelated_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.toml");
    std::fs::write(&path, "[custom]\nkeep = 'owner value'\n").unwrap();
    set_daemon_values_at(
        &path,
        &[
            ("daemon.execution.max_rounds", "7"),
            ("daemon.orchestrator.prompt_recent_messages", "12"),
        ],
    )
    .unwrap();
    let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        value["execution"]["agent_defaults"]["max_rounds"].as_integer(),
        Some(7)
    );
    assert_eq!(
        value["orchestrator"]["memory"]["prompt_recent_messages"].as_integer(),
        Some(12)
    );
    assert_eq!(value["custom"]["keep"].as_str(), Some("owner value"));
}

#[test]
fn invalid_later_entry_never_partially_writes_a_batch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.toml");
    let original = "# keep these bytes\n[execution.agent_defaults]\nmax_rounds = 3\n";
    for invalid in [
        ("unknown.key", "2"),
        ("daemon.execution.max_rounds", "invalid"),
    ] {
        std::fs::write(&path, original).unwrap();
        assert!(
            set_daemon_values_at(&path, &[("daemon.execution.max_rounds", "7"), invalid]).is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}

#[test]
fn malformed_file_and_section_errors_never_partially_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.toml");
    for original in ["not valid = [", "[execution]\nagent_defaults = 42\n"] {
        std::fs::write(&path, original).unwrap();
        assert!(
            set_daemon_values_at(
                &path,
                &[
                    ("daemon.orchestrator.prompt_recent_messages", "12"),
                    ("daemon.execution.max_rounds", "7"),
                ]
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
    let missing = dir.path().join("absent").join("daemon.toml");
    set_daemon_values_at(&missing, &[]).unwrap();
    assert!(!missing.exists());
}
