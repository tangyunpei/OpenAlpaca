use super::*;
use crate::memory::scope_context::MemoryScopeContext;

#[test]
fn test_lead_agent_registry_contains_coordination_tools() {
    // Verify that register_coordination_tools populates the registry with
    // spawn_subagent, check_subagent_status, and wait_for_subagents.
    // batch_spawn is absent when None is passed.
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    struct NoopTool;

    #[async_trait]
    impl BuiltInTool for NoopTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("noop".to_string())
        }
    }

    let registry = ToolRegistry::default();
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "web_search".to_string(),
            description: "test".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        provides_capabilities: vec!["web_search".to_string()],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    ));
    let check_status_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    register_coordination_tools(
        &registry,
        spawn_tool,
        None, // no batch spawn
        check_status_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        None,
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"spawn_subagent".to_string()));
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"check_subagent_status".to_string()));
    assert!(tools.contains(&"wait_for_subagents".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
}

// ── SubagentTracker tests ────────────────────────────────────────

#[test]
fn test_tracker_register_and_status() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));
    assert!(!tracker.all_done());
}

#[test]
fn test_tracker_complete() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.complete("run-1", "Result text".to_string(), true);

    match tracker.get("run-1") {
        Some(SubagentStatus::Completed { content, success }) => {
            assert_eq!(content, "Result text");
            assert!(success);
        }
        other => panic!("Expected Completed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_fail() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.fail("run-1", "Some error".to_string());

    match tracker.get("run-1") {
        Some(SubagentStatus::Failed { error, output }) => {
            assert_eq!(error, "Some error");
            assert!(output.is_empty(), "fail() keeps no output: {output:?}");
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_all_done_mixed() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.register("run-2");
    tracker.register("run-3");

    // Partially complete
    tracker.complete("run-1", "done".to_string(), true);
    assert!(!tracker.all_done());

    tracker.fail("run-2", "err".to_string());
    assert!(!tracker.all_done());

    tracker.complete("run-3", "done too".to_string(), true);
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_summary() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.complete("run-1", "Research result".to_string(), true);
    tracker.register("run-2");
    tracker.fail("run-2", "Timeout".to_string());

    let summary = tracker.summary();
    assert!(summary.contains("run-1"));
    assert!(summary.contains("completed"));
    assert!(summary.contains("Research result"));
    assert!(summary.contains("run-2"));
    assert!(summary.contains("failed"));
    assert!(summary.contains("Timeout"));
}

#[test]
fn test_tracker_empty_summary() {
    let tracker = SubagentTracker::new();
    assert!(tracker.summary().contains("No subagents"));
}

#[test]
fn test_tracker_set_status() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));

    tracker.set_status("run-1", SubagentStatus::Running);
    assert!(matches!(
        tracker.get("run-1"),
        Some(SubagentStatus::Running)
    ));
    assert!(!tracker.all_done());

    tracker.set_status(
        "run-1",
        SubagentStatus::Completed {
            content: "done".to_string(),
            success: true,
        },
    );
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_all_done_with_queued() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    tracker.register("run-2");

    // Both queued — not done
    assert!(!tracker.all_done());

    // One running, one queued — not done
    tracker.set_status("run-1", SubagentStatus::Running);
    assert!(!tracker.all_done());

    // One completed, one queued — not done
    tracker.complete("run-1", "done".to_string(), true);
    assert!(!tracker.all_done());

    // Both completed — done
    tracker.complete("run-2", "done too".to_string(), true);
    assert!(tracker.all_done());
}

#[test]
fn test_tracker_status_counts() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1"); // queued
    tracker.register("run-2"); // queued
    tracker.register("run-3"); // queued
    tracker.register("run-4"); // queued

    let (queued, running, completed, failed) = tracker.status_counts();
    assert_eq!((queued, running, completed, failed), (4, 0, 0, 0));

    tracker.set_status("run-1", SubagentStatus::Running);
    tracker.complete("run-2", "done".to_string(), true);
    tracker.fail("run-3", "err".to_string());

    let (queued, running, completed, failed) = tracker.status_counts();
    assert_eq!((queued, running, completed, failed), (1, 1, 1, 1));
}

#[test]
fn test_tracker_summary_with_queued() {
    let tracker = SubagentTracker::new();

    tracker.register("run-1");
    let summary = tracker.summary();
    assert!(summary.contains("run-1"));
    assert!(summary.contains("queued"));
    assert!(summary.contains("waiting for execution slot"));
}

// ── F2: the summary shows each answer's head and tail within the bound ──

/// The per-answer budget is a function of the loop's inline bound and the
/// number of tracked entries: `(bound − 1 KiB) / entries − 768`, floored
/// at 512 and capped at 8 KiB. The default bound and a batch of 8 give
/// 3,200 bytes an answer; one answer alone gets the cap; at the clamp
/// minimum 4,096 a batch of 8 sits on the floor.
#[test]
fn the_summarys_per_answer_budget_follows_the_bound_and_the_entry_count() {
    assert_eq!(super::tracker::answer_budget(32 * 1024, 8), 3_200);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 1), 8_192);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 2), 8_192);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 4), 7_168);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 16), 1_216);
    assert_eq!(super::tracker::answer_budget(32 * 1024, 25), 512);
    assert_eq!(super::tracker::answer_budget(4_096, 8), 512);
    // Zero entries never divide by zero (the empty summary returns earlier).
    assert_eq!(super::tracker::answer_budget(32 * 1024, 0), 8_192);
}

/// Eight 20 KB answers through the tool itself — header included — stay
/// under the default 32 KiB bound, and every answer's opening and closing
/// words reach the lead.
#[tokio::test]
async fn wait_for_subagents_keeps_eight_twenty_kilobyte_answers_under_the_inline_bound() {
    let tracker = Arc::new(SubagentTracker::new());
    for i in 1..=8 {
        let run_id = format!("run-{i}");
        tracker.register(&run_id);
        tracker.complete(
            &run_id,
            format!("HEAD-{i} {} TAIL-{i}", "x".repeat(20 * 1024)),
            true,
        );
    }
    let tool = WaitForSubagentsTool {
        tracker,
        steering: None,
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();

    assert!(
        result.len() <= 32 * 1024,
        "the whole summary must fit the inline bound, got {} bytes",
        result.len()
    );
    assert!(
        result.starts_with("All subagents finished. 8 completed, 0 failed.\n\n"),
        "got: {}",
        crate::utils::text::byte_prefix(&result, 80)
    );
    for i in 1..=8 {
        assert!(result.contains(&format!("HEAD-{i} ")), "head {i} missing");
        assert!(result.contains(&format!(" TAIL-{i}")), "tail {i} missing");
    }
    assert_eq!(result.matches("the tail follows").count(), 8);
}

/// A CJK answer is cut on character boundaries: with one entry the budget
/// is the 8,192-byte cap, half is 4,096, not a multiple of 3, so the head
/// rounds down to 4,095 bytes (1,365 characters) and the tail rounds up to
/// start at 10,905, 4,095 bytes too. The whole line is pinned.
#[test]
fn a_cjk_answer_is_cut_on_character_boundaries_in_the_summary() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    tracker.complete("run-1", "語".repeat(5000), true);

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: completed — {}\n\n[... 6810 of 15000 bytes elided; the tail follows ...]\n\n{}",
            "語".repeat(1365),
            "語".repeat(1365)
        )
    );
}

/// An answer within its budget is shown whole — 3,000 bytes under the
/// 8,192-byte cap one entry gets.
#[test]
fn an_answer_within_its_budget_is_shown_whole_in_the_summary() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let answer = format!("Report: {}", "r".repeat(2992));
    tracker.complete("run-1", answer.clone(), true);

    assert_eq!(
        tracker.summary(),
        format!("- **run-1**: completed — {answer}")
    );
}

/// The bound is the tracker's, set at construction: an 8 KiB bound gives
/// one entry (8192 − 1024) − 768 = 6,400 bytes, head 3,200 and tail 3,200.
#[test]
fn a_tracker_built_with_a_smaller_bound_cuts_to_it() {
    let tracker = SubagentTracker::with_inline_bytes(8 * 1024);
    tracker.register("run-1");
    tracker.complete("run-1", "y".repeat(10_000), true);

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: completed — {}\n\n[... 3600 of 10000 bytes elided; the tail follows ...]\n\n{}",
            "y".repeat(3200),
            "y".repeat(3200)
        )
    );
}

/// Review focus — more entries than the bound can hold (60 here, past the
/// 25 where the budget floors at 512 bytes): the summary is still produced,
/// still names every entry and shows each answer's opening and end, and is
/// longer than the bound, which the loop's own cut or spill then handles as
/// for any oversized tool result. Nothing is dropped silently.
#[test]
fn a_batch_past_the_bound_still_lists_every_entry() {
    let tracker = SubagentTracker::new();
    for i in 1..=60 {
        let run_id = format!("run-{i}");
        tracker.register(&run_id);
        tracker.complete(
            &run_id,
            format!("BEGIN-{i} {} END-{i}.", "z".repeat(4000)),
            true,
        );
    }
    assert_eq!(super::tracker::answer_budget(32 * 1024, 60), 512);
    let summary = tracker.summary();
    // Each entry is about 595 bytes (prefix, 256-byte head, notice, 256-byte
    // tail, newline): 60 of them are about 35,700 bytes, past the bound.
    for i in 1..=60 {
        assert!(
            summary.contains(&format!("- **run-{i}**: completed — BEGIN-{i} ")),
            "entry {i} missing"
        );
        assert!(summary.contains(&format!(" END-{i}.")), "tail {i} missing");
    }
    assert!(
        summary.len() > 32 * 1024,
        "past the floor the summary may exceed the bound: {} bytes",
        summary.len()
    );
}

// ── F2: a failed subagent's answer is kept whole ──

/// The tracker keeps a failed subagent's whole final content beside its
/// error — 1,221 characters here, past the 500 the old error string held,
/// in a script where 500 chars were 1,500 bytes.
#[test]
fn a_failed_subagents_whole_answer_is_kept() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let answer = format!("{}\n最後の行: /tmp/report.md", "見出し".repeat(400));
    assert!(answer.chars().count() > 500);

    tracker.fail_with_output(
        "run-1",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        answer.clone(),
    );

    match tracker.get("run-1") {
        Some(SubagentStatus::Failed { error, output }) => {
            assert_eq!(error, "Agent 'writer' failed: MaxRounds.");
            assert_eq!(output, answer);
        }
        other => panic!("Expected Failed, got {:?}", other),
    }
    assert!(tracker.all_done());
}

/// The summary shows a failed entry's error, then the answer's head and tail
/// under the same budget a completed answer gets: one entry, the 8,192-byte
/// cap, head 4,096 and a tail of 4,096 that ends on the path.
#[test]
fn the_summary_shows_a_failed_answers_error_and_its_head_and_tail() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    let output = format!("{}/tmp/report.md", "A".repeat(10_000));
    tracker.fail_with_output(
        "run-1",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        output,
    );

    assert_eq!(
        tracker.summary(),
        format!(
            "- **run-1**: failed — Agent 'writer' failed: MaxRounds. Last output: {}\n\n[... 1822 of 10014 bytes elided; the tail follows ...]\n\n{}/tmp/report.md",
            "A".repeat(4096),
            "A".repeat(4082)
        )
    );
}

/// A failure with no output reads as it did before, and a long error text
/// is cut to 512 bytes — 256 head, 256 tail — so a provider's or a plugin's
/// message cannot eat the batch's budget.
#[test]
fn a_failed_entry_with_no_output_shows_its_error_alone_and_a_long_error_is_cut() {
    let tracker = SubagentTracker::new();
    tracker.register("run-1");
    tracker.fail(
        "run-1",
        "Cancelled before starting (parent task was cancelled)".to_string(),
    );
    assert_eq!(
        tracker.summary(),
        "- **run-1**: failed — Cancelled before starting (parent task was cancelled)"
    );

    let tracker = SubagentTracker::new();
    tracker.register("run-2");
    tracker.fail("run-2", "E".repeat(2000));
    let summary = tracker.summary();
    assert_eq!(
        summary,
        format!(
            "- **run-2**: failed — {}\n\n[... 1488 of 2000 bytes elided; the tail follows ...]\n\n{}",
            "E".repeat(256),
            "E".repeat(256)
        )
    );
    assert!(!summary.contains("Last output:"));
}

// ── F2: check_subagent_status returns a failed subagent's whole answer ──

/// The tool returns the kept output whole — 3,015 characters, 9,015 bytes
/// here — after the error; the loop, not the tool, bounds what the model
/// sees.
#[tokio::test]
async fn check_subagent_status_returns_a_failed_subagents_whole_answer() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-f");
    let output = format!("{}\n/tmp/report.md", "結果".repeat(1500));
    tracker.fail_with_output(
        "run-f",
        "Agent 'writer' failed: MaxRounds.".to_string(),
        output.clone(),
    );
    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    let err = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-f"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        format!(
            "Subagent 'run-f' failed: Agent 'writer' failed: MaxRounds.\nIts last output follows:\n{output}"
        )
    );
}

/// A failure that carries no answer names the error alone, as before.
#[tokio::test]
async fn check_subagent_status_on_a_failure_with_no_output_names_the_error_alone() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-g");
    tracker.fail(
        "run-g",
        "Cancelled before starting (parent task was cancelled)".to_string(),
    );
    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    let err = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-g"}))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        "Subagent 'run-g' failed: Cancelled before starting (parent task was cancelled)"
    );
}

// ── F2: the descriptions and the prompt say what the tools return ──

