use super::*;

#[test]
fn test_agent_status_as_str() {
    assert_eq!(AgentStatus::Idle.as_str(), "idle");
    assert_eq!(
        AgentStatus::Busy {
            task_id: "t1".into()
        }
        .as_str(),
        "busy"
    );
    assert_eq!(
        AgentStatus::Waiting {
            waiting_for: "x".into()
        }
        .as_str(),
        "waiting"
    );
    assert_eq!(
        AgentStatus::Error {
            message: "fail".into()
        }
        .as_str(),
        "error"
    );
}

#[test]
fn test_agent_status_is_available() {
    assert!(AgentStatus::Idle.is_available());
    assert!(
        !AgentStatus::Busy {
            task_id: "t1".into()
        }
        .is_available()
    );
}

#[test]
fn test_agent_status_display() {
    assert_eq!(format!("{}", AgentStatus::Idle), "idle");
    assert_eq!(
        format!(
            "{}",
            AgentStatus::Busy {
                task_id: "t1".into()
            }
        ),
        "busy (task: t1)"
    );
}

#[test]
fn test_default_preset() {
    let p = AgentPreset::default();
    assert_eq!(p.temperature, None);
    assert_eq!(p.verbosity, "normal");
}

#[test]
fn test_default_constraints() {
    let c = AgentConstraints::default();
    assert!(c.max_tool_calls.is_none());
    assert!(c.require_confirmation_for.is_empty());
    assert!(c.allowed_capabilities.is_empty());
    assert!(c.denied_capabilities.is_empty());
    assert!(c.allowed_models.is_empty());
    assert!(c.denied_models.is_empty());
}

#[test]
fn test_constraints_deserialize_without_capabilities() {
    // Existing JSON without new fields should deserialize fine via #[serde(default)]
    let json = r#"{"max_tool_calls":10,"require_confirmation_for":[]}"#;
    let c: AgentConstraints = serde_json::from_str(json).unwrap();
    assert_eq!(c.max_tool_calls, Some(10));
    assert!(c.allowed_capabilities.is_empty());
    assert!(c.denied_capabilities.is_empty());
}

#[test]
fn test_constraints_with_capabilities() {
    let json = r#"{
        "max_tool_calls": 5,
        "require_confirmation_for": [],
        "allowed_capabilities": ["web_search", "summarize"],
        "denied_capabilities": ["shell_execute"]
    }"#;
    let c: AgentConstraints = serde_json::from_str(json).unwrap();
    assert_eq!(c.allowed_capabilities, vec!["web_search", "summarize"]);
    assert_eq!(c.denied_capabilities, vec!["shell_execute"]);
}

/// TEMPLATE-TEMP-OPTIONAL (item 24 of the owner's answer of 2026-11-14): a
/// preset with no temperature is stored without one (`preset_json`), and a
/// stored preset reads back whole whether it was written before (with
/// `temperature: 0.5`) or after (without the key) — a missing key is not a
/// parse failure that would replace the persona with the default one.
#[test]
fn a_preset_without_a_temperature_is_stored_and_read_without_one() {
    let stored = serde_json::to_value(AgentPreset::default()).unwrap();
    assert!(stored.get("temperature").is_none(), "{stored}");

    let after: AgentPreset = serde_json::from_str(r#"{"persona":"Count.","verbosity":"normal"}"#)
        .expect("a preset without a temperature reads back");
    assert_eq!(after.persona, "Count.");
    assert!(
        serde_json::to_value(&after)
            .unwrap()
            .get("temperature")
            .is_none()
    );

    let before: AgentPreset =
        serde_json::from_str(r#"{"persona":"Count.","temperature":0.5,"verbosity":"normal"}"#)
            .expect("a preset written before reads back");
    assert_eq!(before.persona, "Count.");
    assert_eq!(serde_json::to_value(&before).unwrap()["temperature"], 0.5);
}
