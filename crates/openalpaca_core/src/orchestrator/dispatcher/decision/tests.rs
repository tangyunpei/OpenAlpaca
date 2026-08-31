use super::*;

#[test]
fn test_dispatch_mode_display() {
    assert_eq!(DispatchMode::LeadAgent.to_string(), "lead_agent");
}

#[test]
fn test_decision_reason_display() {
    assert_eq!(DecisionReason::ModelToolCall.to_string(), "model_tool_call");
}

#[test]
fn test_serde_snake_case_roundtrip() {
    let json = serde_json::to_string(&DispatchMode::LeadAgent).unwrap();
    assert_eq!(json, "\"lead_agent\"");
    let mode: DispatchMode = serde_json::from_str(&json).unwrap();
    assert_eq!(mode, DispatchMode::LeadAgent);

    let json = serde_json::to_string(&DecisionReason::ModelToolCall).unwrap();
    assert_eq!(json, "\"model_tool_call\"");
    let reason: DecisionReason = serde_json::from_str(&json).unwrap();
    assert_eq!(reason, DecisionReason::ModelToolCall);
}
