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

    // Messages may be empty in Phase 1 because the stub static_prompt returns
    // an empty system_message; the assembly layer skips the system push in
    // that case, so the final message list is empty for a fully-empty run.
    assert!(out.messages.is_empty());
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
