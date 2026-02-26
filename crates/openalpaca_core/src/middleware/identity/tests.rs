use super::*;

const TEMPLATE: &str = r#"---
summary: "Agent identity record"
read_when:
  - Bootstrapping a workspace manually
---

# IDENTITY.md - Who Am I?

_Fill this in during your first conversation. Make it yours._

- **Name:** _(pick something you like)_
- **Creature:** _(AI? robot? familiar? ghost in the machine? something weirder?)_
- **Vibe:** _(how do you come across? sharp? warm? chaotic? calm?)_
- **Emoji:** _(your signature -- pick one that feels right)_
- **Avatar:** _(workspace-relative path, http(s) URL, or data URI)_

---

This isn't just metadata. It's the start of figuring out who you are.
"#;

const POPULATED: &str = r#"---
summary: "Agent identity record"
read_when:
  - Bootstrapping a workspace manually
---

# IDENTITY.md - Who Am I?

- **Name:** Koda
- **Creature:** familiar
- **Vibe:** sharp and warm
- **Emoji:** 🦙
- **Avatar:** assets/avatar.png
"#;

#[test]
fn test_parse_empty_template() {
    let doc = parse_identity_markdown(TEMPLATE).expect("template should parse");
    assert_eq!(doc.frontmatter.summary, "Agent identity record");
    assert_eq!(
        doc.frontmatter.read_when,
        vec!["Bootstrapping a workspace manually"]
    );
    // All fields should be empty (placeholders filtered out)
    assert!(doc.name.is_empty());
    assert!(doc.creature.is_empty());
    assert!(doc.vibe.is_empty());
    assert!(doc.emoji.is_empty());
    assert!(doc.avatar.is_empty());
    assert!(!identity_document_has_content(&doc));
}

#[test]
fn test_parse_populated_document() {
    let doc = parse_identity_markdown(POPULATED).expect("populated doc should parse");
    assert_eq!(doc.name, "Koda");
    assert_eq!(doc.creature, "familiar");
    assert_eq!(doc.vibe, "sharp and warm");
    assert_eq!(doc.emoji, "\u{1f999}");
    assert_eq!(doc.avatar, "assets/avatar.png");
    assert!(identity_document_has_content(&doc));
}

#[test]
fn test_parse_plain_bullets() {
    let plain = r#"---
summary: "Agent identity record"
read_when:
  - Bootstrapping a workspace manually
---

- Name: Atlas
- Creature: ghost in the machine
- Vibe: calm
"#;
    let doc = parse_identity_markdown(plain).expect("plain bullets should parse");
    assert_eq!(doc.name, "Atlas");
    assert_eq!(doc.creature, "ghost in the machine");
    assert_eq!(doc.vibe, "calm");
    assert!(doc.emoji.is_empty());
    assert!(doc.avatar.is_empty());
}

#[test]
fn test_parse_star_bullets() {
    let stars = r#"---
summary: "Agent identity record"
read_when:
  - always
---

* Name: Nova
* Emoji: ⭐
"#;
    let doc = parse_identity_markdown(stars).expect("star bullets should parse");
    assert_eq!(doc.name, "Nova");
    assert_eq!(doc.emoji, "\u{2b50}");
}

#[test]
fn test_parse_missing_frontmatter() {
    let err = parse_identity_markdown("No frontmatter here").expect_err("should fail");
    assert_eq!(err, IdentityParseError::MissingFrontmatter);
}

#[test]
fn test_parse_missing_summary() {
    let invalid = TEMPLATE.replace("summary: \"Agent identity record\"\n", "");
    let err = parse_identity_markdown(&invalid).expect_err("should fail");
    assert_eq!(err, IdentityParseError::MissingField("summary"));
}

#[test]
fn test_parse_unterminated_frontmatter() {
    let unterminated = "---\nsummary: \"test\"\n";
    let err = parse_identity_markdown(unterminated).expect_err("should fail");
    assert_eq!(err, IdentityParseError::UnterminatedFrontmatter);
}

