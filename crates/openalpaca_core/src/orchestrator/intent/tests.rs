use super::*;
use crate::orchestrator::skill_catalog::SkillCatalog;
use crate::orchestrator::skill_router::SkillRouter;

fn parser() -> IntentParser {
    IntentParser
}

#[test]
fn test_simple_query() {
    let intent = parser().parse("hello world");
    assert_eq!(
        intent,
        Intent::SimpleQuery {
            query: "hello world".to_string()
        }
    );
}

#[test]
fn test_slash_status() {
    let intent = parser().parse("/status");
    assert_eq!(intent, Intent::TaskQuery { task_id: None });
}

#[test]
fn test_status_with_id() {
    let intent = parser().parse("/status task-123");
    assert_eq!(
        intent,
        Intent::TaskQuery {
            task_id: Some("task-123".to_string())
        }
    );
}

#[test]
fn test_cancel() {
    let intent = parser().parse("/cancel task-456");
    assert_eq!(
        intent,
        Intent::TaskControl {
            task_id: Some("task-456".to_string()),
            action: "cancel".to_string()
        }
    );
}

#[test]
fn test_pause() {
    let intent = parser().parse("/pause task-789");
    assert_eq!(
        intent,
        Intent::TaskControl {
            task_id: Some("task-789".to_string()),
            action: "pause".to_string()
        }
    );
}

#[test]
fn test_bare_task_control_commands() {
    // Bare commands (no id) resolve against the lane's active workflows.
    for (cmd, action) in [
        ("/cancel", "cancel"),
        ("/pause", "pause"),
        ("/resume", "resume"),
        ("/cancel   ", "cancel"), // trailing whitespace, still bare
    ] {
        assert_eq!(
            parser().parse(cmd),
            Intent::TaskControl {
                task_id: None,
                action: action.to_string()
            },
            "command: {cmd:?}"
        );
    }
}

#[test]
fn test_task_control_prefix_words_are_not_commands() {
    // "/cancellation" must not parse as a bare /cancel.
    assert!(matches!(
        parser().parse("/cancellation"),
        Intent::SimpleQuery { .. }
    ));
}

#[test]
fn test_natural_language_task_query() {
    let intent = parser().parse("what are my tasks?");
    assert_eq!(intent, Intent::TaskQuery { task_id: None });
}

#[test]
fn test_natural_language_task_result_query_english() {
    let intent = parser().parse("what is the task result?");
    assert_eq!(intent, Intent::TaskQuery { task_id: None });
}

#[test]
fn test_natural_language_task_result_query_chinese() {
    let intent = parser().parse("任务结果怎么样");
    assert_eq!(intent, Intent::TaskQuery { task_id: None });
}

#[test]
fn test_single_keyword_no_signal() {
    // "search" alone without complexity signal -> SimpleQuery
    let intent = parser().parse("search for cats");
    // "search" matches web_search but no complexity signal -> SimpleQuery
    assert!(matches!(intent, Intent::SimpleQuery { .. }));
}

// --- suggest_tools tests ---

