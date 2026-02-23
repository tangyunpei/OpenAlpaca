    use super::*;

    const VALID_TEMPLATE: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

Learn about the person you're helping. Update this as you go.

## Identity

* Name:
* What to call them:
* Pronouns:
* Timezone:

## Communication Style

(How they like to communicate -- terse vs verbose, formal vs casual, etc.)

## Expertise & Background

(Technical background, domains of expertise, skill level in various areas)

## Projects & Context

(Current projects, tools they use, stack preferences)

## Preferences

(Likes, dislikes, pet peeves, formatting preferences, etc.)

## Notes

(Anything else. Build this over time.)

The more you know, the better you can help. But remember -- you're learning about a person, not building a dossier. Respect the difference.
"#;

    const POPULATED_DOC: &str = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

# USER.md -- About Your Human

## Identity

* Name: Alex
* What to call them: Alex
* Pronouns: they/them
* Timezone: PST (UTC-8)

## Communication Style

Prefers concise, technical responses. No filler or sycophancy.

## Expertise & Background

Senior Rust developer. Strong in systems programming and async patterns. Familiar with Python and TypeScript.

## Projects & Context

Building OpenAlpaca orchestrator. Focus on memory system and agent pipeline.

## Preferences

Code examples over prose. Dark mode. Prefers snake_case.

## Notes