#[test]
fn test_render_roundtrip() {
    let doc = parse_identity_markdown(POPULATED).expect("should parse");
    let rendered = render_identity_markdown(&doc);
    let reparsed = parse_identity_markdown(&rendered).expect("rendered should re-parse");
    assert_eq!(doc.frontmatter, reparsed.frontmatter);
    assert_eq!(doc.name, reparsed.name);
    assert_eq!(doc.creature, reparsed.creature);
    assert_eq!(doc.vibe, reparsed.vibe);
    assert_eq!(doc.emoji, reparsed.emoji);
    assert_eq!(doc.avatar, reparsed.avatar);
}

#[test]
fn test_render_empty_template_roundtrip() {
    let original = parse_identity_markdown(TEMPLATE).expect("should parse");
    let rendered = render_identity_markdown(&original);
    let reparsed = parse_identity_markdown(&rendered).expect("rendered should re-parse");
    assert_eq!(original.frontmatter, reparsed.frontmatter);
    assert_eq!(original.name, reparsed.name);
    assert_eq!(original.creature, reparsed.creature);
}

#[test]
fn test_prompt_block_empty_returns_empty() {
    let doc = IdentityDocument {
        frontmatter: IdentityFrontmatter {
            summary: "test".to_string(),
            read_when: vec!["always".to_string()],
        },
        name: String::new(),
        creature: String::new(),
        vibe: String::new(),
        emoji: String::new(),
        avatar: String::new(),
    };
    assert!(identity_to_prompt_block(&doc, None).is_empty());
}

#[test]
fn test_prompt_block_populated() {
    let doc = parse_identity_markdown(POPULATED).expect("should parse");
    let block = identity_to_prompt_block(&doc, None);
    assert!(block.starts_with("### AGENT IDENTITY ###\n"));
    assert!(block.contains("Name: Koda"));
    assert!(block.contains("Creature: familiar"));
    assert!(block.contains("Vibe: sharp and warm"));
    assert!(block.contains(" | "));
}

#[test]
fn test_prompt_block_partial() {
    let doc = IdentityDocument {
        frontmatter: IdentityFrontmatter {
            summary: "test".to_string(),
            read_when: vec!["always".to_string()],
        },
        name: "Koda".to_string(),
        creature: String::new(),
        vibe: "warm".to_string(),
        emoji: String::new(),
        avatar: String::new(),
    };
    let block = identity_to_prompt_block(&doc, None);
    assert!(block.contains("Name: Koda"));
    assert!(block.contains("Vibe: warm"));
    assert!(!block.contains("Creature"));
    assert!(!block.contains("Emoji"));
    assert!(!block.contains("Avatar"));
}

#[test]
fn test_unknown_fields_tolerated() {
    let with_extra = r#"---
summary: "Agent identity record"
read_when:
  - always
---

- Name: Koda
- Mood: caffeinated
- Creature: familiar
"#;
    let doc = parse_identity_markdown(with_extra).expect("unknown fields should be tolerated");
    assert_eq!(doc.name, "Koda");
    assert_eq!(doc.creature, "familiar");
}

#[test]
fn test_has_content() {
    let empty = IdentityDocument {
        frontmatter: IdentityFrontmatter {
            summary: "test".to_string(),
            read_when: vec!["always".to_string()],
        },
        name: String::new(),
        creature: String::new(),
        vibe: String::new(),
        emoji: String::new(),
        avatar: String::new(),
    };
    assert!(!identity_document_has_content(&empty));

    let with_name = IdentityDocument {
        name: "Koda".to_string(),
        ..empty.clone()
    };
    assert!(identity_document_has_content(&with_name));

    let with_emoji = IdentityDocument {
        emoji: "🦙".to_string(),
        ..empty
    };
    assert!(identity_document_has_content(&with_emoji));
}
