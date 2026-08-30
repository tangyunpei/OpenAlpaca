use super::*;

#[test]
fn test_lookup_exact() {
    let def = lookup("telegram.token").unwrap();
    assert_eq!(def.key, "telegram.token");
    assert!(def.sensitive);
}

#[test]
fn test_lookup_pattern_token() {
    let def = lookup("wechat.token").unwrap();
    assert!(def.sensitive);
    assert!(matches!(def.kind, ConfigKind::String));
}

#[test]
fn test_lookup_pattern_enabled() {
    let def = lookup("wechat.enabled").unwrap();
    assert!(!def.sensitive);
    assert!(matches!(def.kind, ConfigKind::Bool));
}

#[test]
fn test_lookup_unknown() {
    assert!(lookup("wechat.xyz").is_none());
    assert!(lookup("unknown.key").is_none());
}

#[test]
fn test_validate_bool() {
    assert!(validate("telegram.enabled", "true").is_ok());
    assert!(validate("telegram.enabled", "TRUE").is_ok());
    assert!(validate("telegram.enabled", "yes").is_ok());
    assert!(validate("telegram.enabled", "1").is_ok());
    assert!(validate("telegram.enabled", "potato").is_err());
}

#[test]
fn test_validate_enum() {
    assert!(validate("system.debug_level", "info").is_ok());
    assert!(validate("system.debug_level", "INFO").is_ok());
    assert!(validate("system.debug_level", "potato").is_err());
}

#[test]
fn test_validate_int_range() {
    assert!(validate("daemon.execution.max_rounds", "8").is_ok());
    assert!(validate("daemon.execution.max_rounds", "1").is_ok());
    assert!(validate("daemon.execution.max_rounds", "0").is_err());
    assert!(validate("daemon.execution.max_rounds", "abc").is_err());
}

#[test]
fn test_normalize_bool() {
    assert_eq!(normalize("telegram.enabled", "TRUE"), "true");
    assert_eq!(normalize("telegram.enabled", "Yes"), "true");
    assert_eq!(normalize("telegram.enabled", "1"), "true");
    assert_eq!(normalize("telegram.enabled", "false"), "false");
    assert_eq!(normalize("telegram.enabled", "no"), "false");
    assert_eq!(normalize("telegram.enabled", "0"), "false");
}

#[test]
fn test_normalize_enum() {
    assert_eq!(normalize("system.debug_level", "INFO"), "info");
}

#[test]
fn test_categories() {
    let cats = categories();
    assert!(cats.contains(&"Connectors"));
    assert!(cats.contains(&"System"));
    assert!(cats.contains(&"API-Keys"));
    assert!(cats.contains(&"Agents"));
    assert!(cats.contains(&"Daemon"));
    assert!(cats.contains(&"AI"));
}

#[test]
fn test_keys_in_category() {
    let keys = keys_in_category("System");
    assert!(keys.iter().any(|d| d.key == "system.debug_level"));
    // The retired system.max_agents alias no longer exists anywhere.
    assert!(!keys.iter().any(|d| d.key == "system.max_agents"));
}

#[test]
fn test_daemon_keys_in_category() {
    let keys = keys_in_category("Daemon");
    // 48 keys minus the 7 retired DAG/alias keys (Routing V2 Phase 5).
    assert_eq!(keys.len(), 41);
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.prompt_recent_messages")
    );
    assert!(keys.iter().any(|d| d.key == "daemon.execution.max_rounds"));
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.security.max_input_length")
    );
    // New keys
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.summary_min_new_older_messages")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.msg_trunc_chars")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.extract_max_daily_cost_usd")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.execution.max_tools_per_round")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.execution.lead_max_tools_per_round")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.server.heartbeat_interval_secs")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.server.embedding_batch_size")
    );
    // Memory lifecycle keys
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_enabled")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_max_daily_cost_usd")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_min_content_len")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.supersession_distance_threshold")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.fts_jaccard_threshold")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.decay_poll_interval_secs")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.decay_half_life_days")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.decay_min_importance")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.decay_soft_cap")
    );
    // Streaming keys
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.server.stream_chunk_delay_ms")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.server.stream_chunk_words")
    );
    assert!(keys.iter().all(|d| d.backend == ConfigBackend::DaemonToml));
}

