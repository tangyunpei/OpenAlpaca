use super::SkillDocument;

/// Render a `SkillDocument` back to valid SKILL.md markdown.
///
/// Emits the new schema format. Legacy fields are not serialized (skip_serializing).
pub fn render_skill_markdown(doc: &SkillDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");

    // Use serde-saphyr for serialization. Fall back to manual if it fails.
    match serde_saphyr::to_string(&doc.frontmatter) {
        Ok(yaml) => out.push_str(&yaml),
        Err(_) => {
            // Manual fallback for basic fields
            out.push_str(&format!("name: \"{}\"\n", doc.frontmatter.name));
            out.push_str(&format!(
                "description: \"{}\"\n",
                doc.frontmatter.description
            ));
        }
    }
    out.push_str("---\n");

    // -- Body --
    if !doc.body.is_empty() {
        out.push('\n');
        out.push_str(&doc.body);
        out.push('\n');
    }

    out
}

/// Render a skill's instructions into a prompt block for LLM context injection.
///
/// Wraps `doc.body` in `<skill_context>` XML tags with name/description attributes.
/// Budget respects `context.budget_tokens` (default 4000 tokens ≈ 16000 chars).
/// Returns empty string if body is empty (no empty XML tags).
///
/// A body over the budget is cut, and one last line inside the block says so —
/// `[instructions cut at N of M characters]` — so the model knows it holds
/// part of the skill (N-SKILL-4).
pub fn skill_to_prompt_block(doc: &SkillDocument) -> String {
    if doc.body.trim().is_empty() {
        return String::new();
    }

    let budget_chars = if doc.frontmatter.context.budget_tokens > 0 {
        doc.frontmatter.context.budget_tokens * 4
    } else {
        4000 * 4 // default: 4000 tokens ≈ 16000 chars
    };
    let mut truncated: String = doc.body.chars().take(budget_chars).collect();
    let body_chars = doc.body.chars().count();
    if body_chars > budget_chars {
        truncated.push_str(&format!(
            "\n[instructions cut at {budget_chars} of {body_chars} characters]"
        ));
    }
    format!(
        "<skill_context name=\"{}\" description=\"{}\">\n{}\n</skill_context>",
        doc.frontmatter.name,
        doc.frontmatter.description,
        truncated,
    )
}

/// Check whether a skill document has meaningful content (non-empty body).
pub fn skill_document_has_content(doc: &SkillDocument) -> bool {
    !doc.body.trim().is_empty()
}

// N-SKILL-4's tests sit here rather than in `middleware/skill/tests.rs`,
// whose last lines G-base's `step-8-config.patch` anchors on.
#[cfg(test)]
mod tests {
    use super::skill_to_prompt_block;
    use crate::middleware::skill::{SkillDocument, SkillFrontmatter};

    fn doc(body: String) -> SkillDocument {
        SkillDocument {
            frontmatter: SkillFrontmatter {
                name: "Long".to_string(),
                description: "A long skill".to_string(),
                ..SkillFrontmatter::default()
            },
            body,
            sections: std::collections::HashMap::new(),
        }
    }

    /// A body past the default budget (4,000 tokens, 16,000 characters) is
    /// cut, and one line says where and out of how much.
    #[test]
    fn a_cut_body_says_it_was_cut() {
        let block = skill_to_prompt_block(&doc("é".repeat(20_000)));
        let tail: String = block.chars().skip(15_950).collect();
        assert!(
            block.contains("\n[instructions cut at 16000 of 20000 characters]\n</skill_context>"),
            "the block ends: {tail}"
        );
        assert_eq!(block.matches('é').count(), 16_000);
    }

    /// A body within the budget renders exactly as before — no line.
    #[test]
    fn a_body_within_the_budget_renders_unchanged() {
        let block = skill_to_prompt_block(&doc("Do the thing.".to_string()));
        assert_eq!(
            block,
            "<skill_context name=\"Long\" description=\"A long skill\">\nDo the thing.\n</skill_context>"
        );
        let exact = skill_to_prompt_block(&doc("x".repeat(16_000)));
        assert!(
            !exact.contains("instructions cut"),
            "a body of exactly the budget is whole"
        );
    }

    /// `context.budget_tokens` sets the budget the line names.
    #[test]
    fn the_line_names_a_skills_own_budget() {
        let mut long = doc("y".repeat(1_000));
        long.frontmatter.context.budget_tokens = 25;
        let block = skill_to_prompt_block(&long);
        assert!(
            block.contains("[instructions cut at 100 of 1000 characters]"),
            "{block}"
        );
    }
}
