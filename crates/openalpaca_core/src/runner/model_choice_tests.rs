//! The resolver's own rules (C8): M-6f's start line, the refusal's and the
//! parameter's lists. The whole-run behaviour is `lead_agent::model_tests`.

use super::*;
use openalpaca_llm::ProviderType;

fn anthropic_only() -> LlmRouter {
    LlmRouter::single_provider(
        crate::test_util::RecordingProvider::new("hi"),
        ProviderType::Anthropic,
        "claude-haiku-5-5".to_string(),
    )
}

/// M-6f: a `[fallback_chains]` key that is one of the earlier bundle's three
/// Claude pins, not routable and named by no template, is named at start;
/// any other key, a routable one, or one a template still names, is not.
#[test]
fn a_fallback_chain_for_a_claude_id_is_named_at_start() {
    let chains = HashMap::from([
        (
            "claude-sonnet-4-6".to_string(),
            vec!["gpt-6.1-terra".to_string()],
        ),
        (
            "claude-opus-4-6".to_string(),
            vec!["gpt-6.1-sol".to_string()],
        ),
        ("my-own-model".to_string(), vec!["gpt-6-luna".to_string()]),
        (
            "claude-haiku-4-5-20251001".to_string(),
            vec!["gpt-6-luna".to_string()],
        ),
    ]);
    let router = anthropic_only();
    let templates = vec![
        Some("strong".to_string()),
        None,
        Some("claude-opus-4-6".to_string()),
    ];
    assert_eq!(
        stale_fallback_chain_lines(Some(&chains), Some(&router), &templates),
        vec![
            "[fallback_chains].\"claude-haiku-4-5-20251001\" no longer steers the built-in \
             agents: they name inherit or a model alias since this build. Pin their models \
             under [providers.<name>.model_aliases]."
                .to_string(),
            "[fallback_chains].\"claude-sonnet-4-6\" no longer steers the built-in agents: \
             they name inherit or a model alias since this build. Pin their models under \
             [providers.<name>.model_aliases]."
                .to_string(),
        ]
    );
}

/// M-6f: the seeded `llm.toml` carries no `[fallback_chains]` table — its
/// `[orchestrator] fallback_models` sits under the main model's id only in
/// the router's merged map, which this does not read — so a stock install
/// logs nothing.
#[test]
fn the_seeded_llm_toml_logs_no_fallback_chain_line() {
    let seeded = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/release/templates/config/llm.toml"
    ))
    .expect("the release template");
    let config: openalpaca_llm::LlmRouterConfig =
        toml::from_str(&seeded).expect("the seeded llm.toml parses");
    assert!(
        config.fallback_chains.is_none(),
        "no [fallback_chains] table"
    );
    let templates: Vec<Option<String>> = crate::bundled::bundled_templates()
        .into_iter()
        .map(|t| t.frontmatter.model)
        .collect();
    assert!(
        stale_fallback_chain_lines(
            config.fallback_chains.as_ref(),
            Some(&anthropic_only()),
            &templates
        )
        .is_empty()
    );
}

/// M-7: forty ids list as the first thirty, then how many more.
#[test]
fn a_list_of_forty_ids_shows_thirty_and_ten_more() {
    let ids: Vec<String> = (0..40).map(|i| format!("m{i:02}")).collect();
    assert_eq!(
        listed(&ids),
        format!("{}, and 10 more", ids[..30].join(", "))
    );
    assert_eq!(listed(&ids[..3]), "m00, m01, m02");
    assert_eq!(listed(&[]), "none");
}

/// M-8: a definition built outside a run names the words alone; one built
/// for a one-model local install says each alias is that model.
#[test]
fn the_model_parameter_without_a_run_names_the_words_and_one_model_says_so() {
    assert_eq!(
        ModelMenu::default().description(),
        "Run this subagent on another model than its template's: fast, balanced or strong \
         (resolved on this install), or a routable model id. Omit it to use the template's \
         model. The template's cost cap still applies."
    );
    let menu = ModelMenu {
        aliases: ModelAlias::ALL
            .into_iter()
            .map(|a| (a, "qwen3:8b".to_string()))
            .collect(),
        routable: vec!["qwen3:8b".to_string()],
        lead_model: Some("qwen3:8b".to_string()),
    };
    assert_eq!(
        menu.description(),
        "Run this subagent on another model than its template's. Here fast, balanced and \
         strong are all qwen3:8b; or one of: qwen3:8b. Omit it to use the template's model — \
         most templates inherit yours, qwen3:8b. The template's cost cap still applies."
    );
}

/// M-1: a slot naming a model the router cannot route — a provider's dated
/// snapshot id the catalogue lacks — is not the child's parent: the lead's resolved model
/// is (the commit's choice; the design reads the slot alone).
#[test]
fn an_answering_model_the_router_cannot_route_falls_to_the_leads_own() {
    let router = anthropic_only();
    let slot = answering_model();
    assert_eq!(
        parent_of_child(&router, Some(&slot), Some("claude-opus-5-5")).as_deref(),
        Some("claude-opus-5-5"),
        "an empty slot"
    );
    write_answering(&slot, "a-snapshot-id-the-catalogue-lacks");
    assert_eq!(
        parent_of_child(&router, Some(&slot), Some("claude-opus-5-5")).as_deref(),
        Some("claude-opus-5-5")
    );
    write_answering(&slot, "claude-sonnet-5-5");
    assert_eq!(
        parent_of_child(&router, Some(&slot), Some("claude-opus-5-5")).as_deref(),
        Some("claude-sonnet-5-5")
    );
}

/// M-6f's "not routable" filter: an earlier pin's key that is routable here
/// (a key whose discovery still lists the id, registered as discovery
/// registers an unlisted id: unpriced, no window) is not named. Added in
/// C8's review; red with the filter taken out.
#[test]
fn a_routable_earlier_pin_key_is_not_named_at_start() {
    let router = anthropic_only();
    router.model_registry().register(
        "claude-sonnet-4-6".to_string(),
        openalpaca_llm::ModelInfo {
            provider: ProviderType::Anthropic,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 0,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
            long_prompt: None,
            cache_rates: None,
            request_contract: openalpaca_llm::RequestContract::NONE,
        },
    );
    assert!(router.is_routable("claude-sonnet-4-6"));
    let chains = HashMap::from([(
        "claude-sonnet-4-6".to_string(),
        vec!["gpt-6.1-terra".to_string()],
    )]);
    assert!(stale_fallback_chain_lines(Some(&chains), Some(&router), &[None]).is_empty());
}

/// M-6f reads each template's run (its stored set over it, AGENT-SETS): a
/// key a set over a shipped template still names is not called stale.
/// Added in C8's review; red with the start reading the bare template lines.
#[test]
fn a_key_a_stored_set_still_names_is_not_named_at_start() {
    let shared = crate::context::SharedContext::new();
    shared
        .agent_registry
        .set_bundled_layer(crate::bundled::bundled_templates());
    shared.agent_registry.restore_set(
        "code_agent",
        Some(serde_json::json!({ "llm": { "model": "claude-opus-4-6" } })),
    );
    let models = template_models(&shared.agent_registry);
    assert!(models.contains(&Some("claude-opus-4-6".to_string())));
    assert!(models.contains(&Some("strong".to_string())), "lead_agent");
    let chains = HashMap::from([(
        "claude-opus-4-6".to_string(),
        vec!["gpt-6.1-sol".to_string()],
    )]);
    assert!(stale_fallback_chain_lines(Some(&chains), Some(&anthropic_only()), &models).is_empty());
}
