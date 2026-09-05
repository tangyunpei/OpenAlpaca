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

/// Deny-first regression: the deny list is consulted before the allow list,
/// so an unrestricted allow list cannot resurrect a denied capability.
#[test]
fn test_denied_capability_blocked() {
    let denied = vec!["shell_execute".to_string()];
    let result = CapabilityManager::check_agent_capability(
        "agent1",
        "shell_execute",
        &Allowlist::Unrestricted,
        &denied,
    );
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
    let allowed = Allowlist::Only(vec!["web_search".to_string(), "summarize".to_string()]);
    // Allowed tool passes
    assert!(
        CapabilityManager::check_agent_capability("agent1", "web_search", &allowed, &[]).is_ok()
    );
    // Unlisted tool is blocked
    let result =
        CapabilityManager::check_agent_capability("agent1", "shell_execute", &allowed, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::CapabilityNotAllowed { .. } => {}
        other => panic!("Expected CapabilityNotAllowed, got: {:?}", other),
    }
}

#[test]
fn test_unrestricted_allowlist_admits_anything() {
    // The only form that means "no allow-list restriction" — it has to be
    // spelled, never inferred from an empty list.
    assert!(
        CapabilityManager::check_agent_capability(
            "agent1",
            "anything",
            &Allowlist::Unrestricted,
            &[]
        )
        .is_ok()
    );
}

#[test]
fn empty_allowlist_denies_every_non_ambient_capability() {
    // An empty allow list is a total restriction, not an absent one.
    let nothing = Allowlist::Only(vec![]);
    for tool in [
        "shell_execute",
        "web_search",
        "workspace_read",
        "workspace_write",
    ] {
        assert!(
            CapabilityManager::check_agent_capability("agent1", tool, &nothing, &[]).is_err(),
            "empty allow list must deny '{tool}'"
        );
    }

    // The ambient capabilities are granted constructor-side — an agent gets
    // them because `AgentTemplate::to_subagent` appended them to its list, and
    // everything outside that list is still denied.
    let ambient = Allowlist::Only(vec![
        "workspace_read".to_string(),
        "workspace_write".to_string(),
    ]);
    for tool in ["workspace_read", "workspace_write"] {
        assert!(CapabilityManager::check_agent_capability("agent1", tool, &ambient, &[]).is_ok());
    }
    for tool in ["shell_execute", "web_search"] {
        assert!(
            CapabilityManager::check_agent_capability("agent1", tool, &ambient, &[]).is_err(),
            "ambient-only allow list must deny '{tool}'"
        );
    }
}

#[test]
fn deny_beats_allow() {
    // A name on both lists is denied — the deny list is checked first.
    let mut constraints = AgentConstraints {
        allowed_capabilities: vec!["shell_execute".to_string(), "web_search".to_string()],
        denied_capabilities: vec!["shell_execute".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    let allowed = Allowlist::from_agent_constraints(&constraints);

    match CapabilityManager::check_agent_capability(
        "agent1",
        "shell_execute",
        &allowed,
        &constraints.denied_capabilities,
    ) {
        Err(SecurityViolation::CapabilityDenied { capability, .. }) => {
            assert_eq!(capability, "shell_execute");
        }
        other => panic!("Expected CapabilityDenied, got: {:?}", other),
    }
    // The rest of the allow list is unaffected.
    assert!(
        CapabilityManager::check_agent_capability(
            "agent1",
            "web_search",
            &allowed,
            &constraints.denied_capabilities
        )
        .is_ok()
    );
}

/// C5 / X-23: `Allowlist::only` enforces the variant's pre-lowercased
/// contract, so a mixed-case **MCP or plugin** tool name is admitted by the
/// very list that declared it. `check_agent_capability` lowercases the tool
/// name and then compares verbatim, so a verbatim mixed-case entry denied it —
/// deny-side safe, and wrong: the model got a generic capability refusal where
/// the design promises the attributed one.
#[test]
fn a_mixed_case_extension_tool_name_is_admitted_by_its_own_allow_list() {
    let allowed = Allowlist::only(["Acme__Search", "Notion::Query"]);
    assert_eq!(
        allowed,
        Allowlist::Only(vec!["acme__search".to_string(), "notion::query".to_string()])
    );
    for tool in ["Acme__Search", "acme__search", "NOTION::Query"] {
        assert!(
            CapabilityManager::check_agent_capability("plugin:acme", tool, &allowed, &[]).is_ok(),
            "'{tool}' must be admitted by the list that declared it"
        );
    }
    assert!(
        CapabilityManager::check_agent_capability("plugin:acme", "shell_execute", &allowed, &[])
            .is_err(),
        "normalization widens nothing"
    );

    // The contract the constructor exists to enforce: a verbatim mixed-case
    // entry is exactly the bug.
    let verbatim = Allowlist::Only(vec!["Acme__Search".to_string()]);
    assert!(
        CapabilityManager::check_agent_capability("plugin:acme", "Acme__Search", &verbatim, &[])
            .is_err()
    );
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
        CapabilityManager::check_agent_capability(
            "agent1",
            "shell_execute",
            &Allowlist::Unrestricted,
            &constraints.denied_capabilities
        )
        .is_err()
    );

    // Allow list with mixed case should match lowercase tool name (after normalize)
    let mut constraints = AgentConstraints {
        allowed_capabilities: vec!["Web_Search".to_string()],
        ..default_constraints()
    };
    constraints.normalize();
    assert!(
        CapabilityManager::check_agent_capability(
            "agent1",
            "web_search",
            &Allowlist::from_agent_constraints(&constraints),
            &[]
        )
        .is_ok()
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