#[test]
fn test_api_keys_in_category() {
    let keys = keys_in_category("API-Keys");
    assert_eq!(keys.len(), 12);
    assert!(keys.iter().any(|d| d.key == "ai.anthropic.api_key"));
    assert!(keys.iter().any(|d| d.key == "ai.openai.api_key"));
    assert!(keys.iter().any(|d| d.key == "ai.ollama.base_url"));
    assert!(keys.iter().any(|d| d.key == "ai.claude_code.discovery"));
    assert!(keys.iter().any(|d| d.key == "ai.codex.cli_enabled"));
    assert!(keys.iter().all(|d| d.backend == ConfigBackend::LlmToml));
}

#[test]
fn test_agents_in_category() {
    let keys = keys_in_category("Agents");
    assert_eq!(keys.len(), 6);
    assert!(keys.iter().any(|d| d.key == "ai.default_model"));
    assert!(keys.iter().any(|d| d.key == "ai.fallback_models"));
    assert!(keys.iter().any(|d| d.key == "ai.embeddings.enabled"));
    assert!(keys.iter().any(|d| d.key == "ai.embeddings.provider"));
    assert!(keys.iter().any(|d| d.key == "ai.embeddings.model"));
    assert!(keys.iter().any(|d| d.key == "ai.embeddings.dimensions"));
    assert!(keys.iter().all(|d| d.backend == ConfigBackend::LlmToml));
}

#[test]
fn test_ai_key_lookup() {
    let def = lookup("ai.anthropic.api_key").unwrap();
    assert!(def.sensitive);
    assert_eq!(def.backend, ConfigBackend::LlmToml);
    assert_eq!(def.category, "API-Keys");

    let def = lookup("ai.anthropic.enabled").unwrap();
    assert!(!def.sensitive);
    assert!(matches!(def.kind, ConfigKind::Bool));
}

#[test]
fn test_subcategories() {
    let api_subs = subcategories_in_category("API-Keys");
    assert_eq!(api_subs.len(), 5);
    assert!(api_subs.contains(&"Anthropic"));
    assert!(api_subs.contains(&"OpenAI"));
    assert!(api_subs.contains(&"Ollama"));
    assert!(api_subs.contains(&"Claude Code"));
    assert!(api_subs.contains(&"Codex"));

    let agent_subs = subcategories_in_category("Agents");
    assert_eq!(agent_subs.len(), 2);
    assert!(agent_subs.contains(&"Orchestrator"));
    assert!(agent_subs.contains(&"Embeddings"));

    // Connectors/System have no subcategories
    assert!(subcategories_in_category("Connectors").is_empty());
    assert!(subcategories_in_category("System").is_empty());

    // Daemon subcategories
    let daemon_subs = subcategories_in_category("Daemon");
    assert_eq!(daemon_subs.len(), 4);
    assert!(daemon_subs.contains(&"Orchestrator"));
    assert!(daemon_subs.contains(&"Execution"));
    assert!(daemon_subs.contains(&"Security"));
    assert!(daemon_subs.contains(&"Server"));

    // Web Search subcategory is now under AI
    let ai_subs = subcategories_in_category("AI");
    assert!(ai_subs.contains(&"Web Search"));
}

