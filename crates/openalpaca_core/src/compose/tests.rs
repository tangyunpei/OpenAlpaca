//! Phase 1 integration tests for the compose engine.
//!
//! Asserts that `ComposeEngine::compose` runs end-to-end with stub layer
//! outputs, populates all four layer fingerprints plus the composite, and
//! that the per-variant default-modes table matches the spec.

use super::*;
use std::sync::Arc;

use openalpaca_llm::ToolDefinition;

use crate::middleware::identity::IdentityDocument;
use crate::middleware::prompt::{AgentPersona, SystemPersona};
use crate::prompt_ctx::{ContextBundle, ExecutionPath};

/// Build the four per-layer inputs with minimal valid data. Phase 1 stub
/// layers tolerate empty content, so empty Arcs / Vecs / None suffice.
fn empty_compose_inputs() -> (
    PersonaInput,
    StaticPromptInput,
    DynamicContextInput,
    HistoryInput,
) {
    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: None,
    };

    // Layer 1 stub needs only persona_version + mode to compute a fingerprint;
    // Layer 2 wants the persona output, which we compute here so the static
    // prompt fingerprint is derived from a real persona fingerprint.
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: Option::<Arc<AgentPersona>>::None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::<ToolDefinition>::new()),
        connector_status: Arc::new(Vec::<ConnectorSummary>::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: Vec::new(),
        mode: StaticPromptMode::Default,
        model_window: 200_000,
        planner_agents: None,
        planner_protocol_v2: false,
    };

    let dynamic_context_input = DynamicContextInput {
        query: Arc::<str>::from("hi"),
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        memory_retrieval_hash: [0u8; 32],
        context_bundle: Arc::new(ContextBundle::empty()),
        mode: DynamicContextMode::Default,
    };

    let history_input = HistoryInput {
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: None,
        lane_tip_fingerprint: [0u8; 32],
        mode: HistoryMode::Default,
    };

    (
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
    )
}

#[test]
fn test_engine_produces_composed_request_for_all_fingerprints_wired() {
    let engine = ComposeEngine::default();
    let request = ComposeRequest::Social {
        lane_key: "test-lane".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };
    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let out = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        None,
        None,
    );

    // Fingerprints should all be populated (non-zero arrays). Layer stubs
    // always feed something into blake3, so a zero array would indicate
    // an uninitialized path.
    assert!(out.fingerprints.persona != [0u8; 32]);
    assert!(out.fingerprints.static_prompt != [0u8; 32]);
    assert!(out.fingerprints.dynamic_context != [0u8; 32]);
    assert!(out.fingerprints.history != [0u8; 32]);
    // Composite is a hash of the other four, so it's also non-zero.
    assert!(out.fingerprints.composite != [0u8; 32]);

    // Layer trace records the modes that ran.
    assert_eq!(out.layer_trace.persona_mode, PersonaMode::Default);
    assert_eq!(out.layer_trace.static_prompt_mode, StaticPromptMode::Default);
    assert_eq!(
        out.layer_trace.dynamic_context_mode,
        DynamicContextMode::Default
    );

    // Model window passed through to the budget.
    assert_eq!(out.token_budget.model_window, 200_000);

    // Phase 2: real Layer 2 Default mode emits a non-empty system_message
    // (even with empty inputs, the default SystemPersona supplies
    // base_instructions). The assembly layer pushes exactly one system
    // message in that case (layers 3 and 4 still use empty-stub outputs).
    assert_eq!(out.messages.len(), 1, "expected one system message");
    assert_eq!(out.messages[0].role, openalpaca_llm::Role::System);
    assert!(!out.messages[0].content.is_empty());
}

#[test]
fn test_engine_runs_for_all_eight_variants_without_panic() {
    // Phase 1 guarantee: `ComposeEngine::compose` runs end-to-end for each of
    // the eight `ComposeRequest` variants. The stub layers are unaware of the
    // variant (that wiring arrives in Phase 3), so the test here simply
    // exercises every variant constructor and confirms `compose` returns a
    // typed `ComposedRequest` without panicking.
    let engine = ComposeEngine::default();

    // Build a representative instance of each variant. Content is intentionally
    // minimal — compose() takes pre-built layer inputs and ignores variant
    // fields in Phase 1.
    let variants: Vec<ComposeRequest> = vec![
        ComposeRequest::SimpleQuery {
            lane_key: "lane".to_string(),
            agent_persona: Arc::new(AgentPersona {
                role: "r".to_string(),
                tone: "t".to_string(),
                domain_knowledge: vec![],
            }),
            query: "q".to_string(),
            current_parts: None,
            message_source: Arc::<str>::from("cli"),
            overrides: ComposeOverrides::default(),
        },
        ComposeRequest::Skill {
            lane_key: "lane".to_string(),
            agent_persona: Arc::new(AgentPersona {
                role: "r".to_string(),
                tone: "t".to_string(),
                domain_knowledge: vec![],
            }),
            skill_id: "skill".to_string(),
            skill_block: Arc::<str>::from("block"),
            injected_context: Arc::<str>::from("ctx"),
            query: "q".to_string(),
            message_source: Arc::<str>::from("cli"),
            overrides: ComposeOverrides::default(),
        },
        ComposeRequest::Planner {
            idle_agents: Arc::new(vec![]),
            user_message: "plan".to_string(),
            active_tasks_block: None,
            overrides: ComposeOverrides::default(),
        },
        ComposeRequest::Replanner {
            current_plan: Arc::new(PlanState::default()),
            workspace_snapshot: Arc::new(WorkspaceSnapshot::default()),
            overrides: ComposeOverrides::default(),
        },
        ComposeRequest::Social {
            lane_key: "lane".to_string(),
            query: "hi".to_string(),
            overrides: ComposeOverrides::default(),
        },
        // PipelineStep and DagNode need a SubAgent (re-exported as AgentConfig).
        {
            let agent = Arc::new(AgentConfig {
                id: "a".to_string(),
                template_id: "a".to_string(),
                name: "A".to_string(),
                description: None,
                icon: None,
                status: crate::agent::subagent::AgentStatus::Idle,
                current_task: None,
                capabilities: vec![],
                preset: crate::agent::subagent::AgentPreset::default(),
                constraints: crate::agent::subagent::AgentConstraints::default(),
                llm_config: crate::agent::subagent::AgentLlmConfig::default(),
            });
            ComposeRequest::PipelineStep {
                agent: agent.clone(),
                step_index: 0,
                step_description: Arc::<str>::from("desc"),
                scope_block: Arc::<str>::from("scope"),
                output_block: Arc::<str>::from("output"),
                context_package: Arc::new(crate::prompt_ctx::ContextPackage {
                    sections: vec![],
                    total_tokens: 0,
                    budget: 0,
                    sub_agent_window: 200_000,
                }),
                memory_block: None,
                overrides: ComposeOverrides::default(),
            }
        },
        {
            let agent = Arc::new(AgentConfig {
                id: "a".to_string(),
                template_id: "a".to_string(),
                name: "A".to_string(),
                description: None,
                icon: None,
                status: crate::agent::subagent::AgentStatus::Idle,
                current_task: None,
                capabilities: vec![],
                preset: crate::agent::subagent::AgentPreset::default(),
                constraints: crate::agent::subagent::AgentConstraints::default(),
                llm_config: crate::agent::subagent::AgentLlmConfig::default(),
            });
            ComposeRequest::DagNode {
                agent,
                assignment: Arc::<str>::from("assignment"),
                workspace_context: Arc::<str>::from("ws"),
                tools: Arc::new(vec![]),
                overrides: ComposeOverrides::default(),
            }
        },
        ComposeRequest::LeadAgent {
            base_persona: Arc::<str>::from("persona"),
            agents_catalog: Arc::<str>::from("catalog"),
            objective: Arc::<str>::from("objective"),
            overrides: ComposeOverrides::default(),
        },
    ];

    assert_eq!(variants.len(), 8, "all 8 variants must be represented");

    for (idx, req) in variants.iter().enumerate() {
        let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
        let out = engine.compose(
            req,
            p_in,
            sp_in,
            dc_in,
            h_in,
            200_000,
            Arc::new(vec![]),
            None,
            None,
        );
        assert_eq!(
            out.token_budget.model_window, 200_000,
            "variant {idx} dropped model_window"
        );
        assert!(
            out.fingerprints.composite != [0u8; 32],
            "variant {idx} produced zero composite fingerprint"
        );
    }
}

#[test]
fn test_compose_request_default_modes_table() {
    use DynamicContextMode as D;
    use HistoryMode as H;
    use PersonaMode as P;
    use StaticPromptMode as S;

    // SimpleQuery: all Default.
    let req = ComposeRequest::SimpleQuery {
        lane_key: "x".to_string(),
        agent_persona: Arc::new(AgentPersona {
            role: "r".to_string(),
            tone: "t".to_string(),
            domain_knowledge: vec![],
        }),
        query: "q".to_string(),
        current_parts: None,
        message_source: Arc::<str>::from("cli"),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Default);
    assert_eq!(sp, S::Default);
    assert_eq!(dc, D::Default);
    assert!(matches!(h, H::Default));

    // Social: Minimal + SocialMinimal + Skip + Default.
    let req = ComposeRequest::Social {
        lane_key: "x".to_string(),
        query: "q".to_string(),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Minimal);
    assert_eq!(sp, S::SocialMinimal);
    assert_eq!(dc, D::Skip);
    assert!(matches!(h, H::Default));

    // Planner: Minimal + PlannerHierarchical + Skip + Skip.
    let req = ComposeRequest::Planner {
        idle_agents: Arc::new(vec![]),
        user_message: "plan this".to_string(),
        active_tasks_block: None,
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Minimal);
    assert_eq!(sp, S::PlannerHierarchical);
    assert_eq!(dc, D::Skip);
    assert!(matches!(h, H::Skip));

    // Replanner: Minimal + ReplannerHierarchical + Skip + Default.
    // History=Default (not Skip) so the canonical "Evaluate..." current_user_turn
    // flows through Layer 4 into the final messages vec. See Phase 4 Commit 2.
    let req = ComposeRequest::Replanner {
        current_plan: Arc::new(PlanState::default()),
        workspace_snapshot: Arc::new(WorkspaceSnapshot::default()),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Minimal);
    assert_eq!(sp, S::ReplannerHierarchical);
    assert_eq!(dc, D::Skip);
    assert!(matches!(h, H::Default));

    // Phase 6: PipelineStep, DagNode, LeadAgent all use
    // (Skip, SubagentMinimal, Default, Default). Spec errata (pre-migration
    // emits no SystemPersona content → Skip); StaticPromptMode::SubagentMinimal
    // is the new Phase-6 mode carrying the raw_blocks-only emission order;
    // DynamicContextMode::Default routes the ContextPackage's user-targeted
    // sections through Layer 3; HistoryMode::Default lets the caller attach
    // memory / predecessor / current_user_turn entries.

    // LeadAgent: Skip + SubagentMinimal + Default + Default.
    let req = ComposeRequest::LeadAgent {
        base_persona: Arc::<str>::from("base"),
        agents_catalog: Arc::<str>::from("catalog"),
        objective: Arc::<str>::from("obj"),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Skip);
    assert_eq!(sp, S::SubagentMinimal);
    assert_eq!(dc, D::Default);
    assert!(matches!(h, H::Default));

    // PipelineStep: Skip + SubagentMinimal + Default + Default.
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };
    let dummy_agent = Arc::new(SubAgent {
        id: "dummy".to_string(),
        template_id: "dummy".to_string(),
        name: "Dummy".to_string(),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    });
    let req = ComposeRequest::PipelineStep {
        agent: dummy_agent.clone(),
        step_index: 0,
        step_description: Arc::<str>::from("desc"),
        scope_block: Arc::<str>::from("scope"),
        output_block: Arc::<str>::from("out"),
        context_package: Arc::new(crate::prompt_ctx::ContextPackage {
            sections: vec![],
            total_tokens: 0,
            budget: 0,
            sub_agent_window: 200_000,
        }),
        memory_block: None,
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Skip);
    assert_eq!(sp, S::SubagentMinimal);
    assert_eq!(dc, D::Default);
    assert!(matches!(h, H::Default));

    // DagNode: Skip + SubagentMinimal + Default + Default.
    let req = ComposeRequest::DagNode {
        agent: dummy_agent.clone(),
        assignment: Arc::<str>::from("a"),
        workspace_context: Arc::<str>::from("ws"),
        tools: Arc::new(vec![]),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Skip);
    assert_eq!(sp, S::SubagentMinimal);
    assert_eq!(dc, D::Default);
    assert!(matches!(h, H::Default));
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 2 Commit 1 — Layer 1 (Persona) tests
// ──────────────────────────────────────────────────────────────────────────

/// Build a minimal PersonaInput for Layer 1 tests. Populates all three docs
/// in a way that Default/Minimal/Skip modes produce distinct block counts.
fn make_persona_input(version: u64, mode: PersonaMode) -> PersonaInput {
    // SystemPersona::default() has a non-empty base_instructions — good.
    // Leave user_document and identity_document as None so only Default mode
    // can conditionally add blocks; we use populated variants in another
    // helper below to exercise the "more blocks" paths.
    PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: version,
        mode,
        identity_budget: None,
        user_budget: None,
    }
}

#[test]
fn test_persona_layer_fingerprint_deterministic() {
    let input_a = make_persona_input(1, PersonaMode::Default);
    let input_b = make_persona_input(1, PersonaMode::Default);
    let out_a = super::persona::compute(&input_a);
    let out_b = super::persona::compute(&input_b);
    assert_eq!(out_a.fingerprint, out_b.fingerprint);
}

#[test]
fn test_persona_layer_fingerprint_busts_on_version_bump() {
    let input_v1 = make_persona_input(1, PersonaMode::Default);
    let input_v2 = make_persona_input(2, PersonaMode::Default);
    let out_v1 = super::persona::compute(&input_v1);
    let out_v2 = super::persona::compute(&input_v2);
    assert_ne!(out_v1.fingerprint, out_v2.fingerprint);
}

#[test]
fn test_persona_layer_fingerprint_busts_on_mode_change() {
    let fp_default = super::persona::compute(&make_persona_input(1, PersonaMode::Default)).fingerprint;
    let fp_minimal = super::persona::compute(&make_persona_input(1, PersonaMode::Minimal)).fingerprint;
    let fp_skip = super::persona::compute(&make_persona_input(1, PersonaMode::Skip)).fingerprint;
    assert_ne!(fp_default, fp_minimal);
    assert_ne!(fp_default, fp_skip);
    assert_ne!(fp_minimal, fp_skip);
}

#[test]
fn test_persona_fingerprint_busts_on_user_budget_change() {
    // Construct two PersonaInputs that differ only by user_budget.
    let a = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: Some(500),
    };
    let b = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: Some(800),
    };
    assert_ne!(
        super::persona::compute_fingerprint(&a),
        super::persona::compute_fingerprint(&b),
        "user_budget changes must bust the Layer 1 fingerprint"
    );
}

#[test]
fn test_persona_fingerprint_busts_on_identity_budget_change() {
    let a = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: Some(300),
        user_budget: None,
    };
    let b = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: Some(500),
        user_budget: None,
    };
    assert_ne!(
        super::persona::compute_fingerprint(&a),
        super::persona::compute_fingerprint(&b),
    );
}

#[test]
fn test_persona_layer_default_mode_emits_blocks() {
    // Default mode should emit at least the system_persona block
    // (SystemPersona::default() has non-empty base_instructions).
    let input = make_persona_input(1, PersonaMode::Default);
    let out = super::persona::compute(&input);
    assert!(
        !out.blocks.is_empty(),
        "Default mode should emit at least the system_persona block"
    );
    assert!(
        out.blocks.iter().any(|b| b.name == "system_persona"),
        "Default mode must emit a system_persona block"
    );
}

#[test]
fn test_persona_layer_minimal_mode_emits_only_system_persona() {
    let out = super::persona::compute(&make_persona_input(1, PersonaMode::Minimal));
    assert_eq!(out.blocks.len(), 1, "Minimal mode emits exactly 1 block");
    assert_eq!(out.blocks[0].name, "system_persona");
}

#[test]
fn test_persona_layer_skip_mode_emits_no_blocks() {
    let input = make_persona_input(1, PersonaMode::Skip);
    let out = super::persona::compute(&input);
    assert!(out.blocks.is_empty(), "Skip mode should emit no blocks");
}

#[test]
fn test_global_cache_persona_hit_on_second_call() {
    let engine = ComposeEngine::default();
    let input = make_persona_input(1, PersonaMode::Default);

    // First call: miss — builds and inserts into cache.
    let out1 = engine.lookup_or_build_persona(&input, None);
    assert!(!out1.hit, "first call must be a miss");

    // Second call with identical input: hit.
    let out2 = engine.lookup_or_build_persona(&input, None);
    assert!(out2.hit, "second call with identical input must be a hit");

    // Outputs must be pointer-equal (same Arc<PersonaOutput>).
    assert!(Arc::ptr_eq(&out1.output, &out2.output));
}

