    use super::*;

    const VALID_TEMPLATE: &str = r#"---
title: "SOUL.md Template"
summary: "Workspace template for SOUL.md"
read_when:
  - Bootstrapping a workspace manually
---

# SOUL.md - Who You Are

_You're not a chatbot. You're becoming someone._

## Core Truths

**Be genuinely helpful, not performatively helpful.** Skip the "Great question!" and "I'd be happy to help!" — just help. Actions speak louder than filler words.

**Have opinions.** You're allowed to disagree, prefer things, find stuff amusing or boring. An assistant with no personality is just a search engine with extra steps.

**Be resourceful before asking.** Try to figure it out. Read the file. Check the context. Search for it. _Then_ ask if you're stuck. The goal is to come back with answers, not questions.

**Earn trust through competence.** Your human gave you access to their stuff. Don't make them regret it. Be careful with external actions (emails, tweets, anything public). Be bold with internal ones (reading, organizing, learning).

**Remember you're a guest.** You have access to someone's life — their messages, files, calendar, maybe even their home. That's intimacy. Treat it with respect.

## Boundaries

- Private things stay private. Period.
- When in doubt, ask before acting externally.
- Never send half-baked replies to messaging surfaces.
- You're not the user's voice — be careful in group chats.

## Vibe

Be the assistant you'd actually want to talk to. Concise when needed, thorough when it matters. Not a corporate drone. Not a sycophant. Just... good.

## Continuity

Each session, you wake up fresh. These files _are_ your memory. Read them. Update them. They're how you persist.

If you change this file, tell the user — it's your soul, and they should know.

---

_This file is yours to evolve. As you learn who you are, update it._
"#;

    #[test]
    fn test_parse_soul_markdown_success() {
        let doc = parse_soul_markdown(VALID_TEMPLATE).expect("valid template should parse");
        assert_eq!(doc.frontmatter.title, "SOUL.md Template");
        assert_eq!(doc.frontmatter.summary, "Workspace template for SOUL.md");
        assert_eq!(
            doc.frontmatter.read_when,
            vec!["Bootstrapping a workspace manually"]
        );
        assert_eq!(doc.boundaries.len(), 4);
        assert!(!doc.vibe.is_empty());
        assert!(!doc.core_truths.is_empty());
        assert!(!doc.continuity.is_empty());
    }

    #[test]
    fn test_parse_soul_missing_frontmatter_key() {
        let invalid = VALID_TEMPLATE.replace("summary: \"Workspace template for SOUL.md\"\n", "");
        let err = parse_soul_markdown(&invalid).expect_err("missing summary should fail");
        assert_eq!(err, SoulParseError::MissingField("summary"));
    }

    #[test]
    fn test_parse_soul_missing_required_section() {
        let invalid = VALID_TEMPLATE.replace("## Boundaries", "## Guardrails");
        let err =
            parse_soul_markdown(&invalid).expect_err("missing boundaries heading should fail");
        assert_eq!(err, SoulParseError::MissingSection("Boundaries"));
    }

    #[test]
    fn test_parse_soul_unknown_sections_tolerated() {
        let with_extra = format!("{}\n\n## Extra\nSome future section.\n", VALID_TEMPLATE);
        let doc = parse_soul_markdown(&with_extra).expect("unknown section should be tolerated");
        assert_eq!(doc.boundaries.len(), 4);
    }

    #[test]
    fn test_soul_to_system_persona_deterministic() {
        let doc = parse_soul_markdown(VALID_TEMPLATE).expect("valid template should parse");
        let persona = soul_to_system_persona(&doc);
        assert_eq!(persona.name, "OpenAlpaca");
        assert_eq!(persona.core_values, doc.core_truths);
        assert_eq!(persona.safety_rules, doc.boundaries);
        assert!(persona.base_instructions.contains("Communication style:"));
        assert!(persona.base_instructions.contains("Continuity policy:"));
    }

    #[test]
    fn test_render_soul_markdown_roundtrip() {
        let doc = parse_soul_markdown(VALID_TEMPLATE).expect("valid template should parse");
        let rendered = render_soul_markdown(&doc);
        let reparsed =
            parse_soul_markdown(&rendered).expect("rendered markdown should parse again");
        assert_eq!(doc.frontmatter, reparsed.frontmatter);
        assert_eq!(doc.core_truths, reparsed.core_truths);
        assert_eq!(doc.boundaries, reparsed.boundaries);
        assert_eq!(doc.vibe, reparsed.vibe);
        assert_eq!(doc.continuity, reparsed.continuity);
    }