#[test]
fn test_keys_in_subcategory() {
    let anthropic = keys_in_subcategory("API-Keys", "Anthropic");
    assert_eq!(anthropic.len(), 2);
    assert!(anthropic.iter().any(|d| d.key == "ai.anthropic.enabled"));
    assert!(anthropic.iter().any(|d| d.key == "ai.anthropic.api_key"));

    let orch = keys_in_subcategory("Agents", "Orchestrator");
    assert_eq!(orch.len(), 2);
    assert!(orch.iter().any(|d| d.key == "ai.default_model"));
    assert!(orch.iter().any(|d| d.key == "ai.fallback_models"));

    let emb = keys_in_subcategory("Agents", "Embeddings");
    assert_eq!(emb.len(), 4);
    assert!(emb.iter().any(|d| d.key == "ai.embeddings.enabled"));
    assert!(emb.iter().any(|d| d.key == "ai.embeddings.provider"));
    assert!(emb.iter().any(|d| d.key == "ai.embeddings.model"));
    assert!(emb.iter().any(|d| d.key == "ai.embeddings.dimensions"));
}

#[test]
fn test_ai_validate_bool() {
    assert!(validate("ai.anthropic.enabled", "true").is_ok());
    assert!(validate("ai.anthropic.enabled", "potato").is_err());
}

#[test]
fn test_mask_value() {
    assert_eq!(mask_value("abcdef5678"), "****5678");
    assert_eq!(mask_value("abc"), "****");
    assert_eq!(mask_value(""), "****");
}

#[test]
fn test_suggest_key() {
    let suggestions = suggest_key("telegram");
    assert!(suggestions.contains(&"telegram.token"));
    assert!(suggestions.contains(&"telegram.enabled"));

    let suggestions = suggest_key("debug");
    assert!(suggestions.contains(&"system.debug_level"));
}

#[test]
fn test_daemon_key_lookup() {
    let def = lookup("daemon.execution.max_rounds").unwrap();
    assert_eq!(def.backend, ConfigBackend::DaemonToml);
    assert_eq!(def.category, "Daemon");

    // Retired DAG keys are gone from the registry.
    assert!(lookup("daemon.dag.max_concurrent_agents").is_none());
    assert!(lookup("system.max_agents").is_none());

    let def = lookup("daemon.orchestrator.prompt_recent_messages").unwrap();
    assert_eq!(def.default, Some("40"));
}

#[test]
fn test_validate_daemon_int_range() {
    assert!(validate("daemon.execution.lead_max_concurrent_subagents", "3").is_ok());
    assert!(validate("daemon.execution.lead_max_concurrent_subagents", "0").is_err());
}

#[test]
fn test_daemon_server_keys() {
    let server_keys = keys_in_subcategory("Daemon", "Server");
    assert_eq!(server_keys.len(), 10);
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.heartbeat_interval_secs")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.sse_keep_alive_secs")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.event_broadcaster_capacity")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.wake_channel_capacity")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.cleanup_interval_secs")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.stale_timeout_secs")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.embedding_poll_interval_secs")
    );
    assert!(
        server_keys
            .iter()
            .any(|d| d.key == "daemon.server.embedding_batch_size")
    );
    assert!(
        server_keys
            .iter()
            .all(|d| d.backend == ConfigBackend::DaemonToml)
    );
}

#[test]
fn test_validate_new_daemon_keys() {
    // Server keys
    assert!(validate("daemon.server.heartbeat_interval_secs", "5").is_ok());
    assert!(validate("daemon.server.heartbeat_interval_secs", "0").is_err());
    assert!(validate("daemon.server.heartbeat_interval_secs", "301").is_err());
    assert!(validate("daemon.server.embedding_batch_size", "50").is_ok());
    assert!(validate("daemon.server.embedding_batch_size", "0").is_err());

    // Orchestrator keys
    assert!(validate("daemon.orchestrator.summary_min_new_older_messages", "12").is_ok());
    assert!(validate("daemon.orchestrator.extract_every_n_turns", "5").is_ok());

    // Execution keys
    assert!(validate("daemon.execution.max_tools_per_round", "5").is_ok());
    assert!(validate("daemon.execution.max_tools_per_round", "0").is_err());

}

#[test]
fn test_validate_dynamic_connector() {
    assert!(validate("wechat.enabled", "true").is_ok());
    assert!(validate("wechat.enabled", "potato").is_err());
    assert!(validate("wechat.token", "anything").is_ok());
    assert!(validate("wechat.xyz", "anything").is_err());
}