#[test]
fn test_global_cache_persona_miss_on_version_bump() {
    let engine = ComposeEngine::default();
    let input_v1 = make_persona_input(1, PersonaMode::Default);
    let input_v2 = make_persona_input(2, PersonaMode::Default);

    let out1 = engine.lookup_or_build_persona(&input_v1, None);
    assert!(!out1.hit);

    let out2 = engine.lookup_or_build_persona(&input_v2, None);
    assert!(!out2.hit, "different persona_version should miss cache");
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 2 Commit 2 — Layer 2 (Static Prompt) tests
// ──────────────────────────────────────────────────────────────────────────

fn make_static_prompt_input(
    persona: Arc<PersonaOutput>,
    mode: StaticPromptMode,
) -> StaticPromptInput {
    StaticPromptInput {
        persona_output: persona,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(vec![]),
        connector_status: Arc::new(vec![]),
        send_tool_context: None,
        message_source: None,
        raw_blocks: Vec::new(),
        mode,
        model_window: 200_000,
        planner_agents: None,
        planner_protocol_v2: false,
    }
}

fn make_test_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("test tool {}", name),
        parameters: serde_json::json!({}),
        strict: None,
        input_examples: None,
    }
}

#[test]
fn test_static_prompt_layer_default_mode_produces_system_message() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Default,
    )));
    let input = make_static_prompt_input(persona_out, StaticPromptMode::Default);
    let out = super::static_prompt::compute(&input);
    assert!(
        !out.system_message.is_empty(),
        "Default mode should produce a non-empty system message"
    );
    assert!(
        !out.section_registry.is_empty(),
        "Default mode should populate section_registry"
    );
}

#[test]
fn test_static_prompt_layer_social_minimal_mode_matches_inline_format() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Minimal,
    )));
    let input = make_static_prompt_input(persona_out, StaticPromptMode::SocialMinimal);
    let out = super::static_prompt::compute(&input);
    // Social minimal produces the exact <system_instructions> / <agent_role>
    // envelope that simple_query_handler.rs emits verbatim. Verify structure.
    assert!(
        out.system_message.contains("<system_instructions>"),
        "SocialMinimal must wrap in <system_instructions>, got: {}",
        &*out.system_message
    );
    assert!(
        out.system_message.contains("<agent_role>"),
        "SocialMinimal must include <agent_role>, got: {}",
        &*out.system_message
    );
    assert!(
        out.system_message.contains("Role: Assistant"),
        "SocialMinimal must set Role: Assistant, got: {}",
        &*out.system_message
    );
    assert!(
        out.system_message.contains("Tone: Concise and professional"),
        "SocialMinimal must match simple_query_handler's exact Tone string, got: {}",
        &*out.system_message
    );
}

#[test]
fn test_static_prompt_layer_planner_hierarchical_mode() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Minimal,
    )));
    let mut input = make_static_prompt_input(persona_out, StaticPromptMode::PlannerHierarchical);
    // PlannerHierarchical mode reads `planner_agents` for the `<agents>`
    // block — populate with an empty list here (the golden tests cover
    // populated lists end-to-end).
    input.planner_agents = Some(Arc::new(vec![]));
    let out = super::static_prompt::compute(&input);
    // Must produce a non-empty system message (byte-identical structure is
    // asserted by test_golden_planner_byte_identical_protocol_v1/v2).
    assert!(
        !out.system_message.is_empty(),
        "PlannerHierarchical mode should produce a non-empty system message"
    );
    // Planner system prompt always opens with "You are a task planner".
    assert!(
        out.system_message.contains("task planner"),
        "PlannerHierarchical output should match the existing planner prompt structure"
    );
}

#[test]
fn test_static_prompt_fingerprint_busts_on_tool_change() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Default,
    )));
    let mut input_a = make_static_prompt_input(persona_out.clone(), StaticPromptMode::Default);
    input_a.tools = Arc::new(vec![make_test_tool("tool_a")]);
    let fp_a = super::static_prompt::compute(&input_a).fingerprint;

    let mut input_b = make_static_prompt_input(persona_out, StaticPromptMode::Default);
    input_b.tools = Arc::new(vec![make_test_tool("tool_b")]);
    let fp_b = super::static_prompt::compute(&input_b).fingerprint;

    assert_ne!(fp_a, fp_b);
}

#[test]
fn test_static_prompt_fingerprint_busts_on_agent_config_change() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Default,
    )));
    let mut input_a = make_static_prompt_input(persona_out.clone(), StaticPromptMode::Default);
    input_a.agent_config_fingerprint = [1u8; 32];
    let fp_a = super::static_prompt::compute(&input_a).fingerprint;

    let mut input_b = make_static_prompt_input(persona_out, StaticPromptMode::Default);
    input_b.agent_config_fingerprint = [2u8; 32];
    let fp_b = super::static_prompt::compute(&input_b).fingerprint;

    assert_ne!(fp_a, fp_b);
}

#[test]
fn test_static_prompt_fingerprint_busts_on_mode_change() {
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Default,
    )));
    let fp_default =
        super::static_prompt::compute(&make_static_prompt_input(persona_out.clone(), StaticPromptMode::Default))
            .fingerprint;
    let fp_social =
        super::static_prompt::compute(&make_static_prompt_input(persona_out, StaticPromptMode::SocialMinimal))
            .fingerprint;
    assert_ne!(fp_default, fp_social);
}

#[test]
fn test_global_cache_static_prompt_hit() {
    let engine = ComposeEngine::default();
    let persona_out = Arc::new(super::persona::compute(&make_persona_input(
        1,
        PersonaMode::Default,
    )));
    let input = make_static_prompt_input(persona_out, StaticPromptMode::Default);

    let out1 = engine.lookup_or_build_static_prompt(&input, None);
    assert!(!out1.hit);
    let out2 = engine.lookup_or_build_static_prompt(&input, None);
    assert!(out2.hit);
    assert!(Arc::ptr_eq(&out1.output, &out2.output));
}

#[test]
fn test_compose_sets_memo_hits_on_second_call() {
    // End-to-end: two back-to-back compose() calls with identical inputs
    // should produce memo_hits.persona == true and memo_hits.static_prompt ==
    // true on the second call.
    let engine = ComposeEngine::default();
    let request = ComposeRequest::Social {
        lane_key: "lane".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };

    let (p_in1, sp_in1, dc_in1, h_in1) = empty_compose_inputs();
    let out1 = engine.compose(
        &request,
        p_in1,
        sp_in1,
        dc_in1,
        h_in1,
        200_000,
        Arc::new(vec![]),
        None,
        None,
    );
    assert!(
        !out1.layer_trace.memo_hits.persona,
        "first call persona must be a miss"
    );
    assert!(
        !out1.layer_trace.memo_hits.static_prompt,
        "first call static_prompt must be a miss"
    );

    let (p_in2, sp_in2, dc_in2, h_in2) = empty_compose_inputs();
    let out2 = engine.compose(
        &request,
        p_in2,
        sp_in2,
        dc_in2,
        h_in2,
        200_000,
        Arc::new(vec![]),
        None,
        None,
    );
    assert!(
        out2.layer_trace.memo_hits.persona,
        "second call persona must hit cache"
    );
    assert!(
        out2.layer_trace.memo_hits.static_prompt,
        "second call static_prompt must hit cache"
    );
}

#[test]
fn test_compose_layer_cache_events_emitted() {
    use crate::bus::EventBus;
    use crate::events::SystemEvent;

    let engine = ComposeEngine::default();
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    let request = ComposeRequest::Social {
        lane_key: "lane".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };

    // First call — expect 2 Miss events (persona, static_prompt).
    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let _ = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        Some(&bus),
        None,
    );

    let mut persona_miss = false;
    let mut static_miss = false;
    // Drain what's buffered. Use try_recv so the test never hangs. We only
    // assert on L1/L2 here; Phase 3 also emits L3/L4 events, but this test
    // passes `lane: None` so L3/L4 events are valid-but-not-the-focus.
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::ComposeLayerCacheMiss { layer, .. } => match layer {
                LayerId::Persona => persona_miss = true,
                LayerId::StaticPrompt => static_miss = true,
                LayerId::DynamicContext | LayerId::History => {}
            },
            SystemEvent::ComposeLayerCacheHit { layer, .. } => match layer {
                LayerId::Persona | LayerId::StaticPrompt => {
                    panic!("first call should not emit an L1/L2 Hit event")
                }
                LayerId::DynamicContext | LayerId::History => {}
            },
            _ => {}
        }
    }
    assert!(persona_miss, "first call should emit a Persona Miss");
    assert!(static_miss, "first call should emit a StaticPrompt Miss");

    // Second call — expect 2 Hit events for L1/L2. L3/L4 still miss because
    // this test passes `lane: None` (no per-lane cache).
    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let _ = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        Some(&bus),
        None,
    );

    let mut persona_hit = false;
    let mut static_hit = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::ComposeLayerCacheHit { layer, .. } => match layer {
                LayerId::Persona => persona_hit = true,
                LayerId::StaticPrompt => static_hit = true,
                LayerId::DynamicContext | LayerId::History => {}
            },
            SystemEvent::ComposeLayerCacheMiss { layer, .. } => match layer {
                LayerId::Persona | LayerId::StaticPrompt => {
                    panic!("second call should not emit an L1/L2 Miss event")
                }
                LayerId::DynamicContext | LayerId::History => {}
            },
            _ => {}
        }
    }
    assert!(persona_hit, "second call should emit a Persona Hit");
    assert!(static_hit, "second call should emit a StaticPrompt Hit");
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 3 Commit 1 — Layer 3 (Dynamic Context) + Layer 4 (History) tests
// ──────────────────────────────────────────────────────────────────────────

use crate::prompt_ctx::{
    ContextKey, ContextKind, ContextSection, InjectionMode, TrustLevel,
};
use openalpaca_llm::{ChatMessage, Role};

fn make_dyn_ctx_input(mode: DynamicContextMode) -> DynamicContextInput {
    DynamicContextInput {
        query: Arc::from("test query"),
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 1000,
        memory_retrieval_hash: [0u8; 32],
        context_bundle: Arc::new(ContextBundle::empty()),
        mode,
    }
}

fn make_history_input(mode: HistoryMode) -> HistoryInput {
    HistoryInput {
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(vec![]),
        current_user_turn: None,
        lane_tip_fingerprint: [0u8; 32],
        mode,
    }
}

/// Construct a minimal `ContextBundle` with one UserMessage-mode section
/// (memory) and one SystemMessage-mode section (user profile). Exercises
/// both Layer 3 output paths (context_messages + additional_system_blocks).
fn make_test_bundle_with_memory_section() -> ContextBundle {
    ContextBundle {
        sections: vec![
            ContextSection {
                source: "memory",
                kind: ContextKind::Memory,
                content: "retrieved memory snippet".to_string(),
                token_estimate: 10,
                priority: SectionPriority::Normal,
                relevance: 0.8,
                key: ContextKey::Memory(42),
                injection: InjectionMode::UserMessage {
                    tag: "memory".to_string(),
                    trust: TrustLevel::Untrusted,
                },
            },
            ContextSection {
                source: "user_profile",
                kind: ContextKind::UserProfile,
                content: "user likes rust".to_string(),
                token_estimate: 5,
                priority: SectionPriority::High,
                relevance: 1.0,
                key: ContextKey::UserProfile,
                injection: InjectionMode::SystemMessage,
            },
        ],
        total_tokens: 15,
        available_budget: 10_000,
    }
}

#[test]
fn test_dynamic_context_layer_skip_mode_returns_empty() {
    let input = make_dyn_ctx_input(DynamicContextMode::Skip);
    let out = super::dynamic_context::compute(&input);
    assert!(out.context_messages.is_empty());
    assert!(out.additional_system_blocks.is_empty());
}

#[test]
fn test_dynamic_context_layer_fingerprint_deterministic() {
    let a = super::dynamic_context::compute(&make_dyn_ctx_input(DynamicContextMode::Default));
    let b = super::dynamic_context::compute(&make_dyn_ctx_input(DynamicContextMode::Default));
    assert_eq!(a.fingerprint, b.fingerprint);
}

#[test]
fn test_dynamic_context_layer_fingerprint_busts_on_query_change() {
    let mut a_input = make_dyn_ctx_input(DynamicContextMode::Default);
    a_input.query = Arc::from("query A");
    let mut b_input = make_dyn_ctx_input(DynamicContextMode::Default);
    b_input.query = Arc::from("query B");
    assert_ne!(
        super::dynamic_context::compute(&a_input).fingerprint,
        super::dynamic_context::compute(&b_input).fingerprint,
    );
}

#[test]
fn test_dynamic_context_layer_fingerprint_busts_on_memory_change() {
    let mut a = make_dyn_ctx_input(DynamicContextMode::Default);
    a.memory_retrieval_hash = [1u8; 32];
    let mut b = make_dyn_ctx_input(DynamicContextMode::Default);
    b.memory_retrieval_hash = [2u8; 32];
    assert_ne!(
        super::dynamic_context::compute(&a).fingerprint,
        super::dynamic_context::compute(&b).fingerprint,
    );
}

#[test]
fn test_dynamic_context_layer_fingerprint_busts_on_mode_change() {
    let fp_default =
        super::dynamic_context::compute(&make_dyn_ctx_input(DynamicContextMode::Default))
            .fingerprint;
    let fp_skip =
        super::dynamic_context::compute(&make_dyn_ctx_input(DynamicContextMode::Skip)).fingerprint;
    assert_ne!(fp_default, fp_skip);
}

#[test]
fn test_dynamic_context_layer_default_mode_emits_bundle_messages() {
    let mut input = make_dyn_ctx_input(DynamicContextMode::Default);
    input.context_bundle = Arc::new(make_test_bundle_with_memory_section());
    let out = super::dynamic_context::compute(&input);
    // Bundle has 1 UserMessage section and 1 SystemMessage section.
    // Layer 3 should produce at least one in each collection.
    assert!(
        !out.context_messages.is_empty(),
        "UserMessage-mode section should produce a context message"
    );
    assert!(
        !out.additional_system_blocks.is_empty(),
        "SystemMessage-mode section should produce an additional system block"
    );
}

#[test]
fn test_dynamic_context_layer_default_untrusted_wraps_content() {
    // UserMessage { trust: Untrusted } should pass through wrap_untrusted_context
    // before becoming a ChatMessage.
    let mut input = make_dyn_ctx_input(DynamicContextMode::Default);
    input.context_bundle = Arc::new(make_test_bundle_with_memory_section());
    let out = super::dynamic_context::compute(&input);
    let memory_msg = out
        .context_messages
        .iter()
        .find(|m| m.content.contains("retrieved memory snippet"))
        .expect("should have a message mentioning the memory content");
    assert!(
        memory_msg.content.contains("<context_data"),
        "untrusted content should be wrapped in <context_data> envelope"
    );
}

#[test]
fn test_history_layer_skip_mode_empty() {
    let input = make_history_input(HistoryMode::Skip);
    let out = super::history::compute(&input);
    assert!(out.messages.is_empty());
}

#[test]
fn test_history_layer_default_mode_assembles_summary_then_recent_then_current() {
    let mut input = make_history_input(HistoryMode::Default);
    input.summary = Some(Arc::from("prior summary"));
    input.recent_messages = Arc::new(vec![
        ChatMessage::user("old user msg"),
        ChatMessage::assistant("old assistant reply"),
    ]);
    input.current_user_turn = Some(ChatMessage::user("current turn"));

    let out = super::history::compute(&input);

    // Expect 4 messages: summary-as-user, old user, old assistant, current user.
    assert_eq!(out.messages.len(), 4);
    // First is the summary (possibly wrapped).
    assert!(matches!(out.messages[0].role, Role::User));
    // Middle is recent history.
    assert_eq!(out.messages[1].content, "old user msg");
    assert_eq!(out.messages[2].content, "old assistant reply");
    // Last is the current turn.
    assert_eq!(out.messages.last().unwrap().content, "current turn");
}

#[test]
fn test_history_layer_default_mode_untrusted_summary_is_wrapped() {
    let mut input = make_history_input(HistoryMode::Default);
    input.summary = Some(Arc::from("prior summary"));
    input.summary_wrap_mode = SummaryWrapMode::UntrustedWrap;
    let out = super::history::compute(&input);
    assert_eq!(out.messages.len(), 1);
    assert!(
        out.messages[0].content.contains("<context_data"),
        "UntrustedWrap should wrap the summary in <context_data>"
    );
}

#[test]
fn test_history_layer_default_mode_plain_summary_is_not_wrapped() {
    let mut input = make_history_input(HistoryMode::Default);
    input.summary = Some(Arc::from("prior summary"));
    input.summary_wrap_mode = SummaryWrapMode::Plain;
    let out = super::history::compute(&input);
    assert_eq!(out.messages.len(), 1);
    assert_eq!(out.messages[0].content, "prior summary");
}

#[test]
fn test_history_layer_first_step_only_mode_emits_memory_user_message() {
    let memory_block: Arc<str> = Arc::from("retrieved memory block");
    let input = HistoryInput {
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(vec![]),
        current_user_turn: None,
        lane_tip_fingerprint: [0u8; 32],
        mode: HistoryMode::FirstStepOnly {
            memory_block: memory_block.clone(),
        },
    };
    let out = super::history::compute(&input);
    assert_eq!(out.messages.len(), 1);
    assert!(matches!(out.messages[0].role, Role::User));
    assert_eq!(out.messages[0].content, "retrieved memory block");
}

