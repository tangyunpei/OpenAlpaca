use super::*;

fn default_constraints() -> AgentConstraints {
    AgentConstraints::default()
}

#[test]
fn test_system_principal_bypasses() {
    let cap = Capability {
        name: "system.shutdown".to_string(),
    };
    assert!(CapabilityManager::check_principal(&Principal::System, &cap, &Scope::Global).is_ok());
}

#[test]
fn test_external_user_blocked_high_risk() {
    let cap = Capability {
        name: "system.shutdown".to_string(),
    };
    let principal = Principal::External {
        provider: "telegram".to_string(),
        id: "user123".to_string(),
    };
    let result = CapabilityManager::check_principal(&principal, &cap, &Scope::Global);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::AccessDenied { reason } => {
            assert!(reason.contains("Permission Denied"));
        }
        other => panic!("Expected AccessDenied, got: {:?}", other),
    }
}

#[test]
fn test_user_principal_allowed() {
    let cap = Capability {
        name: "chat.respond".to_string(),
    };
    let principal = Principal::User {
        global_id: "user1".to_string(),
    };
    assert!(CapabilityManager::check_principal(&principal, &cap, &Scope::Global).is_ok());
}

#[test]
fn test_denied_capability_blocked() {
    let constraints = AgentConstraints {
        denied_capabilities: vec!["shell_execute".to_string()],
        ..default_constraints()
    };
    let result = CapabilityManager::check_agent_capability("agent1", "shell_execute", &constraints);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::CapabilityDenied {
            agent_id,
            capability,
        } => {
            assert_eq!(agent_id, "agent1");
            assert_eq!(capability, "shell_execute");
        }
        other => panic!("Expected CapabilityDenied, got: {:?}", other),
    }
}

#[test]
fn test_allowed_capability_enforced() {
    let constraints = AgentConstraints {
        allowed_capabilities: vec!["web_search".to_string(), "summarize".to_string()],
        ..default_constraints()
    };
    // Allowed tool passes
    assert!(
        CapabilityManager::check_agent_capability("agent1", "web_search", &constraints).is_ok()
    );
    // Unlisted tool is blocked
    let result = CapabilityManager::check_agent_capability("agent1", "shell_execute", &constraints);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::CapabilityNotAllowed { .. } => {}
        other => panic!("Expected CapabilityNotAllowed, got: {:?}", other),
    }
}

#[test]
fn test_empty_allow_list_allows_all() {
    let constraints = default_constraints();
    assert!(CapabilityManager::check_agent_capability("agent1", "anything", &constraints).is_ok());
}

#[test]
fn test_capability_check_case_insensitive() {
    // Deny list with mixed case should match lowercase tool name (after normalize)
    let mut constraints = AgentConstraints {
        denied_capabilities: vec!["Shell_Execute".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    assert!(
        CapabilityManager::check_agent_capability("agent1", "shell_execute", &constraints).is_err()
    );

    // Allow list with mixed case should match lowercase tool name (after normalize)
    let mut constraints = AgentConstraints {
        allowed_capabilities: vec!["Web_Search".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    assert!(
        CapabilityManager::check_agent_capability("agent1", "web_search", &constraints).is_ok()
    );
}

// ── Model access tests ──────────────────────────────────────

#[test]
fn test_model_access_no_constraints() {
    let constraints = default_constraints();
    assert!(
        CapabilityManager::check_model_access("agent1", "claude-sonnet-4-5-20250929", &constraints)
            .is_ok()
    );
}

#[test]
fn test_model_access_denied_model() {
    let constraints = AgentConstraints {
        denied_models: vec!["gpt-4o".to_string()],
        ..default_constraints()
    };
    let result = CapabilityManager::check_model_access("agent1", "gpt-4o", &constraints);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::UnauthorizedModelAccess {
            agent_id, model_id, ..
        } => {
            assert_eq!(agent_id, "agent1");
            assert_eq!(model_id, "gpt-4o");
        }
        other => panic!("Expected UnauthorizedModelAccess, got: {:?}", other),
    }
}

#[test]
fn test_model_access_not_in_allow_list() {
    let constraints = AgentConstraints {
        allowed_models: vec!["claude-sonnet-4-5-20250929".to_string()],
        ..default_constraints()
    };
    let result = CapabilityManager::check_model_access("agent1", "gpt-4o", &constraints);
    assert!(result.is_err());
}

#[test]
fn test_model_access_in_allow_list() {
    let constraints = AgentConstraints {
        allowed_models: vec!["claude-sonnet-4-5-20250929".to_string()],
        ..default_constraints()
    };
    assert!(
        CapabilityManager::check_model_access("agent1", "claude-sonnet-4-5-20250929", &constraints)
            .is_ok()
    );
}

#[test]
fn test_unauthorized_model_access_display() {
    let v = SecurityViolation::UnauthorizedModelAccess {
        agent_id: "a1".to_string(),
        model_id: "gpt-4o".to_string(),
        reason: "denied".to_string(),
    };
    let s = format!("{}", v);
    assert!(s.contains("a1"));
    assert!(s.contains("gpt-4o"));
}

#[test]
fn test_model_access_case_insensitive() {
    // Deny list with different case should still match (after normalize)
    let mut constraints = AgentConstraints {
        denied_models: vec!["GPT-4o".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    assert!(CapabilityManager::check_model_access("agent1", "gpt-4o", &constraints).is_err());
    assert!(CapabilityManager::check_model_access("agent1", "GPT-4O", &constraints).is_err());

    // Allow list with different case should still match (after normalize)
    let mut constraints = AgentConstraints {
        allowed_models: vec!["Claude-Sonnet-4-5-20250929".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    assert!(
        CapabilityManager::check_model_access("agent1", "claude-sonnet-4-5-20250929", &constraints)
            .is_ok()
    );
    assert!(
        CapabilityManager::check_model_access("agent1", "CLAUDE-SONNET-4-5-20250929", &constraints)
            .is_ok()
    );
}