#[test]
fn check_subagent_status_description_says_it_returns_the_whole_answer() {
    assert_eq!(
        check_subagent_status_tool_definition().description,
        "Check one spawned subagent by its run_id. Queued or running: says so. \
         Completed: returns the subagent's whole answer — a long one arrives as a stub \
         naming a result_ref; page it with read_result. Failed: returns the error and the \
         subagent's whole last output, as its head and tail when long. wait_for_subagents \
         shows only the head and the tail of each answer; call this for the one you need \
         in full."
    );
}

#[test]
fn wait_for_subagents_description_says_it_shows_each_answers_head_and_tail() {
    assert_eq!(
        wait_for_subagents_tool_definition().description,
        "Wait for ALL previously spawned subagents to complete, including any that are \
         queued for execution. Returns a summary of all results: each subagent's answer as \
         its head and its tail, with a notice naming the bytes between them, sized so that \
         a batch of eight fits one tool result. Read one answer in full with \
         check_subagent_status. Use this after spawning all subagents to collect all \
         outputs before synthesizing a response."
    );
}

#[test]
fn the_lead_prompt_says_check_subagent_status_returns_the_whole_answer() {
    let engine = crate::compose::ComposeEngine::new(16);
    let prompt =
        build_lead_agent_prompt_from_templates(&engine, "Test persona", &[], 200_000, None, None);
    assert!(
        prompt.contains(
            "check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
             queued, running, completed, or failed. Completed: returns the whole answer — a long one \
             arrives as a stub to page with read_result. Failed: returns the error and the last \
             output, as head and tail when long."
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
             queued for execution. Returns a summary of all results: each answer's head and tail, \
             with a notice of what lies between — read one in full with check_subagent_status. \
             Call this after spawning all subagents."
        ),
        "{prompt}"
    );
}

#[tokio::test]
async fn test_check_subagent_status_tool() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-abc");
    tracker.complete("run-abc", "Done!".to_string(), true);

    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    // Completed
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-abc"}))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("completed successfully"));

    // Unknown
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "no-such"}))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_check_subagent_status_tool_queued() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-queued");

    let tool = CheckSubagentStatusTool {
        tracker: tracker.clone(),
    };

    // Queued
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
        .await;
    assert!(result.is_ok());
    let msg = result.unwrap();
    assert!(msg.contains("queued"));
    assert!(msg.contains("execution slot"));

    // Transition to Running
    tracker.set_status("run-queued", SubagentStatus::Running);
    let result = tool
        .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
        .await;
    assert!(result.is_ok());
    assert!(result.unwrap().contains("still running"));
}

// ── Batch spawn tool tests ────────────────────────────────────────

#[test]
fn test_batch_spawn_tool_definition_includes_agents() {
    use crate::agent::template::{AgentSource, AgentTemplateFrontmatter};

    let templates = vec![AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: "researcher".to_string(),
            name: "Researcher".to_string(),
            description: "Research agent".to_string(),
            icon: None,
            singleton: false,
            capabilities: vec!["web_search".to_string()],
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
    }];

    let def = spawn_subagents_batch_tool_definition(&templates);
    assert_eq!(def.name, "spawn_subagents_batch");
    assert!(def.description.contains("researcher"));
    assert!(def.description.contains("Researcher"));
}

#[test]
fn test_batch_spawn_tool_hidden_when_disabled() {
    // When batch_spawn is disabled (None passed), spawn_subagents_batch should NOT
    // appear in the registry.
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    ));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    let registry = ToolRegistry::default();
    register_coordination_tools(
        &registry,
        spawn_tool,
        None, // batch disabled
        check_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        None,
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"spawn_subagent".to_string()));
}

#[test]
fn test_batch_spawn_tool_present_when_enabled() {
    // When batch_spawn is enabled (Some passed), spawn_subagents_batch should
    // appear in the registry.
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    ));
    let batch_tool = Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())));
    let check_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool { tracker, steering: None });

    let registry = ToolRegistry::default();
    register_coordination_tools(
        &registry,
        spawn_tool,
        batch_tool, // batch enabled
        check_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&[]),
        Some(spawn_subagents_batch_tool_definition(&[])),
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );

    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"spawn_subagents_batch".to_string()));
    assert!(tools.contains(&"spawn_subagent".to_string()));
}

#[tokio::test]
async fn test_batch_spawn_empty_array_error() {
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker,
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(), // workspace
        None, // confirmation_broker
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    ));
    let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

    let result = batch_tool
        .execute(&serde_json::json!({"subagents": []}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least 1"));
}

#[tokio::test]
async fn test_batch_spawn_exceeds_max_error() {
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker,
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(), // workspace
        None, // confirmation_broker
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    ));
    let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

    // 9 items should fail (max 8)
    let items: Vec<serde_json::Value> = (0..9)
        .map(|i| {
            serde_json::json!({
                "agent_id": format!("agent-{}", i),
                "objective": format!("task-{}", i)
            })
        })
        .collect();
    let result = batch_tool
        .execute(&serde_json::json!({"subagents": items}))
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("max 8"));
}

#[test]
fn test_lead_agent_registry_exposes_command_backend_tools() {
    // Verify that command-backend tools registered in the ToolRegistry are
    // reported by command_backend_tool_names() so SandboxManager can treat
    // them as shell-like tools.
    use crate::tools::registry::{RegisteredTool, ToolBackend};

    let registry = ToolRegistry::default();
    // Register a command-backend tool
    registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: "my_cmd_tool".to_string(),
            description: "test command tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::Command {
            command: "echo".to_string(),
            args_template: Some("hello".to_string()),
            timeout_secs: 10,
        },
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: "test-0.0.0".into(),
        author: "test".into(),
        created_at: chrono::Utc::now(),
    }).unwrap();

    let shell_tools = registry.command_backend_tool_names();
    assert!(
        shell_tools.contains(&"my_cmd_tool".to_string()),
        "command_backend_tool_names() should include command-backend tools. Got: {:?}",
        shell_tools
    );
}

// ── Phase P4: LA-2 batch spawn prompt test ──

#[test]
fn test_lead_agent_prompt_includes_batch_spawn_instruction() {
    let engine = crate::compose::ComposeEngine::new(16);
    let prompt =
        build_lead_agent_prompt_from_templates(&engine, "Test persona", &[], 200_000, None, None);
    assert!(
        prompt.contains("spawn_subagents_batch"),
        "Lead agent prompt should mention spawn_subagents_batch tool"
    );
    assert!(
        prompt.contains("3+ independent subagents"),
        "Lead agent prompt should instruct using batch for 3+ subagents"
    );
}

#[test]
fn test_workflow_contract_suffix_gates_interjection_protocol() {
    // Steering attached: both sections, interjection protocol first.
    let with_steering = super::prompt::workflow_contract_suffix(true);
    assert!(with_steering.contains("<interjection_protocol>"));
    assert!(with_steering.contains("<user_interjection>"));
    assert!(with_steering.contains("post_update"));
    assert!(with_steering.contains("queue_followup"));
    assert!(with_steering.contains("<completion_report>"));

    // No steering inbox: completion-report contract only (spec §2b is
    // unconditional; the interjection protocol rides with the rail).
    let without_steering = super::prompt::workflow_contract_suffix(false);
    assert!(!without_steering.contains("<interjection_protocol>"));
    assert!(without_steering.contains("<completion_report>"));
    assert!(without_steering.contains("user-facing completion report"));
}

// ── Phase P4: LA-3 prompt template caching test ──

#[test]
fn test_spawn_subagent_prompt_template_substitution() {
    let template = "\
        <identity>\n{PERSONA}\n</identity>\n\n\
        <scope>\nYou are a subagent.\n</scope>\n\n\
        <constraints>\nIndependent.\n</constraints>{TOOL_GUIDANCE}";

    let result = template
        .replace("{PERSONA}", "I am a researcher")
        .replace("{TOOL_GUIDANCE}", "\n\nTools: search, browse");

    assert!(result.contains("<identity>\nI am a researcher\n</identity>"));
    assert!(result.contains("Tools: search, browse"));
    assert!(!result.contains("{PERSONA}"));
    assert!(!result.contains("{TOOL_GUIDANCE}"));
}

// ── Routing V2: workflow tools (post_update / queue_followup) ──

/// Drain matching events from a broadcast receiver without blocking.
fn recv_all(
    rx: &mut tokio::sync::broadcast::Receiver<crate::events::SystemEvent>,
) -> Vec<crate::events::SystemEvent> {
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

#[tokio::test]
async fn test_post_update_persists_conversation_and_publishes_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let bus = EventBus::default();
    let mut rx = bus.subscribe();

    let tool = PostUpdateTool::new(
        Some(db.clone()),
        bus,
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "cli".to_string(),
    );
    let result = tool
        .execute(&serde_json::json!({"message": "Halfway there"}))
        .await
        .unwrap();
    assert!(result.contains("posted"));

    // Persisted as an assistant message on the lane conversation.
    let conv_repo = openalpaca_storage::ConversationRepository::new(&db);
    let messages = conv_repo.list_recent_by_lane("junpei:cli", 10).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert_eq!(messages[0].content, "Halfway there");
    assert_eq!(messages[0].source.as_deref(), Some("cli"));

    // WorkflowProgress published with lane + task identity.
    let events = recv_all(&mut rx);
    assert!(events.iter().any(|ev| matches!(
        ev,
        crate::events::SystemEvent::WorkflowProgress { task_id, lane_key, message, .. }
            if task_id == "task-1" && lane_key == "junpei:cli" && message == "Halfway there"
    )));
}

#[tokio::test]
async fn test_post_update_rejects_empty_message() {
    let tool = PostUpdateTool::new(
        None,
        EventBus::default(),
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "cli".to_string(),
    );
    assert!(tool.execute(&serde_json::json!({})).await.is_err());
    assert!(tool.execute(&serde_json::json!({"message": "  "})).await.is_err());
}

#[tokio::test]
async fn test_queue_followup_inserts_row_and_publishes_event() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let bus = EventBus::default();
    let mut rx = bus.subscribe();

    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        bus,
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    let result = tool
        .execute(&serde_json::json!({"description": "Also update the docs"}))
        .await
        .unwrap();
    assert!(result.contains("Follow-up queued"));

    // Row inserted with the reconstructed principal and source task.
    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, "followup");
    assert_eq!(rows[0].content, "Also update the docs");
    assert_eq!(rows[0].source_task_id.as_deref(), Some("task-1"));
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );

    // FollowupQueued published with the row id.
    let events = recv_all(&mut rx);
    assert!(events.iter().any(|ev| matches!(
        ev,
        crate::events::SystemEvent::FollowupQueued { lane_key, followup_id, kind, .. }
            if lane_key == "junpei:cli" && *followup_id == rows[0].id && kind == "followup"
    )));
}

#[tokio::test]
async fn test_queue_followup_system_owner_maps_to_system_principal() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        EventBus::default(),
        "task-1".to_string(),
        "system:internal".to_string(),
        "system".to_string(),
        false,
    );
    tool.execute(&serde_json::json!({"description": "scheduled follow-up"}))
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("system:internal").unwrap();
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(principal, crate::security::policy::Principal::System);
}

/// F4 — the row records the turn's project root, which is what the follow-up
/// runner hands back to the gateway (`apps/openalpacad/src/followup.rs:44`),
/// never the path the client sent. The turn sent a directory two levels
/// below the `.git` marker; the row holds the canonical root.
#[tokio::test]
async fn a_queue_followup_from_the_main_loop_stores_the_turns_resolved_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let sent = project.path().join("src").join("lib");
    std::fs::create_dir_all(&sent).unwrap();
    let sent = sent.to_string_lossy().to_string();
    // The root as the resolver spells it (`store::projects::resolve`):
    // canonical, which on macOS turns `/var/folders/…` into
    // `/private/var/folders/…`.
    let root = project
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "fallback-owner".to_string(),
        false,
    );
    // The context the main loop builds (`simple_query_handler.rs:150-163`):
    // the turn's scope context resolves the root; the path as sent rides
    // beside it.
    let scope = MemoryScopeContext::for_request(Some(sent.as_str()));
    assert_eq!(scope.request_workspace_root.as_deref(), Some(root.as_str()));
    let ctx = crate::tools::registry::ToolContext {
        owner_id: Some("junpei".to_string()),
        principal: Some(crate::security::policy::Principal::User {
            global_id: "junpei".to_string(),
        }),
        request_workspace_root: scope.request_workspace_root.clone(),
        workspace_path: Some(sent.clone()),
        ..Default::default()
    };
    tool.execute_with_context(&serde_json::json!({"description": "Do it later"}), &ctx)
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_task_id, None);
    assert_eq!(
        rows[0].workspace_path.as_deref(),
        Some(root.as_str()),
        "the row must hold the resolved root, not the path as sent ({sent})"
    );
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
}

/// F4 — the lead's context carries no `workspace_path` (`run_lead_agent`'s
/// `tool_ctx`, `runner/lead_agent/mod.rs`) and does carry
/// `request_workspace_root`, the run's project. A follow-up a workflow queues
/// must run in that project; until this fix the row held NULL and the
/// follow-up turn ran with none.
#[tokio::test]
async fn a_queue_followup_from_the_lead_stores_the_runs_project_root() {
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let root = MemoryScopeContext::for_request(project.path().to_str())
        .request_workspace_root
        .expect("a directory carrying .git resolves to itself");
    assert_eq!(
        root,
        project
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string()
    );

    // The lead's `QueueFollowupTool::new` call (in `run_lead_agent`):
    // identity from the task's `created_by`, the run as `source_task_id`.
    let tool = QueueFollowupTool::new(
        Some(db.clone()),
        EventBus::default(),
        "task-1".to_string(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    // The context exactly as `run_lead_agent` builds it (its `tool_ctx`):
    // no principal, no scope, no workspace_path — and the run's root.
    let ctx = crate::tools::registry::ToolContext {
        agent_id: Some("lead_agent::abc".to_string()),
        agent_instance_id: Some("lead_agent::abc".to_string()),
        task_id: Some("task-1".to_string()),
        owner_id: Some("junpei".to_string()),
        workspace_id: Some(root.clone()),
        request_workspace_root: Some(root.clone()),
        lane_key: Some("junpei:cli".to_string()),
        source: Some("cli".to_string()),
        ..Default::default()
    };
    tool.execute_with_context(
        &serde_json::json!({"description": "Also update the docs"}),
        &ctx,
    )
    .await
    .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].source_task_id.as_deref(), Some("task-1"));
    assert_eq!(
        rows[0].workspace_path.as_deref(),
        Some(root.as_str()),
        "a follow-up promised by a workflow must re-enter in the run's project"
    );
    // No principal on the lead's context: the constructor's `created_by` stands in.
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
}