#[test]
fn test_history_layer_fingerprint_busts_on_lane_tip_advance() {
    let mut a = make_history_input(HistoryMode::Default);
    a.lane_tip_fingerprint = [1u8; 32];
    let mut b = make_history_input(HistoryMode::Default);
    b.lane_tip_fingerprint = [2u8; 32];
    assert_ne!(
        super::history::compute(&a).fingerprint,
        super::history::compute(&b).fingerprint
    );
}

#[test]
fn test_history_layer_fingerprint_busts_on_current_turn_change() {
    let mut a = make_history_input(HistoryMode::Default);
    a.current_user_turn = Some(ChatMessage::user("A"));
    let mut b = make_history_input(HistoryMode::Default);
    b.current_user_turn = Some(ChatMessage::user("B"));
    assert_ne!(
        super::history::compute(&a).fingerprint,
        super::history::compute(&b).fingerprint
    );
}

#[test]
fn test_history_layer_fingerprint_busts_on_summary_change() {
    let mut a = make_history_input(HistoryMode::Default);
    a.summary = Some(Arc::from("summary A"));
    let mut b = make_history_input(HistoryMode::Default);
    b.summary = Some(Arc::from("summary B"));
    assert_ne!(
        super::history::compute(&a).fingerprint,
        super::history::compute(&b).fingerprint
    );
}

#[test]
fn test_history_layer_fingerprint_deterministic() {
    let a = make_history_input(HistoryMode::Default);
    let b = make_history_input(HistoryMode::Default);
    assert_eq!(
        super::history::compute(&a).fingerprint,
        super::history::compute(&b).fingerprint
    );
}

#[test]
fn test_hash_opt_msg_distinguishes_text_from_parts() {
    use super::fingerprint::hash_opt_msg;
    use openalpaca_llm::ContentPart;
    // Two messages with same content but one has parts populated — fingerprint
    // must differ so the history cache busts on multimodal content changes.
    let msg_text = ChatMessage::user("hello");
    let mut msg_parts = ChatMessage::user("hello");
    msg_parts.parts = Some(vec![ContentPart::Text {
        text: "hello".to_string(),
    }]);
    let fp_text = hash_opt_msg(&Some(msg_text));
    let fp_parts = hash_opt_msg(&Some(msg_parts));
    assert_ne!(fp_text, fp_parts);
}

#[test]
fn test_hash_opt_msg_none_vs_empty() {
    use super::fingerprint::hash_opt_msg;
    let none_fp = hash_opt_msg(&None);
    let empty_fp = hash_opt_msg(&Some(ChatMessage::user("")));
    assert_ne!(none_fp, empty_fp, "None and Some(empty) must not collide");
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 3 Commit 2 — Per-lane cache + assembly trim integration tests
// ──────────────────────────────────────────────────────────────────────────

use crate::lane::{ConversationLane, LaneKey};

#[test]
fn test_per_lane_cache_hit_dynamic_context_on_same_turn() {
    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));

    let input = make_dyn_ctx_input(DynamicContextMode::Default);

    // First call: miss, populates the lane slot.
    let out1 = engine.lookup_or_build_dynamic_context(&input, Some(&lane));
    assert!(!out1.hit);

    // Second call with identical fingerprint: hit.
    let out2 = engine.lookup_or_build_dynamic_context(&input, Some(&lane));
    assert!(out2.hit);
    assert!(Arc::ptr_eq(&out1.output, &out2.output));
}

#[test]
fn test_per_lane_cache_dynamic_context_skipped_when_lane_none() {
    let engine = ComposeEngine::default();
    let input = make_dyn_ctx_input(DynamicContextMode::Default);

    // Without a lane, every call misses (no cache storage).
    let out1 = engine.lookup_or_build_dynamic_context(&input, None);
    assert!(!out1.hit);
    let out2 = engine.lookup_or_build_dynamic_context(&input, None);
    assert!(!out2.hit);
}

#[test]
fn test_per_lane_cache_history_hit_on_same_turn() {
    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));

    let input = make_history_input(HistoryMode::Default);
    let out1 = engine.lookup_or_build_history(&input, Some(&lane));
    assert!(!out1.hit);

    let out2 = engine.lookup_or_build_history(&input, Some(&lane));
    assert!(out2.hit);
    assert!(Arc::ptr_eq(&out1.output, &out2.output));
}

#[test]
fn test_per_lane_cache_history_bust_on_tip_advance() {
    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));

    let mut input = make_history_input(HistoryMode::Default);
    input.lane_tip_fingerprint = [1u8; 32];
    let out1 = engine.lookup_or_build_history(&input, Some(&lane));
    assert!(!out1.hit);

    // Advance tip — fingerprint changes → miss.
    input.lane_tip_fingerprint = [2u8; 32];
    let out2 = engine.lookup_or_build_history(&input, Some(&lane));
    assert!(!out2.hit);
}

#[test]
fn test_compose_end_to_end_all_four_layers_memoized_on_second_call() {
    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));

    let request = ComposeRequest::Social {
        lane_key: "user:cli".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };

    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let out1 = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        None,
        Some(&lane),
    );
    // First call: all 4 layers miss.
    assert!(!out1.layer_trace.memo_hits.persona);
    assert!(!out1.layer_trace.memo_hits.static_prompt);
    assert!(!out1.layer_trace.memo_hits.dynamic_context);
    assert!(!out1.layer_trace.memo_hits.history);

    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let out2 = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        None,
        Some(&lane),
    );
    // Second call with identical lane + inputs: all 4 layers hit.
    assert!(
        out2.layer_trace.memo_hits.persona,
        "second call persona must hit global cache"
    );
    assert!(
        out2.layer_trace.memo_hits.static_prompt,
        "second call static_prompt must hit global cache"
    );
    assert!(
        out2.layer_trace.memo_hits.dynamic_context,
        "second call dynamic_context must hit per-lane cache"
    );
    assert!(
        out2.layer_trace.memo_hits.history,
        "second call history must hit per-lane cache"
    );
}

#[test]
fn test_compose_lane_tip_advance_busts_only_history() {
    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));

    let request = ComposeRequest::Social {
        lane_key: "user:cli".to_string(),
        query: "q".to_string(),
        overrides: ComposeOverrides::default(),
    };

    // First compose — all miss.
    let (p_in, sp_in, dc_in, mut h_in) = empty_compose_inputs();
    h_in.lane_tip_fingerprint = [1u8; 32];
    let _out1 = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        None,
        Some(&lane),
    );

    // Second compose — bump lane tip; dynamic context unchanged.
    let (p_in, sp_in, dc_in, mut h_in) = empty_compose_inputs();
    h_in.lane_tip_fingerprint = [2u8; 32];
    let out2 = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        None,
        Some(&lane),
    );
    assert!(
        out2.layer_trace.memo_hits.persona,
        "persona still cached in global"
    );
    assert!(
        out2.layer_trace.memo_hits.static_prompt,
        "static_prompt still cached in global"
    );
    assert!(
        out2.layer_trace.memo_hits.dynamic_context,
        "dynamic_context unchanged on same lane — should still hit"
    );
    assert!(
        !out2.layer_trace.memo_hits.history,
        "lane_tip_fingerprint changed — history must miss"
    );
}

#[test]
fn test_compose_emits_four_cache_events_per_call() {
    use crate::bus::EventBus;
    use crate::events::SystemEvent;

    let engine = ComposeEngine::default();
    let lane = ConversationLane::new(LaneKey::new("user", "cli"));
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();

    let request = ComposeRequest::Social {
        lane_key: "user:cli".to_string(),
        query: "q".to_string(),
        overrides: ComposeOverrides::default(),
    };
    let (p_in, sp_in, dc_in, h_in) = empty_compose_inputs();
    let _ = engine.compose(
        &request,
        p_in,
        sp_in,
        dc_in,
        h_in,
        200_000,
        Arc::new(vec![]),
        Some(&bus),
        Some(&lane),
    );

    // Drain the bus and assert we saw one event per layer (all Miss on the
    // first call).
    let mut seen_persona = false;
    let mut seen_static = false;
    let mut seen_dyn = false;
    let mut seen_hist = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::ComposeLayerCacheMiss { layer, .. }
            | SystemEvent::ComposeLayerCacheHit { layer, .. } => match layer {
                LayerId::Persona => seen_persona = true,
                LayerId::StaticPrompt => seen_static = true,
                LayerId::DynamicContext => seen_dyn = true,
                LayerId::History => seen_hist = true,
            },
            _ => {}
        }
    }
    assert!(seen_persona, "compose should emit a Persona event");
    assert!(seen_static, "compose should emit a StaticPrompt event");
    assert!(seen_dyn, "compose should emit a DynamicContext event");
    assert!(seen_hist, "compose should emit a History event");
}

#[test]
fn test_assembly_budget_trim_drops_oldest_non_system_history() {
    use super::assembly::{self, AssemblyInput};

    // Build a history with enough messages to blow a tiny budget.
    // Each "padded " message contributes ~(content.len()/4) tokens.
    let big: String = "a".repeat(4000);
    let history_messages: Vec<ChatMessage> = (0..4)
        .map(|i| {
            if i % 2 == 0 {
                ChatMessage::user(&big)
            } else {
                ChatMessage::assistant(&big)
            }
        })
        .collect();

    let persona = PersonaOutput {
        blocks: vec![],
        fingerprint: [0u8; 32],
    };
    let static_prompt = StaticPromptOutput {
        system_message: Arc::from("system"),
        section_registry: vec!["system"],
        fingerprint: [0u8; 32],
    };
    let dyn_ctx = DynamicContextOutput {
        context_messages: vec![],
        additional_system_blocks: vec![],
        fingerprint: [0u8; 32],
    };
    let history = HistoryOutput {
        messages: history_messages.clone(),
        fingerprint: [0u8; 32],
    };
    let layer_trace = LayerTrace {
        persona_mode: PersonaMode::Minimal,
        static_prompt_mode: StaticPromptMode::Default,
        dynamic_context_mode: DynamicContextMode::Skip,
        history_mode: HistoryMode::Default,
        memo_hits: LayerMemoHits::default(),
    };

    // Tiny model window — 2000 tokens, threshold = 1500. History alone
    // contributes ~4000 tokens. Trimming must fire.
    let out = assembly::compose(AssemblyInput {
        persona: &persona,
        static_prompt: &static_prompt,
        dynamic_context: &dyn_ctx,
        history: &history,
        tools: Arc::new(vec![]),
        model_window: 2000,
        layer_trace,
    });

    assert!(
        out.section_registry
            .iter()
            .any(|name| *name == "<trimmed:history>"),
        "assembly should record a <trimmed:history> marker when trimming fires"
    );
    // Final message list must be shorter than (history_len + 1) — some
    // non-system messages were dropped.
    assert!(
        out.messages.len() < history_messages.len() + 1,
        "assembly should drop at least one history message when over budget"
    );
    // System message at index 0 is preserved.
    assert_eq!(out.messages[0].role, Role::System);
}

#[test]
fn test_assembly_budget_trim_preserves_system_message() {
    use super::assembly::{self, AssemblyInput};

    // Huge system prompt + a few history messages. Even when trimming fires,
    // the system message at index 0 must survive.
    let huge_sys: String = "s".repeat(20_000);
    let big: String = "a".repeat(4000);
    let history = HistoryOutput {
        messages: vec![ChatMessage::user(&big), ChatMessage::assistant(&big)],
        fingerprint: [0u8; 32],
    };
    let persona = PersonaOutput {
        blocks: vec![],
        fingerprint: [0u8; 32],
    };
    let static_prompt = StaticPromptOutput {
        system_message: Arc::from(huge_sys.as_str()),
        section_registry: vec!["system"],
        fingerprint: [0u8; 32],
    };
    let dyn_ctx = DynamicContextOutput {
        context_messages: vec![],
        additional_system_blocks: vec![],
        fingerprint: [0u8; 32],
    };
    let layer_trace = LayerTrace {
        persona_mode: PersonaMode::Minimal,
        static_prompt_mode: StaticPromptMode::Default,
        dynamic_context_mode: DynamicContextMode::Skip,
        history_mode: HistoryMode::Default,
        memo_hits: LayerMemoHits::default(),
    };

    let out = assembly::compose(AssemblyInput {
        persona: &persona,
        static_prompt: &static_prompt,
        dynamic_context: &dyn_ctx,
        history: &history,
        tools: Arc::new(vec![]),
        model_window: 2000, // threshold 1500 — below sys prompt alone
        layer_trace,
    });

    // Index 0 is still the system message (assembly never drops it).
    assert_eq!(out.messages[0].role, Role::System);
    // All remaining messages after trim must be system or not — assembly only
    // drops non-system. Verify by iterating: at least one message survives and
    // the first is System.
    assert!(!out.messages.is_empty());
}

