    use super::*;

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
                task_id: "task-456".to_string(),
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
                task_id: "task-789".to_string(),
                action: "pause".to_string()
            }
        );
    }

    #[test]
    fn test_complex_multi_skill() {
        let intent = parser().parse("research about Rust and write a summary");
        match intent {
            Intent::ComplexTask {
                required_skills, ..
            } => {
                assert!(required_skills.contains(&"web_search".to_string()));
                assert!(required_skills.contains(&"text_generate".to_string()));
                assert!(required_skills.contains(&"summarize".to_string()));
            }
            _ => panic!("Expected ComplexTask, got {:?}", intent),
        }
    }

    #[test]
    fn test_natural_language_task_query() {
        let intent = parser().parse("what are my tasks?");
        assert_eq!(intent, Intent::TaskQuery { task_id: None });
    }

    #[test]
    fn test_single_keyword_no_signal() {
        // "search" alone without complexity signal -> SimpleQuery
        let intent = parser().parse("search for cats");
        // "search" matches web_search but no complexity signal -> SimpleQuery
        assert!(matches!(intent, Intent::SimpleQuery { .. }));
    }

    #[test]
    fn test_single_keyword_with_signal() {
        let intent = parser().parse("can you search for cats");
        match intent {
            Intent::ComplexTask {
                required_skills, ..
            } => {
                assert!(required_skills.contains(&"web_search".to_string()));
            }
            _ => panic!("Expected ComplexTask, got {:?}", intent),
        }
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

    // --- update_soul intent routing tests ---

    #[test]
    fn test_suggest_tools_update_soul_persona() {
        let tools = parser().suggest_tools("update my persona to be more friendly");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_edit_soul_md_suppresses_file_write() {
        let tools = parser().suggest_tools("edit SOUL.md with new vibe");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
        assert!(
            !tools.contains(&"file_write".to_string()),
            "update_soul should suppress file_write: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_change_personality() {
        let tools = parser().suggest_tools("change personality to pirate");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_write_readme_no_update_soul() {
        let tools = parser().suggest_tools("write README.md with installation instructions");
        assert!(
            !tools.contains(&"update_soul".to_string()),
            "Should NOT suggest update_soul for unrelated write: {:?}",
            tools
        );
    }

    // --- parse_with_skills tests ---

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

    fn make_test_catalog() -> (TempDir, SkillCatalog) {
        let tmp = TempDir::new().unwrap();
        create_test_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code for bugs"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
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
command: "explain-code"
trigger_patterns:
  - "explain.*code"
  - "what does.*do"
auto_load: false
---

## Instructions

Explain step by step.
"#,
        );
        create_test_skill_dir(
            tmp.path(),
            "commit-message",
            r#"---
name: "Commit Message"
description: "Generate commit messages"
command: "commit"
trigger_patterns:
  - "commit message"
  - "git commit"
auto_load: false
---

## Instructions

Generate a conventional commit.
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), crate::middleware::skill::SkillScope::Project);
        (tmp, catalog)
    }

    #[test]
    fn test_parse_with_skills_slash_command_review() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/review some code", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "some code");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_slash_command_explain() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/explain-code main.rs", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Explain Code");
                assert_eq!(query, "main.rs");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_slash_command_no_query() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/review", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "/review"); // full text when no query
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_trigger_match() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("please review my code for bugs", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "please review my code for bugs");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_trigger_commit() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("generate a git commit message", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Commit Message");
                assert_eq!(query, "generate a git commit message");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_no_match_fallthrough() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("hello world", &catalog);
        assert!(
            matches!(intent, Intent::SimpleQuery { .. }),
            "Should fall through to SimpleQuery, got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_with_skills_unknown_slash_fallthrough() {
        let (_tmp, catalog) = make_test_catalog();
        // /status is a built-in command, not a skill — should fall through to parse()
        let intent = parser().parse_with_skills("/status", &catalog);
        assert!(
            matches!(intent, Intent::TaskQuery { .. }),
            "Should fall through to TaskQuery, got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_with_skills_empty_catalog() {
        let catalog = SkillCatalog::new();
        let intent = parser().parse_with_skills("/review some code", &catalog);
        // No skills loaded — slash command won't match, falls through
        // "/review some code" doesn't match built-in slash commands either
        // It should become a SimpleQuery
        assert!(
            matches!(intent, Intent::SimpleQuery { .. }),
            "Should fall through to SimpleQuery with empty catalog, got {:?}",
            intent
        );
    }

    #[test]
    fn test_skill_invocation_intent_type() {
        let intent = Intent::SkillInvocation {
            skill_name: "Test".to_string(),
            query: "test query".to_string(),
        };
        assert_eq!(intent.intent_type(), "skill_invocation");
    }

    // --- fast path eligibility tests ---

    #[test]
    fn test_fast_path_greeting() {
        assert!(parser().is_fast_path_eligible("hello"));
    }

    #[test]
    fn test_fast_path_short_question() {
        assert!(parser().is_fast_path_eligible("what time is it?"));
    }

    #[test]
    fn test_fast_path_complexity_signal_ineligible() {
        assert!(!parser().is_fast_path_eligible("can you help me with this?"));
    }

    #[test]
    fn test_fast_path_long_content_ineligible() {
        let long = "a".repeat(201);
        assert!(!parser().is_fast_path_eligible(&long));
    }

    #[test]
    fn test_fast_path_task_verb_ineligible() {
        assert!(!parser().is_fast_path_eligible("create a task to fix the bug"));
    }

    #[test]
    fn test_fast_path_delegation_ineligible() {
        assert!(!parser().is_fast_path_eligible("delegate this to the researcher"));
    }

    #[test]
    fn test_fast_path_step_by_step_ineligible() {
        assert!(!parser().is_fast_path_eligible("do this step by step"));
    }

    #[test]
    fn test_fast_path_multi_skill_ineligible() {
        assert!(!parser().is_fast_path_eligible("research and summarize this"));
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
        catalog.scan_directory(
            tmp.path(),
            crate::middleware::skill::SkillScope::Project,
        );
        (tmp, catalog)
    }

    #[test]
    fn test_router_selects_correct_skill() {
        let (_tmp, catalog) = make_router_test_catalog();
        let router = SkillRouter::new(0.65, 0.45);

        let intent =
            parser().parse_with_skills_and_router("review code for bugs", &catalog, &router);
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
        let intent =
            parser().parse_with_skills_and_router("/explain-code main.rs", &catalog, &router);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Explain Code");
                assert_eq!(query, "main.rs");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_router_no_match_falls_through() {
        let (_tmp, catalog) = make_router_test_catalog();
        let router = SkillRouter::new(0.65, 0.45);

        let intent =
            parser().parse_with_skills_and_router("hello world", &catalog, &router);
        assert!(
            matches!(intent, Intent::SimpleQuery { .. }),
            "Should fall through to SimpleQuery, got {:?}",
            intent
        );
    }