Likes to work late. Coffee over tea.
"#;

    #[test]
    fn test_parse_empty_template() {
        let doc = parse_user_markdown(VALID_TEMPLATE).expect("template should parse");
        assert_eq!(doc.frontmatter.title, "USER.md");
        assert_eq!(doc.frontmatter.summary, "User profile record");
        assert_eq!(
            doc.frontmatter.read_when,
            vec!["Bootstrapping a workspace manually"]
        );
        // All sections should be empty (template placeholders are parenthesized hints)
        assert!(doc.identity.is_empty());
        assert!(
            !user_document_has_content(&doc) || doc.communication_style.contains("How they like")
        );
    }

    #[test]
    fn test_parse_populated_document() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("populated doc should parse");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
        assert_eq!(
            doc.identity.get("What to call them"),
            Some(&"Alex".to_string())
        );
        assert_eq!(doc.identity.get("Pronouns"), Some(&"they/them".to_string()));
        assert_eq!(
            doc.identity.get("Timezone"),
            Some(&"PST (UTC-8)".to_string())
        );
        assert!(doc.communication_style.contains("concise"));
        assert!(doc.expertise.contains("Rust"));
        assert!(doc.projects.contains("OpenAlpaca"));
        assert!(doc.preferences.contains("Dark mode"));
        assert!(doc.notes.contains("Coffee"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let err = parse_user_markdown("No frontmatter here").expect_err("should fail");
        assert_eq!(err, UserParseError::MissingFrontmatter);
    }

    #[test]
    fn test_parse_missing_summary() {
        let invalid = VALID_TEMPLATE.replace("summary: \"User profile record\"\n", "");
        let err = parse_user_markdown(&invalid).expect_err("should fail");
        assert_eq!(err, UserParseError::MissingField("summary"));
    }

    #[test]
    fn test_render_roundtrip() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        let rendered = render_user_markdown(&doc);
        let reparsed = parse_user_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
        assert_eq!(doc.identity, reparsed.identity);
        assert_eq!(doc.communication_style, reparsed.communication_style);
        assert_eq!(doc.expertise, reparsed.expertise);
        assert_eq!(doc.projects, reparsed.projects);
        assert_eq!(doc.preferences, reparsed.preferences);
        assert_eq!(doc.notes, reparsed.notes);
    }

    #[test]
    fn test_render_empty_template_roundtrip() {
        let original = parse_user_markdown(VALID_TEMPLATE).expect("should parse");
        let rendered = render_user_markdown(&original);
        let reparsed = parse_user_markdown(&rendered).expect("rendered should re-parse");
        assert_eq!(original.frontmatter, reparsed.frontmatter);
        assert_eq!(original.identity, reparsed.identity);
    }

    #[test]
    fn test_prompt_block_empty_returns_empty() {
        // A document with only template hints but no real content in identity
        let doc = UserDocument {
            frontmatter: UserFrontmatter {
                title: "USER.md".to_string(),
                summary: "User profile record".to_string(),
                read_when: vec!["Bootstrapping".to_string()],
            },
            identity: HashMap::new(),
            communication_style: String::new(),
            expertise: String::new(),
            projects: String::new(),
            preferences: String::new(),
            notes: String::new(),
        };
        assert!(user_to_prompt_block(&doc, None).is_empty());
    }

    #[test]
    fn test_prompt_block_populated() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        let block = user_to_prompt_block(&doc, None);
        assert!(block.starts_with("### USER PROFILE ###\n"));
        assert!(block.contains("Name: Alex"));
        assert!(block.contains("Timezone: PST"));
        assert!(block.contains("Style:"));
        assert!(block.contains("Background:"));
        assert!(block.len() <= 1000 + "### USER PROFILE ###\n".len());
    }

    #[test]
    fn test_has_content_populated() {
        let doc = parse_user_markdown(POPULATED_DOC).expect("should parse");
        assert!(user_document_has_content(&doc));
    }

    #[test]
    fn test_has_content_identity_only_is_not_enough() {
        // Identity alone shouldn't satisfy the content check — bootstrap
        // needs to gather at least one other section (expertise, preferences, etc.)
        let doc = UserDocument {
            frontmatter: UserFrontmatter {
                title: "USER.md".to_string(),
                summary: "User profile record".to_string(),
                read_when: vec!["Bootstrapping a workspace manually".to_string()],
            },
            identity: {
                let mut m = HashMap::new();
                m.insert("Name".to_string(), "Alice".to_string());
                m
            },
            communication_style: String::new(),
            expertise: String::new(),
            projects: String::new(),
            preferences: String::new(),
            notes: String::new(),
        };
        assert!(
            !user_document_has_content(&doc),
            "Identity-only doc should NOT count as having content"
        );
    }

    #[test]
    fn test_has_content_identity_plus_one_section() {
        // Identity + at least one other section should be enough
        let doc = UserDocument {
            frontmatter: UserFrontmatter {
                title: "USER.md".to_string(),
                summary: "User profile record".to_string(),
                read_when: vec!["Bootstrapping a workspace manually".to_string()],
            },
            identity: {
                let mut m = HashMap::new();
                m.insert("Name".to_string(), "Alice".to_string());
                m
            },
            communication_style: String::new(),
            expertise: "Rust, Python".to_string(),
            projects: String::new(),
            preferences: String::new(),
            notes: String::new(),
        };
        assert!(
            user_document_has_content(&doc),
            "Identity + one other section should count as having content"
        );
    }

    #[test]
    fn test_has_content_no_identity_but_other_sections() {
        // Other sections without identity should NOT count
        let doc = UserDocument {
            frontmatter: UserFrontmatter {
                title: "USER.md".to_string(),
                summary: "User profile record".to_string(),
                read_when: vec!["Bootstrapping a workspace manually".to_string()],
            },
            identity: HashMap::new(),
            communication_style: String::new(),
            expertise: "Rust".to_string(),
            projects: String::new(),
            preferences: String::new(),
            notes: String::new(),
        };
        assert!(
            !user_document_has_content(&doc),
            "Without identity, other sections alone should not count"
        );
    }

    #[test]
    fn test_unknown_sections_tolerated() {
        let with_extra = format!(
            "{}\n\n## Extra Section\nSome future content.\n",
            POPULATED_DOC
        );
        let doc = parse_user_markdown(&with_extra).expect("unknown section should be tolerated");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
    }

    #[test]
    fn test_extra_identity_keys() {
        let custom = r#"---
title: "USER.md"
summary: "User profile record"
read_when:
  - Bootstrapping a workspace manually
---

## Identity

* Name: Alex
* Company: Acme Corp
* Role: CTO
"#;
        let doc = parse_user_markdown(custom).expect("should parse");
        assert_eq!(doc.identity.get("Name"), Some(&"Alex".to_string()));
        assert_eq!(doc.identity.get("Company"), Some(&"Acme Corp".to_string()));
        assert_eq!(doc.identity.get("Role"), Some(&"CTO".to_string()));
    }
