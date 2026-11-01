use super::{detect_active_send_hints, sanitize_parts_for_dispatch};
use openalpaca_llm::{ChatMessage, ContentPart, ImageSource};

#[test]
fn sanitize_parts_for_dispatch_drops_empty_text_parts() {
    let parts = vec![
        ContentPart::Text {
            text: "".to_string(),
        },
        ContentPart::Text {
            text: "  \n\t".to_string(),
        },
        ContentPart::Text {
            text: "keep me".to_string(),
        },
    ];

    let sanitized = sanitize_parts_for_dispatch(parts);
    assert_eq!(sanitized.len(), 1);
    match &sanitized[0] {
        ContentPart::Text { text } => assert_eq!(text, "keep me"),
        other => panic!("expected text part, got {other:?}"),
    }
}

#[test]
fn sanitize_parts_for_dispatch_replaces_unresolved_file_asset_images() {
    let parts = vec![ContentPart::Image {
        source: ImageSource::FileAsset {
            file_id: "f1".to_string(),
            media_type: "image/jpeg".to_string(),
        },
        detail: None,
    }];

    let sanitized = sanitize_parts_for_dispatch(parts);
    assert_eq!(sanitized.len(), 1);
    match &sanitized[0] {
        ContentPart::Text { text } => {
            assert_eq!(text, "[image attached — unresolved file asset reference]")
        }
        other => panic!("expected placeholder text, got {other:?}"),
    }
}

// --- detect_active_send_hints tests ---