#[test]
fn test_assembly_populates_per_layer_token_budget() {
    use super::assembly::{self, AssemblyInput};

    let persona = PersonaOutput {
        blocks: vec![SystemBlock {
            name: "system_persona",
            content: Arc::from("persona content"),
            priority: SectionPriority::Critical,
        }],
        fingerprint: [0u8; 32],
    };
    let static_prompt = StaticPromptOutput {
        system_message: Arc::from("this is the static system message"),
        section_registry: vec!["system"],
        fingerprint: [0u8; 32],
    };
    let dyn_ctx = DynamicContextOutput {
        context_messages: vec![ChatMessage::user("dynamic context message")],
        additional_system_blocks: vec![],
        fingerprint: [0u8; 32],
    };
    let history = HistoryOutput {
        messages: vec![ChatMessage::user("history message")],
        fingerprint: [0u8; 32],
    };
    let layer_trace = LayerTrace {
        persona_mode: PersonaMode::Default,
        static_prompt_mode: StaticPromptMode::Default,
        dynamic_context_mode: DynamicContextMode::Default,
        history_mode: HistoryMode::Default,
        memo_hits: LayerMemoHits::default(),
    };

    let out = assembly::compose(AssemblyInput {
        persona: &persona,
        static_prompt: &static_prompt,
        dynamic_context: &dyn_ctx,
        history: &history,
        tools: Arc::new(vec![]),
        model_window: 200_000,
        layer_trace,
    });

    assert!(
        out.token_budget.static_prompt_tokens > 0,
        "static_prompt_tokens should be populated"
    );
    assert!(
        out.token_budget.persona_tokens > 0,
        "persona_tokens should be populated from the persona blocks"
    );
    assert!(
        out.token_budget.dynamic_context_tokens > 0,
        "dynamic_context_tokens should cover context messages"
    );
    assert!(
        out.token_budget.history_tokens > 0,
        "history_tokens should cover history messages"
    );
    assert_eq!(out.token_budget.model_window, 200_000);
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 4 Commit 1 — Social fast path migration golden-output test
// ──────────────────────────────────────────────────────────────────────────

/// Golden-output: asserts the migrated Social path produces byte-identical
/// `ComposedRequest.messages` to what the pre-migration inline `format!`
/// at `simple_query_handler.rs:520-528` would have produced.
///
/// The pre-migration code was:
///
/// ```ignore
/// let system_prompt = format!(
///     "<system_instructions>\n{}\n</system_instructions>\n\n\
///      <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
///     system_persona.base_instructions
/// );
///
/// let mut messages = Vec::with_capacity(2 + ctx.recent_messages.len());
/// messages.push(ChatMessage::system(&system_prompt));
/// messages.extend(ctx.recent_messages.iter().cloned());
/// messages.push(ChatMessage::user(query));
/// ```
///
/// Routing through `compose(ComposeRequest::Social{..})` with
/// `PersonaMode::Minimal` + `StaticPromptMode::SocialMinimal` +
/// `DynamicContextMode::Skip` + `HistoryMode::Default` (and no summary) must
/// produce the identical message sequence.
#[test]
fn test_golden_social_fast_path_byte_identical() {
    use openalpaca_llm::Role;
    use std::num::NonZeroUsize;

    // Canned SystemPersona mimicking production content.
    let system_persona = Arc::new(SystemPersona {
        base_instructions: "You are OpenAlpaca, a helpful assistant.".to_string(),
        ..SystemPersona::default()
    });

    // Canned recent_messages.
    let recent = vec![
        ChatMessage::user("previous turn"),
        ChatMessage::assistant("previous reply"),
    ];
    let query = "thanks!";

    // === Pre-migration reference string (copied verbatim from the
    //     production inline format! at simple_query_handler.rs:520-523) ===
    let expected_system = format!(
        "<system_instructions>\n{}\n</system_instructions>\n\n\
         <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
        system_persona.base_instructions
    );

    // === Reference messages vec (matches simple_query_handler.rs:525-528) ===
    let mut expected_messages: Vec<ChatMessage> = Vec::with_capacity(2 + recent.len());
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(recent.iter().cloned());
    expected_messages.push(ChatMessage::user(query));

    // === Via the engine ===
    let engine = ComposeEngine::new(NonZeroUsize::new(16).unwrap().get());

    let persona_input = PersonaInput {
        system_persona: system_persona.clone(),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Minimal,
        identity_budget: None,
        user_budget: None,
    };

    // Minimal mode places SystemPersona.base_instructions as the first block's
    // content. Pre-compute here so StaticPromptInput has the real Arc; compose()
    // will overwrite with the cache-hit Arc on subsequent calls.
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::<ToolDefinition>::new()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: Vec::new(),
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SocialMinimal,
        model_window: 8192,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(query),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Skip,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(recent.clone()),
        current_user_turn: Some(ChatMessage::user(query)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::Social {
        lane_key: "social_lane".to_string(),
        query: query.to_string(),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        None,
    );

    // Byte-identical assertion. `ChatMessage` does not derive `PartialEq`, so
    // compare role + content field-by-field. The Social fast path never
    // populates parts/tool_calls/tool_call_id, and neither does the compose
    // output for this request shape — a role+content match is sufficient.
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "Social migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch"
        );
        // Sanity: Social path never populates these.
        assert!(got.parts.is_none(), "message {idx} parts should be None");
        assert!(
            got.tool_calls.is_none(),
            "message {idx} tool_calls should be None"
        );
        assert!(
            got.tool_call_id.is_none(),
            "message {idx} tool_call_id should be None"
        );
    }

    // Explicit check: first message is the system prompt, last is the current
    // user turn.
    assert_eq!(composed.messages[0].role, Role::System);
    assert_eq!(composed.messages.last().unwrap().role, Role::User);
    assert_eq!(composed.messages.last().unwrap().content, query);
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 4 Commit 2 — Replanner migration golden-output test
// ──────────────────────────────────────────────────────────────────────────

/// Golden-output: asserts the migrated Replanner path produces a byte-identical
/// system message to what the pre-migration `Replanner::build_replan_prompt`
/// (orchestrator/replanner/mod.rs lines 109-213) would have produced.
///
/// Routing through `compose(ComposeRequest::Replanner{..})` with
/// `PersonaMode::Minimal` + `StaticPromptMode::ReplannerHierarchical` +
/// `DynamicContextMode::Skip` + `HistoryMode::Default` (carrying the
/// canonical "Evaluate…" current_user_turn) must reproduce the pre-migration
/// system prompt exactly.
#[test]
fn test_golden_replanner_byte_identical() {
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };
    use openalpaca_llm::Role;

    // Canned inputs mirroring build_replan_prompt signature.
    let original_objective = "Process customer feedback into a report.";
    let dag_nodes_text = "\
- [node_1] \"Extract feedback\" (agent: parser) — COMPLETED — extracted 42 entries\n\
- [node_2] \"Categorize feedback\" (agent: classifier) — RUNNING\n\
- [node_3] \"Write report\" (agent: writer) — PENDING (dependencies not met)\n";
    let workspace_summary = "feedback_entries: 42 items\n";
    let replans_so_far: usize = 1;

    let make_agent = |id: &str, name: &str, desc: &str| SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: name.to_string(),
        description: Some(desc.to_string()),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    };
    let agents: Vec<SubAgent> = vec![
        make_agent("parser", "Parser", "Parses raw text"),
        make_agent("writer", "Writer", "Writes reports"),
    ];

    // === Expected system prompt (mirror of build_replan_prompt output) ===
    let expected_system = {
        let mut p = String::from(
            "You are a task replanner for OpenAlpaca. Evaluate whether the current \
             execution plan is still on track or needs modification.\n\n",
        );
        p.push_str(&format!(
            "<original_objective>\n{}\n</original_objective>\n\n",
            original_objective
        ));
        p.push_str("<dag_state>\n");
        p.push_str(dag_nodes_text);
        p.push_str("</dag_state>\n\n");
        p.push_str("<workspace>\n");
        p.push_str(workspace_summary);
        p.push_str("</workspace>\n\n");
        // Pre-migration `build_replan_prompt` emitted <available_agents>
        // BEFORE <context> — mirror that exact order here so the fixture
        // validates byte-identical preservation (not just internal
        // consistency).
        p.push_str("<available_agents>\n");
        for a in &agents {
            let desc = a.description.as_deref().unwrap_or("No description");
            p.push_str(&format!(
                "- ID: \"{}\", Name: \"{}\", Description: \"{}\"\n",
                a.id, a.name, desc
            ));
        }
        p.push_str("</available_agents>\n\n");
        p.push_str(&format!(
            "<context>\nReplans so far: {} (be conservative — avoid unnecessary changes)\n</context>\n\n",
            replans_so_far
        ));
        p.push_str(
            r#"<response_format>
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.

If the plan is on track:
{"decision": "continue"}

If the plan needs modification (replace remaining PENDING/READY nodes with new nodes):
{"decision": "modify_dag", "dag": {"nodes": [
  {"node_id": "new_1", "title": "...", "description": "...", "agent_id": "...", "agent_name": "...", "depends_on": [], "workspace_keys": [], "output_key": "..."},
  ...
]}}

If the task should be abandoned:
{"decision": "abort", "reason": "Explanation of why the task cannot be completed"}
</response_format>

<rules>
- Prefer "continue" unless completed results clearly show the remaining plan is wrong
- A "modify_dag" replaces only PENDING/READY/SKIPPED nodes; COMPLETED/RUNNING nodes are kept
- New nodes in modify_dag can reference output_keys from already-completed nodes
- Use exact agent_id values from the Available Agents list
- 2-8 nodes max in modified DAG
- Only abort if the task is fundamentally impossible given completed results
</rules>
"#,
        );
        p
    };

    // === Via the engine ===
    let engine = ComposeEngine::new(16);

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Minimal,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::new()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: vec![
            SystemBlock {
                name: "original_objective",
                content: Arc::<str>::from(format!(
                    "<original_objective>\n{}\n</original_objective>\n\n",
                    original_objective
                )),
                priority: SectionPriority::High,
            },
            SystemBlock {
                name: "dag_state",
                content: Arc::<str>::from(format!(
                    "<dag_state>\n{}</dag_state>\n\n",
                    dag_nodes_text
                )),
                priority: SectionPriority::High,
            },
            SystemBlock {
                name: "workspace",
                content: Arc::<str>::from(format!(
                    "<workspace>\n{}</workspace>\n\n",
                    workspace_summary
                )),
                priority: SectionPriority::Normal,
            },
            SystemBlock {
                name: "context",
                content: Arc::<str>::from(format!(
                    "<context>\nReplans so far: {} (be conservative — avoid unnecessary changes)\n</context>\n\n",
                    replans_so_far
                )),
                priority: SectionPriority::Normal,
            },
        ],
        planner_agents: Some(Arc::new(agents.clone())),
        planner_protocol_v2: false,
        mode: StaticPromptMode::ReplannerHierarchical,
        model_window: 8192,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(""),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Skip,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: Some(ChatMessage::user(
            "Evaluate the current task progress and decide whether to continue, \
             modify the plan, or abort.",
        )),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::Replanner {
        current_plan: Arc::new(PlanState::default()),
        workspace_snapshot: Arc::new(WorkspaceSnapshot::default()),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        None,
    );

    // Byte-identical assertion on the system message.
    let system_msg = composed
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .expect("expected a system message");
    assert_eq!(
        system_msg.content, expected_system,
        "Replanner migration produced non-byte-identical system prompt"
    );

    // User message carrying the canonical "Evaluate..." turn.
    assert!(
        composed.messages.iter().any(|m| matches!(m.role, Role::User)
            && m.content.starts_with("Evaluate the current task progress")),
        "Replanner migration must include the canonical 'Evaluate...' user turn"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 4 Commit 3 — Planner migration golden-output tests
// ──────────────────────────────────────────────────────────────────────────

/// Build the pre-migration planner system prompt as it was produced by the
/// now-deleted `task_planner::prompt::build_hierarchical_prompt(&agents, v2)`.
/// Kept inline in this test module so the Phase 4 Commit 3 migration is
/// asserted byte-for-byte against the old helper's output.
///
/// The two r#"..."# blocks below are pasted verbatim from the pre-migration
/// helper (task_planner/prompt.rs:156-263). Any future edit to the Planner
/// prompt must update this fixture too.
fn expected_planner_system_message(
    agents: &[crate::agent::subagent::SubAgent],
    plan_protocol_v2: bool,
) -> String {
    let mut prompt = String::from(
        "You are a task planner for OpenAlpaca. Classify the user message and, \
         for complex tasks, decompose into a DAG of sub-tasks.\n\n",
    );

    // Mirror format_agent_list (pre-migration task_planner/prompt.rs:10-36).
    prompt.push_str("<agents>\n");
    if agents.is_empty() {
        prompt.push_str("No agents are currently available.\n");
    } else {
        for agent in agents.iter() {
            let desc = agent.description.as_deref().unwrap_or("No description");
            let capabilities_str: Vec<String> = agent
                .capabilities
                .iter()
                .map(|s| format!("{} ({:.1})", s.name, s.proficiency))
                .collect();
            prompt.push_str(&format!(
                "<agent id=\"{}\" name=\"{}\">\n{}\nCapabilities: {}\n</agent>\n",
                agent.id,
                agent.name,
                desc,
                if capabilities_str.is_empty() {
                    "none".to_string()
                } else {
                    capabilities_str.join(", ")
                }
            ));
        }
    }
    prompt.push_str("</agents>\n");

    prompt.push_str(
        r#"
<instructions>
Classify the user's message into one of two categories:
- "simple_query": greetings, short questions, casual conversation, or anything answerable directly without agent work.
- "complex_task": multi-step tasks that require one or more agents to execute.

Think step-by-step before producing your JSON response:
1. Is this a simple greeting, question, or chat message? If yes, classify as "simple_query".
2. If it is a task, are all steps known upfront and predictable, or is it exploratory/dynamic?
3. Which available agents have the right skills for the task?
4. Write your reasoning into the "reasoning" field, then produce the JSON.

For complex tasks, choose exactly one execution strategy:
- Set "use_lead_agent": true when the task is genuinely exploratory, requires iterative refinement, or when the number of steps cannot be determined (e.g. debugging, open-ended research, creative exploration).
- Provide a "dag" with nodes when steps are enumerable upfront (even if partially dependent). Use DAG when multiple independent sub-tasks are visible in the user's message.
- Choose lead agent when the task is genuinely exploratory, adaptive, or requires iterative refinement. If the steps are clear, prefer DAG.

When choosing an execution strategy:
- lead_agent: Task is exploratory, adaptive, or requires iterative refinement.
- dag: 2+ steps known upfront; some steps can run in parallel.
- pipeline (assignments array): Steps are known upfront AND strictly sequential with no parallelism.
</instructions>

<examples>
Example 1 — Simple query:
User: "Hello, how are you?"
{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting, not a task.", "dag": null, "use_lead_agent": false}

Example 2 — Complex task with lead agent (exploratory):
User: "Research the best caching strategy for our REST API and recommend one."
{"classification": "complex_task", "title": "Research API caching strategies", "assignments": [], "reasoning": "This is an open-ended research task. The user wants evaluation of options, which requires iterative exploration. Using lead agent.", "dag": null, "use_lead_agent": true}

Example 3 — Complex task with DAG (predictable steps):
User: "Translate this document into French, Spanish, and German."
{"classification": "complex_task", "title": "Translate document into 3 languages", "assignments": [], "reasoning": "All three translations are known upfront and independent. Using a DAG with parallel nodes.", "dag": {"nodes": [
  {"node_id": "node_1", "title": "Translate to French", "description": "Translate the document into French.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "french_translation"},
  {"node_id": "node_2", "title": "Translate to Spanish", "description": "Translate the document into Spanish.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "spanish_translation"},
  {"node_id": "node_3", "title": "Translate to German", "description": "Translate the document into German.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "german_translation"}
]}, "use_lead_agent": false}

Example 4 — Complex task with DAG (sequential dependencies):
User: "Read the report, summarize key findings, then send the summary to the team."
{"classification": "complex_task", "title": "Read, summarize, and send report", "assignments": [], "reasoning": "Three steps with sequential dependencies: read → summarize → send. Using DAG with dependency edges.", "dag": {"nodes": [
  {"node_id": "n1", "title": "Read report", "description": "Read and extract content from the report.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": [], "workspace_keys": [], "output_key": "report_content"},
  {"node_id": "n2", "title": "Summarize findings", "description": "Summarize the key findings from the report.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": ["n1"], "workspace_keys": ["report_content"], "output_key": "summary"},
  {"node_id": "n3", "title": "Send summary", "description": "Send the summary to the team.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": ["n2"], "workspace_keys": ["summary"]}
]}, "use_lead_agent": false}

Example 5 — Sequential pipeline (strict linear dependency, no parallelism):
User: "Read the data file, analyze the trends, and write a report."
{"classification": "complex_task", "title": "Analyze data and write report", "assignments": [
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Read and parse the data file", "matched_skills": ["file_read"]},
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Analyze trends in the data", "matched_skills": ["analysis"]},
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Write the final report", "matched_skills": ["text_generate"]}
], "reasoning": "Strict linear pipeline: each step depends on the previous. No parallelism opportunity.", "dag": null, "use_lead_agent": false}

</examples>

<critical>
IMPORTANT: Regardless of the language of the user's message, you MUST ALWAYS respond with
ONLY a valid JSON object. Never reply conversationally. Never respond in the user's language.
Your ENTIRE output must be a single JSON object starting with '{' and ending with '}'.
</critical>

<format>
Respond with ONLY a single JSON object. No markdown fences, no explanation, no other text.

JSON schema:
{"classification": "simple_query" | "complex_task", "title": string | null, "assignments": [], "reasoning": "...", "dag": null | {"nodes": [...]}, "use_lead_agent": boolean}

When "classification" is "complex_task", you MUST provide exactly one execution path:
1. "use_lead_agent": true (with "dag": null) — for exploratory or dynamic tasks
2. "dag" with 2-8 nodes (with "use_lead_agent": false) — for fully predictable tasks
Do NOT set both "use_lead_agent": true and "dag" simultaneously.
Returning "complex_task" with no DAG and use_lead_agent=false is INVALID.
</format>

<rules>
DAG construction rules:
- Each node is a sub-task assigned to one agent (use exact agent_id values from the agents list)
- "depends_on": list of node_ids that must complete before this node starts
- Nodes with no shared dependencies run in parallel — express parallelism for independent tasks
- "workspace_keys": workspace entries this node reads (from other nodes' output_key)
- "output_key": workspace key where this node writes its result
- 2-8 nodes maximum
- Decompose into distinct stages that require different skills
</rules>
"#,
    );

    if plan_protocol_v2 {
        prompt.push_str(
            r#"

<v2_protocol>
Additional optional fields (v2 protocol):
- "execution_mode": "lead_agent" | "dag" | "pipeline" — explicit execution path.
  When set, this takes priority over use_lead_agent/dag inference.
- "predictability_score": 0.0-1.0 — your confidence that all task steps are known upfront.
  0.0 = fully exploratory, 1.0 = fully predictable.

When you include "execution_mode", you SHOULD also set "predictability_score".
Example:
{"classification": "complex_task", "title": "Batch process items", "assignments": [], "reasoning": "...", "dag": {...}, "use_lead_agent": false, "execution_mode": "dag", "predictability_score": 0.9}
</v2_protocol>
"#,
        );
    }

    prompt
}

/// Canned SubAgent fixture for the planner golden tests. Mirrors the pattern
/// used by the Replanner golden test above.
fn make_planner_test_agent(
    id: &str,
    name: &str,
    desc: &str,
    caps: Vec<(&str, f32)>,
) -> crate::agent::subagent::SubAgent {
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Capability, SubAgent,
    };
    SubAgent {
        id: id.to_string(),
        template_id: id.to_string(),
        name: name.to_string(),
        description: Some(desc.to_string()),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: caps
            .into_iter()
            .map(|(n, p)| Capability {
                name: n.to_string(),
                category: "test".to_string(),
                proficiency: p,
            })
            .collect(),
        preset: AgentPreset::default(),
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

/// Shared setup for both planner golden tests: builds `ComposeRequest::Planner`
/// + the four layer inputs, invokes `compose()`, and returns the composed
///   messages. Factoring this out keeps the v1/v2 tests DRY.
fn run_planner_compose(
    agents: &[crate::agent::subagent::SubAgent],
    user_message: &str,
    plan_protocol_v2: bool,
) -> ComposedRequest {
    use crate::prompt_ctx::AgentSummary;
    use openalpaca_llm::ChatMessage;

    let engine = ComposeEngine::new(16);

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: 0,
        mode: PersonaMode::Minimal,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::new()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: Vec::new(),
        planner_agents: Some(Arc::new(agents.to_vec())),
        planner_protocol_v2: plan_protocol_v2,
        mode: StaticPromptMode::PlannerHierarchical,
        model_window: 8192,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(user_message),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Skip,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: Some(ChatMessage::user(user_message)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::Planner {
        idle_agents: Arc::new(
            agents
                .iter()
                .map(|a| AgentSummary {
                    name: a.name.clone(),
                    role: a.description.clone().unwrap_or_default(),
                    step: 0,
                })
                .collect(),
        ),
        user_message: user_message.to_string(),
        active_tasks_block: None,
        overrides: ComposeOverrides::default(),
    };

    engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        None,
    )
}

/// Golden-output: asserts the migrated Planner path (via
/// `compose(ComposeRequest::Planner{..})`) produces a byte-identical system
/// message to what the now-deleted
/// `task_planner::prompt::build_hierarchical_prompt(&agents, false)` would
/// have produced, for a representative set of agents.
#[test]
fn test_golden_planner_byte_identical_protocol_v1() {
    use openalpaca_llm::Role;

    let agents = vec![
        make_planner_test_agent(
            "parser",
            "Parser",
            "Parses structured data.",
            vec![("parse_json", 0.9)],
        ),
        make_planner_test_agent("writer", "Writer", "Writes reports.", vec![]),
    ];
    let user_message = "Translate this document into French, Spanish, and German.";

    let expected_system = expected_planner_system_message(&agents, false);

    let composed = run_planner_compose(&agents, user_message, false);

    let system_msg = composed
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .expect("expected system message");
    assert_eq!(
        system_msg.content, expected_system,
        "Planner migration produced non-byte-identical system prompt for plan_protocol_v2=false"
    );

    // And the user turn is preserved verbatim.
    let user_turns: Vec<_> = composed
        .messages
        .iter()
        .filter(|m| matches!(m.role, Role::User))
        .collect();
    assert_eq!(user_turns.len(), 1);
    assert_eq!(user_turns[0].content, user_message);
}

/// Same as `test_golden_planner_byte_identical_protocol_v1` but with
/// `planner_protocol_v2 = true`. Expected output has the
/// `<v2_protocol>...</v2_protocol>` trailer appended.
#[test]
fn test_golden_planner_byte_identical_protocol_v2() {
    use openalpaca_llm::Role;

    let agents = vec![make_planner_test_agent(
        "solo",
        "Solo",
        "Handles everything.",
        vec![("generalist", 0.7)],
    )];
    let user_message = "Please batch-process these items: A, B, C.";

    let expected_system = expected_planner_system_message(&agents, true);

    let composed = run_planner_compose(&agents, user_message, true);

    let system_msg = composed
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .expect("expected system message");
    assert_eq!(
        system_msg.content, expected_system,
        "Planner migration produced non-byte-identical system prompt for plan_protocol_v2=true"
    );

    // The v2 trailer must be present in the migrated output.
    assert!(
        system_msg.content.contains("<v2_protocol>"),
        "v2_protocol trailer missing in planner_protocol_v2=true output"
    );
    assert!(
        system_msg.content.contains("predictability_score"),
        "predictability_score missing in planner_protocol_v2=true output"
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 5 Commit 1 — Skill Invocation migration golden-output test
// ──────────────────────────────────────────────────────────────────────────

/// Golden-output: asserts the migrated Skill Invocation path produces
/// byte-identical `ComposedRequest.messages` to what the pre-migration
/// `PromptBuilder` chain + manual message construction at
/// `orchestrator/skill/invocation.rs:114-440` would have produced.
///
/// Routing through `compose(ComposeRequest::Skill{..})` with
/// `PersonaMode::Default` + `StaticPromptMode::Default` +
/// `DynamicContextMode::Default` (empty bundle) + `HistoryMode::Default` must
/// produce the identical message sequence.
///
/// The scenario exercises the common "simple" Skill invocation shape:
/// a non-empty `skill_block`, empty `injected_context`, empty `identity_block`
/// / `bootstrap_block`, no connector statuses, a single non-send tool, no
/// session summary, and two recent messages.
#[test]
fn test_golden_skill_invocation_byte_identical() {
    use crate::prompt::PromptBuilder;
    use openalpaca_llm::{ChatMessage, Role};

    // ── Canned inputs mirroring the skill invocation pre-migration scene. ──
    let system_persona_val = SystemPersona {
        name: "OpenAlpaca".to_string(),
        core_values: vec![
            "Act as the user's trusted local AI agent".to_string(),
            "Provide structured output when asked".to_string(),
        ],
        safety_rules: vec![
            "Confirm before destructive actions".to_string(),
        ],
        base_instructions: "You are OpenAlpaca, a helpful local assistant.".to_string(),
    };
    let agent_persona_val = AgentPersona {
        role: "Assistant".to_string(),
        tone: "Concise and professional".to_string(),
        domain_knowledge: vec![],
    };
    let identity_block = ""; // No identity document.
    let bootstrap_block = ""; // No bootstrap document.
    let skill_block_text = "Active skill: test_skill\n\nDo the thing.";
    let injected_context = ""; // No injected context.
    let source_text = "cli";
    let query = "run the skill";
    let recent = vec![
        ChatMessage::user("prior turn"),
        ChatMessage::assistant("prior reply"),
    ];
    let tool_defs: Vec<ToolDefinition> = vec![ToolDefinition {
        name: "demo_tool".to_string(),
        description: "a demo tool".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "arg": { "type": "string" }
            }
        }),
        strict: None,
        input_examples: None,
    }];
    let model_window: usize = 200_000;

    // ── Reference: reconstruct pre-migration PromptBuilder chain. ──
    //
    // Matches orchestrator/skill/invocation.rs:114-295 for the "no connectors,
    // no send tool, no injected context, no identity, no bootstrap" shape:
    //
    //   system_persona -> agent_persona -> identity -> bootstrap ->
    //   raw_system_block("skill_body") -> message_source -> tools.
    let built = PromptBuilder::new(model_window)
        .system_persona(&system_persona_val)
        .agent_persona(&agent_persona_val)
        .identity(identity_block)
        .bootstrap(bootstrap_block)
        .raw_system_block("skill_body", skill_block_text, SectionPriority::High)
        .message_source(source_text)
        .tools(&tool_defs)
        .build();
    let expected_system = built.system_message.clone();

    // Reference messages: [system, ...built.context_messages, ...recent, user(query)].
    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + recent.len() + 1);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    // No summary in this scenario → skip.
    expected_messages.extend(recent.iter().cloned());
    expected_messages.push(ChatMessage::user(query));

    // ── Via the engine: PersonaMode::Default + StaticPromptMode::Default. ──
    let engine = ComposeEngine::new(16);

    let persona_input = PersonaInput {
        system_persona: Arc::new(system_persona_val.clone()),
        // No user/identity documents for this golden scenario.
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: None,
    };
    // Pre-compute so StaticPromptInput has a real Arc; compose() will overwrite
    // on cache hit.
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    // Skill-mode Static Prompt input: skill_block via the dedicated field (routes
    // to raw_system_block "skill_body" at High priority inside Layer 2 Default).
    // message_source, tools populated; connector_status empty; send_tool_context
    // None; raw_blocks empty (no injected_context in this scenario).
    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: Some(Arc::new(agent_persona_val.clone())),
        agent_config_fingerprint: [0u8; 32],
        skill_block: Some(Arc::<str>::from(skill_block_text)),
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: Some(Arc::<str>::from(source_text)),
        raw_blocks: Vec::new(),
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SkillInvocationDefault,
        model_window: model_window as u32,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(query),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SkillInvocation {
            skill_id: "test_skill".to_string(),
        },
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(recent.clone()),
        current_user_turn: Some(ChatMessage::user(query)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::Skill {
        lane_key: "skill_lane".to_string(),
        agent_persona: Arc::new(agent_persona_val.clone()),
        skill_id: "test_skill".to_string(),
        skill_block: Arc::<str>::from(skill_block_text),
        injected_context: Arc::<str>::from(injected_context),
        query: query.to_string(),
        message_source: Arc::<str>::from(source_text),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion (field-by-field; ChatMessage lacks PartialEq). ──
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "Skill migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        // ContentPart doesn't derive PartialEq — assert both sides are None
        // (no multimodal content in this text-only scenario).
        assert!(
            got.parts.is_none(),
            "message {idx} got parts should be None"
        );
        assert!(
            expected.parts.is_none(),
            "message {idx} expected parts should be None"
        );
        // Same for tool_calls (no ToolCall PartialEq derive).
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert!(
            expected.tool_calls.is_none(),
            "message {idx} expected tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Sanity: first message is the system prompt, last is the current user turn.
    assert_eq!(composed.messages[0].role, Role::System);
    assert_eq!(composed.messages.last().unwrap().role, Role::User);
    assert_eq!(composed.messages.last().unwrap().content, query);
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 5 Commit 2 — SimpleQuery migration golden-output tests
// ──────────────────────────────────────────────────────────────────────────

/// Golden-output: asserts the migrated SimpleQuery path (text-only) produces
/// byte-identical `ComposedRequest.messages` to what the pre-migration
/// `PromptBuilder` chain + manual message construction at
/// `orchestrator/query_handler/simple_query_handler.rs:196-307` would have
/// produced.
///
/// Routing through `compose(ComposeRequest::SimpleQuery{..})` with
/// `PersonaMode::Default` + `StaticPromptMode::Default` +
/// `DynamicContextMode::Default` (empty bundle) + `HistoryMode::Default` must
/// produce the identical message sequence.
///
/// The scenario exercises the common "simple" SimpleQuery invocation shape:
/// canned persona + agent_persona, empty identity + bootstrap, non-empty
/// skills_catalog, a single non-send tool, empty connector_status, no session
/// summary, and two recent messages. Since there's no `send` tool, the
/// `send_context` and `send_rules` blocks are NOT emitted.
#[test]
fn test_golden_simple_query_text_only_byte_identical() {
    use crate::prompt::PromptBuilder;
    use openalpaca_llm::{ChatMessage, Role};

    // ── Canned inputs mirroring the SimpleQuery pre-migration scene. ──
    let system_persona_val = SystemPersona {
        name: "OpenAlpaca".to_string(),
        core_values: vec![
            "Act as the user's trusted local AI agent".to_string(),
            "Provide structured output when asked".to_string(),
        ],
        safety_rules: vec![
            "Confirm before destructive actions".to_string(),
        ],
        base_instructions: "You are OpenAlpaca, a helpful local assistant.".to_string(),
    };
    let agent_persona_val = AgentPersona {
        role: "Assistant".to_string(),
        tone: "Concise and professional".to_string(),
        domain_knowledge: vec![],
    };
    let identity_block = ""; // No identity document.
    let bootstrap_block = ""; // No bootstrap document.
    let skills_catalog_str = "<skills_catalog>\n- some_skill\n</skills_catalog>";
    let source_text = "cli";
    let query = "hello world";
    let recent = vec![
        ChatMessage::user("previous turn"),
        ChatMessage::assistant("previous reply"),
    ];
    let tool_defs: Vec<ToolDefinition> = vec![ToolDefinition {
        name: "demo_tool".to_string(),
        description: "a demo tool".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "arg": { "type": "string" }
            }
        }),
        strict: None,
        input_examples: None,
    }];
    let model_window: usize = 200_000;

    // ── Reference: reconstruct pre-migration PromptBuilder chain. ──
    //
    // Matches orchestrator/query_handler/simple_query_handler.rs:196-248 for
    // the "no connectors, no send tool, empty identity, empty bootstrap,
    // non-empty skills_catalog" shape:
    //
    //   system_persona -> agent_persona -> identity -> bootstrap ->
    //   skills_catalog -> tools -> message_source -> context_bundle(empty).
    //
    // Conditionals omitted (no-send-tool path): connector_guidance, send_context,
    // send_rules.
    let built = PromptBuilder::new(model_window)
        .system_persona(&system_persona_val)
        .agent_persona(&agent_persona_val)
        .identity(identity_block)
        .bootstrap(bootstrap_block)
        .skills_catalog(skills_catalog_str)
        .tools(&tool_defs)
        .message_source(source_text)
        .context_bundle(&ContextBundle::empty())
        .build();
    let expected_system = built.system_message.clone();

    // Reference messages: [system, ...built.context_messages, ...recent, user(query)].
    // No summary in this scenario → skip.
    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + recent.len() + 1);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    expected_messages.extend(recent.iter().cloned());
    expected_messages.push(ChatMessage::user(query));

    // ── Via the engine: PersonaMode::Default + StaticPromptMode::Default. ──
    let engine = ComposeEngine::new(16);

    let persona_input = PersonaInput {
        system_persona: Arc::new(system_persona_val.clone()),
        // No user/identity documents for this golden scenario.
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: None,
    };
    // Pre-compute so StaticPromptInput has a real Arc; compose() will overwrite
    // on cache hit.
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: Some(Arc::new(agent_persona_val.clone())),
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: Some(Arc::<str>::from(skills_catalog_str)),
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: Some(Arc::<str>::from(source_text)),
        raw_blocks: Vec::new(),
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::Default,
        model_window: model_window as u32,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(query),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(recent.clone()),
        current_user_turn: Some(ChatMessage::user(query)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::SimpleQuery {
        lane_key: "simple_lane".to_string(),
        agent_persona: Arc::new(agent_persona_val.clone()),
        query: query.to_string(),
        current_parts: None,
        message_source: Arc::<str>::from(source_text),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion (field-by-field; ChatMessage lacks PartialEq). ──
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "SimpleQuery migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        // ContentPart doesn't derive PartialEq — assert both sides are None
        // (no multimodal content in this text-only scenario).
        assert!(
            got.parts.is_none(),
            "message {idx} got parts should be None"
        );
        assert!(
            expected.parts.is_none(),
            "message {idx} expected parts should be None"
        );
        // Same for tool_calls (no ToolCall PartialEq derive).
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert!(
            expected.tool_calls.is_none(),
            "message {idx} expected tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Sanity: first message is the system prompt, last is the current user turn.
    assert_eq!(composed.messages[0].role, Role::System);
    assert_eq!(composed.messages.last().unwrap().role, Role::User);
    assert_eq!(composed.messages.last().unwrap().content, query);
}

/// Golden-output: asserts the migrated SimpleQuery path (multimodal) produces
/// byte-identical `ComposedRequest.messages` for the aggregation logic of
/// Layer 4. Specifically: when `current_parts = Some(...)`, the final user
/// message in the composed output has `parts = Some(pre_adapted_parts)`.
///
/// Layer 4 remains pure. Per the migration plan, `adapt_parts_for_model` runs
/// loader-side BEFORE compose() is invoked, so in this golden test we simulate
/// that by passing the parts through as-is (identity adapter) — the
/// byte-identical assertion covers only Layer 4's aggregation of
/// `HistoryInput.current_user_turn` into the final messages vec, NOT the
/// adapter's transformation logic (which is covered by the orchestrator
/// integration tests at `orchestrator/tests.rs:1057`).
#[test]
fn test_golden_simple_query_multimodal_byte_identical() {
    use crate::prompt::PromptBuilder;
    use openalpaca_llm::{ChatMessage, ContentPart, ImageSource, Role};

    // ── Canned inputs mirroring the SimpleQuery pre-migration scene. ──
    let system_persona_val = SystemPersona {
        name: "OpenAlpaca".to_string(),
        core_values: vec![
            "Act as the user's trusted local AI agent".to_string(),
        ],
        safety_rules: vec![
            "Confirm before destructive actions".to_string(),
        ],
        base_instructions: "You are OpenAlpaca, a helpful local assistant.".to_string(),
    };
    let agent_persona_val = AgentPersona {
        role: "Assistant".to_string(),
        tone: "Concise and professional".to_string(),
        domain_knowledge: vec![],
    };
    let identity_block = "";
    let bootstrap_block = "";
    let skills_catalog_str = "<skills_catalog>\n- some_skill\n</skills_catalog>";
    let source_text = "cli";
    let query = "hello";
    let recent = vec![
        ChatMessage::user("previous turn"),
        ChatMessage::assistant("previous reply"),
    ];
    let tool_defs: Vec<ToolDefinition> = vec![];
    let model_window: usize = 200_000;

    // Multimodal parts (the loader would `sanitize_parts_for_dispatch` +
    // `adapt_parts_for_model` these before passing to compose; for this golden
    // we simulate an identity adapter so we can compare Layer 4's aggregation
    // byte-identically against what the pre-migration code produced for the
    // same pre-adapted input).
    let adapted_parts: Vec<ContentPart> = vec![
        ContentPart::Text { text: "hello".to_string() },
        ContentPart::Image {
            source: ImageSource::Url {
                url: "https://example.com/img.png".to_string(),
            },
            detail: None,
        },
    ];

    // ── Reference: reconstruct pre-migration PromptBuilder chain + manual
    // message construction. ──
    //
    // Matches orchestrator/query_handler/simple_query_handler.rs:259-307 for
    // the multimodal branch:
    //   messages.push(system)
    //   messages.extend(built.context_messages)
    //   (no summary)
    //   messages.extend(adapted_recent)
    //   messages.push(ChatMessage::user_with_parts(adapted_parts))
    let built = PromptBuilder::new(model_window)
        .system_persona(&system_persona_val)
        .agent_persona(&agent_persona_val)
        .identity(identity_block)
        .bootstrap(bootstrap_block)
        .skills_catalog(skills_catalog_str)
        // No tools in this scenario.
        .message_source(source_text)
        .context_bundle(&ContextBundle::empty())
        .build();
    let expected_system = built.system_message.clone();

    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + recent.len() + 1);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    expected_messages.extend(recent.iter().cloned());
    expected_messages.push(ChatMessage::user_with_parts(adapted_parts.clone()));

    // ── Via the engine ──
    let engine = ComposeEngine::new(16);

    let persona_input = PersonaInput {
        system_persona: Arc::new(system_persona_val.clone()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Default,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: Some(Arc::new(agent_persona_val.clone())),
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: Some(Arc::<str>::from(skills_catalog_str)),
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: Some(Arc::<str>::from(source_text)),
        raw_blocks: Vec::new(),
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::Default,
        model_window: model_window as u32,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(query),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    // The key test: current_user_turn uses `user_with_parts(...)` so Layer 4
    // carries the multimodal `parts` through byte-identically to the final
    // message.
    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(recent.clone()),
        current_user_turn: Some(ChatMessage::user_with_parts(adapted_parts.clone())),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::SimpleQuery {
        lane_key: "simple_lane".to_string(),
        agent_persona: Arc::new(agent_persona_val.clone()),
        query: query.to_string(),
        current_parts: Some(adapted_parts.clone()),
        message_source: Arc::<str>::from(source_text),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion (field-by-field) ──
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "SimpleQuery multimodal migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        assert_eq!(
            got.parts.is_some(),
            expected.parts.is_some(),
            "message {idx} parts presence mismatch"
        );
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert!(
            expected.tool_calls.is_none(),
            "message {idx} expected tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Explicit check: last message carries the pre-adapted parts.
    let last = composed.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    let last_parts = last.parts.as_ref().expect("last message must have parts");
    assert_eq!(last_parts.len(), adapted_parts.len());
    // First part is text "hello"
    match &last_parts[0] {
        ContentPart::Text { text } => assert_eq!(text, "hello"),
        other => panic!("expected Text part, got {:?}", other),
    }
    // Second part is image URL
    match &last_parts[1] {
        ContentPart::Image { source, detail } => {
            match source {
                ImageSource::Url { url } => assert_eq!(url, "https://example.com/img.png"),
                other => panic!("expected Url source, got {:?}", other),
            }
            assert!(detail.is_none());
        }
        other => panic!("expected Image part, got {:?}", other),
    }
    // Sanity: first message is the system prompt.
    assert_eq!(composed.messages[0].role, Role::System);
}

// ──────────────────────────────────────────────────────────────────────────
// Phase 6 Commit 1 — Pipeline Step migration golden-output test
// ──────────────────────────────────────────────────────────────────────────

/// Golden-output: asserts the migrated Pipeline Step path produces
/// byte-identical `ComposedRequest.messages` to what the pre-migration
/// `PromptBuilder` chain + manual message construction at
/// `orchestrator/dispatcher/pipeline_step.rs:341-453` would have produced.
///
/// Routing through `compose(ComposeRequest::PipelineStep{..})` with
/// `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal` +
/// `DynamicContextMode::Default` + `HistoryMode::Default` reproduces the
/// pre-migration PromptBuilder chain output byte-identically.
///
/// Scenario: step 0 of 2 (so step_description becomes the current_user_turn
/// and a non-empty memory block arrives as the immediately-preceding user
/// message — mirrors pre-migration `messages.push(ChatMessage::user(&block));
/// messages.push(ChatMessage::user(&pctx.description));`). Includes:
///   - `agent_identity`, `assignment`, `scope`, `output_format` raw_blocks
///   - Pre-rendered tools via `format_tool_guidance(...)` — pushed as a
///     raw_block named "tools" at the same position `.tools(...)` would have
///     placed it in the pre-migration fluent chain
///   - Non-empty connector_block → emitted as a raw_block named
///     "connector_guidance" (not via `.connector_guidance(...)` — pre-migration
///     code at pipeline_step.rs:362-368 also uses raw_system_block)
///   - ContextPackage with TaskDescription (SystemPrompt), PredecessorOutput
///     (UserMessage Untrusted), and WorkspaceArtifact (UserMessage Untrusted).
///     Migration excludes TaskDescription from the bundle it feeds to Layer 3
///     and pushes the equivalent content as a raw_block named "task_description"
///     at the end of raw_blocks (same position the pre-migration `build()`
///     rendered bundle's SystemPrompt section inline in system_parts).
#[test]
fn test_golden_pipeline_step_byte_identical() {
    use crate::middleware::prompt::format_tool_guidance;
    use crate::prompt::PromptBuilder;
    use crate::prompt_ctx::package::{AgentSummary, HandoffContext};
    use crate::prompt_ctx::{
        ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, PackageSection,
        PackageSectionKind, TrustLevel,
    };
    use openalpaca_llm::Role;

    // ── Canned pipeline-step scene ──────────────────────────────────────
    // Mirrors the pre-migration arguments at pipeline_step.rs:84-92. The
    // agent.preset.persona is canned, role_description is canned, step=0,
    // total_agents=2 (so is_last_step=false), previous_output=Some(...) so
    // HandoffContext exists, workspace_context=non-empty.
    let agent_persona_text = "I am a test agent persona.";
    let role_description = "Analyze the dataset for anomalies.";
    let step: usize = 0;
    let total_agents: usize = 2;
    let memory_block_text = "<memory>prior relevant fact</memory>";
    let previous_output_text = "Preliminary analysis complete.";
    let workspace_context_text = "workspace: dataset.csv\n";
    let connector_block_text =
        "<connector_status>\n- telegram: active\n</connector_status>";
    let description = "Find anomalies in sales data.";
    let model_window: usize = 200_000;

    // Pre-migration output_note branch for non-last step.
    let output_note =
        "Your output will be passed to the next agent in the pipeline. Be thorough and \
         include all relevant details so the next agent can build on your work.";

    // Canned tool definitions.
    let tool_defs: Vec<ToolDefinition> = vec![ToolDefinition {
        name: "analyze".to_string(),
        description: "Analyze a dataset".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } }
        }),
        strict: None,
        input_examples: None,
    }];

    // ── Pre-migration reference: PromptBuilder chain. ───────────────────
    let identity_block = format!("<identity>\n{}\n</identity>", agent_persona_text);
    let assignment_block = format!(
        "<assignment>\nRole: {}\nPipeline step: {} of {}\n</assignment>",
        role_description,
        step + 1,
        total_agents
    );
    let scope_block = "<scope>\nFocus on completing your assigned role. \
                        Do not attempt work outside your role description.\n</scope>";
    let output_block = format!("<output-format>\n{}\n</output-format>", output_note);

    // Pre-migration ContextPackage includes TaskDescription (SystemPrompt),
    // PredecessorOutput (UserMessage Untrusted), WorkspaceArtifact (UserMessage
    // Untrusted). We construct both the reference bundle (for the pre-migration
    // PromptBuilder chain) and the migration bundle (same sections minus
    // TaskDescription — the migration re-emits TaskDescription content as a
    // raw_block named "task_description").
    let handoff = HandoffContext {
        producer: AgentSummary {
            name: "unknown".to_string(),
            role: "pipeline_predecessor".to_string(),
            step: 0,
        },
        task_assigned: description.to_string(),
        output: previous_output_text.to_string(),
        decisions: vec![],
        handoff_notes: None,
    };
    let predecessor_content = handoff.format();
    let predecessor_tokens = handoff.token_estimate();

    let package_sections_ref = vec![
        PackageSection {
            kind: PackageSectionKind::TaskDescription,
            content: role_description.to_string(),
            token_estimate: role_description.len() / 4,
            priority: SectionPriority::Critical,
        },
        PackageSection {
            kind: PackageSectionKind::PredecessorOutput,
            content: predecessor_content.clone(),
            token_estimate: predecessor_tokens,
            priority: SectionPriority::High,
        },
        PackageSection {
            kind: PackageSectionKind::WorkspaceArtifact,
            content: workspace_context_text.to_string(),
            token_estimate: workspace_context_text.len() / 4,
            priority: SectionPriority::Normal,
        },
    ];
    let total_tokens_ref: usize =
        package_sections_ref.iter().map(|s| s.token_estimate).sum();
    let context_package_ref = crate::prompt_ctx::ContextPackage {
        sections: package_sections_ref,
        total_tokens: total_tokens_ref,
        budget: total_tokens_ref + 1000,
        sub_agent_window: model_window,
    };
    let bundle_ref = context_package_ref.to_bundle();

    let built = PromptBuilder::new(model_window)
        .raw_system_block("agent_identity", &identity_block, SectionPriority::High)
        .raw_system_block("assignment", &assignment_block, SectionPriority::Critical)
        .raw_system_block("scope", scope_block, SectionPriority::Normal)
        .raw_system_block("output_format", &output_block, SectionPriority::Normal)
        .tools(&tool_defs)
        .raw_system_block(
            "connector_guidance",
            connector_block_text,
            SectionPriority::Normal,
        )
        .context_bundle(&bundle_ref)
        .build();
    let expected_system = built.system_message.clone();

    // Reference messages: [system, ...built.context_messages, memory_user (step 0),
    // user(description)].
    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + 2);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    expected_messages.push(ChatMessage::user(memory_block_text));
    expected_messages.push(ChatMessage::user(description));

    // ── Via the engine: PersonaMode::Skip + StaticPromptMode::SubagentMinimal. ──
    //
    // Caller pre-renders tools via `format_tool_guidance(...)` and pushes the
    // result as raw_block "tools". Connector block is pushed as raw_block
    // "connector_guidance". Task-description text is pushed as raw_block
    // "task_description" at the end. The bundle fed to Layer 3 omits
    // TaskDescription (the migration's equivalent of inline-append-to-system
    // is the "task_description" raw_block).
    let engine = ComposeEngine::new(16);

    let tools_rendered = format_tool_guidance(&tool_defs);
    let raw_blocks_for_engine: Vec<SystemBlock> = vec![
        SystemBlock {
            name: "agent_identity",
            content: Arc::<str>::from(identity_block.clone()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "assignment",
            content: Arc::<str>::from(assignment_block.clone()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "scope",
            content: Arc::<str>::from(scope_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "output_format",
            content: Arc::<str>::from(output_block.clone()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "tools",
            content: Arc::<str>::from(tools_rendered),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "connector_guidance",
            content: Arc::<str>::from(connector_block_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "task_description",
            content: Arc::<str>::from(role_description.to_string()),
            priority: SectionPriority::Critical,
        },
    ];

    // Migration bundle: same PredecessorOutput + WorkspaceArtifact sections
    // as pre-migration, but TaskDescription omitted (it now lives in
    // raw_blocks as "task_description").
    let migration_bundle = ContextBundle {
        sections: vec![
            ContextSection {
                source: PackageSectionKind::PredecessorOutput.source_name(),
                kind: ContextKind::WorkspaceArtifact,
                content: predecessor_content.clone(),
                token_estimate: predecessor_tokens,
                priority: SectionPriority::High,
                relevance: 1.0,
                key: ContextKey::Package(
                    PackageSectionKind::PredecessorOutput.key_name(),
                ),
                injection: InjectionMode::UserMessage {
                    tag: "predecessor_output".to_string(),
                    trust: TrustLevel::Untrusted,
                },
            },
            ContextSection {
                source: PackageSectionKind::WorkspaceArtifact.source_name(),
                kind: ContextKind::WorkspaceArtifact,
                content: workspace_context_text.to_string(),
                token_estimate: workspace_context_text.len() / 4,
                priority: SectionPriority::Normal,
                relevance: 1.0,
                key: ContextKey::Package(
                    PackageSectionKind::WorkspaceArtifact.key_name(),
                ),
                injection: InjectionMode::UserMessage {
                    tag: "workspace_artifact".to_string(),
                    trust: TrustLevel::Untrusted,
                },
            },
        ],
        total_tokens: predecessor_tokens + workspace_context_text.len() / 4,
        available_budget: 1000,
    };

    // Construct a canned SubAgent for the request variant. Pre-migration
    // reads `agent.preset.persona`; we mirror that with the canned text.
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };
    let agent = Arc::new(SubAgent {
        id: "test-agent-01".to_string(),
        template_id: "test-agent".to_string(),
        name: "Test Agent".to_string(),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset {
            persona: agent_persona_text.to_string(),
            ..AgentPreset::default()
        },
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    });

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: raw_blocks_for_engine,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(migration_bundle),
        query: Arc::from(description),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::SimpleQuery,
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    // HistoryMode::Default: step-0 memory block arrives as a `recent_messages`
    // entry (comes BEFORE current_user_turn in Layer 4 Default's emission
    // order). current_user_turn = user(description) mirrors pre-migration's
    // final `messages.push(ChatMessage::user(&pctx.description))`.
    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
        recent_messages: Arc::new(vec![ChatMessage::user(memory_block_text)]),
        current_user_turn: Some(ChatMessage::user(description)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::PipelineStep {
        agent: agent.clone(),
        step_index: step,
        step_description: Arc::<str>::from(description),
        scope_block: Arc::<str>::from(scope_block),
        output_block: Arc::<str>::from(output_block.clone()),
        context_package: Arc::new(crate::prompt_ctx::ContextPackage {
            sections: vec![],
            total_tokens: 0,
            budget: 0,
            sub_agent_window: model_window,
        }),
        memory_block: Some(Arc::<str>::from(memory_block_text)),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion. ───────────────────────────────────────
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "Pipeline Step migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        assert!(
            got.parts.is_none(),
            "message {idx} got parts should be None"
        );
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Sanity: first message is system, last is user(description), and the
    // memory block lives at the index just before the description.
    assert_eq!(composed.messages[0].role, Role::System);
    let last = composed.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last.content, description);
    let second_to_last = &composed.messages[composed.messages.len() - 2];
    assert_eq!(second_to_last.role, Role::User);
    assert_eq!(second_to_last.content, memory_block_text);
}

/// Phase 6 Commit 2 — DAG Node golden fixture.
///
/// Asserts byte-identical `ComposedRequest.messages` between:
///   (a) pre-migration `PromptBuilder` chain at
///       `runner/dag_executor/node_runner.rs:85-215` and
///   (b) migrated `ComposeEngine::compose(ComposeRequest::DagNode{...})` with
///       `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal` +
///       `DynamicContextMode::Default` + `HistoryMode::Default`.
///
/// Scene mirrors the pre-migration DAG node with a non-empty
/// `connector_suffix`, a non-empty tool list, and a non-empty
/// `workspace_context`. Includes:
///   - `agent_identity`, `assignment`, `scope`, `output_format` raw_blocks
///   - Pre-rendered tools via `format_tool_guidance(...)` — pushed as a
///     raw_block named "tools" at the same position `.tools(...)` would have
///     placed it in the pre-migration fluent chain.
///   - Non-empty `connector_suffix` → emitted as a raw_block named
///     "connector_guidance".
///   - No `ContextPackage` routing through Layer 3 — DAG Node pre-migration
///     NEVER calls `.context_bundle()`, so the bundle fed to Layer 3 is
///     empty and emits zero messages/blocks.
///   - `task_description` (the overall task description, NOT the node
///     description) arrives as `HistoryInput.recent_messages[0]`.
///   - `workspace_context` (if non-empty) arrives as
///     `HistoryInput.current_user_turn` wrapped in the pre-migration
///     `<workspace>...</workspace>` envelope. Pre-migration injected it as
///     a plain user message (NOT through `wrap_untrusted_context`), so the
///     migration preserves that placement by constructing the wrapped string
///     loader-side and passing it as a plain `ChatMessage::user` on the
///     history input.
#[test]
fn test_golden_dag_node_byte_identical() {
    use crate::middleware::prompt::format_tool_guidance;
    use crate::prompt::PromptBuilder;
    use openalpaca_llm::Role;

    // ── Canned DAG-node scene. Mirrors the pre-migration arguments at
    //    runner/dag_executor/node_runner.rs:78-95 + 109-116 + 192-215. ─────
    let agent_persona_text = "I am a DAG node test agent persona.";
    let node_title = "ingest-data";
    let node_description = "Ingest the raw CSV and produce a cleaned row stream.";
    let task_description =
        "Analyze all sales data for Q1 and deliver an exception report.";
    let workspace_context_text =
        "## Shared Workspace\n\n### [q1_sales.csv] (by planner, type: Context)\ncol_a,col_b\n1,2\n\n";
    let connector_guidance_text =
        "<connector_status>\n- discord: active\n</connector_status>";
    let model_window: usize = 200_000;

    // Canned tool definitions (same single tool as Pipeline golden test).
    let tool_defs: Vec<ToolDefinition> = vec![ToolDefinition {
        name: "workspace_read".to_string(),
        description: "Read a workspace entry by key".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "key": { "type": "string" } }
        }),
        strict: None,
        input_examples: None,
    }];

    // ── Pre-migration reference: PromptBuilder chain at node_runner.rs:97-107. ──
    let identity_block = format!("<identity>\n{}\n</identity>", agent_persona_text);
    let assignment_block = format!(
        "<assignment>\nSub-task: {}\nDescription: {}\n</assignment>",
        node_title, node_description,
    );
    let scope_block = "<scope>\nYou are responsible for completing only the sub-task described above. \
        Do not attempt work outside your assignment. Your output will be stored \
        in the workspace for downstream nodes that depend on your results.\n</scope>";
    let output_block = "<output-format>\nProvide a complete, self-contained result for your sub-task. Other agents \
        will consume your output, so be specific and include all relevant details.\n</output-format>";

    // Pre-migration constructs `connector_suffix` as "\n" + connector_guidance
    // when the guidance is non-empty (see node_runner.rs:91-95). This only
    // occurs when connector_guidance is non-empty; otherwise the raw_block is
    // skipped entirely.
    let connector_suffix = format!("\n{}", connector_guidance_text);

    let built = PromptBuilder::new(model_window)
        .raw_system_block("agent_identity", &identity_block, SectionPriority::High)
        .raw_system_block("assignment", &assignment_block, SectionPriority::Critical)
        .raw_system_block("scope", scope_block, SectionPriority::Normal)
        .raw_system_block("output_format", output_block, SectionPriority::Normal)
        .tools(&tool_defs)
        .raw_system_block(
            "connector_guidance",
            &connector_suffix,
            SectionPriority::Normal,
        )
        .build();
    let expected_system = built.system_message.clone();

    // Pre-migration messages vec at node_runner.rs:193-215:
    //   [system, user(task_description), user(<workspace>...</workspace>)]
    // `built.context_messages` is empty because pre-migration DAG Node does
    // NOT call `.context_bundle()`.
    let workspace_wrapped = format!(
        "<workspace>\n\
         The following entries contain results from upstream sub-tasks. \
         Use this data to complete your assignment. You can also call \
         workspace_read and workspace_write to access or update entries.\n\n\
         {}\n\
         </workspace>",
        workspace_context_text
    );
    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + 2);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    expected_messages.push(ChatMessage::user(task_description));
    expected_messages.push(ChatMessage::user(&workspace_wrapped));

    // ── Via the engine: PersonaMode::Skip + StaticPromptMode::SubagentMinimal. ──
    //
    // Caller pre-renders tools via `format_tool_guidance(...)` and pushes the
    // result as raw_block "tools". Connector suffix is pushed as raw_block
    // "connector_guidance". The bundle fed to Layer 3 is empty (DAG Node has
    // no ContextPackage) so `DynamicContextMode::Default` emits nothing.
    //
    // `HistoryMode::Default` order: summary (None) → recent_messages →
    // current_user_turn. To match pre-migration byte-identically we put:
    //   - task_description → recent_messages[0]
    //   - workspace_wrapped → current_user_turn (when ws non-empty)
    let engine = ComposeEngine::new(16);

    let tools_rendered = format_tool_guidance(&tool_defs);
    let raw_blocks_for_engine: Vec<SystemBlock> = vec![
        SystemBlock {
            name: "agent_identity",
            content: Arc::<str>::from(identity_block.clone()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "assignment",
            content: Arc::<str>::from(assignment_block.clone()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "scope",
            content: Arc::<str>::from(scope_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "output_format",
            content: Arc::<str>::from(output_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "tools",
            content: Arc::<str>::from(tools_rendered),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "connector_guidance",
            content: Arc::<str>::from(connector_suffix.clone()),
            priority: SectionPriority::Normal,
        },
    ];

    // Canned SubAgent for the request variant.
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };
    let agent = Arc::new(SubAgent {
        id: "dag-test-agent-01".to_string(),
        template_id: "dag-test-agent".to_string(),
        name: "DAG Test Agent".to_string(),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset {
            persona: agent_persona_text.to_string(),
            ..AgentPreset::default()
        },
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    });

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: raw_blocks_for_engine,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    // Empty bundle — DAG Node has no ContextPackage flowing through Layer 3.
    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(task_description),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::DagNode {
            node_id: "node_1".to_string(),
        },
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    // task_description → recent_messages[0]; workspace_wrapped → current_user_turn.
    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(vec![ChatMessage::user(task_description)]),
        current_user_turn: Some(ChatMessage::user(&workspace_wrapped)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::DagNode {
        agent: agent.clone(),
        assignment: Arc::<str>::from(assignment_block.clone()),
        workspace_context: Arc::<str>::from(workspace_context_text),
        tools: Arc::new(tool_defs.clone()),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion. ───────────────────────────────────────
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "DAG Node migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        assert!(
            got.parts.is_none(),
            "message {idx} got parts should be None"
        );
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Sanity: first message is system, last is the workspace-wrapped user
    // message, and the task_description lives at the index just before the
    // workspace wrap.
    assert_eq!(composed.messages[0].role, Role::System);
    let last = composed.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last.content, workspace_wrapped);
    let second_to_last = &composed.messages[composed.messages.len() - 2];
    assert_eq!(second_to_last.role, Role::User);
    assert_eq!(second_to_last.content, task_description);
}

/// Phase 6 Commit 3 — Lead Agent (build_lead_agent_prompt_from_templates) golden fixture.
///
/// Asserts byte-identical system message content between:
///   (a) pre-migration `PromptBuilder` chain at
///       `runner/lead_agent/prompt.rs:12-138` and
///   (b) migrated `ComposeEngine::compose(ComposeRequest::LeadAgent{...})` with
///       `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal` +
///       `DynamicContextMode::Skip` + `HistoryMode::Skip`.
///
/// `build_lead_agent_prompt_from_templates` returns ONLY the system prompt
/// string; the caller at `runner/lead_agent/mod.rs:217` then concatenates
/// `format_tool_guidance` + `connector_suffix` inline before building the
/// message vec. The migration therefore only needs to preserve the system
/// message content emitted by the 8 raw_blocks — no tools, no connector
/// guidance, no context bundle, no history flow through this call site.
#[test]
fn test_golden_lead_agent_byte_identical() {
    use crate::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
    use crate::prompt::PromptBuilder;
    use openalpaca_llm::Role;
    use std::collections::HashMap;
    use crate::prompt_ctx::SectionPriority;

    // ── Canned Lead Agent scene. Mirrors the pre-migration arguments at
    //    runner/lead_agent/prompt.rs:12-138 + runner/lead_agent/mod.rs:217. ──
    let base_persona = "I am a Lead Agent persona for the golden test.";
    let model_window: usize = 200_000;

    // Two worker-agent templates so `<agents>` block is non-empty.
    let templates = vec![
        AgentTemplate {
            frontmatter: AgentTemplateFrontmatter {
                id: "researcher".to_string(),
                name: "Researcher".to_string(),
                description: "Does research and fact-finding.".to_string(),
                icon: None,
                singleton: false,
                capabilities: vec!["web_search".to_string(), "read_docs".to_string()],
                denied_capabilities: vec![],
                temperature: 0.5,
                verbosity: "normal".to_string(),
                model: None,
                fallback_models: vec![],
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: None,
                max_rounds: None,
                require_confirmation_for: vec![],
            },
            body: String::new(),
            sections: HashMap::new(),
            source: AgentSource::default(),
        },
        AgentTemplate {
            frontmatter: AgentTemplateFrontmatter {
                id: "coder".to_string(),
                name: "Coder".to_string(),
                description: "Writes production Rust.".to_string(),
                icon: None,
                singleton: false,
                capabilities: vec![],
                denied_capabilities: vec![],
                temperature: 0.5,
                verbosity: "normal".to_string(),
                model: None,
                fallback_models: vec![],
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: None,
                max_rounds: None,
                require_confirmation_for: vec![],
            },
            body: String::new(),
            sections: HashMap::new(),
            source: AgentSource::default(),
        },
    ];

    // ── Pre-migration reference: PromptBuilder chain at prompt.rs:41-135. ──
    // Rebuild the `<agents>` block the same way `build_lead_agent_prompt_from_templates`
    // does — including the `capabilities=[...]` rendering.
    let mut agents_block = String::from("<agents>\n");
    for t in &templates {
        let fm = &t.frontmatter;
        let capabilities_str = if fm.capabilities.is_empty() {
            "none".to_string()
        } else {
            fm.capabilities.join(", ")
        };
        agents_block.push_str(&format!(
            "- id=\"{}\" name=\"{}\" capabilities=[{}]: {}\n",
            fm.id, fm.name, capabilities_str, fm.description
        ));
    }
    agents_block.push_str("</agents>");

    // Copy the 5 other raw_block string constants VERBATIM from
    // runner/lead_agent/prompt.rs:46-134.
    let role_text = "<role>\n\
         You are a Lead Agent orchestrating a complex task. You are responsible for analyzing \
         the user's request, decomposing it into sub-objectives, delegating work to specialized \
         subagents, and synthesizing their results into a final response.\n\
         Do not attempt to perform specialized work (coding, research, analysis) yourself when \
         a suitable subagent is available. Your value is in orchestration and synthesis.\n\
         </role>";
    let workflow_text = "<workflow>\n\
         Step 1: Analyze the user's request. Identify the core goal and any constraints.\n\
         Step 2: Decompose into sub-objectives. Each sub-objective should map to one subagent.\n\
         Step 3: Spawn ALL subagents for independent objectives in a single round. Match each \
         sub-objective to the best agent by skills. Spawning is always immediate — the system \
         automatically manages execution ordering based on available LLM capacity. Subagents may \
         be queued if capacity is limited — this is handled automatically and transparently.\n\
         Step 4: Collect results. Call wait_for_subagents to block until all complete (including \
         queued ones), or check_subagent_status for individual progress.\n\
         Step 5: Evaluate and iterate. If a subagent failed or produced incomplete results, \
         retry with an adjusted objective or a different agent.\n\
         Step 6: Synthesize. Combine all subagent outputs into a coherent final response \
         that directly addresses the user's original request.\n\
         </workflow>";
    let delegation_text = "<delegation-criteria>\n\
         Spawn subagents when:\n\
         - Tasks can run in parallel (e.g., research + implementation are independent)\n\
         - Tasks require isolated context or specialized skills\n\
         - Tasks involve independent workstreams that do not need shared state\n\n\
         Work directly (do NOT spawn) when:\n\
         - The task is simple enough to answer from your own knowledge\n\
         - You are synthesizing, summarizing, or formatting existing results\n\
         - The task requires maintaining context across sequential steps that one agent handles best\n\
         </delegation-criteria>";
    let tool_patterns_text = "<tools>\n\
         spawn_subagent: Spawning is always immediate — returns a run_id instantly. The system \
         automatically queues execution if LLM capacity is limited. Spawn all independent \
         objectives in a single round before waiting — this is the preferred pattern.\n\
         spawn_subagents_batch: When spawning 3+ independent subagents, use spawn_subagents_batch \
         for parallel spawning instead of individual spawn_subagent calls. This is more efficient \
         and reduces round-trips.\n\
         check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
         queued, running, completed, or failed.\n\
         wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
         queued for execution. Returns a summary of all results. Call this after spawning all \
         subagents.\n\
         workspace_read / workspace_write: Share context between subagents. Write setup data before spawning; \
         read results after completion.\n\
         </tools>";
    let failure_recovery_text = "<failure-recovery>\n\
         If a subagent fails:\n\
         1. Read the error message to understand the failure type.\n\
         2. If the objective was too broad, split it into smaller sub-objectives and retry.\n\
         3. If the agent lacked the right skills, try a different agent.\n\
         4. If repeated failures occur, complete that sub-objective directly yourself.\n\
         5. Never silently drop a failed sub-objective — always report what succeeded and what did not.\n\
         </failure-recovery>";
    let output_expectations_text = "<output>\n\
         Your final response must directly address the user's original request. \
         Synthesize all subagent results into a single coherent answer. \
         Do not simply list raw subagent outputs — integrate, summarize, and resolve any conflicts.\n\
         </output>";

    let built = PromptBuilder::new(model_window)
        .raw_system_block("lead_persona", base_persona, SectionPriority::Critical)
        .raw_system_block("lead_role", role_text, SectionPriority::Critical)
        .raw_system_block("agents_catalog", &agents_block, SectionPriority::High)
        .raw_system_block("workflow", workflow_text, SectionPriority::High)
        .raw_system_block("delegation_criteria", delegation_text, SectionPriority::Normal)
        .raw_system_block("tool_patterns", tool_patterns_text, SectionPriority::Normal)
        .raw_system_block("failure_recovery", failure_recovery_text, SectionPriority::Normal)
        .raw_system_block(
            "output_expectations",
            output_expectations_text,
            SectionPriority::Normal,
        )
        .build();
    let expected_system = built.system_message.clone();

    // ── Via the engine: PersonaMode::Skip + StaticPromptMode::SubagentMinimal. ──
    //
    // The 8 raw_blocks go through StaticPromptInput.raw_blocks in canonical
    // registration order. No tools, no connector_guidance, no context_bundle,
    // no history are threaded through — build_lead_agent_prompt_from_templates
    // returns only the system prompt string.
    let engine = ComposeEngine::new(16);

    let raw_blocks_for_engine: Vec<SystemBlock> = vec![
        SystemBlock {
            name: "lead_persona",
            content: Arc::<str>::from(base_persona.to_string()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "lead_role",
            content: Arc::<str>::from(role_text.to_string()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "agents_catalog",
            content: Arc::<str>::from(agents_block.clone()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "workflow",
            content: Arc::<str>::from(workflow_text.to_string()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "delegation_criteria",
            content: Arc::<str>::from(delegation_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "tool_patterns",
            content: Arc::<str>::from(tool_patterns_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "failure_recovery",
            content: Arc::<str>::from(failure_recovery_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "output_expectations",
            content: Arc::<str>::from(output_expectations_text.to_string()),
            priority: SectionPriority::Normal,
        },
    ];

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::new()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: raw_blocks_for_engine,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    // No context bundle, no history — build_lead_agent_prompt_from_templates
    // returns ONLY the system prompt string.
    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(""),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::LeadAgent,
        reserved_tokens: 0,
        mode: DynamicContextMode::Skip,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: None,
        mode: HistoryMode::Skip,
    };

    let request = ComposeRequest::LeadAgent {
        base_persona: Arc::<str>::from(base_persona.to_string()),
        agents_catalog: Arc::<str>::from(agents_block.clone()),
        objective: Arc::<str>::from(""),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(Vec::new()),
        None,
        None,
    );

    // ── Byte-identical assertion on system message content. ─────────────
    assert_eq!(
        composed.messages.len(),
        1,
        "Lead Agent prompt should yield exactly 1 system message (got {} messages)",
        composed.messages.len()
    );
    assert_eq!(composed.messages[0].role, Role::System);
    assert_eq!(
        composed.messages[0].content, expected_system,
        "Lead Agent migration system_message mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
        composed.messages[0].content, expected_system
    );
}

/// Phase 6 Commit 3 — Lead Agent spawn_subagent tool golden fixture.
///
/// Asserts byte-identical `ComposedRequest.messages` between:
///   (a) pre-migration `PromptBuilder` chain at
///       `runner/lead_agent/tools.rs:280-315` and
///   (b) migrated `ComposeEngine::compose(ComposeRequest::DagNode{...})` with
///       `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal` +
///       `DynamicContextMode::Default` + `HistoryMode::Default`.
///
/// Scene mirrors the pre-migration `SpawnSubagentTool::execute` path:
///   - 4 raw_blocks (agent_identity, scope, output_format, constraints)
///   - `.tools(&tools)` — pre-rendered via format_tool_guidance
///   - `.context_bundle(&bundle)` — emits via Layer 3
///   - messages vec = [system, ...context_messages, user(objective)]
///
/// The pre-migration call path reuses `ComposeRequest::DagNode` because the
/// shape is a perfect match: raw_blocks + tools + context_bundle + objective
/// as the current user turn.
#[test]
fn test_golden_lead_agent_spawn_subagent_byte_identical() {
    use crate::middleware::prompt::format_tool_guidance;
    use crate::prompt::PromptBuilder;
    use crate::prompt_ctx::package::{PackageSection, PackageSectionKind};
    use openalpaca_llm::Role;

    // ── Canned spawn_subagent scene. Mirrors the pre-migration arguments
    //    at runner/lead_agent/tools.rs:280-315. ─────────────────────────
    let agent_persona_text = "I am a subagent test persona.";
    let objective = "Gather facts about the target system.";
    let model_window: usize = 200_000;

    // Canned tool set — `resolve_agent_tools` result.
    let tool_defs: Vec<ToolDefinition> = vec![ToolDefinition {
        name: "memory_search".to_string(),
        description: "Search user memories".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } }
        }),
        strict: None,
        input_examples: None,
    }];

    // Canned ContextPackage — simulates `context_manager.distill(...)` output.
    // One WorkspaceArtifact section → routed as UserMessage Untrusted.
    let workspace_content = "Shared notes about target system.";
    let package_sections = vec![PackageSection {
        kind: PackageSectionKind::WorkspaceArtifact,
        content: workspace_content.to_string(),
        token_estimate: workspace_content.len() / 4,
        priority: SectionPriority::Normal,
    }];
    let context_package = crate::prompt_ctx::ContextPackage {
        sections: package_sections,
        total_tokens: workspace_content.len() / 4,
        budget: 0,
        sub_agent_window: model_window,
    };
    let bundle = context_package.to_bundle();

    // ── Pre-migration reference: PromptBuilder chain at tools.rs:281-310. ──
    let identity_block = format!("<identity>\n{}\n</identity>", agent_persona_text);
    let scope_block = "<scope>\nYou are a subagent working on a single objective assigned by a lead agent. \
                 Focus exclusively on your assigned objective. Do not attempt work outside your scope.\n</scope>";
    let output_block = "<output-format>\nProvide a clear, complete result. Start with a brief summary of what you accomplished, \
                 followed by the detailed output. The lead agent will use your result to synthesize a \
                 final response, so be thorough and specific.\n</output-format>";
    let constraints_block = "<constraints>\nYou operate independently — you cannot communicate with other subagents directly. \
                 Use workspace_read and workspace_write tools to access or share data across agents.\n</constraints>";

    let built = PromptBuilder::new(model_window)
        .raw_system_block("agent_identity", &identity_block, SectionPriority::High)
        .raw_system_block("scope", scope_block, SectionPriority::Normal)
        .raw_system_block("output_format", output_block, SectionPriority::Normal)
        .raw_system_block("constraints", constraints_block, SectionPriority::Normal)
        .tools(&tool_defs)
        .context_bundle(&bundle)
        .build();
    let expected_system = built.system_message.clone();

    // Pre-migration messages vec at tools.rs:312-315:
    //   [system, ...context_messages (from context_bundle), user(objective)]
    let mut expected_messages: Vec<ChatMessage> =
        Vec::with_capacity(1 + built.context_messages.len() + 1);
    expected_messages.push(ChatMessage::system(&expected_system));
    expected_messages.extend(built.context_messages.iter().cloned());
    expected_messages.push(ChatMessage::user(objective));

    // ── Via the engine: PersonaMode::Skip + StaticPromptMode::SubagentMinimal. ──
    //
    // Caller pre-renders tools via `format_tool_guidance(...)` and pushes the
    // result as raw_block "tools". The bundle routes through Layer 3
    // (DynamicContextMode::Default). Objective is the current_user_turn.
    let engine = ComposeEngine::new(16);

    let tools_rendered = format_tool_guidance(&tool_defs);
    let raw_blocks_for_engine: Vec<SystemBlock> = vec![
        SystemBlock {
            name: "agent_identity",
            content: Arc::<str>::from(identity_block.clone()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "scope",
            content: Arc::<str>::from(scope_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "output_format",
            content: Arc::<str>::from(output_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "constraints",
            content: Arc::<str>::from(constraints_block.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "tools",
            content: Arc::<str>::from(tools_rendered),
            priority: SectionPriority::Normal,
        },
    ];

    // Canned SubAgent for the request variant. spawn_subagent uses
    // ComposeRequest::DagNode (shape-equivalent: raw_blocks + tools + bundle).
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent,
    };
    let agent = Arc::new(SubAgent {
        id: "spawn-subagent-test-01".to_string(),
        template_id: "spawn-subagent-test".to_string(),
        name: "Spawn Subagent Test".to_string(),
        description: None,
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset {
            persona: agent_persona_text.to_string(),
            ..AgentPreset::default()
        },
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    });

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(super::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(tool_defs.clone()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks: raw_blocks_for_engine,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    // Bundle flows through Layer 3 (DynamicContextMode::Default) — same
    // routing as PromptBuilder::context_bundle.
    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(bundle),
        query: Arc::from(objective),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::LeadAgent,
        reserved_tokens: 0,
        mode: DynamicContextMode::Default,
    };

    // objective → current_user_turn; no summary/recent.
    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: Some(ChatMessage::user(objective)),
        mode: HistoryMode::Default,
    };

    let request = ComposeRequest::DagNode {
        agent: agent.clone(),
        assignment: Arc::<str>::from(objective.to_string()),
        workspace_context: Arc::<str>::from(""),
        tools: Arc::new(tool_defs.clone()),
        overrides: ComposeOverrides::default(),
    };

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(tool_defs.clone()),
        None,
        None,
    );

    // ── Byte-identical assertion. ───────────────────────────────────────
    assert_eq!(
        composed.messages.len(),
        expected_messages.len(),
        "spawn_subagent migration produced wrong message count: got {} vs expected {}",
        composed.messages.len(),
        expected_messages.len()
    );
    for (idx, (got, expected)) in composed
        .messages
        .iter()
        .zip(expected_messages.iter())
        .enumerate()
    {
        assert_eq!(
            got.role, expected.role,
            "message {idx} role mismatch: got {:?} expected {:?}",
            got.role, expected.role
        );
        assert_eq!(
            got.content, expected.content,
            "message {idx} content mismatch:\n--- GOT ---\n{}\n--- EXPECTED ---\n{}\n",
            got.content, expected.content
        );
        assert!(
            got.parts.is_none(),
            "message {idx} got parts should be None"
        );
        assert!(
            got.tool_calls.is_none(),
            "message {idx} got tool_calls should be None"
        );
        assert_eq!(
            got.tool_call_id, expected.tool_call_id,
            "message {idx} tool_call_id mismatch"
        );
    }

    // Sanity: first message is system, last is user(objective).
    assert_eq!(composed.messages[0].role, Role::System);
    let last = composed.messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    assert_eq!(last.content, objective);
}

// ═══════════════════════════════════════════════════════════════════
// Task 2: hash_agent_config
// ═══════════════════════════════════════════════════════════════════

fn make_agent_for_hash_test(persona: &str) -> crate::agent::subagent::SubAgent {
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, SubAgent};

    SubAgent {
        id: "test_agent".to_string(),
        template_id: "test_agent".to_string(),
        name: "Test Agent".to_string(),
        description: Some("A test agent.".to_string()),
        icon: None,
        status: AgentStatus::Idle,
        current_task: None,
        capabilities: vec![],
        preset: AgentPreset {
            persona: persona.to_string(),
            temperature: 0.7,
            verbosity: "normal".to_string(),
        },
        constraints: AgentConstraints::default(),
        llm_config: AgentLlmConfig::default(),
    }
}

#[test]
fn test_hash_agent_config_deterministic() {
    let a1 = make_agent_for_hash_test("You are a coder.");
    let a2 = make_agent_for_hash_test("You are a coder.");
    assert_eq!(
        super::fingerprint::hash_agent_config(&a1),
        super::fingerprint::hash_agent_config(&a2),
        "hash_agent_config must be deterministic"
    );
}

#[test]
fn test_hash_agent_config_busts_on_persona_change() {
    let a1 = make_agent_for_hash_test("You are a coder.");
    let a2 = make_agent_for_hash_test("You are a writer.");
    assert_ne!(
        super::fingerprint::hash_agent_config(&a1),
        super::fingerprint::hash_agent_config(&a2),
        "preset.persona differences must bust the agent_config fingerprint"
    );
}

// ═══════════════════════════════════════════════════════════════════
// Task 3: Layer 1 MissReason attribution
// ═══════════════════════════════════════════════════════════════════

/// Minimal PersonaInput with mode + version controllable. Defaults keep all
/// other fields at their neutral values (None, version 0).
fn make_persona_input_with_mode(mode: PersonaMode, version: u64) -> PersonaInput {
    PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(Option::<IdentityDocument>::None),
        persona_version: version,
        mode,
        identity_budget: None,
        user_budget: None,
    }
}

#[test]
fn test_l1_miss_firstbuild_on_cold_cache() {
    let engine = ComposeEngine::new(16);
    let input = make_persona_input_with_mode(PersonaMode::Default, 0);
    let result = engine.lookup_or_build_persona(&input, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::FirstBuild));
}

#[test]
fn test_l1_miss_attributes_persona_version_change() {
    let engine = ComposeEngine::new(16);
    let a = make_persona_input_with_mode(PersonaMode::Default, 1);
    let b = make_persona_input_with_mode(PersonaMode::Default, 2);
    let _ = engine.lookup_or_build_persona(&a, None);
    let result = engine.lookup_or_build_persona(&b, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::PersonaChanged));
}

#[test]
fn test_l1_miss_attributes_identity_budget_change() {
    let engine = ComposeEngine::new(16);
    let mut a = make_persona_input_with_mode(PersonaMode::Default, 0);
    a.identity_budget = Some(300);
    let mut b = make_persona_input_with_mode(PersonaMode::Default, 0);
    b.identity_budget = Some(500);
    let _ = engine.lookup_or_build_persona(&a, None);
    let result = engine.lookup_or_build_persona(&b, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::IdentityBudgetChanged));
}

#[test]
fn test_l1_miss_attributes_user_budget_change() {
    let engine = ComposeEngine::new(16);
    let mut a = make_persona_input_with_mode(PersonaMode::Default, 0);
    a.user_budget = Some(1000);
    let mut b = make_persona_input_with_mode(PersonaMode::Default, 0);
    b.user_budget = Some(2000);
    let _ = engine.lookup_or_build_persona(&a, None);
    let result = engine.lookup_or_build_persona(&b, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::UserBudgetChanged));
}

#[test]
fn test_l1_miss_attributes_mode_change() {
    let engine = ComposeEngine::new(16);
    let a = make_persona_input_with_mode(PersonaMode::Default, 0);
    let b = make_persona_input_with_mode(PersonaMode::Minimal, 0);
    let _ = engine.lookup_or_build_persona(&a, None);
    let result = engine.lookup_or_build_persona(&b, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::ModeChanged));
}

#[test]
fn test_l1_hit_has_no_miss_reason() {
    let engine = ComposeEngine::new(16);
    let input = make_persona_input_with_mode(PersonaMode::Default, 0);
    let _ = engine.lookup_or_build_persona(&input, None);
    let result = engine.lookup_or_build_persona(&input, None);
    assert!(result.hit);
    assert_eq!(result.miss_reason, None);
}

// ═══════════════════════════════════════════════════════════════════
// Task 3: Layer 2 MissReason attribution
// ═══════════════════════════════════════════════════════════════════

/// Build a default `StaticPromptInput` using the canonical empty-inputs
/// helper. Returns the second tuple element.
fn make_default_static_prompt_input() -> StaticPromptInput {
    let (_, s, _, _) = empty_compose_inputs();
    s
}

/// Seed the L2 cache with `first`, then probe with `second`. Panics if the
/// second lookup was a hit (caller must ensure the two inputs bust the
/// fingerprint).
fn seed_l2_and_probe(
    engine: &ComposeEngine,
    first: StaticPromptInput,
    second: StaticPromptInput,
) -> Option<MissReason> {
    let _ = engine.lookup_or_build_static_prompt(&first, None);
    let result = engine.lookup_or_build_static_prompt(&second, None);
    assert!(!result.hit);
    result.miss_reason
}

#[test]
fn test_l2_miss_attributes_tools_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.tools = Arc::new(vec![ToolDefinition {
        name: "tool_a".to_string(),
        description: "A".to_string(),
        parameters: serde_json::json!({}),
        strict: None,
        input_examples: None,
    }]);
    let mut b = base.clone();
    b.tools = Arc::new(vec![ToolDefinition {
        name: "tool_b".to_string(),
        description: "B".to_string(),
        parameters: serde_json::json!({}),
        strict: None,
        input_examples: None,
    }]);
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::ToolsChanged)
    );
}

#[test]
fn test_l2_miss_attributes_bootstrap_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.bootstrap = Some(Arc::<str>::from("bootstrap v1"));
    let mut b = base.clone();
    b.bootstrap = Some(Arc::<str>::from("bootstrap v2"));
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::BootstrapChanged)
    );
}