// -- Anthropic setup-token validation --

#[test]
fn test_validate_anthropic_setup_token_valid() {
    // 80+ chars with correct prefix
    let token = format!("sk-ant-oat01-{}", "a".repeat(67));
    assert!(validate_anthropic_setup_token(&token).is_ok());
}

#[test]
fn test_validate_anthropic_setup_token_bad_prefix() {
    let token = format!("sk-openai-{}", "a".repeat(80));
    let err = validate_anthropic_setup_token(&token).unwrap_err();
    assert!(err.contains("sk-ant-oat01-"));
}

#[test]
fn test_validate_anthropic_setup_token_too_short() {
    let token = "sk-ant-oat01-short";
    let err = validate_anthropic_setup_token(token).unwrap_err();
    assert!(err.contains("too short"));
}

#[test]
fn test_validate_anthropic_setup_token_empty() {
    assert!(validate_anthropic_setup_token("").is_err());
    assert!(validate_anthropic_setup_token("   ").is_err());
}

// -- OpenAI API key validation --

#[test]
fn test_validate_openai_api_key_valid() {
    let key = format!("sk-{}", "x".repeat(40));
    assert!(validate_openai_api_key(&key).is_ok());
}

#[test]
fn test_validate_openai_api_key_bad_prefix() {
    let key = format!("pk-{}", "x".repeat(40));
    let err = validate_openai_api_key(&key).unwrap_err();
    assert!(err.contains("sk-"));
}

#[test]
fn test_validate_openai_api_key_too_short() {
    let key = "sk-short";
    let err = validate_openai_api_key(key).unwrap_err();
    assert!(err.contains("too short"));
}

#[test]
fn test_validate_openai_rejects_anthropic_key() {
    let key = format!("sk-ant-oat01-{}", "a".repeat(67));
    let err = validate_openai_api_key(&key).unwrap_err();
    assert!(err.contains("Anthropic"));
}

// -- Anthropic API key validation --

#[test]
fn test_validate_anthropic_api_key_valid() {
    let key = format!("sk-ant-api03-{}", "x".repeat(27));
    assert!(validate_anthropic_api_key(&key).is_ok());
}

#[test]
fn test_validate_anthropic_api_key_rejects_oat() {
    let key = format!("sk-ant-oat01-{}", "a".repeat(67));
    let err = validate_anthropic_api_key(&key).unwrap_err();
    assert!(err.contains("setup-token"));
}

#[test]
fn test_validate_anthropic_api_key_rejects_openai() {
    let key = format!("sk-{}", "x".repeat(40));
    let err = validate_anthropic_api_key(&key).unwrap_err();
    assert!(err.contains("sk-ant-"));
}

#[test]
fn test_validate_anthropic_api_key_too_short() {
    let key = "sk-ant-api03-abc";
    let err = validate_anthropic_api_key(key).unwrap_err();
    assert!(err.contains("too short"));
}

#[test]
fn test_validate_anthropic_api_key_empty() {
    assert!(validate_anthropic_api_key("").is_err());
    assert!(validate_anthropic_api_key("   ").is_err());
}

// -- validate_key_for_provider dispatch --

#[test]
fn test_validate_key_for_provider_dispatches() {
    // anthropic → API key validator
    let key = format!("sk-ant-api03-{}", "x".repeat(27));
    assert!(validate_key_for_provider("anthropic", &key).is_ok());

    // anthropic rejects oat
    let oat = format!("sk-ant-oat01-{}", "a".repeat(67));
    assert!(validate_key_for_provider("anthropic", &oat).is_err());

    // openai → OpenAI validator
    let oai = format!("sk-{}", "x".repeat(40));
    assert!(validate_key_for_provider("openai", &oai).is_ok());

    // ollama → always Ok
    assert!(validate_key_for_provider("ollama", "anything").is_ok());

    // unknown → always Ok
    assert!(validate_key_for_provider("unknown_provider", "anything").is_ok());
}
