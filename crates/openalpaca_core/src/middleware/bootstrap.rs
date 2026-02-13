//! First-run onboarding (BOOTSTRAP.md) parsing and prompt injection.
//!
//! Unlike IDENTITY.md / USER.md / SOUL.md, BOOTSTRAP.md is **temporary**: it
//! exists only after a fresh install and is deleted once the agent has populated
//! both its identity and the user profile.  The body is free-form markdown that
//! gets injected verbatim into the system prompt as onboarding instructions.

use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapFrontmatter {
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapDocument {
    pub frontmatter: BootstrapFrontmatter,
    /// Raw markdown body — injected as-is into the system prompt.
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
}

impl fmt::Display for BootstrapParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
        }
    }
}

impl std::error::Error for BootstrapParseError {}

// ---------------------------------------------------------------------------
// Frontmatter helpers (inlined from identity.rs / soul.rs to avoid coupling)
// ---------------------------------------------------------------------------

fn strip_outer_quotes(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), BootstrapParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(BootstrapParseError::MissingFrontmatter);
    }

    let mut frontmatter = Vec::new();
    let mut body = Vec::new();
    let mut in_frontmatter = true;

    for line in lines {
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            frontmatter.push(line.to_string());
            continue;
        }
        body.push(line.to_string());
    }

    if in_frontmatter {
        return Err(BootstrapParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

fn parse_frontmatter(lines: &[String]) -> Result<BootstrapFrontmatter, BootstrapParseError> {
    let mut summary: Option<String> = None;
    let mut read_when: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("summary:") {
            summary = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if trimmed.starts_with("read_when:") {
            idx += 1;
            while idx < lines.len() {
                let item = lines[idx].trim();
                if item.is_empty() {
                    idx += 1;
                    continue;
                }
                if let Some(v) = item.strip_prefix("- ") {
                    read_when.push(strip_outer_quotes(v));
                    idx += 1;
                    continue;
                }
                break;
            }
            continue;
        }

        idx += 1;
    }

    let summary = summary.ok_or(BootstrapParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(BootstrapParseError::MissingField("read_when"));
    }

    Ok(BootstrapFrontmatter { summary, read_when })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a BOOTSTRAP.md file into a `BootstrapDocument`.
///
/// The body is preserved verbatim (free-form markdown).
/// Only the YAML frontmatter (`summary` and `read_when`) is required.
pub fn parse_bootstrap_markdown(input: &str) -> Result<BootstrapDocument, BootstrapParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;

    // Join body lines preserving original formatting.
    // Trim leading/trailing blank lines but keep interior formatting intact.
    let body = body_lines.join("\n");
    let body = body.trim().to_string();

    Ok(BootstrapDocument { frontmatter, body })
}

/// Returns true if the document has any meaningful body content.
pub fn bootstrap_document_has_content(doc: &BootstrapDocument) -> bool {
    !doc.body.trim().is_empty()
}

/// Render a `BootstrapDocument` into a `### BOOTSTRAP ###` prompt block.
///
/// Returns an empty string if the body is blank.
pub fn bootstrap_to_prompt_block(doc: &BootstrapDocument) -> String {
    if !bootstrap_document_has_content(doc) {
        return String::new();
    }

    let mut block = String::from("### BOOTSTRAP ###\n");
    block.push_str(&doc.body);
    block.push('\n');
    block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = r#"---
summary: "First-run onboarding ritual"
read_when:
  - Bootstrapping a workspace manually
---

# BOOTSTRAP.md - Hello, World

_You just woke up. Time to figure out who you are._

There is no memory yet. This is a fresh workspace, so it is normal that memory files are empty until you fill them in.

## The Conversation

Don't interrogate. Don't be robotic. Just... talk.

Start with something like:

> "Hey. I just came online. Who am I? Who are you?"

Then figure out together:

1. **Your name** -- What should they call you?
2. **Your nature** -- What kind of creature are you?
3. **Your vibe** -- Formal? Casual? Snarky? Warm?
4. **Your emoji** -- Everyone needs a signature.

Offer suggestions if they're stuck. Have fun with it.

## After You Know Who You Are

Use your tools to save what you learned:

- Call `update_identity` (mode: "sections") with your name, creature, vibe, and emoji
- Call `update_user` (mode: "sections") with their name, how to address them, timezone, and any notes

Then talk about:
- What matters to them
- How they want you to behave
- Any boundaries or preferences

If they want to update your soul (core values, boundaries, vibe), use the `update_soul` tool together.

## When You're Done

Once IDENTITY.md and USER.md have real content, this file will be automatically deleted. You don't need a bootstrap script anymore -- you're you now.

---

_Good luck out there. Make it count._
"#;

    #[test]
    fn test_parse_template() {
        let doc = parse_bootstrap_markdown(TEMPLATE).expect("template should parse");
        assert_eq!(doc.frontmatter.summary, "First-run onboarding ritual");
        assert_eq!(
            doc.frontmatter.read_when,
            vec!["Bootstrapping a workspace manually"]
        );
        assert!(doc.body.contains("# BOOTSTRAP.md - Hello, World"));
        assert!(doc.body.contains("Don't interrogate"));
        assert!(doc.body.contains("update_identity"));
        assert!(doc.body.contains("_Good luck out there. Make it count._"));
    }

    #[test]
    fn test_parse_preserves_body_formatting() {
        let doc = parse_bootstrap_markdown(TEMPLATE).expect("template should parse");
        // Body should preserve headings, blockquotes, lists, emphasis
        assert!(doc.body.contains("## The Conversation"));
        assert!(doc.body.contains("> \"Hey. I just came online."));
        assert!(doc.body.contains("1. **Your name**"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let err = parse_bootstrap_markdown("No frontmatter here").expect_err("should fail");
        assert_eq!(err, BootstrapParseError::MissingFrontmatter);
    }

    #[test]
    fn test_parse_unterminated_frontmatter() {
        let unterminated = "---\nsummary: \"test\"\n";
        let err = parse_bootstrap_markdown(unterminated).expect_err("should fail");
        assert_eq!(err, BootstrapParseError::UnterminatedFrontmatter);
    }

    #[test]
    fn test_parse_missing_summary() {
        let invalid = "---\nread_when:\n  - always\n---\nBody text\n";
        let err = parse_bootstrap_markdown(invalid).expect_err("should fail");
        assert_eq!(err, BootstrapParseError::MissingField("summary"));
    }

    #[test]
    fn test_parse_missing_read_when() {
        let invalid = "---\nsummary: \"test\"\n---\nBody text\n";
        let err = parse_bootstrap_markdown(invalid).expect_err("should fail");
        assert_eq!(err, BootstrapParseError::MissingField("read_when"));
    }

    #[test]
    fn test_parse_empty_body() {
        let empty_body = "---\nsummary: \"test\"\nread_when:\n  - always\n---\n\n\n";
        let doc = parse_bootstrap_markdown(empty_body).expect("should parse");
        assert!(doc.body.is_empty());
        assert!(!bootstrap_document_has_content(&doc));
    }

    #[test]
    fn test_has_content_with_body() {
        let doc = parse_bootstrap_markdown(TEMPLATE).expect("should parse");
        assert!(bootstrap_document_has_content(&doc));
    }

    #[test]
    fn test_prompt_block_empty_returns_empty() {
        let doc = BootstrapDocument {
            frontmatter: BootstrapFrontmatter {
                summary: "test".to_string(),
                read_when: vec!["always".to_string()],
            },
            body: String::new(),
        };
        assert!(bootstrap_to_prompt_block(&doc).is_empty());
    }

    #[test]
    fn test_prompt_block_with_content() {
        let doc = parse_bootstrap_markdown(TEMPLATE).expect("should parse");
        let block = bootstrap_to_prompt_block(&doc);
        assert!(block.starts_with("### BOOTSTRAP ###\n"));
        assert!(block.contains("# BOOTSTRAP.md - Hello, World"));
        assert!(block.contains("Don't interrogate"));
    }

    #[test]
    fn test_prompt_block_whitespace_only_body() {
        let doc = BootstrapDocument {
            frontmatter: BootstrapFrontmatter {
                summary: "test".to_string(),
                read_when: vec!["always".to_string()],
            },
            body: "   \n  \n   ".to_string(),
        };
        assert!(!bootstrap_document_has_content(&doc));
        assert!(bootstrap_to_prompt_block(&doc).is_empty());
    }

    #[test]
    fn test_parse_minimal_document() {
        let minimal = "---\nsummary: \"Minimal\"\nread_when:\n  - always\n---\nHello world\n";
        let doc = parse_bootstrap_markdown(minimal).expect("should parse");
        assert_eq!(doc.frontmatter.summary, "Minimal");
        assert_eq!(doc.body, "Hello world");
    }

    #[test]
    fn test_parse_multiple_read_when() {
        let multi = "---\nsummary: \"test\"\nread_when:\n  - first run\n  - manual reset\n---\nBody\n";
        let doc = parse_bootstrap_markdown(multi).expect("should parse");
        assert_eq!(doc.frontmatter.read_when, vec!["first run", "manual reset"]);
    }
}
