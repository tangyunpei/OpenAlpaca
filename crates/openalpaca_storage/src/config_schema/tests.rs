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
    assert!(validate("daemon.orchestrator.routing.tool_selection", "core_union").is_ok());
    assert!(validate("daemon.orchestrator.routing.tool_selection", "CORE_UNION").is_ok());
    assert!(validate("daemon.orchestrator.routing.tool_selection", "potato").is_err());
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
    assert_eq!(
        normalize("daemon.orchestrator.routing.tool_selection", "FULL"),
        "full"
    );
}

#[test]
fn test_categories() {
    let cats = categories();
    assert!(cats.contains(&"Connectors"));
    // The System category was retired (its keys were parsed-but-dead no-ops).
    assert!(!cats.contains(&"System"));
    assert!(cats.contains(&"API-Keys"));
    assert!(cats.contains(&"Agents"));
    assert!(cats.contains(&"Daemon"));
    assert!(cats.contains(&"AI"));
}

#[test]
fn test_keys_in_category() {
    // The retired System category has no keys any more.
    assert!(keys_in_category("System").is_empty());
    let keys = keys_in_category("Connectors");
    assert!(keys.iter().any(|d| d.key == "telegram.token"));
}

#[test]
fn test_daemon_keys_in_category() {
    let keys = keys_in_category("Daemon");
    // 41 keys plus the 8 [orchestrator.routing] keys (2026-08-30 wiring audit)
    // plus the 2 [execution.artifacts] keys (plan §4.6), less the 2
    // simulated-chunking keys the real chat stream retired (S1), plus the 2
    // extraction count keys (memory redesign, plan 3.4), less the 8 keys of
    // the deleted SQL memory engine — the two supersession thresholds, the
    // four decay knobs and the two embedding-indexer knobs (plan 3.6), plus
    // Learning v2's five (design 09 §1.10: the three
    // `[orchestrator.learning]` keys, `extract_max_calls_per_day`, and the
    // `memory_confidence_threshold` row it never had), less
    // `extract_every_n_turns`, which Learning v2's cadence replaced, plus
    // index-first recall's `recall_mode` (design 09 §13.2) — the
    // conversation summaries' `learning.episode_max_per_day` (§13.3) came
    // and went: a summary is projected with no call (design 11 §8, CQ-2) —
    // plus the carry-over's `carry_over_turns` (design 11 §8, CQ-5), plus
    // the skill catalog's `skills_catalog_budget` (N-SKILL-1).
    assert_eq!(keys.len(), 50);
    for gone in [
        "daemon.orchestrator.extract_every_n_turns",
        "daemon.orchestrator.learning.episode_max_per_day",
    ] {
        assert!(!keys.iter().any(|d| d.key == gone), "{gone} has a row");
    }
    for key in [
        "daemon.orchestrator.learning.enabled",
        "daemon.orchestrator.learning.idle_secs",
        "daemon.orchestrator.learning.turns_per_flush",
        "daemon.orchestrator.extract_max_calls_per_day",
        "daemon.orchestrator.memory_confidence_threshold",
        "daemon.orchestrator.recall_mode",
        "daemon.orchestrator.carry_over_turns",
    ] {
        assert!(
            keys.iter().any(|d| d.key == key),
            "{key} has no registry row"
        );
    }
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.execution.artifacts.max_artifact_bytes")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.execution.artifacts.max_versions_per_artifact")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.routing.steering_enabled")
    );
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
    // Memory lifecycle keys
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_enabled")
    );
    // The two extraction count keys (memory redesign, plan 3.4)
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.extract_max_items_per_call")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_max_items_per_run")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_max_daily_cost_usd")
    );
    assert!(
        keys.iter()
            .any(|d| d.key == "daemon.orchestrator.task_extract_min_content_len")
    );
    // The SQL memory engine's knobs went with it (memory redesign, plan 3.6):
    // nothing supersedes by distance, decays, or indexes embeddings.
    assert!(keys.iter().all(|d| {
        !d.key.starts_with("daemon.orchestrator.decay_")
            && !d.key.starts_with("daemon.server.embedding_")
            && d.key != "daemon.orchestrator.supersession_distance_threshold"
            && d.key != "daemon.orchestrator.fts_jaccard_threshold"
    }));
    // S1: the two simulated-chunking keys are gone with the fake they
    // paced — the provider's own deltas are the chunks now.
    assert!(
        keys.iter()
            .all(|d| !d.key.starts_with("daemon.server.stream_chunk"))
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
    // The four `ai.embeddings.*` rows went with the embedder (plan 3.6).
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().any(|d| d.key == "ai.default_model"));
    assert!(keys.iter().any(|d| d.key == "ai.fallback_models"));
    assert!(keys.iter().all(|d| !d.key.starts_with("ai.embeddings.")));
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
    assert_eq!(agent_subs.len(), 1);
    assert!(agent_subs.contains(&"Orchestrator"));
    assert!(!agent_subs.contains(&"Embeddings"));

    // Connectors/System have no subcategories
    assert!(subcategories_in_category("Connectors").is_empty());
    assert!(subcategories_in_category("System").is_empty());

    // Daemon subcategories
    let daemon_subs = subcategories_in_category("Daemon");
    assert_eq!(daemon_subs.len(), 5);
    assert!(daemon_subs.contains(&"Orchestrator"));
    assert!(daemon_subs.contains(&"Execution"));
    assert!(daemon_subs.contains(&"Security"));
    assert!(daemon_subs.contains(&"Server"));
    assert!(daemon_subs.contains(&"Routing"));

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

    assert!(keys_in_subcategory("Agents", "Embeddings").is_empty());
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

    let suggestions = suggest_key("routing");
    assert!(suggestions.contains(&"daemon.orchestrator.routing.tool_selection"));
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
    // Ten, less `stream_chunk_delay_ms` and `stream_chunk_words` (S1), less
    // the two embedding-indexer knobs (plan 3.6).
    assert_eq!(server_keys.len(), 6);
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
            .all(|d| d.backend == ConfigBackend::DaemonToml)
    );
}

