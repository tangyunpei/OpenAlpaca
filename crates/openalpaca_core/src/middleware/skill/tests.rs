    use super::*;

    const VALID_SKILL: &str = r#"---
name: "Code Review"
description: "Review code for bugs, style issues, and improvements"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
read_when:
  - "User asks for code review"
---

## Instructions

When performing a code review, follow these steps:

1. Read the file(s) specified by the user
2. Analyze the code for bugs, style issues, security concerns
3. Provide feedback organized by severity

## Style

Be constructive and specific. Reference line numbers.
"#;

    const MINIMAL_SKILL: &str = r#"---
name: "Minimal"
description: "A minimal skill"
---
"#;

    #[test]
    fn test_parse_skill_frontmatter_only() {
        let fm = parse_skill_frontmatter(VALID_SKILL).expect("valid skill should parse");
        assert_eq!(fm.name, "Code Review");
        assert_eq!(
            fm.description,
            "Review code for bugs, style issues, and improvements"
        );
        // Legacy command field is populated
        assert_eq!(fm.command, Some("review".to_string()));
        // Legacy compat: command -> invoke.slash
        assert_eq!(fm.invoke.slash, Some("/review".to_string()));
        // effective_slash_command should return "review"
        assert_eq!(fm.effective_slash_command(), Some("review".to_string()));
        // Legacy trigger_patterns are populated
        assert_eq!(fm.trigger_patterns, vec!["review.*code", "code review"]);
        // Legacy compat: trigger_patterns -> routing.intent
        assert_eq!(fm.routing.intent, vec!["review.*code", "code review"]);
        // Legacy tools_required -> tools.allow
        assert_eq!(fm.tools_required, vec!["file_read"]);
        assert_eq!(fm.tools.allow, vec!["file_read"]);
        assert!(!fm.auto_load);
        assert_eq!(fm.read_when, vec!["User asks for code review"]);
    }

    #[test]
    fn test_parse_skill_full() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
        assert_eq!(doc.frontmatter.name, "Code Review");
        assert!(!doc.body.is_empty());
        assert!(doc.body.contains("When performing a code review"));
        assert!(doc.sections.contains_key("Instructions"));
        assert!(doc.sections.contains_key("Style"));
        assert!(doc.sections["Style"].contains("constructive"));
    }

    #[test]
    fn test_parse_minimal_skill() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal skill should parse");
        assert_eq!(doc.frontmatter.name, "Minimal");
        assert_eq!(doc.frontmatter.description, "A minimal skill");
        assert_eq!(doc.frontmatter.command, None);
        assert!(doc.frontmatter.trigger_patterns.is_empty());
        assert!(doc.frontmatter.tools_required.is_empty());
        assert!(!doc.frontmatter.auto_load);
        assert!(doc.frontmatter.read_when.is_empty());
        assert!(doc.body.is_empty());
        assert!(doc.sections.is_empty());
    }

    #[test]
    fn test_parse_missing_name() {
        let input = "---\ndescription: \"test\"\n---\n";
        let err = parse_skill_frontmatter(input).expect_err("missing name should fail");
        assert_eq!(err, SkillParseError::MissingField("name"));
    }

    #[test]
    fn test_parse_missing_description() {
        let input = "---\nname: \"test\"\n---\n";
        let err = parse_skill_frontmatter(input).expect_err("missing description should fail");
        assert_eq!(err, SkillParseError::MissingField("description"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let input = "# No frontmatter\nJust a heading.";
        let err = parse_skill_frontmatter(input).expect_err("no frontmatter should fail");
        assert_eq!(err, SkillParseError::MissingFrontmatter);
    }

    #[test]
    fn test_parse_unterminated_frontmatter() {
        let input = "---\nname: \"test\"\ndescription: \"test\"\n";
        let err = parse_skill_frontmatter(input).expect_err("unterminated should fail");
        assert_eq!(err, SkillParseError::UnterminatedFrontmatter);
    }

    #[test]
    fn test_auto_load_true() {
        let input = "---\nname: \"test\"\ndescription: \"test\"\nauto_load: true\n---\n";
        let fm = parse_skill_frontmatter(input).expect("should parse");
        assert!(fm.auto_load);
        // Legacy compat: auto_load=true -> invoke.mode="auto"
        assert_eq!(fm.invoke.mode, "auto");
    }

    #[test]
    fn test_render_roundtrip() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("valid skill should parse");
        let rendered = render_skill_markdown(&doc);
        let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
        // Name, description should round-trip
        assert_eq!(doc.frontmatter.name, reparsed.frontmatter.name);
        assert_eq!(doc.frontmatter.description, reparsed.frontmatter.description);
        // Body sections should match
        assert_eq!(
            doc.sections.keys().collect::<Vec<_>>().len(),
            reparsed.sections.keys().collect::<Vec<_>>().len()
        );
        for (key, value) in &doc.sections {
            let reparsed_value = reparsed.sections.get(key).expect("section should exist");
            assert_eq!(
                value.trim(),
                reparsed_value.trim(),
                "Section '{}' content should match",
                key
            );
        }
    }

    #[test]
    fn test_render_minimal_roundtrip() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("minimal should parse");
        let rendered = render_skill_markdown(&doc);
        let reparsed = parse_skill_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(doc.frontmatter.name, reparsed.frontmatter.name);
        assert_eq!(
            doc.frontmatter.description,
            reparsed.frontmatter.description
        );
    }

    #[test]
    fn test_skill_to_prompt_block_nonempty() {
        let doc = parse_skill_markdown(VALID_SKILL).expect("should parse");
        let block = skill_to_prompt_block(&doc);
        assert!(block.starts_with("### SKILL CONTEXT: Code Review ###"));
        assert!(block.contains("When performing a code review"));
    }

    #[test]
    fn test_skill_to_prompt_block_empty_body() {
        let doc = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
        let block = skill_to_prompt_block(&doc);
        assert!(block.is_empty());
    }

    #[test]
    fn test_skill_document_has_content() {
        let full = parse_skill_markdown(VALID_SKILL).expect("should parse");
        assert!(skill_document_has_content(&full));

        let minimal = parse_skill_markdown(MINIMAL_SKILL).expect("should parse");
        assert!(!skill_document_has_content(&minimal));
    }

    #[test]
    fn test_unknown_frontmatter_fields_tolerated() {
        let input = r#"---
name: "test"
description: "test"
unknown_field: "hello"
another_unknown:
  - "item1"
---
"#;
        let fm = parse_skill_frontmatter(input).expect("unknown fields should be tolerated");
        assert_eq!(fm.name, "test");
    }

    #[test]
    fn test_unquoted_frontmatter_values() {
        let input = r#"---
name: My Skill
description: A skill without quotes
command: my-skill
auto_load: false
---
"#;
        let fm = parse_skill_frontmatter(input).expect("unquoted values should parse");
        assert_eq!(fm.name, "My Skill");
        assert_eq!(fm.description, "A skill without quotes");
        assert_eq!(fm.command, Some("my-skill".to_string()));
    }

    #[test]
    fn test_new_schema_fields() {
        let input = r#"---
name: "Advanced Skill"
description: "A skill using the new schema"
invoke:
  mode: auto
  slash: "/advanced"
routing:
  intent:
    - "advanced.*query"
  keywords:
    - "advanced"
  weights:
    base: 0.3
tools:
  allow:
    - "file_read"
    - "file_write"
  deny:
    - "shell_exec"
permissions:
  level: readwrite
output:
  format: markdown
---

## Instructions

Do something advanced.
"#;
        let fm = parse_skill_frontmatter(input).expect("new schema should parse");
        assert_eq!(fm.name, "Advanced Skill");
        assert_eq!(fm.invoke.mode, "auto");
        assert_eq!(fm.invoke.slash, Some("/advanced".to_string()));
        assert_eq!(fm.routing.intent, vec!["advanced.*query"]);
        assert_eq!(fm.routing.keywords, vec!["advanced"]);
        assert!((fm.routing.weights.base - 0.3).abs() < f64::EPSILON);
        // Other weights should be defaults
        assert!((fm.routing.weights.intent_weight - 0.45).abs() < f64::EPSILON);
        assert_eq!(fm.tools.allow, vec!["file_read", "file_write"]);
        assert_eq!(fm.tools.deny, vec!["shell_exec"]);
        assert_eq!(fm.permissions.level, "readwrite");
        assert_eq!(fm.output.format, Some("markdown".to_string()));
        assert_eq!(fm.effective_slash_command(), Some("advanced".to_string()));
    }

    #[test]
    fn test_legacy_compat_auto_load_sets_invoke_mode() {
        let input = "---\nname: test\ndescription: test\nauto_load: true\n---\n";
        let fm = parse_skill_frontmatter(input).expect("should parse");
        assert_eq!(fm.invoke.mode, "auto");
    }

    #[test]
    fn test_legacy_compat_does_not_override_new_fields() {
        let input = r#"---
name: test
description: test
command: old-cmd
invoke:
  slash: "/new-cmd"
trigger_patterns:
  - "old pattern"
routing:
  intent:
    - "new pattern"
tools_required:
  - "old_tool"
tools:
  allow:
    - "new_tool"
---
"#;
        let fm = parse_skill_frontmatter(input).expect("should parse");
        // New fields should NOT be overridden by legacy
        assert_eq!(fm.invoke.slash, Some("/new-cmd".to_string()));
        assert_eq!(fm.routing.intent, vec!["new pattern"]);
        assert_eq!(fm.tools.allow, vec!["new_tool"]);
    }
