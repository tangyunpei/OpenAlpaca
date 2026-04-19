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

    // Replanner: same modes as Planner.
    let req = ComposeRequest::Replanner {
        current_plan: Arc::new(PlanState::default()),
        workspace_snapshot: Arc::new(WorkspaceSnapshot::default()),
        overrides: ComposeOverrides::default(),
    };
    let (p, sp, dc, h) = req.default_modes();
    assert_eq!(p, P::Minimal);
    assert_eq!(sp, S::PlannerHierarchical);
    assert_eq!(dc, D::Skip);
    assert!(matches!(h, H::Skip));

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
    // Planner mode calls task_planner::prompt::build_hierarchical_prompt,
    // which takes a list of idle agents — populate via the new planner_agents
    // field.
    input.planner_agents = Some(Arc::new(vec![]));
    let out = super::static_prompt::compute(&input);
    // Must produce a non-empty system message (specific structure asserted
    // in the Phase 4 Planner migration golden test).
    assert!(
        !out.system_message.is_empty(),
        "PlannerHierarchical mode should produce a non-empty system message"
    );
    // build_hierarchical_prompt always opens with "You are a task planner".
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
    );

    let mut persona_miss = false;
    let mut static_miss = false;
    // Drain what's buffered. Use try_recv so the test never hangs.
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::ComposeLayerCacheMiss { layer, .. } => match layer {
                LayerId::Persona => persona_miss = true,
                LayerId::StaticPrompt => static_miss = true,
                _ => {}
            },
            SystemEvent::ComposeLayerCacheHit { .. } => {
                panic!("first call should not emit Hit events")
            }
            _ => {}
        }
    }
    assert!(persona_miss, "first call should emit a Persona Miss");
    assert!(static_miss, "first call should emit a StaticPrompt Miss");

    // Second call — expect 2 Hit events.
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
    );

    let mut persona_hit = false;
    let mut static_hit = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            SystemEvent::ComposeLayerCacheHit { layer, .. } => match layer {
                LayerId::Persona => persona_hit = true,
                LayerId::StaticPrompt => static_hit = true,
                _ => {}
            },
            SystemEvent::ComposeLayerCacheMiss { .. } => {
                panic!("second call should not emit Miss events")
            }
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