#[test]
fn detect_send_flow_with_telegram_and_send_keyword() {
    let messages = vec![ChatMessage::assistant(
        "I can help you send a message via Telegram using the send tool.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_no_channel_no_send() {
    let messages = vec![ChatMessage::assistant(
        "Sure, I'll help you with that task.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_empty_messages() {
    assert!(!detect_active_send_hints(&[], &[]).send);
}

#[test]
fn detect_send_flow_user_only_messages() {
    let messages = vec![ChatMessage::user(
        "send message to telegram",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_channel_without_send_keyword() {
    let messages = vec![ChatMessage::assistant(
        "Telegram is a messaging platform.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_chinese_context_without_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram发送消息给您的联系人。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_chinese_context_with_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram使用`send`工具发送消息。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_only_recent_messages() {
    let mut messages = Vec::new();
    messages.push(ChatMessage::assistant(
        "I'll send via Telegram using `send`.",
    ));
    for _ in 0..2 {
        messages.push(ChatMessage::assistant("Here is some other info."));
    }
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_survives_bursty_user_messages() {
    let messages = vec![
        ChatMessage::assistant("你的 Telegram 收件人是？"),
        ChatMessage::user("等一下"),
        ChatMessage::user("用 default"),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_discussion_about_telegram_no_tool() {
    let messages = vec![ChatMessage::assistant(
        "Telegram is great for groups and channels.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_tier2_chinese_recipient_solicitation() {
    let messages = vec![ChatMessage::assistant(
        "你的 Telegram 收件人是？",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_tier2_english_recipient_solicitation() {
    let messages = vec![ChatMessage::assistant(
        "Who should I send it to on Telegram?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_tier2_no_match_without_recipient_keyword() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram发送消息给您的联系人。",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(!hints.send);
}

#[test]
fn detect_send_flow_with_quoted_send_tool_name() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool to send your photo via Telegram.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn detect_send_flow_send_parens() {
    let messages = vec![ChatMessage::assistant(
        "Let me call send( for your iMessage attachment.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_followup_with_send_tool() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool to send your photo via Telegram. What's the recipient?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_followup_with_call_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll call send to send your text via Telegram. Who should I send it to?",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_send_tool_mentioned_twice() {
    let messages = vec![ChatMessage::assistant(
        "I can use send for text or call send for attachments via Telegram.",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_recipient_only_defaults_to_send() {
    let messages = vec![ChatMessage::assistant(
        "你的 Telegram 收件人是？",
    )];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn keepalive_tier2_cross_message_backtrack() {
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via Telegram.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    assert!(hints.send);
}

#[test]
fn tier2_cross_channel_still_detects_send() {
    // Both turns mention send-related content → send should be active
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via iMessage.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let hints = detect_active_send_hints(&messages, &[]);
    // Turn N-1 has "send" (Tier 1), Turn N has Tier 2 → both set send=true
    assert!(hints.send);
}

// --- resolve_send_tool_choice unit tests ---

use super::resolve_send_tool_choice;
use openalpaca_llm::ToolChoice;

#[test]
fn tool_choice_send_present_pins_send() {
    let choice = resolve_send_tool_choice(true);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn tool_choice_no_send_returns_none() {
    let choice = resolve_send_tool_choice(false);
    assert_eq!(choice, None);
}

// --- apply_send_keepalive direct tests ---

use super::apply_send_keepalive;

#[test]
fn detect_send_hints_custom_channel_via_active_channels() {
    // "matrix" is NOT in FALLBACK_CHANNEL_KW
    let messages = vec![ChatMessage::assistant(
        "Your Matrix recipient is?",
    )];
    // With active_channels containing "matrix": should detect Tier 2 hit
    let active = vec!["matrix".to_string()];
    let hints = detect_active_send_hints(&messages, &active);
    assert!(hints.send);
    // Without: should NOT detect (proves dynamic path works)
    let hints_empty = detect_active_send_hints(&messages, &[]);
    assert!(!hints_empty.send);
}

#[test]
fn apply_keepalive_injects_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool via Telegram.",
    )];
    let mut tool_names = vec!["web_fetch".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
}

#[test]
fn apply_keepalive_skips_when_intent_has_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the send tool via Telegram.",
    )];
    let mut tool_names = vec!["send".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(intent_send);
    // No duplicate
    assert_eq!(
        tool_names.iter().filter(|t| *t == "send").count(),
        1
    );
}

#[test]
fn apply_keepalive_injects_send_when_hinted() {
    let messages = vec![ChatMessage::assistant(
        "I can use send for text or call send for attachments via Telegram.",
    )];
    let mut tool_names = vec![];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
}

// --- R48: file_edit beside file_write on a project turn ---

use super::offer_file_edit_beside_file_write;

#[test]
fn a_project_turn_that_suggests_file_write_is_offered_file_edit_once() {
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("edit src/main.rs so the retry count is 5");
    assert!(
        tool_names.contains(&"file_write".to_string()),
        "{tool_names:?}"
    );
    offer_file_edit_beside_file_write(&mut tool_names, true);
    offer_file_edit_beside_file_write(&mut tool_names, true);
    assert_eq!(
        tool_names.iter().filter(|n| *n == "file_edit").count(),
        1,
        "{tool_names:?}"
    );
}

#[test]
fn a_turn_with_no_project_is_not_offered_file_edit() {
    let mut tool_names = vec!["file_write".to_string()];
    offer_file_edit_beside_file_write(&mut tool_names, false);
    assert_eq!(tool_names, vec!["file_write".to_string()]);
}

#[test]
fn a_project_turn_that_does_not_suggest_file_write_is_not_offered_file_edit() {
    let mut tool_names = vec!["web_search".to_string()];
    offer_file_edit_beside_file_write(&mut tool_names, true);
    assert_eq!(tool_names, vec!["web_search".to_string()]);
}

// --- R42 and the lead's rulings R-T6, R-T14: the file readers ---

use super::offer_file_readers;

#[test]
fn a_project_turn_is_offered_file_read_and_file_list_once() {
    let mut tool_names = vec!["web_search".to_string()];
    offer_file_readers(&mut tool_names, true);
    offer_file_readers(&mut tool_names, true);
    assert_eq!(tool_names, ["web_search", "file_read", "file_list"]);
}

#[test]
fn file_list_comes_wherever_file_read_is() {
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("read file notes.md");
    assert!(tool_names.contains(&"file_read".to_string()), "{tool_names:?}");
    offer_file_readers(&mut tool_names, false);
    assert_eq!(
        tool_names.iter().filter(|n| *n == "file_list").count(),
        1,
        "{tool_names:?}"
    );
}

#[test]
fn a_turn_with_no_project_and_no_file_read_is_offered_no_reader() {
    let mut tool_names = vec!["web_search".to_string()];
    offer_file_readers(&mut tool_names, false);
    assert_eq!(tool_names, ["web_search"]);
}

// --- end-to-end conflict path: intent + keep-alive → tool_choice ---

#[test]
fn keepalive_plain_text_continuation_resolves_to_send() {
    let messages = vec![ChatMessage::assistant(
        "I'll use the \"send\" tool via Telegram.",
    )];
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("好的，发吧");
    let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

    let has_send = tool_names.contains(&"send".to_string());
    let choice = resolve_send_tool_choice(has_send);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn keepalive_text_send_continuation_resolves_to_send() {
    let messages = vec![ChatMessage::assistant(
        "好的，我将通过Telegram使用`send`工具发送消息。",
    )];
    let parser = crate::orchestrator::intent::IntentParser;
    let mut tool_names = parser.suggest_tools("发消息给他");
    let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

    let has_send = tool_names.contains(&"send".to_string());
    let choice = resolve_send_tool_choice(has_send);
    assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
}

#[test]
fn apply_keepalive_cross_channel_injects_send() {
    let messages = vec![
        ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via iMessage.",
        ),
        ChatMessage::user("等一下"),
        ChatMessage::assistant(
            "好的，你的 Telegram 收件人是？",
        ),
    ];
    let mut tool_names = vec!["web_fetch".to_string()];
    let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
    assert!(!intent_send);
    assert!(tool_names.contains(&"send".to_string()));
    assert_eq!(tool_names.len(), 2); // web_fetch + send
}

/// N-TOOL-1 (Z1-05), through the front door: the main loop's sandbox cuts a
/// tool at `[execution.agent_defaults] max_tool_runtime_secs`, and the
/// `shell_execute` definition the model is handed states that same figure —
/// 60 s by default, and whatever the key is set to.
#[cfg(unix)]
mod main_loop_tool_timeout {
    use crate::bus::EventBus;
    use crate::context::SharedContext;
    use crate::daemon_config::DaemonConfig;
    use crate::gateway::HandleRequest;
    use crate::lane::LaneManager;
    use crate::middleware::prompt::SystemPersona;
    use crate::orchestrator::{Orchestrator, skill_catalog, skill_router};
    use crate::runner::LoopConfig;
    use crate::security::gate::SecurityGate;
    use crate::security::policy::{Principal, Scope};
    use crate::security::sandbox::SandboxManager;
    use crate::test_util::{
        GONE_WITHIN, GuardedProject, KillOnFail, pid_from_file, wait_until_gone,
    };
    use crate::tools::ToolRegistry;
    use arc_swap::ArcSwap;
    use async_trait::async_trait;
    use openalpaca_llm::{
        ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType,
        Role, ToolCall, Usage,
    };
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use uuid::Uuid;

    /// Calls `shell_execute` once with `command` (when given), then answers;
    /// keeps every request it was handed.
    struct CallsTheShell {
        command: Option<String>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    #[async_trait]
    impl LlmProvider for CallsTheShell {
        fn name(&self) -> &str {
            "calls-the-shell"
        }
        fn supports_tools(&self) -> bool {
            true
        }
        async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
            let first = {
                let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
                seen.push(request);
                seen.len() == 1
            };
            let (tool_calls, finish_reason, content) = match (&self.command, first) {
                (Some(command), true) => (
                    vec![ToolCall {
                        id: "tc_shell".to_string(),
                        name: "shell_execute".to_string(),
                        arguments: serde_json::json!({ "command": command }),
                    }],
                    FinishReason::ToolUse,
                    String::new(),
                ),
                _ => (vec![], FinishReason::Stop, "done".to_string()),
            };
            Ok(ChatResponse {
                content,
                tool_calls,
                model: "mock-model".to_string(),
                usage: Usage::default(),
                finish_reason,
                thinking: None,
                parts: None,
            })
        }
    }

    /// The main loop's surface is the whole registry ("full"), and a
    /// destructive call is answered by the policy (no person in a test).
    fn config(max_tool_runtime_secs: Option<u64>) -> DaemonConfig {
        let mut config = DaemonConfig::default();
        config.orchestrator.routing.tool_selection = "full".to_string();
        config.security.auto_approve_confirmations = true;
        if let Some(secs) = max_tool_runtime_secs {
            config.execution.agent_defaults.max_tool_runtime_secs = secs;
        }
        config
    }

    /// One main-loop turn in `project`, with the registered `shell_execute`
    /// on its surface; every request the model was handed.
    async fn turn(
        config: DaemonConfig,
        project: &GuardedProject,
        command: Option<String>,
    ) -> Vec<ChatRequest> {
        let registry = ToolRegistry::default();
        for tool in crate::tools::builtins::builtin_tools(None, None, None) {
            if tool.definition.name == "shell_execute" {
                registry.register(tool).unwrap();
            }
        }
        let registry = Arc::new(registry);
        let bus = EventBus::default();
        let gate = Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
            registry.clone(),
            bus.clone(),
        ))));
        let provider = Arc::new(CallsTheShell {
            command,
            seen: Mutex::new(Vec::new()),
        });
        let router = LlmRouter::single_provider(
            provider.clone(),
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = Orchestrator::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            bus,
            SystemPersona::default(),
            Some(Arc::new(router)),
            LoopConfig::default(),
            gate,
            registry,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
            Arc::new(ArcSwap::from_pointee(config)),
        );
        orch.handle_message(HandleRequest {
            request_id: Uuid::new_v4(),
            source: "cli".to_string(),
            content: "run the build".to_string(),
            principal: Principal::System,
            scope: Scope::Global,
            lane_key: "owner-test:cli".to_string(),
            workspace_path: Some(project.root.to_string_lossy().into_owned()),
            stream_id: None,
            model_override: None,
            unattended: false,
            turn_sink: None,
            session_id: None,
        })
        .await
        .expect("the turn answers");
        let seen = provider.seen.lock().unwrap_or_else(|p| p.into_inner());
        seen.clone()
    }

    fn shell_description(request: &ChatRequest) -> String {
        request
            .tools
            .iter()
            .find(|tool| tool.name == "shell_execute")
            .expect("the main loop's surface carries shell_execute")
            .description
            .clone()
    }

    #[tokio::test]
    async fn the_main_loops_shell_definition_states_the_default_cut() {
        let project = GuardedProject::new();
        let seen = turn(config(None), &project, None).await;
        let description = shell_description(&seen[0]);
        assert!(
            description.contains("still running after 60 seconds is stopped"),
            "{description}"
        );
        assert!(!description.contains("300"), "{description}");
    }

    /// The key moves both: the sandbox cuts the command at 1 s (killing what
    /// it started, R45), and the definition says 1 second.
    #[tokio::test]
    async fn a_changed_max_tool_runtime_secs_moves_the_cut_and_the_stated_figure() {
        let project = GuardedProject::new();
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let command = format!(
            "cd '{}'; sleep 5 & echo $! > '{}'; wait",
            dir.path().display(),
            pidfile.display()
        );
        let seen = turn(config(Some(1)), &project, Some(command)).await;
        let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        let _kill = KillOnFail(pid);

        let result = seen
            .get(1)
            .and_then(|request| request.messages.iter().find(|m| m.role == Role::Tool))
            .expect("the command's result reached the model")
            .content
            .clone();
        assert!(result.contains("timed out after 1s"), "{result:?}");
        let description = shell_description(&seen[0]);
        assert!(
            description.contains("still running after 1 second is stopped"),
            "{description}"
        );
        assert!(
            wait_until_gone(pid, GONE_WITHIN).await,
            "the background sleep {pid} outlived the main loop's cut"
        );
    }
}

// --- N-SKILL-1: `<available_skills>` (audit Z3-02) ---

use super::Orchestrator;
use crate::daemon_config::DaemonConfig;
use crate::orchestrator::skill_catalog::SkillCatalog;
use std::sync::Arc;

/// An orchestrator over `catalog`, with nothing else wired: the catalog block
/// reads the catalog and the daemon config and nothing more.
fn orchestrator_over(catalog: Arc<SkillCatalog>, daemon_config: DaemonConfig) -> Orchestrator {
    use crate::bus::EventBus;
    use crate::security::gate::SecurityGate;
    use crate::security::sandbox::SandboxManager;
    use crate::tools::ToolRegistry;

    let bus = EventBus::default();
    let registry = Arc::new(ToolRegistry::default());
    let gate = Arc::new(SecurityGate::new(Arc::new(SandboxManager::with_defaults(
        registry.clone(),
        bus.clone(),
    ))));
    Orchestrator::new(
        Arc::new(crate::context::SharedContext::new()),
        Arc::new(crate::lane::LaneManager::new()),
        bus,
        crate::middleware::prompt::SystemPersona::default(),
        None,
        crate::runner::LoopConfig::default(),
        gate,
        registry,
        None,
        catalog,
        Arc::new(crate::orchestrator::skill_router::SkillRouter::new(
            0.65, 0.45,
        )),
        Arc::new(arc_swap::ArcSwap::from_pointee(daemon_config)),
    )
}

/// A catalog scanned from a temporary directory holding one SKILL.md per
/// `(directory, frontmatter lines)`.
fn scanned_catalog(skills: &[(String, String)]) -> (tempfile::TempDir, Arc<SkillCatalog>) {
    let tmp = tempfile::TempDir::new().unwrap();
    for (dir, frontmatter) in skills {
        let skill_dir = tmp.path().join(dir);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\n{frontmatter}---\n\n## Instructions\n\nDo it.\n"),
        )
        .unwrap();
    }
    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), crate::middleware::skill::SkillScope::Project);
    (tmp, Arc::new(catalog))
}

/// The skill each line of the block names: the text between `- ` and the
/// first space or colon.
fn listed_names(block: &str) -> Vec<String> {
    block
        .lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|rest| {
            rest.split([' ', ':'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Twenty skills with their slash commands, named as their directories.
fn twenty_skills() -> Vec<(String, String)> {
    (1..=20)
        .map(|n| {
            let id = format!("skill-{n:02}");
            let frontmatter = format!(
                "name: \"{id}\"\n\
                 description: \"Does the {id} job for the person who asks for it\"\n\
                 invoke:\n  slash: \"/{id}\"\n"
            );
            (id, frontmatter)
        })
        .collect()
}

/// Twenty skills overflow the default budget, and every one of them is still
/// listed — past the budget by its name alone. The parent `break`s at the
/// first line that does not fit and drops the rest without a word.
#[test]
fn available_skills_lists_every_skill_names_only_past_the_budget() {
    let skills = twenty_skills();
    let (_tmp, catalog) = scanned_catalog(&skills);
    let orch = orchestrator_over(catalog, DaemonConfig::default());

    let block = orch.build_skills_catalog_block();
    let listed = listed_names(&block);
    let missing: Vec<&str> = skills
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| !listed.iter().any(|l| l == id))
        .collect();
    assert!(
        missing.is_empty(),
        "skills dropped from the block: {missing:?}\n{block}"
    );
    assert_eq!(listed.len(), 20, "{block}");
    assert!(
        !block.contains("job for the person"),
        "over the budget every line is a name only: {block}"
    );
    assert!(block.contains("- skill-07 (/skill-07)\n"), "{block}");
}

/// The budget is `[orchestrator.prompt_budgets] skills_catalog_budget`: one
/// large enough carries every description.
#[test]
fn the_catalog_budget_is_read_from_the_daemon_config() {
    let skills = twenty_skills();
    let (_tmp, catalog) = scanned_catalog(&skills);
    let mut config = DaemonConfig::default();
    assert_eq!(
        config.orchestrator.prompt_budgets.skills_catalog_budget,
        500
    );
    config.orchestrator.prompt_budgets.skills_catalog_budget = 20_000;
    let orch = orchestrator_over(catalog, config);

    let block = orch.build_skills_catalog_block();
    assert_eq!(listed_names(&block).len(), 20, "{block}");
    assert!(
        block.contains(
            "- skill-20 (/skill-20): Does the skill-20 job for the person who asks for it\n"
        ),
        "{block}"
    );
}

/// A skill with no slash command is listed by its id, which is what
/// `invoke_skill` resolves; the parent listed its display name, which
/// resolves to nothing.
#[test]
fn a_skill_without_a_slash_is_listed_by_an_id_invoke_skill_resolves() {
    let (_tmp, catalog) = scanned_catalog(&[(
        "explain-code".to_string(),
        "name: \"Explain Code\"\ndescription: \"Explain what code does\"\n".to_string(),
    )]);
    let orch = orchestrator_over(catalog.clone(), DaemonConfig::default());

    let block = orch.build_skills_catalog_block();
    let listed = listed_names(&block);
    assert_eq!(listed.len(), 1, "{block}");
    for name in &listed {
        assert!(
            catalog.get_by_command(name).is_some(),
            "`{name}` is listed but invoke_skill cannot resolve it:\n{block}"
        );
    }
    assert!(
        block.contains("- explain-code: Explain what code does\n"),
        "{block}"
    );
}

/// The block tells the model how to run a skill — the main loop holds
/// `invoke_skill` — rather than saying only the user can.
#[test]
fn available_skills_names_invoke_skill() {
    let (_tmp, catalog) = scanned_catalog(&[(
        "code-review".to_string(),
        "name: \"Code Review\"\ndescription: \"Review code\"\ninvoke:\n  slash: \"/review\"\n"
            .to_string(),
    )]);
    let orch = orchestrator_over(catalog, DaemonConfig::default());

    let block = orch.build_skills_catalog_block();
    assert!(block.starts_with("<available_skills>\n"), "{block}");
    assert!(block.contains("invoke_skill"), "{block}");
    // The main loop's user can type a skill's slash command; the lead's
    // block leaves this sentence out.
    assert!(
        block.contains("The user can also type a skill's /slash command."),
        "{block}"
    );
    assert!(
        block.contains("- code-review (/review): Review code\n"),
        "{block}"
    );
    assert!(block.ends_with("</available_skills>"), "{block}");
}