/// Review focus — a turn whose path no longer exists (a CLI started in a
/// directory since deleted, a row from an older build): the path cannot
/// be canonicalised, so the resolver matches it against registered
/// project roots only (storage design §3); the guarded home store
/// registers none, so it yields no root and the row holds NULL — the
/// same as a path under no project. Nothing panics,
/// nothing is stored as sent.
#[tokio::test]
async fn a_queue_followup_from_a_path_that_no_longer_exists_stores_no_project() {
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::test_util::HomeStoreGuard::set(&dir.path().join("home"));
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let gone = tempfile::tempdir().unwrap();
    let sent = gone.path().join("deleted").join("project");
    let sent = sent.to_string_lossy().to_string();
    drop(gone);
    assert!(!std::path::Path::new(&sent).exists());

    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    let scope = MemoryScopeContext::for_request(Some(sent.as_str()));
    assert_eq!(scope.request_workspace_root, None);
    let ctx = crate::tools::registry::ToolContext {
        owner_id: Some("junpei".to_string()),
        request_workspace_root: scope.request_workspace_root.clone(),
        workspace_path: Some(sent.clone()),
        ..Default::default()
    };
    tool.execute_with_context(&serde_json::json!({"description": "Later"}), &ctx)
        .await
        .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].workspace_path, None,
        "a vanished path is no project, not a stored string"
    );
}

#[tokio::test]
async fn test_queue_followup_ctx_without_principal_falls_back_to_created_by() {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let tool = QueueFollowupTool::for_main_loop(
        Some(db.clone()),
        EventBus::default(),
        "junpei:cli".to_string(),
        "junpei".to_string(),
        false,
    );
    tool.execute_with_context(
        &serde_json::json!({"description": "later"}),
        &crate::tools::registry::ToolContext::default(),
    )
    .await
    .unwrap();

    let repo = openalpaca_storage::repository::FollowupRepository::new(&db);
    let rows = repo.list_queued_by_lane("junpei:cli").unwrap();
    let principal: crate::security::policy::Principal =
        serde_json::from_str(&rows[0].principal_json).unwrap();
    assert_eq!(
        principal,
        crate::security::policy::Principal::User {
            global_id: "junpei".to_string()
        }
    );
    assert_eq!(rows[0].workspace_path, None);
}

// ── Routing V2: wait_for_subagents steering interrupt ──────────────

fn wait_steering_msg(text: &str) -> crate::runner::steering::SteeringMsg {
    crate::runner::steering::SteeringMsg {
        text: text.to_string(),
        request_id: uuid::Uuid::new_v4(),
        principal: crate::security::policy::Principal::System,
        scope: crate::security::policy::Scope::Global,
        workspace_path: None,
        received_at: chrono::Utc::now(),
        origin: crate::runner::steering::SteeringOrigin::User,
    }
}