#[test]
fn test_validate_new_daemon_keys() {
    // Server keys
    assert!(validate("daemon.server.heartbeat_interval_secs", "5").is_ok());
    assert!(validate("daemon.server.heartbeat_interval_secs", "0").is_err());
    assert!(validate("daemon.server.heartbeat_interval_secs", "301").is_err());
    assert!(validate("daemon.server.stale_timeout_secs", "30").is_ok());
    assert!(validate("daemon.server.stale_timeout_secs", "0").is_err());

    // Orchestrator keys
    assert!(validate("daemon.orchestrator.summary_min_new_older_messages", "12").is_ok());
    // Learning v2 (design 09 §1.10)
    assert!(validate("daemon.orchestrator.learning.enabled", "false").is_ok());
    assert!(validate("daemon.orchestrator.learning.idle_secs", "0").is_ok());
    assert!(validate("daemon.orchestrator.learning.idle_secs", "3601").is_err());
    assert!(validate("daemon.orchestrator.learning.turns_per_flush", "0").is_err());
    assert!(validate("daemon.orchestrator.learning.turns_per_flush", "100").is_ok());
    assert!(validate("daemon.orchestrator.extract_max_calls_per_day", "0").is_ok());
    assert!(validate("daemon.orchestrator.extract_max_calls_per_day", "10001").is_err());
    assert!(validate("daemon.orchestrator.memory_confidence_threshold", "0.65").is_ok());
    assert!(validate("daemon.orchestrator.memory_confidence_threshold", "1.5").is_err());
    assert!(validate("daemon.orchestrator.extract_max_items_per_call", "12").is_ok());
    assert!(validate("daemon.orchestrator.extract_max_items_per_call", "51").is_err());
    // Index-first recall (design 09 §13.2): two values, `index` the default.
    assert!(validate("daemon.orchestrator.recall_mode", "index").is_ok());
    assert!(validate("daemon.orchestrator.recall_mode", "excerpts").is_ok());
    assert!(validate("daemon.orchestrator.recall_mode", "snippets").is_err());
    assert_eq!(
        normalize("daemon.orchestrator.recall_mode", "EXCERPTS"),
        "excerpts"
    );
    assert_eq!(
        lookup("daemon.orchestrator.recall_mode").and_then(|d| d.default),
        Some("index")
    );

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