#[test]
fn test_suggest_tools_write_readme() {
    let tools = parser().suggest_tools("write README.md with installation instructions");
    assert!(
        tools.contains(&"file_write".to_string()),
        "Expected file_write, got: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_write_file_named() {
    let tools = parser().suggest_tools("write a file named README with docs");
    assert!(
        tools.contains(&"file_write".to_string()),
        "Expected file_write, got: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_write_story_no_file() {
    let tools = parser().suggest_tools("write me a story about files");
    assert!(
        !tools.contains(&"file_write".to_string()),
        "Should NOT have file_write: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_version_no_file_write() {
    let tools = parser().suggest_tools("support v1.x series");
    assert!(
        !tools.contains(&"file_write".to_string()),
        "Should NOT have file_write: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_update_version_no_file_write() {
    let tools = parser().suggest_tools("update to v1.2 and ship it");
    assert!(
        !tools.contains(&"file_write".to_string()),
        "Should NOT have file_write: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_fetch_url() {
    let tools = parser().suggest_tools("fetch https://example.com");
    assert!(
        tools.contains(&"web_fetch".to_string()),
        "Expected web_fetch, got: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_hello_world_empty() {
    let tools = parser().suggest_tools("hello world");
    assert!(tools.is_empty(), "Expected empty, got: {:?}", tools);
}

#[test]
fn test_suggest_tools_run_command() {
    let tools = parser().suggest_tools("run command ls -la");
    assert!(
        tools.contains(&"shell_execute".to_string()),
        "Expected shell_execute, got: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_multi_tool_ordering() {
    let tools = parser().suggest_tools("fetch https://example.com and search for docs");
    assert!(tools.contains(&"web_fetch".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
    // web_fetch comes before web_search in ToolFlags::to_vec
    let fetch_idx = tools.iter().position(|t| t == "web_fetch").unwrap();
    let search_idx = tools.iter().position(|t| t == "web_search").unwrap();
    assert!(
        fetch_idx < search_idx,
        "web_fetch should come before web_search"
    );
}

// --- update_persona intent routing tests ---

#[test]
fn test_suggest_tools_update_persona_soul() {
    let tools = parser().suggest_tools("update my persona to be more friendly");
    assert!(
        tools.contains(&"update_persona".to_string()),
        "Should suggest update_persona: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_edit_soul_md_suppresses_file_write() {
    let tools = parser().suggest_tools("edit SOUL.md with new vibe");
    assert!(
        tools.contains(&"update_persona".to_string()),
        "Should suggest update_persona: {:?}",
        tools
    );
    assert!(
        !tools.contains(&"file_write".to_string()),
        "update_persona should suppress file_write: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_change_personality() {
    let tools = parser().suggest_tools("change personality to pirate");
    assert!(
        tools.contains(&"update_persona".to_string()),
        "Should suggest update_persona: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_write_readme_no_update_persona() {
    let tools = parser().suggest_tools("write README.md with installation instructions");
    assert!(
        !tools.contains(&"update_persona".to_string()),
        "Should NOT suggest update_persona for unrelated write: {:?}",
        tools
    );
}

// --- skill catalog helpers ---

use std::io::Write;
use tempfile::TempDir;

fn create_test_skill_dir(
    parent: &std::path::Path,
    name: &str,
    content: &str,
) -> std::path::PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let md_path = dir.join("SKILL.md");
    let mut f = std::fs::File::create(&md_path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    dir
}

#[test]
fn test_skill_invocation_intent_type() {
    let intent = Intent::SkillInvocation {
        skill_name: "Test".to_string(),
        query: "test query".to_string(),
    };
    assert_eq!(intent.intent_type(), "skill_invocation");
}

// --- parse_with_skills_and_router tests ---

fn make_router_test_catalog() -> (TempDir, SkillCatalog) {
    let tmp = TempDir::new().unwrap();
    create_test_skill_dir(
        tmp.path(),
        "code-review",
        r#"---
name: "Code Review"
description: "Review code for bugs"
invoke:
  mode: auto
  slash: "/review"
routing:
  intent:
    - "review code"
  keywords:
    - "bugs"
    - "style"
---

## Instructions

Review the code.
"#,
    );
    create_test_skill_dir(
        tmp.path(),
        "explain-code",
        r#"---
name: "Explain Code"
description: "Explain what code does"
invoke:
  mode: auto
  slash: "/explain-code"
routing:
  intent:
    - "explain code"
---

## Instructions

Explain step by step.
"#,
    );

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), crate::middleware::skill::SkillScope::Project);
    (tmp, catalog)
}

#[test]
fn test_router_selects_correct_skill() {
    let (_tmp, catalog) = make_router_test_catalog();
    let router = SkillRouter::new(0.65, 0.45);

    let intent = parser().parse_with_skills_and_router("review code for bugs", &catalog, &router);
    match intent {
        Intent::SkillInvocation { skill_name, query } => {
            assert_eq!(skill_name, "Code Review");
            assert_eq!(query, "review code for bugs");
        }
        other => panic!("Expected SkillInvocation, got {:?}", other),
    }
}

#[test]
fn test_router_slash_command_takes_priority() {
    let (_tmp, catalog) = make_router_test_catalog();
    let router = SkillRouter::new(0.65, 0.45);

    // Slash command should work even if router would select something else
    let intent = parser().parse_with_skills_and_router("/explain-code main.rs", &catalog, &router);
    match intent {
        Intent::SkillInvocation { skill_name, query } => {
            assert_eq!(skill_name, "Explain Code");
            assert_eq!(query, "main.rs");
        }
        other => panic!("Expected SkillInvocation, got {:?}", other),
    }
}

#[test]
fn test_router_auto_select_records_usage_for_recency() {
    let (_tmp, catalog) = make_router_test_catalog();
    let router = SkillRouter::new(0.65, 0.45);

    // Auto-selection at the production site must write the recency tier
    // via record_usage (wiring audit §2: recency bonus needs a writer).
    let intent = parser().parse_with_skills_and_router("review code for bugs", &catalog, &router);
    assert!(matches!(intent, Intent::SkillInvocation { .. }));

    // A subsequent route sees the recency bonus for the selected skill.
    let result = router.route("review code for bugs", &catalog);
    let score = result
        .scores
        .iter()
        .find(|s| s.skill_id == "code-review")
        .expect("code-review must be scored");
    assert!(
        score.recency_bonus > 0.0,
        "auto-select must record usage; recency_bonus = {}",
        score.recency_bonus
    );
}

#[test]
fn test_router_no_match_falls_through() {
    let (_tmp, catalog) = make_router_test_catalog();
    let router = SkillRouter::new(0.65, 0.45);

    let intent = parser().parse_with_skills_and_router("hello world", &catalog, &router);
    assert!(
        matches!(intent, Intent::SimpleQuery { .. }),
        "Should fall through to SimpleQuery, got {:?}",
        intent
    );
}

// --- send tool suggestion tests (connector awareness) ---

#[test]
fn test_suggest_tools_bare_telegram_no_send() {
    // Bare "telegram" without a send verb should NOT suggest send
    let tools = parser().suggest_tools("how do I set up Telegram?");
    assert!(
        !tools.contains(&"send".to_string()),
        "Bare 'telegram' should NOT suggest send: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_bare_imessage_no_send() {
    let tools = parser().suggest_tools("configure imessage connector");
    assert!(
        !tools.contains(&"send".to_string()),
        "Bare 'imessage' should NOT suggest send: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_via_telegram() {
    let tools = parser().suggest_tools("send a message via telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'send via telegram': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_via_imessage() {
    let tools = parser().suggest_tools("send this to John via imessage");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'send via imessage': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_message_phrase() {
    let tools = parser().suggest_tools("send message to Bob");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'send message': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_forward_to() {
    let tools = parser().suggest_tools("forward to my phone");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'forward to': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_chinese_send_telegram() {
    let tools = parser().suggest_tools("发一条消息到telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for Chinese+telegram: {:?}",
        tools
    );
}

// --- expanded keyword pattern tests (Change 1) ---

#[test]
fn test_suggest_tools_chinese_give_imessage() {
    let tools = parser().suggest_tools("可以给我的imessage发消息");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '给...imessage': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_via_imessage() {
    let tools = parser().suggest_tools("通过imessage发消息");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '通过imessage': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_give_telegram_sms() {
    let tools = parser().suggest_tools("给telegram发短信");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '给telegram发短信': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_text_to_via_telegram() {
    let tools = parser().suggest_tools("text to John via telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'text to...via telegram': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_msg_to() {
    let tools = parser().suggest_tools("msg to my friend on telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'msg to': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_chinese_message_telegram() {
    let tools = parser().suggest_tools("请给telegram发送测试短信");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '短信+telegram': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_via_imessage_english() {
    let tools = parser().suggest_tools("send greetings via imessage");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for 'via imessage': {:?}",
        tools
    );
}

// --- P1a: "给" without send-semantic verb should NOT trigger send ---

#[test]
fn test_suggest_tools_chinese_give_intro_telegram_no_send() {
    let tools = parser().suggest_tools("给我介绍一下telegram");
    assert!(
        !tools.contains(&"send".to_string()),
        "'给我介绍一下telegram' should NOT suggest send: {:?}",
        tools
    );
}

// Verify existing negative tests still pass with expanded patterns

#[test]
fn test_suggest_tools_bare_telegram_no_send_still_negative() {
    let tools = parser().suggest_tools("how do I set up Telegram?");
    assert!(
        !tools.contains(&"send".to_string()),
        "Bare 'telegram' should still NOT suggest send: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_bare_imessage_no_send_still_negative() {
    let tools = parser().suggest_tools("configure imessage connector");
    assert!(
        !tools.contains(&"send".to_string()),
        "Bare 'imessage' should still NOT suggest send: {:?}",
        tools
    );
}

// --- send tool file-related suggestion tests ---

#[test]
fn test_suggest_tools_send_file_via_telegram() {
    let tools = parser().suggest_tools("send a file via telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for file via telegram: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_photo_via_imessage() {
    let tools = parser().suggest_tools("send a photo via imessage");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for photo via imessage: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_image_via_telegram() {
    let tools = parser().suggest_tools("send image to friend via telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for image via telegram: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_document() {
    let tools = parser().suggest_tools("send document to Bob");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for document: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_chinese_send_file() {
    let tools = parser().suggest_tools("发文件到telegram");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '发文件到telegram': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_chinese_send_photo() {
    let tools = parser().suggest_tools("发图片给朋友");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '发图片给朋友': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_chinese_send_video() {
    let tools = parser().suggest_tools("发视频到imessage");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '发视频到imessage': {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_send_message_without_file() {
    let tools = parser().suggest_tools("send greetings via imessage");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send without file keywords: {:?}",
        tools
    );
}

// --- multi-turn plain text continuation: intent must be empty ---

#[test]
fn test_suggest_tools_plain_continuation_chinese_no_tools() {
    let tools = parser().suggest_tools("好的，发吧");
    assert!(
        tools.is_empty(),
        "Plain continuation should suggest no tools: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_plain_continuation_english_no_tools() {
    let tools = parser().suggest_tools("ok go ahead");
    assert!(
        tools.is_empty(),
        "Plain continuation should suggest no tools: {:?}",
        tools
    );
}

#[test]
fn test_suggest_tools_text_send_continuation() {
    let tools = parser().suggest_tools("发消息给他");
    assert!(
        tools.contains(&"send".to_string()),
        "Expected send for '发消息给他': {:?}",
        tools
    );
}

// --- is_social_message tests ---

#[test]
fn test_social_message_ok() {
    assert!(parser().is_social_message("ok"));
}

#[test]
fn test_social_message_thanks() {
    assert!(parser().is_social_message("thanks"));
}

#[test]
fn test_social_message_chinese() {
    assert!(parser().is_social_message("好的"));
}

#[test]
fn test_social_message_whitespace() {
    assert!(parser().is_social_message("  ok  "));
}

#[test]
fn test_social_message_case_insensitive() {
    assert!(parser().is_social_message("OK"));
}

#[test]
fn test_social_message_not_question() {
    assert!(!parser().is_social_message("what is a closure?"));
}

#[test]
fn test_social_message_not_task() {
    assert!(!parser().is_social_message("fix the bug"));
}

#[test]
fn test_social_message_yes() {
    // handlers.rs guards against send-flow breakage via ActiveSendHints check
    assert!(parser().is_social_message("yes"));
}
