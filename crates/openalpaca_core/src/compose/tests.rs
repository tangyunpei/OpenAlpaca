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

    // LeadAgent: Minimal + Default + Skip + Skip.
    let req = ComposeRequest::LeadAgent {
        base_persona: Arc::<str>::from("base"),
        agents_catalog: Arc::<str>::from("catalog"),
        objective: Arc::<str>::from("obj"),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Minimal);
    assert_eq!(sp, S::Default);
    assert_eq!(dc, D::Skip);
    assert!(matches!(h, H::Skip));
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