#[tokio::test]
async fn test_wait_for_subagents_interrupted_by_steering_push() {
    // A push while the tool is blocked in its select must break the wait.
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1"); // never completes — wait would block 600s
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    let tool = Arc::new(WaitForSubagentsTool {
        tracker,
        steering: Some(inbox.clone()),
    });

    let waiter = {
        let tool = tool.clone();
        tokio::spawn(async move { tool.execute(&serde_json::json!({})).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    inbox.push(wait_steering_msg("adjust course")).unwrap();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
        .await
        .expect("steering push must interrupt the wait")
        .unwrap()
        .unwrap();
    assert!(result.contains("Wait interrupted"), "got: {result}");
    assert!(result.contains("1 queued"), "got: {result}");
}

#[tokio::test]
async fn test_wait_for_subagents_pre_queued_steering_returns_immediately() {
    // No lost wakeup: a message pushed BEFORE the wait starts must still
    // interrupt within the first iteration (the pre-check catches it).
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1"); // never completes
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    inbox.push(wait_steering_msg("queued before wait")).unwrap();
    let tool = WaitForSubagentsTool {
        tracker,
        steering: Some(inbox),
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tool.execute(&serde_json::json!({})),
    )
    .await
    .expect("pre-queued interjection must interrupt without waiting")
    .unwrap();
    assert!(result.contains("Wait interrupted"), "got: {result}");
}

#[tokio::test]
async fn test_wait_for_subagents_without_steering_completes_normally() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    tracker.complete("run-1", "done".to_string(), true);
    let tool = WaitForSubagentsTool {
        tracker,
        steering: None,
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();
    assert!(result.contains("All subagents finished"));
}

#[tokio::test]
async fn test_wait_for_subagents_closed_empty_inbox_does_not_interrupt() {
    // A closed, empty inbox is treated as absent — the wait completes on
    // subagent completion instead of spinning or interrupting.
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    tracker.complete("run-1", "done".to_string(), true);
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::default());
    inbox.close_and_drain();
    let tool = WaitForSubagentsTool {
        tracker,
        steering: Some(inbox),
    };
    let result = tool.execute(&serde_json::json!({})).await.unwrap();
    assert!(result.contains("All subagents finished"));
}

#[tokio::test(start_paused = true)]
async fn wait_for_subagents_returns_when_the_last_subagent_finishes_just_after_its_check() {
    // The lost wakeup: the last completion lands after `all_done()` read
    // the map and before the wait listens for a change. `notify_waiters`
    // keeps no permit, so a waiter that registers only afterwards sleeps
    // until the 600 s cap. The hook puts the completion in that window;
    // under a paused clock a lost wakeup reaches the 5 s timeout at once.
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    tracker.after_next_unfinished_check(|t| t.complete("run-1", "done".to_string(), true));
    let tool = WaitForSubagentsTool {
        tracker,
        steering: None,
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tool.execute(&serde_json::json!({})),
    )
    .await
    .expect("a completion just after the check must end the wait, not the 600 s cap")
    .unwrap();
    assert!(result.contains("All subagents finished"), "got: {result}");
    assert!(result.contains("1 completed"), "got: {result}");
}

#[tokio::test(start_paused = true)]
async fn wait_for_subagents_returns_when_the_last_subagent_finishes_while_it_waits() {
    let tracker = Arc::new(SubagentTracker::new());
    tracker.register("run-1");
    let tool = Arc::new(WaitForSubagentsTool {
        tracker: tracker.clone(),
        steering: None,
    });
    let waiter = {
        let tool = tool.clone();
        tokio::spawn(async move { tool.execute(&serde_json::json!({})).await })
    };
    tokio::task::yield_now().await;
    tracker.complete("run-1", "done".to_string(), true);

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .expect("a completion while the wait listens must end it")
        .unwrap()
        .unwrap();
    assert!(result.contains("All subagents finished"), "got: {result}");
}

// ── Plugin-backed subagent spawning ─────────────────────────────────

/// Stub plugin executor: accepts the spawn, records the instructions,
/// and completes on the first step.
struct CompletingPluginExecutor {
    instructions: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for CompletingPluginExecutor {
    async fn spawn(
        &self,
        _instance_id: &str,
        _task_id: &str,
        instructions: &str,
        _context: &serde_json::Value,
    ) -> Result<bool, String> {
        *self.instructions.lock().unwrap() = Some(instructions.to_string());
        Ok(true)
    }

    async fn step(
        &self,
        _instance_id: &str,
        _tool_results: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String> {
        Ok(("complete".to_string(), "plugin result".to_string(), vec![]))
    }

    async fn stop(&self, _instance_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn plugin_id(&self) -> &str {
        "test_plugin"
    }

    fn agent_id(&self) -> &str {
        "plugin_researcher"
    }
}

#[tokio::test]
async fn test_spawn_subagent_executes_plugin_backed_template() {
    use crate::agent::template::{AgentSource, AgentTemplateFrontmatter};

    let executor = Arc::new(CompletingPluginExecutor {
        instructions: std::sync::Mutex::new(None),
    });
    let template = AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: "plugin_researcher".to_string(),
            name: "Plugin Researcher".to_string(),
            description: "Plugin-backed research agent".to_string(),
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
        source: AgentSource::Plugin {
            plugin_id: "test_plugin".to_string(),
            executor: executor.clone(),
        },
    };

    let shared_context = Arc::new(SharedContext::new());
    assert!(shared_context.agent_registry.register_template(template));

    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        shared_context,
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    );

    let msg = spawn_tool
        .execute(&serde_json::json!({
            "agent_id": "plugin_researcher",
            "objective": "Summarize the design doc"
        }))
        .await
        .unwrap();
    assert!(msg.contains("spawned"), "got: {msg}");

    // The plugin loop runs in a background task — wait for the tracker.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !tracker.all_done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "plugin-backed subagent never completed; summary: {}",
            tracker.summary()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // The result lands in the tracker with the plugin's output.
    let statuses = tracker.statuses.lock().unwrap();
    assert_eq!(statuses.len(), 1);
    let (run_id, status) = statuses.iter().next().unwrap();
    assert!(
        run_id.starts_with("plugin_researcher::"),
        "unexpected run_id: {run_id}"
    );
    match status {
        SubagentStatus::Completed { content, success } => {
            assert!(success);
            assert_eq!(content, "plugin result");
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    // The plugin received instructions carrying the objective.
    let instructions = executor.instructions.lock().unwrap();
    let instructions = instructions.as_ref().expect("plugin spawn was never called");
    assert!(
        instructions.contains("Summarize the design doc"),
        "instructions missing objective: {instructions}"
    );
}

#[test]
fn test_register_workflow_tools_registers_both() {
    let registry = ToolRegistry::default();
    register_workflow_tools(
        &registry,
        Arc::new(PostUpdateTool::new(
            None,
            EventBus::default(),
            "task-1".to_string(),
            "junpei:cli".to_string(),
            "cli".to_string(),
        )),
        Arc::new(QueueFollowupTool::new(
            None,
            EventBus::default(),
            "task-1".to_string(),
            "junpei:cli".to_string(),
            "junpei".to_string(),
            false,
        )),
    );
    let tools = registry.registered_tool_names();
    assert!(tools.contains(&"post_update".to_string()));
    assert!(tools.contains(&"queue_followup".to_string()));
}

// ── Tool/skill wiring Chunk 3: extension tools + invoke_skill on the lead ──

/// Scripted provider: pops one canned response per call and records each
/// request's tool names + messages so tests can inspect what the lead loop
/// (and any nested skill loop) actually sent.
struct ScriptedProvider {
    responses: std::sync::Mutex<std::collections::VecDeque<openalpaca_llm::ChatResponse>>,
    seen_tool_names: std::sync::Mutex<Vec<Vec<String>>>,
    seen_messages: std::sync::Mutex<Vec<Vec<openalpaca_llm::ChatMessage>>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<openalpaca_llm::ChatResponse>) -> Arc<Self> {
        Arc::new(Self {
            responses: std::sync::Mutex::new(responses.into()),
            seen_tool_names: std::sync::Mutex::new(Vec::new()),
            seen_messages: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn tool_names(&self, call: usize) -> Vec<String> {
        self.seen_tool_names.lock().unwrap()[call].clone()
    }

    fn calls(&self) -> usize {
        self.seen_tool_names.lock().unwrap().len()
    }
}

fn scripted_response(
    content: &str,
    tool_calls: Vec<openalpaca_llm::ToolCall>,
) -> openalpaca_llm::ChatResponse {
    openalpaca_llm::ChatResponse {
        content: content.to_string(),
        finish_reason: if tool_calls.is_empty() {
            openalpaca_llm::FinishReason::Stop
        } else {
            openalpaca_llm::FinishReason::ToolUse
        },
        tool_calls,
        model: "claude-sonnet-4-20250514".to_string(),
        usage: openalpaca_llm::Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
        thinking: None,
        parts: None,
    }
}

#[async_trait]
impl openalpaca_llm::LlmProvider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        self.seen_tool_names
            .lock()
            .unwrap()
            .push(request.tools.iter().map(|t| t.name.clone()).collect());
        self.seen_messages
            .lock()
            .unwrap()
            .push(request.messages.as_ref().clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| openalpaca_llm::LlmError::Http("script exhausted".to_string()))
    }
}

fn scripted_router(provider: Arc<ScriptedProvider>) -> Arc<LlmRouter> {
    Arc::new(LlmRouter::single_provider(
        provider,
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    ))
}

/// Production-shaped lead agent: template with `orchestration` capability
/// run through `to_subagent`, so the constraint allowlist is non-empty
/// (capabilities + workspace + coordination grants) — the interesting case
/// for the sandbox-policy extension.
fn lead_subagent() -> SubAgent {
    let agent = crate::test_util::make_agent("lead_agent", vec!["orchestration", "memory_read"]);
    let template = crate::test_util::template_from_agent(&agent);
    template.to_subagent("lead-1", "task-1")
}

struct CountingPluginExec {
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl openalpaca_api::plugin_traits::PluginToolExecutor for CountingPluginExec {
    async fn execute(
        &self,
        _tool_name: &str,
        _arguments: &serde_json::Value,
    ) -> Result<String, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("plugin ok".to_string())
    }

    fn plugin_id(&self) -> &str {
        "plug"
    }
}

fn register_extension_tool(
    registry: &ToolRegistry,
    name: &str,
    backend: crate::tools::registry::ToolBackend,
) {
    registry
        .register(crate::tools::registry::RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: format!("{name} tool"),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                strict: None,
                input_examples: None,
            },
            backend,
            provides_capabilities: vec![name.to_string()],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
}

fn mcp_backend() -> crate::tools::registry::ToolBackend {
    crate::tools::registry::ToolBackend::Mcp {
        client: Arc::new(openalpaca_mcp::McpClient::disconnected_for_tests("srv")),
        remote_name: "echo".to_string(),
        server_name: "srv".to_string(),
        generation: 0,
    }
}

fn fixture_skill_catalog(tmp: &tempfile::TempDir) -> Arc<crate::orchestrator::skill_catalog::SkillCatalog> {
    let dir = tmp.path().join("echo-skill");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: \"Echo Skill\"\ndescription: \"Echoes the query back\"\n---\nEcho the user's query back.\n",
    )
    .unwrap();
    let catalog = Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new());
    catalog.scan_directory(tmp.path(), crate::middleware::skill::SkillScope::Project);
    catalog
}

#[allow(clippy::too_many_arguments)]
async fn run_lead_for_test(
    provider: Arc<ScriptedProvider>,
    tool_registry: Arc<ToolRegistry>,
    skill_catalog: Arc<crate::orchestrator::skill_catalog::SkillCatalog>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    bus: EventBus,
) -> LeadAgentResult {
    run_lead_agent(
        &lead_subagent(),
        "do the thing",
        scripted_router(provider),
        tool_registry,
        Arc::new(SharedContext::new()),
        bus,
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &daemon_config,
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        skill_catalog,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await
}

#[tokio::test]
async fn test_lead_loop_request_carries_extension_defs_and_invoke_skill() {
    let registry = Arc::new(ToolRegistry::default());
    register_extension_tool(&registry, "srv__echo", mcp_backend());
    register_extension_tool(
        &registry,
        "plug::do",
        crate::tools::registry::ToolBackend::Plugin(Arc::new(CountingPluginExec {
            calls: std::sync::atomic::AtomicUsize::new(0),
        })),
    );

    let provider = ScriptedProvider::new(vec![scripted_response("done", vec![])]);
    let result = run_lead_for_test(
        provider.clone(),
        registry,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        EventBus::default(),
    )
    .await;

    assert!(result.success, "finish: {:?}", result.loop_result.finish_reason);
    assert_eq!(provider.calls(), 1);
    let names = provider.tool_names(0);
    for expected in [
        "spawn_subagent",
        "check_subagent_status",
        "wait_for_subagents",
        "workspace_read",
        "workspace_write",
        "srv__echo",
        "plug::do",
        "invoke_skill",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "round-1 request missing {expected}: {names:?}"
        );
    }
    // The system prompt carries the skills/integrations guidance suffix.
    let messages = provider.seen_messages.lock().unwrap();
    let system = &messages[0][0];
    assert!(
        system.content.contains("<skills-and-integrations>"),
        "lead system prompt missing extension guidance"
    );
    assert!(system.content.contains("invoke_skill"));
}

#[tokio::test]
async fn test_lead_executes_extension_tool_through_sandbox() {
    let registry = Arc::new(ToolRegistry::default());
    let exec = Arc::new(CountingPluginExec {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    register_extension_tool(
        &registry,
        "plug::do",
        crate::tools::registry::ToolBackend::Plugin(exec.clone()),
    );
    // An MCP-named tool on the surface too, so the allowlist covers both
    // extension namespaces (execution is exercised via the plugin backend —
    // a disconnected MCP client cannot round-trip in tests).
    register_extension_tool(&registry, "srv__echo", mcp_backend());

    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let provider = ScriptedProvider::new(vec![
        scripted_response(
            "",
            vec![openalpaca_llm::ToolCall {
                id: "call-1".to_string(),
                name: "plug::do".to_string(),
                arguments: serde_json::json!({}),
            }],
        ),
        scripted_response("all done", vec![]),
    ]);

    let result = run_lead_for_test(
        provider.clone(),
        registry,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        bus,
    )
    .await;

    assert!(result.success, "finish: {:?}", result.loop_result.finish_reason);
    assert_eq!(result.final_content, "all done");
    assert_eq!(
        exec.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "plugin executor should run exactly once via the lead sandbox"
    );
    // The sandbox admitted the call — no SecurityViolation for it.
    let mut violations = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let crate::events::SystemEvent::SecurityViolation { tool_name, reason, .. } = e {
            violations.push(format!("{tool_name}: {reason}"));
        }
    }
    assert!(
        violations.is_empty(),
        "unexpected security violations: {violations:?}"
    );
}

#[test]
fn test_plain_subagent_does_not_inherit_extension_tools() {
    use crate::security::capabilities::{Allowlist, CapabilityManager};

    // A non-orchestration worker template with its own declared capability.
    let agent = crate::test_util::make_agent("researcher", vec!["research"]);
    let template = crate::test_util::template_from_agent(&agent);
    let subagent = template.to_subagent("researcher-1", "task-1");

    // The list its *constraints* spell is template-scoped: declared capability
    // + workspace only. (At the spawn path that list is then widened with the
    // subagent's own resolved surface — never the lead's; the spawn-path tests
    // below cover that.)
    let allowed = &subagent.constraints.allowed_capabilities;
    assert!(allowed.iter().any(|c| c == "research"));
    let allowlist = Allowlist::from_agent_constraints(&subagent.constraints);
    let denied = &subagent.constraints.denied_capabilities;
    for blanket in ["invoke_skill", "srv__echo", "plug::do", "spawn_subagent"] {
        assert!(
            !allowed.iter().any(|c| c == blanket),
            "worker allowlist must not carry the lead's blanket grant: {allowed:?}"
        );
        assert!(
            CapabilityManager::check_agent_capability(&subagent.id, blanket, &allowlist, denied)
                .is_err(),
            "worker sandbox must deny undeclared tool {blanket}"
        );
    }
    // Declared tools still pass.
    assert!(
        CapabilityManager::check_agent_capability("researcher-1", "research", &allowlist, denied)
            .is_ok()
    );
}

#[tokio::test]
async fn test_invoke_skill_through_lead_path_runs_fixture_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = fixture_skill_catalog(&tmp);

    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    // Call 1: lead loop asks for invoke_skill. Call 2: the nested skill loop
    // (no tools of its own). Call 3: lead loop synthesizes the final answer.
    let provider = ScriptedProvider::new(vec![
        scripted_response(
            "",
            vec![openalpaca_llm::ToolCall {
                id: "call-1".to_string(),
                name: "invoke_skill".to_string(),
                arguments: serde_json::json!({"skill": "echo-skill", "query": "say hi"}),
            }],
        ),
        scripted_response("nested skill output", vec![]),
        scripted_response("lead done", vec![]),
    ]);

    let result = run_lead_for_test(
        provider.clone(),
        Arc::new(ToolRegistry::default()),
        catalog,
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        bus,
    )
    .await;

    assert!(result.success, "finish: {:?}", result.loop_result.finish_reason);
    assert_eq!(result.final_content, "lead done");
    assert_eq!(provider.calls(), 3, "lead round, nested skill round, lead round");
    // The nested skill loop runs without the lead's tool surface.
    assert!(
        provider.tool_names(1).is_empty(),
        "nested skill loop should carry only the skill's declared tools: {:?}",
        provider.tool_names(1)
    );
    // The skill's output flowed back into the lead conversation as the
    // invoke_skill tool result (visible in the round-2 lead request).
    let messages = provider.seen_messages.lock().unwrap();
    assert!(
        messages[2].iter().any(|m| m.content.contains("nested skill output")),
        "lead round 2 should carry the skill output as a tool result"
    );
    drop(messages);

    // Lifecycle events from the invoke_skill adapter.
    let mut completed = false;
    while let Ok(e) = rx.try_recv() {
        if let crate::events::SystemEvent::SkillCompleted { skill_id, output_preview, .. } = e {
            assert_eq!(skill_id, "Echo Skill");
            assert_eq!(output_preview, "nested skill output");
            completed = true;
        }
    }
    assert!(completed, "missing SkillCompleted event for the nested skill");
}

// ── Session event log (§5.5) ────────────────────────────────────────

/// A subagent lane opens and closes in the run's transcript, beside the 037
/// span writes. The records carry the span id as `span_id`, so `?span_id=` on
/// the reader is a pure filter over one globally ordered log (P-20) — never a
/// per-agent file.
#[tokio::test]
async fn a_subagent_lane_is_narrated_into_the_runs_session_log() {
    use crate::agent::template::{AgentSource, AgentTemplateFrontmatter};
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let executor = Arc::new(CompletingPluginExecutor {
        instructions: std::sync::Mutex::new(None),
    });
    let template = AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id: "plugin_researcher".to_string(),
            name: "Plugin Researcher".to_string(),
            description: "Plugin-backed research agent".to_string(),
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
        source: AgentSource::Plugin {
            plugin_id: "test_plugin".to_string(),
            executor: executor.clone(),
        },
    };

    let shared_context = Arc::new(SharedContext::new());
    assert!(shared_context.agent_registry.register_template(template));

    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-lead");
    // The dispatcher registers the run's handle here; the spawn tool reads it
    // back by task id, exactly as it reads the steering inbox.
    shared_context.register_task_session_log("task-1", handle.clone());

    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        shared_context,
        EventBus::default(),
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    );

    spawn_tool
        .execute(&serde_json::json!({
            "agent_id": "plugin_researcher",
            "objective": "Summarize the design doc"
        }))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !tracker.all_done() {
        assert!(tokio::time::Instant::now() < deadline, "subagent never finished");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(handle.flush().await);

    let records = read_records(&dir.path().join("sess-lead")).unwrap();
    let kinds: Vec<&str> = records.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, vec!["subagent_open", "subagent_close"], "{kinds:?}");

    let open = &records[0];
    assert_eq!(open.task_id.as_deref(), Some("task-1"));
    assert_eq!(open.data["template"], "plugin_researcher");
    assert_eq!(open.data["role"], "subagent");
    assert!(open.data["objective"]
        .as_str()
        .unwrap()
        .contains("Summarize the design doc"));
    // P-17: a plugin-backed lane names the extension that ran it.
    assert_eq!(open.data["plugin_id"], "test_plugin");

    let close = &records[1];
    assert_eq!(close.data["state"], "done");
    assert_eq!(close.data["output_preview"], "plugin result");
    // Both halves name the same 037 span, which is the whole filter.
    assert_eq!(open.span_id, close.span_id);
    assert_eq!(open.span_id.as_deref(), open.data["span_id"].as_str());
}

// ── §5.6c: the resumed run's first request ────────────────────────────

/// A `ReplayPlan` holding one complete round, as `session_log::replay`
/// rebuilds one.
fn replayed_round() -> crate::session_log::replay::ReplayPlan {
    crate::session_log::replay::ReplayPlan {
        messages: vec![
            openalpaca_llm::ChatMessage {
                role: openalpaca_llm::Role::Assistant,
                content: "reading the file".to_string(),
                parts: None,
                tool_calls: Some(vec![openalpaca_llm::ToolCall {
                    id: "tu-1".to_string(),
                    name: "file_read".to_string(),
                    arguments: serde_json::json!({"path": "a.rs"}),
                }]),
                tool_call_id: None,
            },
            openalpaca_llm::ChatMessage::tool_result("tu-1", "fn main() {}"),
        ],
        rounds: 1,
        tool_results: 1,
        from_seq: Some(1),
        to_seq: Some(2),
        last_ts: Some(
            chrono::DateTime::parse_from_rfc3339("2026-09-06T10:00:01Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        ..Default::default()
    }
}

/// The whole shape of a resume, in one request: the persona and the objective
/// are composed fresh, the run's own rounds sit behind them in the order the
/// log recorded them, and the synthetic interjection arrives **through the
/// steering rail** — so the model reads it as the `<user_interjection>` its
/// prompt already teaches it to obey, not as a second objective.
#[tokio::test]
async fn a_resumed_run_is_primed_with_its_replayed_rounds_and_the_interjection() {
    let provider = ScriptedProvider::new(vec![scripted_response("carrying on", vec![])]);
    let config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let inbox = Arc::new(crate::runner::steering::SteeringInbox::new(8));
    let note = crate::session_log::replay::resume_interjection(
        chrono::DateTime::parse_from_rfc3339("2026-09-06T10:00:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    inbox
        .push(crate::runner::steering::SteeringMsg {
            text: note.clone(),
            request_id: uuid::Uuid::new_v4(),
            principal: crate::security::policy::Principal::System,
            scope: crate::security::policy::Scope::Global,
            workspace_path: None,
            received_at: chrono::Utc::now(),
            origin: crate::runner::steering::SteeringOrigin::Daemon,
        })
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    run_lead_agent(
        &lead_subagent(),
        "do the thing",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &config,
        MemoryScopeContext::global_only(),
        None,
        Some(inbox),
        None,
        "lead::task-1",
        "",
        None,
        fixture_skill_catalog(&tmp),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        Some(crate::session_log::replay::ResumeHistory {
            plan: replayed_round(),
            inline_note: None,
        }),
        false,
        None,
    )
    .await;

    let messages = provider.seen_messages.lock().unwrap();
    let first = &messages[0];
    assert_eq!(first[0].role, openalpaca_llm::Role::System, "persona, fresh");
    assert_eq!(
        first[1].content, "do the thing",
        "the objective from the row, not from the log"
    );
    // The replayed round, verbatim — the assistant's `tool_use` and the
    // result that answered it.
    assert_eq!(first[2].role, openalpaca_llm::Role::Assistant);
    assert_eq!(first[2].content, "reading the file");
    assert_eq!(
        first[2].tool_calls.as_ref().unwrap()[0].name,
        "file_read",
        "the recorded call is described, never dispatched"
    );
    assert_eq!(first[3].role, openalpaca_llm::Role::Tool);
    assert_eq!(first[3].content, "fn main() {}");
    // …and the rail's note last, as the daemon's own narration: a
    // `<system_note>`, never a `<user_interjection>` the user never wrote.
    let last = first.last().unwrap();
    assert_eq!(last.role, openalpaca_llm::Role::User);
    assert!(
        last.content.starts_with(crate::runner::steering::SYSTEM_NOTE_PREFIX),
        "the rail wraps it as a system note: {}",
        last.content
    );
    assert!(
        !last.content.contains("user_interjection"),
        "daemon-authored text is never attributed to the user: {}",
        last.content
    );
    assert!(last.content.contains("This run was interrupted at 2026-09-06T10:00:01"));
    assert!(last.content.contains("Do not repeat side-effecting tool calls already recorded."));
}

/// With steering off there is no rail, so the same text has to arrive on the
/// same channel by the only other route: the rebuilt history's last message,
/// wrapped identically — the same `<system_note>`, so the model cannot tell
/// which path carried it.
#[tokio::test]
async fn with_no_steering_rail_the_resume_note_is_still_an_interjection() {
    let provider = ScriptedProvider::new(vec![scripted_response("carrying on", vec![])]);
    let config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let note = crate::session_log::replay::resume_interjection(chrono::Utc::now());

    let tmp = tempfile::tempdir().unwrap();
    run_lead_agent(
        &lead_subagent(),
        "do the thing",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &config,
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        fixture_skill_catalog(&tmp),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        Some(crate::session_log::replay::ResumeHistory {
            plan: replayed_round(),
            inline_note: Some(note),
        }),
        false,
        None,
    )
    .await;

    let messages = provider.seen_messages.lock().unwrap();
    let last = messages[0].last().unwrap();
    assert!(
        last.content.starts_with(crate::runner::steering::SYSTEM_NOTE_PREFIX),
        "{}",
        last.content
    );
    assert!(last.content.contains("Do not repeat side-effecting tool calls already recorded."));
}

/// What the rebuild had to leave out belongs in the **log** as well as in the
/// model's context: the `resume` record names the trim (`trimmed_from_seq`
/// plus its reason) and the rounds no result ever answered, so a later reader
/// of the transcript sees the seam and the gap rather than inferring both
/// from a round count.
#[tokio::test]
async fn the_resume_record_names_the_trim_and_the_rounds_it_dropped() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    let provider = ScriptedProvider::new(vec![scripted_response("carrying on", vec![])]);
    let config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
    let dir = tempfile::tempdir().unwrap();
    let service = SessionLogService::new(
        dir.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    );
    let handle = service.handle_for("sess-resume");

    let plan = crate::session_log::replay::ReplayPlan {
        trimmed_from_seq: Some(40),
        trim_reason: Some("the session log's oldest segments were removed".to_string()),
        dropped_incomplete_rounds: 2,
        ..replayed_round()
    };

    let tmp = tempfile::tempdir().unwrap();
    run_lead_agent(
        &lead_subagent(),
        "do the thing",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &config,
        MemoryScopeContext::global_only(),
        None,
        None,
        Some(handle.clone()),
        "lead::task-1",
        "",
        None,
        fixture_skill_catalog(&tmp),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        Some(crate::session_log::replay::ResumeHistory {
            plan,
            inline_note: None,
        }),
        false,
        None,
    )
    .await;
    assert!(handle.flush().await);

    let records = read_records(&dir.path().join("sess-resume")).unwrap();
    let resume = records
        .iter()
        .find(|r| r.kind == "resume")
        .expect("the resume record names the slice this run was primed from");
    assert_eq!(resume.task_id.as_deref(), Some("task-1"));
    assert_eq!(resume.data["trimmed_from_seq"], 40);
    assert!(
        resume.data["trimmed_reason"]
            .as_str()
            .is_some_and(|r| r.contains("oldest segments")),
        "{}",
        resume.data
    );
    assert_eq!(resume.data["dropped_incomplete_rounds"], 2);
}

// ── M5: the run is budgeted against the model that answers ──────────────

/// A router shaped like an Ollama-only install: the configured default is the
/// Claude id every shipped template pins, Anthropic is **not** loaded, and the
/// one loaded provider holds a local 8 192-token model.
fn ollama_only_router(provider: Arc<ScriptedProvider>) -> Arc<LlmRouter> {
    let router = LlmRouter::single_provider(
        provider,
        openalpaca_llm::ProviderType::Ollama,
        "claude-sonnet-4-20250514".to_string(),
    );
    router.model_registry().register(
        "qwen3:8b".to_string(),
        openalpaca_llm::routing::model_registry::ModelInfo {
            provider: openalpaca_llm::ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 8192,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
        },
    );
    Arc::new(router)
}

/// **M5.** The lead's own prompt budget used to be the pinned model's window
/// — 200 000 for a Claude id no local install can reach — so the loop never
/// compacted and the provider refused the overflowing request. The budget is
/// now the window of the model that actually answers, and the telemetry names
/// that model rather than the pin.
#[tokio::test]
async fn the_lead_is_budgeted_against_the_model_that_answers() {
    let provider = ScriptedProvider::new(vec![scripted_response("done", vec![])]);
    let bus = EventBus::default();
    let mut events = bus.subscribe();

    let result = run_lead_agent(
        &lead_subagent(),
        "do the thing",
        ollama_only_router(provider),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        bus.clone(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(
        result.success,
        "finish: {:?}",
        result.loop_result.finish_reason
    );

    let mut budget = None;
    while let Ok(event) = events.try_recv() {
        if let crate::events::SystemEvent::ContextBudgetComputed {
            model, window_size, ..
        } = event
        {
            budget = Some((model, window_size));
            break;
        }
    }
    let (model, window_size) = budget.expect("the lead publishes its context budget");
    assert_eq!(
        window_size, 8192,
        "the budget is the local model's window, not the unroutable pin's 200 000"
    );
    assert_eq!(
        model, "qwen3:8b",
        "…and the event names the model the window came from"
    );
}

/// **M6.** The declaration travels with the run: a workflow started by a
/// client that cannot answer a confirmation refuses the tool at once, and the
/// model reads the refusal as its tool result — which is how it reaches the
/// completion report.
#[tokio::test]
async fn an_unattended_run_tells_the_model_the_tool_cannot_be_approved() {
    let registry = Arc::new(ToolRegistry::default());
    register_extension_tool(&registry, "srv__echo", mcp_backend());

    // A lead whose template makes that tool confirm-listed.
    let lead = {
        let agent = crate::test_util::make_agent("lead_agent", vec!["orchestration", "srv__echo"]);
        let template = crate::test_util::template_from_agent(&agent);
        let mut lead = template.to_subagent("lead-1", "task-1");
        lead.constraints.require_confirmation_for = vec!["srv__echo".to_string()];
        lead
    };

    let provider = ScriptedProvider::new(vec![
        scripted_response(
            "",
            vec![openalpaca_llm::ToolCall {
                id: "tc_1".to_string(),
                name: "srv__echo".to_string(),
                arguments: serde_json::json!({}),
            }],
        ),
        scripted_response("I could not do that without approval.", vec![]),
    ]);

    let started = std::time::Instant::now();
    let result = run_lead_agent(
        &lead,
        "do the thing",
        scripted_router(provider.clone()),
        registry,
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        // A broker *is* attached — this is not the no-broker fail-closed path.
        Some(Arc::new(
            crate::security::confirmation::ConfirmationBroker::new(),
        )),
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        true,
        None,
    )
    .await;
    let elapsed = started.elapsed();

    assert!(
        result.success,
        "the run finishes and reports; it does not hang: {:?}",
        result.loop_result.finish_reason
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the refusal is immediate, took {elapsed:?}"
    );

    // Round 2's messages carry the tool result the model actually read.
    let second_round = provider.seen_messages.lock().unwrap()[1].clone();
    let saw_refusal = second_round
        .iter()
        .any(|m| m.content.contains("needs your approval"));
    assert!(
        saw_refusal,
        "the model must read why the tool did not run: {:?}",
        second_round
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
    );
}

// ── S3: a subagent's rounds carry a budget ───────────────────────────

/// A provider that can tell the lead's rounds from a subagent's, so a
/// concurrently running subagent cannot consume the lead's next scripted
/// answer. The subagent's system prompt carries the `<scope>` block
/// `spawn_subagent` builds; nothing else does.
struct LeadAndSubagentProvider {
    lead: std::sync::Mutex<std::collections::VecDeque<openalpaca_llm::ChatResponse>>,
    subagent_reply: String,
}

#[async_trait]
impl openalpaca_llm::LlmProvider for LeadAndSubagentProvider {
    fn name(&self) -> &str {
        "lead-and-subagent"
    }
    fn supports_tools(&self) -> bool {
        true
    }
    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        let is_subagent = request
            .messages
            .first()
            .is_some_and(|m| m.content.contains("You are a subagent"));
        if is_subagent {
            return Ok(scripted_response(&self.subagent_reply, vec![]));
        }
        let next = self
            .lead
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front();
        Ok(next.unwrap_or_else(|| scripted_response("done", vec![])))
    }
}

/// **S3.** A subagent's loop was handed `context_budget: None`. Two
/// consequences, one root: its `round` records wrote `"context": null`, so a
/// reader could not say how close the lane was to overflowing, and the loop's
/// budget-aware compaction never ran at all — on an 8 192-token local model
/// the subagent walks into the provider's refusal instead of compacting. The
/// budget is the *answering* model's window (8 192 here, not the template's
/// unroutable Claude pin), with the fixed zone registered.
#[tokio::test]
async fn a_subagents_rounds_are_budgeted_against_the_model_that_answers() {
    use crate::session_log::{SessionLogLimits, SessionLogService, read_records};

    // A worker template for the lead to spawn.
    let shared = Arc::new(SharedContext::new());
    let worker = crate::test_util::make_agent("worker", vec!["memory_read"]);
    shared
        .agent_registry
        .register_template(crate::test_util::template_from_agent(&worker));

    let provider = Arc::new(LeadAndSubagentProvider {
        lead: std::sync::Mutex::new(
            vec![
                scripted_response(
                    "",
                    vec![openalpaca_llm::ToolCall {
                        id: "call-1".to_string(),
                        name: "spawn_subagent".to_string(),
                        arguments: serde_json::json!({
                            "agent_id": "worker",
                            "objective": "count the alpacas",
                        }),
                    }],
                ),
                scripted_response(
                    "",
                    vec![openalpaca_llm::ToolCall {
                        id: "call-2".to_string(),
                        name: "wait_for_subagents".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                ),
                scripted_response("all done", vec![]),
            ]
            .into(),
        ),
        subagent_reply: "seventeen".to_string(),
    });

    let router = LlmRouter::single_provider(
        provider,
        openalpaca_llm::ProviderType::Ollama,
        "claude-sonnet-4-20250514".to_string(),
    );
    router.model_registry().register(
        "qwen3:8b".to_string(),
        openalpaca_llm::routing::model_registry::ModelInfo {
            provider: openalpaca_llm::ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 8192,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
        },
    );

    let logs = tempfile::tempdir().unwrap();
    let service = Arc::new(SessionLogService::new(
        logs.path().to_path_buf(),
        None,
        SessionLogLimits::default(),
        "test".to_string(),
    ));
    let handle = service.handle_for("sess-subagent-budget");
    // The dispatcher registers the run's log so `spawn_subagent` can find it
    // (`SpawnSubagentTool::session_log`); without it a subagent narrates
    // nowhere, whatever budget it was given.
    shared.register_task_session_log("task-1", handle.clone());

    let result = run_lead_agent(
        &lead_subagent(),
        "do the thing",
        Arc::new(router),
        Arc::new(ToolRegistry::default()),
        shared,
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        "user-1",
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        Some(handle.clone()),
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(
        result.success,
        "finish: {:?}",
        result.loop_result.finish_reason
    );
    assert!(handle.flush().await);

    let records = read_records(&logs.path().join("sess-subagent-budget")).unwrap();
    let subagent_rounds: Vec<_> = records
        .iter()
        .filter(|r| r.kind == "round" && r.span_id.as_deref() != Some("lead::task-1"))
        .collect();
    assert!(
        !subagent_rounds.is_empty(),
        "the subagent wrote no round record at all: {:?}",
        records.iter().map(|r| (&r.kind, &r.span_id)).collect::<Vec<_>>()
    );
    for round in &subagent_rounds {
        let context = &round.data["context"];
        assert!(
            !context.is_null(),
            "a subagent round must carry its budget, not null"
        );
        assert_eq!(
            context["window"], 8192,
            "the budget is the answering model's window, not the pin's 200 000"
        );
        assert!(
            context["system_prompt"].as_u64().unwrap_or(0) > 0,
            "the fixed zone is registered, or compaction never sees it: {context}"
        );
    }

    // …and the lead's own rounds keep theirs, fixed zone included (S3).
    let lead_rounds: Vec<_> = records
        .iter()
        .filter(|r| r.kind == "round" && r.span_id.as_deref() == Some("lead::task-1"))
        .collect();
    assert!(!lead_rounds.is_empty(), "the lead wrote no round record");
    for round in &lead_rounds {
        assert_eq!(round.data["context"]["window"], 8192);
        assert!(
            round.data["context"]["system_prompt"].as_u64().unwrap_or(0) > 0,
            "the lead's fixed zone reached the budget it compacts against"
        );
    }
}

// ── A subagent's allow list admits the tools its capabilities resolve to ──

/// A built-in backend that counts its calls and answers with one fixed
/// string. It stands in for the real `web_search` — same tool name, same
/// provided capability — so no test here touches the network.
struct CountingBuiltIn {
    calls: std::sync::atomic::AtomicUsize,
    reply: &'static str,
}

impl CountingBuiltIn {
    fn new(reply: &'static str) -> Arc<Self> {
        Arc::new(Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            reply,
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl crate::tools::registry::BuiltInTool for CountingBuiltIn {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.reply.to_string())
    }
}

/// Register a tool whose NAME differs from the capability it PROVIDES — the
/// shape of `web_search`/`web_fetch` → `web_access`, `memory_search` →
/// `memory_read`, `send` → `messaging`.
fn register_capability_tool(
    registry: &ToolRegistry,
    name: &str,
    capability: &str,
    backend: Arc<CountingBuiltIn>,
) {
    registry
        .register(crate::tools::registry::RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: format!("{name} tool"),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
                strict: None,
                input_examples: None,
            },
            backend: crate::tools::registry::ToolBackend::BuiltIn(backend),
            provides_capabilities: vec![capability.to_string()],
            exempt_from_timeout: false,
            annotations: None,
            version: "test-0.0.0".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
}

/// A file-sourced worker template, as `config/agents/research_agent.md` is.
fn register_worker_template(shared: &SharedContext, id: &str, capabilities: Vec<&str>, denied: Vec<&str>) {
    let agent = crate::test_util::make_agent(id, capabilities);
    let mut template = crate::test_util::template_from_agent(&agent);
    template.frontmatter.denied_capabilities = denied.into_iter().map(str::to_string).collect();
    assert!(matches!(
        template.source,
        crate::agent::template::AgentSource::Internal
    ));
    assert!(shared.agent_registry.register_template(template));
}

/// Drive the production spawn path — `SpawnSubagentTool::execute` — to the end
/// of the subagent's loop, and hand back every security violation the run
/// published.
async fn spawn_and_finish(
    provider: Arc<ScriptedProvider>,
    registry: Arc<ToolRegistry>,
    shared: Arc<SharedContext>,
    agent_id: &str,
) -> Vec<String> {
    let bus = EventBus::new(256);
    let mut rx = bus.subscribe();
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = SpawnSubagentTool::new(
        scripted_router(provider),
        registry,
        shared,
        bus,
        None,
        "task-1".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker.clone(),
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    );

    spawn_tool
        .execute(&serde_json::json!({
            "agent_id": agent_id,
            "objective": "Find out how many alpacas live in Peru"
        }))
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !tracker.all_done() {
        assert!(tokio::time::Instant::now() < deadline, "subagent never finished");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let mut violations = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let crate::events::SystemEvent::SecurityViolation { tool_name, reason, .. } = e {
            violations.push(format!("{tool_name}: {reason}"));
        }
    }
    violations
}

/// The tool results the model was shown in its `call`-th request.
fn tool_results_seen(provider: &ScriptedProvider, call: usize) -> Vec<String> {
    provider.seen_messages.lock().unwrap()[call]
        .iter()
        .filter(|m| m.role == openalpaca_llm::Role::Tool)
        .map(|m| m.content.clone())
        .collect()
}

fn one_tool_call_then_text(tool: &str) -> Arc<ScriptedProvider> {
    ScriptedProvider::new(vec![
        scripted_response(
            "",
            vec![openalpaca_llm::ToolCall {
                id: "call-1".to_string(),
                name: tool.to_string(),
                arguments: serde_json::json!({"query": "alpacas in Peru"}),
            }],
        ),
        scripted_response("about four million", vec![]),
    ])
}

/// A template grants a CAPABILITY (`web_access`); the tools that provide it
/// have their own NAMES (`web_search`, `web_fetch`). The subagent is handed
/// `web_search` on its surface, so its sandbox must admit the call — a tool
/// offered and then refused as "not in allow list" is a research agent that
/// cannot search.
#[tokio::test]
async fn a_subagent_may_call_the_tool_its_capability_resolves_to() {
    let shared = Arc::new(SharedContext::new());
    register_worker_template(&shared, "researcher", vec!["web_access"], vec![]);

    let registry = Arc::new(ToolRegistry::default());
    let web_search = CountingBuiltIn::new("FIXED SEARCH RESULT: 4.1 million alpacas");
    register_capability_tool(&registry, "web_search", "web_access", web_search.clone());

    let provider = one_tool_call_then_text("web_search");
    let violations = spawn_and_finish(provider.clone(), registry, shared, "researcher").await;

    // The subagent really was offered the tool…
    assert!(
        provider.tool_names(0).contains(&"web_search".to_string()),
        "surface: {:?}",
        provider.tool_names(0)
    );
    // …and the result the model read is the tool's, not a refusal.
    let results = tool_results_seen(&provider, 1);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("FIXED SEARCH RESULT"),
        "the model was shown: {:?} (violations: {violations:?})",
        results[0]
    );
    assert_eq!(web_search.calls(), 1, "the backend runs exactly once");
    assert!(violations.is_empty(), "{violations:?}");
}

/// **Deny wins, by capability.** A template that grants `web_access` and
/// denies it again resolves to a surface without `web_search`
/// (`resolve_capabilities` withholds a tool that provides a denied
/// capability), so nothing admits the name — and a model that calls it anyway
/// is refused by the sandbox, not served.
#[tokio::test]
async fn a_denied_capability_keeps_its_tool_off_the_surface_and_out_of_the_allow_list() {
    let shared = Arc::new(SharedContext::new());
    register_worker_template(&shared, "researcher", vec!["web_access"], vec!["web_access"]);

    let registry = Arc::new(ToolRegistry::default());
    let web_search = CountingBuiltIn::new("FIXED SEARCH RESULT");
    register_capability_tool(&registry, "web_search", "web_access", web_search.clone());

    let provider = one_tool_call_then_text("web_search");
    let violations = spawn_and_finish(provider.clone(), registry, shared, "researcher").await;

    assert!(
        !provider.tool_names(0).contains(&"web_search".to_string()),
        "a denied capability's tool must not be offered: {:?}",
        provider.tool_names(0)
    );
    let results = tool_results_seen(&provider, 1);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("not in allow list"),
        "the direct call must be refused: {:?}",
        results[0]
    );
    assert_eq!(web_search.calls(), 0, "a refused tool never runs");
    assert_eq!(violations.len(), 1, "{violations:?}");
}

/// **Deny wins, by tool name.** Denying `web_search` itself leaves the
/// capability's other tools alone and beats the admission the surface would
/// otherwise have earned it: the sandbox reads the deny list first.
#[tokio::test]
async fn a_denied_tool_name_beats_the_surface_that_would_have_admitted_it() {
    let shared = Arc::new(SharedContext::new());
    register_worker_template(&shared, "researcher", vec!["web_access"], vec!["web_search"]);

    let registry = Arc::new(ToolRegistry::default());
    let web_search = CountingBuiltIn::new("FIXED SEARCH RESULT");
    register_capability_tool(&registry, "web_search", "web_access", web_search.clone());

    let provider = one_tool_call_then_text("web_search");
    let violations = spawn_and_finish(provider.clone(), registry, shared, "researcher").await;

    let results = tool_results_seen(&provider, 1);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("Capability 'web_search' denied"),
        "deny is checked before allow: {:?}",
        results[0]
    );
    assert_eq!(web_search.calls(), 0, "a denied tool never runs");
    assert_eq!(violations.len(), 1, "{violations:?}");
}

/// **Fail closed.** The allow list is widened by the subagent's own surface
/// and by nothing else: a tool that is registered but that none of the
/// template's capabilities resolve to is still refused.
#[tokio::test]
async fn a_registered_tool_outside_the_subagents_surface_is_still_refused() {
    let shared = Arc::new(SharedContext::new());
    register_worker_template(&shared, "researcher", vec!["web_access"], vec![]);

    let registry = Arc::new(ToolRegistry::default());
    let web_search = CountingBuiltIn::new("FIXED SEARCH RESULT");
    let shell = CountingBuiltIn::new("SHELL RAN");
    register_capability_tool(&registry, "web_search", "web_access", web_search.clone());
    register_capability_tool(&registry, "shell_execute", "shell_execute", shell.clone());

    let provider = one_tool_call_then_text("shell_execute");
    let violations = spawn_and_finish(provider.clone(), registry, shared, "researcher").await;

    let surface = provider.tool_names(0);
    assert!(surface.contains(&"web_search".to_string()), "{surface:?}");
    assert!(!surface.contains(&"shell_execute".to_string()), "{surface:?}");
    let results = tool_results_seen(&provider, 1);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("Capability 'shell_execute' not in allow list"),
        "{:?}",
        results[0]
    );
    assert_eq!(shell.calls(), 0, "an ungranted tool never runs");
    assert_eq!(web_search.calls(), 0);
    assert_eq!(violations.len(), 1, "{violations:?}");
}

/// A plugin executor that asks for one tool, then completes with whatever the
/// daemon handed back for it.
struct ToolRequestingPluginExecutor {
    tool: &'static str,
    fed_back: std::sync::Mutex<Option<serde_json::Value>>,
}

#[async_trait]
impl openalpaca_api::plugin_traits::PluginAgentExecutor for ToolRequestingPluginExecutor {
    async fn spawn(
        &self,
        _instance_id: &str,
        _task_id: &str,
        _instructions: &str,
        _context: &serde_json::Value,
    ) -> Result<bool, String> {
        Ok(true)
    }

    async fn step(
        &self,
        _instance_id: &str,
        tool_results: Option<&serde_json::Value>,
    ) -> Result<(String, String, Vec<serde_json::Value>), String> {
        match tool_results {
            None => Ok((
                "tool_request".to_string(),
                String::new(),
                vec![serde_json::json!({"tool": self.tool, "arguments": {"query": "alpacas"}})],
            )),
            Some(results) => {
                *self.fed_back.lock().unwrap() = Some(results.clone());
                Ok(("complete".to_string(), "plugin done".to_string(), vec![]))
            }
        }
    }

    async fn stop(&self, _instance_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn plugin_id(&self) -> &str {
        "test_plugin"
    }

    fn agent_id(&self) -> &str {
        "plugin_researcher"
    }
}

/// A plugin-backed template runs no LLM loop, but its tool requests are
/// proxied through the same sandbox under the same policy — so the same rule
/// holds: the capability it was granted admits the tool that provides it.
#[tokio::test]
async fn a_plugin_backed_subagent_may_call_the_tool_its_capability_resolves_to() {
    use crate::agent::template::AgentSource;

    let executor = Arc::new(ToolRequestingPluginExecutor {
        tool: "web_search",
        fed_back: std::sync::Mutex::new(None),
    });
    let agent = crate::test_util::make_agent("plugin_researcher", vec!["web_access"]);
    let mut template = crate::test_util::template_from_agent(&agent);
    template.source = AgentSource::Plugin {
        plugin_id: "test_plugin".to_string(),
        executor: executor.clone(),
    };
    let shared = Arc::new(SharedContext::new());
    assert!(shared.agent_registry.register_template(template));

    let registry = Arc::new(ToolRegistry::default());
    let web_search = CountingBuiltIn::new("FIXED SEARCH RESULT");
    register_capability_tool(&registry, "web_search", "web_access", web_search.clone());

    // No model is ever called on this branch.
    let provider = ScriptedProvider::new(vec![]);
    let violations =
        spawn_and_finish(provider.clone(), registry, shared, "plugin_researcher").await;

    assert_eq!(provider.calls(), 0);
    let fed_back = executor.fed_back.lock().unwrap().clone().expect("no tool result fed back");
    assert_eq!(fed_back[0]["tool"], "web_search");
    assert_eq!(fed_back[0]["result"], "FIXED SEARCH RESULT", "{fed_back}");
    assert_eq!(web_search.calls(), 1);
    assert!(violations.is_empty(), "{violations:?}");
}

/// The lead's `spawn_subagent` logs an 80-byte preview of the objective at
/// INFO — the daemon's default level — *before* the self-spawn and depth
/// guards run, so `&objective[..80]` was a live panic on any objective whose
/// byte 80 fell inside a character. A lead agent's objective is model-authored
/// text, and a Chinese one puts a 3-byte character across that cut.
///
/// `#[traced_test]` matters: `tracing::info!` does not evaluate its field
/// expressions unless a subscriber has enabled the callsite, so without a
/// subscriber this test would pass against the broken code.
#[tokio::test]
#[tracing_test::traced_test]
async fn spawn_subagent_survives_a_multibyte_objective_preview() {
    let tracker = Arc::new(SubagentTracker::new());
    let spawn_tool = SpawnSubagentTool::new(
        Arc::new(openalpaca_llm::LlmRouter::new(
            std::collections::HashMap::new(),
            openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            std::collections::HashMap::new(),
            Arc::new(openalpaca_llm::CostTracker::new(
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
            )),
            "test-model".to_string(),
        )),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        "task-utf8".to_string(),
        "user-1".to_string(),
        "test-lead".to_string(),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        None,
        tracker,
        0,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        MemoryScopeContext::global_only(),
        None,
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::prompt_ctx::section::ContextBundle::empty()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        false,
    );

    // 40 Chinese characters = 120 bytes; byte 80 lands inside the 27th.
    let objective = "请把这些会议记录整理成一份摘要并列出待办事项谢谢你帮忙啊好的没问题再见吧朋友";
    assert!(objective.len() > 80);
    assert!(!objective.is_char_boundary(80));

    // The agent id is the lead's own template, so execute() refuses right
    // after the log line — the preview is what we are exercising, not a spawn.
    let result = spawn_tool
        .execute(&serde_json::json!({
            "agent_id": "test-lead",
            "objective": objective,
        }))
        .await;

    let err = result.expect_err("self-spawn is refused");
    assert!(err.contains("cannot spawn itself"), "{err}");
    assert!(logs_contain("Lead agent spawning subagent"));
}

// ── Memory injection scope (audit finding F1), through the service ────

/// One owner with a global note and one note in each of two projects, all
/// three matching the task description these tests prime with; the
/// temp home store and the in-memory index the service runs on.
struct LeadMemoryScene {
    _env: crate::test_util::HomeStoreGuard,
    root: tempfile::TempDir,
    service: Arc<MemoryService>,
    global: openalpaca_storage::memory_files::NoteId,
    proj_a: openalpaca_storage::memory_files::NoteId,
    proj_b: openalpaca_storage::memory_files::NoteId,
}

const LEAD_OWNER: &str = "user-1";

impl LeadMemoryScene {
    fn project(&self, name: &str) -> std::path::PathBuf {
        self.root.path().join("ws").join(name).canonicalize().unwrap()
    }

    fn workspace(&self, name: &str) -> MemoryScopeContext {
        let root = self.project(name).to_string_lossy().into_owned();
        MemoryScopeContext {
            workspace_id: Some(root.clone()),
            request_workspace_root: Some(root),
        }
    }
}

fn lead_memory_scene() -> LeadMemoryScene {
    use crate::memory::service::StoreInput;
    use openalpaca_storage::memory_files::NoteKind;

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let env = crate::test_util::HomeStoreGuard::set(&home.canonicalize().unwrap());
    let db = openalpaca_storage::Database::open(&root.path().join("db.sqlite")).unwrap();
    openalpaca_storage::IdentityRepository::new(&db)
        .create_global_user(LEAD_OWNER, None)
        .unwrap();
    let service = MemoryService::new(
        Some(db),
        openalpaca_storage::memory_index::MemoryIndex::open_in_memory().unwrap(),
    );
    service.rebuild_index().unwrap();
    for name in ["projA", "projB"] {
        let dir = root.path().join("ws").join(name);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        openalpaca_storage::store::projects::register(&dir.canonicalize().unwrap()).unwrap();
    }
    let store = |workspace: Option<&std::path::Path>, title: &str, body: &str, kind: NoteKind| {
        let hint = match workspace {
            Some(path) => WorkspaceHint::Path(path),
            None => WorkspaceHint::None,
        };
        let ctx = service
            .context(
                LEAD_OWNER,
                hint,
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .unwrap();
        service
            .store(
                &ctx,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(kind),
                    ..Default::default()
                },
            )
            .unwrap()
            .id
    };
    let proj_a_dir = root.path().join("ws").join("projA").canonicalize().unwrap();
    let proj_b_dir = root.path().join("ws").join("projB").canonicalize().unwrap();
    let global = store(
        None,
        "Status reports",
        "# Status reports\n\nThe owner prefers terse status reports.\n",
        NoteKind::Preference,
    );
    let proj_a = store(
        Some(&proj_a_dir),
        "Project A status reports",
        "# Project A status reports\n\nprojA status reports are posted to the release channel.\n",
        NoteKind::Fact,
    );
    let proj_b = store(
        Some(&proj_b_dir),
        "Project B status reports",
        "# Project B status reports\n\nprojB status reports are weekly.\n",
        NoteKind::Fact,
    );
    LeadMemoryScene {
        _env: env,
        root,
        service: Arc::new(service),
        global,
        proj_a,
        proj_b,
    }
}

fn delivered_ids(
    delivery: &Option<(String, Vec<crate::memory::service::MemoryUsed>)>,
) -> Vec<openalpaca_storage::memory_files::NoteId> {
    let (_, used) = delivery.as_ref().expect("something was delivered");
    used.iter().map(|u| u.id).collect()
}

/// A workflow started with no workspace — a connector lane, a scheduled
/// skill, the GUI with no project open — primes the lead with the owner's
/// **global** partition only: no project's notes reach it.
#[test]
fn lead_memory_without_workspace_reads_the_global_partition_only() {
    let scene = lead_memory_scene();
    let ctx = lead_memory_context(
        &scene.service,
        LEAD_OWNER,
        &MemoryScopeContext::global_only(),
        "task-1",
        "user-1:cli",
        "cli",
    )
    .expect("the owner is a global_user row");
    assert_eq!(ctx.project_root, None);
    assert_eq!(ctx.attribution.task_id.as_deref(), Some("task-1"));

    let delivery = lead_memory_delivery(&scene.service, &ctx, "status reports", RecallMode::Index);
    assert_eq!(delivered_ids(&delivery), vec![scene.global]);
    let (block, _) = delivery.unwrap();
    assert!(block.starts_with("<memory_context>\n"), "{block}");
    assert!(block.contains(crate::memory::service::TRUST_LABEL));
    assert!(!block.contains(&scene.proj_a.to_string()));
    assert!(!block.contains(&scene.proj_b.to_string()));
}

/// The workspace branch: a run inside project A reads that project's notes
/// and the global ones, and nothing from project B.
#[test]
fn lead_memory_with_workspace_reads_its_project_and_global() {
    let scene = lead_memory_scene();
    let ctx = lead_memory_context(
        &scene.service,
        LEAD_OWNER,
        &scene.workspace("projA"),
        "task-1",
        "user-1:cli",
        "cli",
    )
    .unwrap();
    assert_eq!(ctx.project_root.as_deref(), Some(scene.project("projA").as_path()));

    let delivery = lead_memory_delivery(&scene.service, &ctx, "status reports", RecallMode::Index);
    let mut ids = delivered_ids(&delivery);
    ids.sort();
    let mut expected = vec![scene.global, scene.proj_a];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(!ids.contains(&scene.proj_b));
}

/// Design 09 §13.2: the lead's opening recall is delivered the way
/// `[orchestrator.memory] recall_mode` says, like every turn's. Under
/// `index`, a task three notes match is primed with the top 2 as excerpts
/// and the third listed under the index block's header, reported with
/// reason `index`; under `excerpts`, all three are excerpts.
#[test]
fn lead_memory_follows_the_recall_mode() {
    use crate::memory::service::{INDEX_BLOCK_HEADER, StoreInput, UsedReason};
    use openalpaca_storage::memory_files::NoteKind;

    let scene = lead_memory_scene();
    let owner = scene
        .service
        .context(
            LEAD_OWNER,
            WorkspaceHint::None,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    for (title, body) in [
        (
            "Status report format",
            "# Status report format\n\nStatus reports lead with the blockers.\n",
        ),
        (
            "Status report audience",
            "# Status report audience\n\nStatus reports go to the whole team.\n",
        ),
    ] {
        scene
            .service
            .store(
                &owner,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(NoteKind::Preference),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    let ctx = lead_memory_context(
        &scene.service,
        LEAD_OWNER,
        &MemoryScopeContext::global_only(),
        "task-1",
        "user-1:cli",
        "cli",
    )
    .unwrap();

    let (block, used) =
        lead_memory_delivery(&scene.service, &ctx, "status reports", RecallMode::Index).unwrap();
    assert_eq!(block.matches(INDEX_BLOCK_HEADER).count(), 1, "{block}");
    let reasons: Vec<UsedReason> = used.iter().map(|u| u.reason).collect();
    assert_eq!(
        reasons,
        [UsedReason::Recall, UsedReason::Recall, UsedReason::Index]
    );

    let (block, used) =
        lead_memory_delivery(&scene.service, &ctx, "status reports", RecallMode::Excerpts).unwrap();
    assert!(!block.contains(INDEX_BLOCK_HEADER), "{block}");
    assert_eq!(used.len(), 3);
    assert!(used.iter().all(|u| u.reason == UsedReason::Recall));
}

/// A `System` run (`/v1/command process`) and an unlinked `provider:id`
/// owner have no partition (design §3): the lead is primed with nothing,
/// and nothing is created for them.
#[test]
fn a_run_whose_owner_is_not_a_global_user_row_has_no_memory_context() {
    let scene = lead_memory_scene();
    for owner in ["system", "telegram:12345", ""] {
        assert!(
            lead_memory_context(
                &scene.service,
                owner,
                &MemoryScopeContext::global_only(),
                "task-1",
                "user-1:cli",
                "cli",
            )
            .is_none(),
            "{owner}"
        );
    }
}

/// **Plan 3.3, one profile injection.** The lead prompt carries the daemon's
/// `USER.md` exactly once — rendered by the persona layer's own block, right
/// after the lead persona — and nothing at all when there is no document.
#[test]
fn the_lead_prompt_carries_the_user_document_exactly_once() {
    let engine = crate::compose::ComposeEngine::new(16);
    let doc = crate::middleware::user::parse_user_markdown(
        crate::test_util::PROFILE_DOC,
    )
    .unwrap();
    let prompt = build_lead_agent_prompt_from_templates(
        &engine,
        "Test persona",
        &[],
        200_000,
        Some(&doc),
        Some(1000),
    );
    assert_eq!(prompt.matches("### USER PROFILE ###").count(), 1, "{prompt}");
    assert!(prompt.contains("Name: Alex"), "{prompt}");
    let persona_at = prompt.find("Test persona").unwrap();
    let profile_at = prompt.find("### USER PROFILE ###").unwrap();
    let role_at = prompt.find("<role>").unwrap();
    assert!(persona_at < profile_at && profile_at < role_at, "{prompt}");

    let without =
        build_lead_agent_prompt_from_templates(&engine, "Test persona", &[], 200_000, None, None);
    assert_eq!(without.matches("### USER PROFILE ###").count(), 0);
}

/// End to end: the lead's first request carries its profile once in the
/// system prompt, its memory as one quoted-data user message before the
/// objective, and the delivery lands in the ring under the task id.
#[tokio::test]
async fn a_lead_run_is_primed_with_its_memory_once_and_its_profile_once() {
    let scene = lead_memory_scene();
    let provider = ScriptedProvider::new(vec![scripted_response("done", vec![])]);
    let doc = crate::middleware::user::parse_user_markdown(
        crate::test_util::PROFILE_DOC,
    )
    .unwrap();
    let daemon_config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));

    let result = run_lead_agent(
        &lead_subagent(),
        "write the status reports",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        Some(scene.service.clone()),
        Some(doc),
        "task-1",
        LEAD_OWNER,
        "user-1:cli",
        "cli",
        &daemon_config,
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(result.success);

    let seen = provider.seen_messages.lock().unwrap();
    let messages = &seen[0];
    assert!(matches!(messages[0].role, openalpaca_llm::Role::System));
    assert_eq!(
        messages[0].content.matches("### USER PROFILE ###").count(),
        1,
        "{}",
        messages[0].content
    );
    let profile_copies: usize = messages
        .iter()
        .map(|m| m.content.matches("### USER PROFILE ###").count())
        .sum();
    assert_eq!(profile_copies, 1);
    assert!(matches!(messages[1].role, openalpaca_llm::Role::User));
    assert!(messages[1].content.starts_with("<memory_context>\n"), "{}", messages[1].content);
    assert!(messages[1].content.contains(&scene.global.to_string()));
    assert!(!messages[1].content.contains(&scene.proj_a.to_string()));
    assert_eq!(messages[2].content, "write the status reports");

    let ring = scene.service.recently_used(10);
    assert_eq!(ring.len(), 1);
    assert_eq!(ring[0].id, scene.global);
    assert_eq!(ring[0].task_id.as_deref(), Some("task-1"));
    assert_eq!(ring[0].lane_key.as_deref(), Some("user-1:cli"));
}

/// The lead's opening memory message on `scene` for "write the status
/// reports", through the real run under `config`.
async fn lead_opening_memory(scene: &LeadMemoryScene, config: DaemonConfig) -> String {
    let provider = ScriptedProvider::new(vec![scripted_response("done", vec![])]);
    let daemon_config = Arc::new(ArcSwap::from_pointee(config));
    let result = run_lead_agent(
        &lead_subagent(),
        "write the status reports",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        None,
        Some(scene.service.clone()),
        None,
        "task-1",
        LEAD_OWNER,
        "user-1:cli",
        "cli",
        &daemon_config,
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(result.success);
    let seen = provider.seen_messages.lock().unwrap();
    let message = &seen[0][1];
    assert!(
        message.content.starts_with("<memory_context>\n"),
        "{}",
        message.content
    );
    message.content.clone()
}

/// End to end, the lead reads `[orchestrator.memory] recall_mode` from the
/// daemon config it is started with (L10b review, finding 3): three notes
/// match, so the two modes differ — the default `index` primes the run with
/// the index block's header, `excerpts` without it.
#[tokio::test]
async fn a_lead_run_is_primed_in_the_configured_recall_mode() {
    use crate::memory::service::{INDEX_BLOCK_HEADER, StoreInput};
    use openalpaca_storage::memory_files::NoteKind;

    let scene = lead_memory_scene();
    let owner = scene
        .service
        .context(
            LEAD_OWNER,
            WorkspaceHint::None,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    for (title, body) in [
        (
            "Status report format",
            "# Status report format\n\nStatus reports lead with the blockers.\n",
        ),
        (
            "Status report audience",
            "# Status report audience\n\nStatus reports go to the whole team.\n",
        ),
    ] {
        scene
            .service
            .store(
                &owner,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(NoteKind::Preference),
                    ..Default::default()
                },
            )
            .unwrap();
    }

    let index = lead_opening_memory(&scene, DaemonConfig::default()).await;
    assert_eq!(index.matches(INDEX_BLOCK_HEADER).count(), 1, "{index}");

    let mut config = DaemonConfig::default();
    config.orchestrator.memory.recall_mode = RecallMode::Excerpts;
    let excerpts = lead_opening_memory(&scene, config).await;
    assert!(!excerpts.contains(INDEX_BLOCK_HEADER), "{excerpts}");
    assert_eq!(excerpts.matches("[/memory]\n").count(), 3, "{excerpts}");
}

// ── Design 09 §6.2: a subagent's notes reach *recently used* ──────────

/// A provider for a lead that spawns one worker, and a worker that searches
/// memory once and then answers. It keeps the tools the worker was offered.
struct SubagentSearchesMemory {
    lead: std::sync::Mutex<std::collections::VecDeque<openalpaca_llm::ChatResponse>>,
    subagent: std::sync::Mutex<std::collections::VecDeque<openalpaca_llm::ChatResponse>>,
    subagent_tools: std::sync::Mutex<Vec<String>>,
    lead_tools: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl openalpaca_llm::LlmProvider for SubagentSearchesMemory {
    fn name(&self) -> &str {
        "subagent-searches-memory"
    }
    fn supports_tools(&self) -> bool {
        true
    }
    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        let is_subagent = request
            .messages
            .first()
            .is_some_and(|m| m.content.contains("You are a subagent"));
        let queue = if is_subagent {
            let mut offered = self
                .subagent_tools
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if offered.is_empty() {
                offered.extend(request.tools.iter().map(|t| t.name.clone()));
            }
            &self.subagent
        } else {
            let mut offered = self.lead_tools.lock().unwrap_or_else(|p| p.into_inner());
            if offered.is_empty() {
                offered.extend(request.tools.iter().map(|t| t.name.clone()));
            }
            &self.lead
        };
        let next = queue.lock().unwrap_or_else(|p| p.into_inner()).pop_front();
        Ok(next.unwrap_or_else(|| scripted_response("done", vec![])))
    }
}

/// Design 09 §6.2 (LR15), on the real spawn path: a subagent reaches memory
/// only through its own memory tools, and its `memory_search` lands in the
/// recently-used ring carrying the run's `task_id` — `ToolContext.task_id`
/// is set for every subagent and the service copies it into the delivery's
/// attribution. The worker's template names no memory capability, so the
/// search is there by the ambient grant (§5). The lead is given no memory
/// service, so nothing it recalls can put a row in the ring.
#[tokio::test]
async fn a_subagents_memory_search_records_its_task_id_in_the_ring() {
    let scene = lead_memory_scene();
    let shared = Arc::new(SharedContext::new());
    let worker = crate::test_util::make_agent("worker", vec![]);
    shared
        .agent_registry
        .register_template(crate::test_util::template_from_agent(&worker));
    // The read surface is registered globally in production; mirror that.
    let registry = ToolRegistry::default();
    for tool in crate::tools::builtins::memory_read_tools(scene.service.clone()) {
        registry.register(tool).unwrap();
    }

    let provider = Arc::new(SubagentSearchesMemory {
        lead: std::sync::Mutex::new(
            vec![
                scripted_response(
                    "",
                    vec![openalpaca_llm::ToolCall {
                        id: "call-1".to_string(),
                        name: "spawn_subagent".to_string(),
                        arguments: serde_json::json!({
                            "agent_id": "worker",
                            "objective": "find out how the owner wants status reports",
                        }),
                    }],
                ),
                scripted_response(
                    "",
                    vec![openalpaca_llm::ToolCall {
                        id: "call-2".to_string(),
                        name: "wait_for_subagents".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                ),
                scripted_response("terse, as noted", vec![]),
            ]
            .into(),
        ),
        subagent: std::sync::Mutex::new(
            vec![
                scripted_response(
                    "",
                    vec![openalpaca_llm::ToolCall {
                        id: "sub-call-1".to_string(),
                        name: "memory_search".to_string(),
                        arguments: serde_json::json!({ "query": "status reports" }),
                    }],
                ),
                scripted_response("The owner prefers terse status reports.", vec![]),
            ]
            .into(),
        ),
        subagent_tools: std::sync::Mutex::new(Vec::new()),
        lead_tools: std::sync::Mutex::new(Vec::new()),
    });
    let router = LlmRouter::single_provider(
        provider.clone(),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-4-20250514".to_string(),
    );

    let result = run_lead_agent(
        &lead_subagent(),
        "how should the status reports read?",
        Arc::new(router),
        Arc::new(registry),
        shared,
        EventBus::default(),
        None,
        None,
        None,
        "task-1",
        LEAD_OWNER,
        "user-1:cli",
        "cli",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        None,
    )
    .await;
    assert!(
        result.success,
        "finish: {:?}",
        result.loop_result.finish_reason
    );
    let offered = provider.subagent_tools.lock().unwrap().clone();
    assert!(
        offered.iter().any(|t| t == "memory_search"),
        "the worker was offered memory_search: {offered:?}"
    );
    // Design 09 §13.1: the search answers an index, so the read that
    // follows it — and the timeline — come with it, on the worker's surface
    // (the `memory_read` capability's) and on the lead's hand-built one.
    let lead_offered = provider.lead_tools.lock().unwrap().clone();
    for name in ["memory_search", "memory_read", "memory_timeline"] {
        assert!(
            offered.iter().any(|t| t == name),
            "the worker was offered {name}: {offered:?}"
        );
        assert!(
            lead_offered.iter().any(|t| t == name),
            "the lead was offered {name}: {lead_offered:?}"
        );
    }

    let ring = scene.service.recently_used(10);
    assert_eq!(ring.len(), 1, "{ring:?}");
    assert_eq!(ring[0].id, scene.global);
    assert_eq!(ring[0].reason, crate::memory::service::UsedReason::Tool);
    assert_eq!(ring[0].task_id.as_deref(), Some("task-1"));
    // Subagents are lane-detached: the row names the run, not a lane.
    assert!(ring[0].lane_key.is_none(), "{:?}", ring[0].lane_key);
}

// ── Design 11 §4, CQ-6: the chat's working state reaches the lead ────

/// The lead run over `db`, started from `session_id`, answering "done"; the
/// messages of its one request.
async fn lead_first_request(
    db: &openalpaca_storage::Database,
    session_id: Option<&str>,
) -> Vec<openalpaca_llm::ChatMessage> {
    let provider = ScriptedProvider::new(vec![scripted_response("done", vec![])]);
    let result = run_lead_agent(
        &lead_subagent(),
        "Book the Alfama hotel",
        scripted_router(provider.clone()),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        Some(db.clone()),
        None,
        None,
        "task-1",
        "user-1",
        "user-1:gui",
        "gui",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        session_id,
    )
    .await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);
    provider.seen_messages.lock().unwrap()[0].clone()
}

/// Owner decision CQ-6: a run started from a conversation is given that
/// conversation's working state — one block, right after its objective, as
/// `WorkingState::render` writes it; a run from a conversation with no state,
/// or from none, is given no block.
#[tokio::test]
async fn the_lead_is_given_the_chats_working_state_after_its_objective() {
    use crate::orchestrator::working_state::{BLOCK_PREFIX, VERSION, WorkingState};

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
    let repo = openalpaca_storage::ConversationRepository::new(&db);
    let session = repo
        .get_or_create_active_session("user-1:gui", "gui", None)
        .unwrap();
    let blocks = |messages: &[openalpaca_llm::ChatMessage]| {
        messages
            .iter()
            .filter(|m| m.content.starts_with(BLOCK_PREFIX))
            .count()
    };
    assert_eq!(
        blocks(&lead_first_request(&db, Some(&session.id)).await),
        0,
        "a conversation with no state gives no block"
    );

    let state = WorkingState {
        v: VERSION,
        topic: "Lisbon trip".to_string(),
        goal: "Book the October trip".to_string(),
        done: vec!["Flight booked".to_string()],
        asked: Vec::new(),
        decisions: vec!["A hotel in Alfama, not Baixa".to_string()],
        open: Vec::new(),
        next: "Book the Alfama hotel".to_string(),
        refs: Vec::new(),
    };
    assert!(
        repo.update_working_state(&session.id, 0, &serde_json::to_string(&state).unwrap(), 0)
            .unwrap()
    );
    let updated_at = repo
        .get_session(&session.id)
        .unwrap()
        .unwrap()
        .working_state_updated_at
        .unwrap();

    let messages = lead_first_request(&db, Some(&session.id)).await;
    let objective = messages
        .iter()
        .position(|m| m.content == "Book the Alfama hotel")
        .expect("the objective");
    assert_eq!(blocks(&messages), 1, "one working-state block");
    assert_eq!(
        messages[objective + 1].content,
        state.render(&updated_at, 4000),
        "the block follows the objective"
    );
    assert_eq!(
        blocks(&lead_first_request(&db, None).await),
        0,
        "no conversation, no block"
    );
}

/// A lead whose rounds are scripted and whose summariser call — told apart
/// by the working-state prompt's system text — answers `state`; every
/// summariser prompt and every lead request is kept.
struct LeadCompactionProvider {
    rounds: std::sync::Mutex<std::collections::VecDeque<openalpaca_llm::ChatResponse>>,
    state: String,
    writer_prompts: std::sync::Mutex<Vec<String>>,
    lead_requests: std::sync::Mutex<Vec<Vec<openalpaca_llm::ChatMessage>>>,
}

#[async_trait]
impl openalpaca_llm::LlmProvider for LeadCompactionProvider {
    fn name(&self) -> &str {
        "lead-compaction"
    }
    fn supports_tools(&self) -> bool {
        true
    }
    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        let (system, _) = crate::orchestrator::working_state::render_prompt(None, "");
        if request
            .messages
            .first()
            .is_some_and(|m| m.content == system)
        {
            self.writer_prompts
                .lock()
                .unwrap()
                .push(request.messages[1].content.clone());
            return Ok(scripted_response(&self.state, vec![]));
        }
        self.lead_requests
            .lock()
            .unwrap()
            .push(request.messages.as_ref().clone());
        let next = self.rounds.lock().unwrap().pop_front();
        Ok(next.unwrap_or_else(|| scripted_response("done", vec![])))
    }
}

/// Design 11 §3.3 with CQ-6: the lead's compaction folds its older messages
/// into the chat's working state it was handed — the summariser reads that
/// state as the previous one — and the list then holds the new state's block
/// alone, the chat's taken out.
#[tokio::test]
async fn the_leads_compaction_folds_into_the_chats_working_state() {
    use crate::orchestrator::working_state::{BLOCK_PREFIX, VERSION, WorkingState};

    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("t.db")).unwrap();
    let repo = openalpaca_storage::ConversationRepository::new(&db);
    let session = repo
        .get_or_create_active_session("user-1:gui", "gui", None)
        .unwrap();
    let chat = WorkingState {
        v: VERSION,
        topic: "Lisbon trip".to_string(),
        goal: "Book the October trip".to_string(),
        done: vec!["Flight booked".to_string()],
        asked: Vec::new(),
        decisions: vec!["A hotel in Alfama, not Baixa".to_string()],
        open: Vec::new(),
        next: "Book the Alfama hotel".to_string(),
        refs: Vec::new(),
    };
    let chat_json = serde_json::to_string(&chat).unwrap();
    assert!(
        repo.update_working_state(&session.id, 0, &chat_json, 0)
            .unwrap()
    );

    let check = |content: &str| {
        scripted_response(
            content,
            vec![openalpaca_llm::ToolCall {
                id: "call".to_string(),
                name: "check_subagent_status".to_string(),
                arguments: serde_json::json!({ "subagent_run_id": "none" }),
            }],
        )
    };
    // Two short rounds, then one whose ~7 500 tokens put the next round past
    // the 8 192-token window's summary tier.
    let provider = Arc::new(LeadCompactionProvider {
        rounds: std::sync::Mutex::new(
            vec![check("first"), check("second"), check(&"x".repeat(30_000))].into(),
        ),
        state: r#"{"topic":"Lisbon hotel booking","goal":"Book the October trip","done":["Flight booked","Checked the subagents"],"decisions":["A hotel in Alfama, not Baixa"],"open":[],"next":"Book the Alfama hotel","refs":[]}"#.to_string(),
        writer_prompts: std::sync::Mutex::new(Vec::new()),
        lead_requests: std::sync::Mutex::new(Vec::new()),
    });
    let router = LlmRouter::single_provider(
        provider.clone(),
        openalpaca_llm::ProviderType::Ollama,
        "claude-sonnet-4-20250514".to_string(),
    );
    router.model_registry().register(
        "qwen3:8b".to_string(),
        openalpaca_llm::routing::model_registry::ModelInfo {
            provider: openalpaca_llm::ProviderType::Ollama,
            input_price_per_million: 0.0,
            output_price_per_million: 0.0,
            context_window: 8192,
            discovered: true,
            supports_image: false,
            supports_audio: false,
            supports_document: false,
            supports_reasoning: false,
            supports_tools: true,
            declared: false,
        },
    );

    let result = run_lead_agent(
        &lead_subagent(),
        "Book the Alfama hotel",
        Arc::new(router),
        Arc::new(ToolRegistry::default()),
        Arc::new(SharedContext::new()),
        EventBus::default(),
        Some(db.clone()),
        None,
        None,
        "task-1",
        "user-1",
        "user-1:gui",
        "gui",
        &Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        MemoryScopeContext::global_only(),
        None,
        None,
        None,
        "lead::task-1",
        "",
        None,
        Arc::new(crate::orchestrator::skill_catalog::SkillCatalog::new()),
        Arc::new(crate::prompt_ctx::ContextManager::noop()),
        Arc::new(crate::compose::ComposeEngine::new(16)),
        None,
        false,
        Some(&session.id),
    )
    .await;
    assert!(result.success, "{:?}", result.loop_result.finish_reason);

    let prompts = provider.writer_prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1, "one summariser call");
    assert!(
        prompts[0].contains(&format!("<previous_state>\n{chat_json}\n</previous_state>")),
        "the chat's state is the previous one: {}",
        prompts[0]
    );
    let requests = provider.lead_requests.lock().unwrap().clone();
    let last = requests.last().unwrap();
    let blocks: Vec<&str> = last
        .iter()
        .filter(|m| m.content.starts_with(BLOCK_PREFIX))
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(blocks.len(), 1, "one state block after the compaction");
    assert!(
        blocks[0].contains("Topic: Lisbon hotel booking"),
        "{}",
        blocks[0]
    );
}