#[test]
fn test_l2_miss_attributes_mode_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.mode = StaticPromptMode::Default;
    let mut b = base.clone();
    b.mode = StaticPromptMode::SubagentMinimal;
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::ModeChanged)
    );
}

#[test]
fn test_l2_miss_attributes_model_window_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.model_window = 8192;
    let mut b = base.clone();
    b.model_window = 200_000;
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::ModelWindowChanged)
    );
}

#[test]
fn test_l2_miss_attributes_persona_change() {
    // Changing persona_output.fingerprint propagates via persona_fp sub-field.
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    let mut persona_out = (*a.persona_output).clone();
    persona_out.fingerprint = [1u8; 32];
    a.persona_output = Arc::new(persona_out);
    let mut b = base.clone();
    let mut persona_out = (*b.persona_output).clone();
    persona_out.fingerprint = [2u8; 32];
    b.persona_output = Arc::new(persona_out);
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::PersonaChanged)
    );
}

#[test]
fn test_l2_miss_attributes_agent_config_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.agent_config_fingerprint = [1u8; 32];
    let mut b = base.clone();
    b.agent_config_fingerprint = [2u8; 32];
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::AgentConfigChanged)
    );
}

#[test]
fn test_l2_miss_attributes_raw_blocks_change() {
    let engine = ComposeEngine::new(16);
    let base = make_default_static_prompt_input();
    let mut a = base.clone();
    a.raw_blocks = vec![];
    let mut b = base.clone();
    b.raw_blocks = vec![SystemBlock {
        name: "new_block",
        content: Arc::<str>::from("content"),
        priority: SectionPriority::Normal,
    }];
    assert_eq!(
        seed_l2_and_probe(&engine, a, b),
        Some(MissReason::RawBlocksChanged)
    );
}

#[test]
fn test_l2_miss_firstbuild_on_cold_cache() {
    let engine = ComposeEngine::new(16);
    let input = make_default_static_prompt_input();
    let result = engine.lookup_or_build_static_prompt(&input, None);
    assert!(!result.hit);
    assert_eq!(result.miss_reason, Some(MissReason::FirstBuild));
}

// ═══════════════════════════════════════════════════════════════════
// Task 4: Per-lane cache activation
// ═══════════════════════════════════════════════════════════════════

fn make_minimal_lane() -> Arc<crate::lane::ConversationLane> {
    use crate::lane::{ConversationLane, LaneKey};
    Arc::new(ConversationLane::new(LaneKey::new("test_user", "test_source")))
}

#[test]
fn test_per_lane_cache_hit_on_simple_query_second_call() {
    let engine = ComposeEngine::new(16);
    let lane = make_minimal_lane();
    let (persona_input, static_prompt_input, dynamic_context_input, mut history_input) =
        empty_compose_inputs();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let request = ComposeRequest::SimpleQuery {
        lane_key: "test_lane".to_string(),
        agent_persona: Arc::new(AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise".to_string(),
            domain_knowledge: vec![],
        }),
        query: "first".to_string(),
        current_parts: None,
        message_source: Arc::<str>::from("test"),
        overrides: ComposeOverrides::default(),
    };

    let composed_a = engine.compose(
        &request,
        persona_input.clone(),
        static_prompt_input.clone(),
        dynamic_context_input.clone(),
        history_input.clone(),
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(!composed_a.layer_trace.memo_hits.dynamic_context);
    assert!(!composed_a.layer_trace.memo_hits.history);

    let composed_b = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(composed_b.layer_trace.memo_hits.dynamic_context);
    assert!(composed_b.layer_trace.memo_hits.history);
}

#[test]
fn test_per_lane_cache_busts_on_tip_advance_simple_query() {
    let engine = ComposeEngine::new(16);
    let lane = make_minimal_lane();
    let (persona_input, static_prompt_input, dynamic_context_input, mut history_input) =
        empty_compose_inputs();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let request = ComposeRequest::SimpleQuery {
        lane_key: "test_lane".to_string(),
        agent_persona: Arc::new(AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise".to_string(),
            domain_knowledge: vec![],
        }),
        query: "first".to_string(),
        current_parts: None,
        message_source: Arc::<str>::from("test"),
        overrides: ComposeOverrides::default(),
    };

    let _ = engine.compose(
        &request,
        persona_input.clone(),
        static_prompt_input.clone(),
        dynamic_context_input.clone(),
        history_input.clone(),
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );

    lane.record_message();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(!composed.layer_trace.memo_hits.history, "L4 must bust when tip advances");
    assert!(composed.layer_trace.memo_hits.dynamic_context, "L3 stays hit");
}

#[test]
fn test_per_lane_cache_hit_on_social_second_call() {
    let engine = ComposeEngine::new(16);
    let lane = make_minimal_lane();
    let (persona_input, static_prompt_input, dynamic_context_input, mut history_input) =
        empty_compose_inputs();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let request = ComposeRequest::Social {
        lane_key: "test_lane".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };

    let composed_a = engine.compose(
        &request,
        persona_input.clone(),
        static_prompt_input.clone(),
        dynamic_context_input.clone(),
        history_input.clone(),
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(!composed_a.layer_trace.memo_hits.dynamic_context);
    assert!(!composed_a.layer_trace.memo_hits.history);

    let composed_b = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(composed_b.layer_trace.memo_hits.dynamic_context);
    assert!(composed_b.layer_trace.memo_hits.history);
}

#[test]
fn test_per_lane_cache_busts_on_tip_advance_social() {
    let engine = ComposeEngine::new(16);
    let lane = make_minimal_lane();
    let (persona_input, static_prompt_input, dynamic_context_input, mut history_input) =
        empty_compose_inputs();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let request = ComposeRequest::Social {
        lane_key: "test_lane".to_string(),
        query: "hi".to_string(),
        overrides: ComposeOverrides::default(),
    };

    let _ = engine.compose(
        &request,
        persona_input.clone(),
        static_prompt_input.clone(),
        dynamic_context_input.clone(),
        history_input.clone(),
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );

    lane.record_message();
    history_input.lane_tip_fingerprint = lane.compute_tip_fingerprint();

    let composed = engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        8192,
        Arc::new(Vec::new()),
        None,
        Some(&lane),
    );
    assert!(!composed.layer_trace.memo_hits.history, "L4 must bust when tip advances");
    assert!(composed.layer_trace.memo_hits.dynamic_context, "L3 stays hit");
}
