use super::SkillDocument;

/// Render a `SkillDocument` back to valid SKILL.md markdown.
///
/// Emits the new schema format. Legacy fields are not serialized (skip_serializing).
pub fn render_skill_markdown(doc: &SkillDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");

    // Use serde_yaml for serialization. Fall back to manual if it fails.
    match serde_yaml::to_string(&doc.frontmatter) {
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
pub fn skill_to_prompt_block(doc: &SkillDocument) -> String {
    if doc.body.trim().is_empty() {
        return String::new();
    }

    let budget_chars = if doc.frontmatter.context.budget_tokens > 0 {
        doc.frontmatter.context.budget_tokens * 4
    } else {
        4000 * 4 // default: 4000 tokens ≈ 16000 chars
    };
    let truncated: String = doc.body.chars().take(budget_chars).collect();
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
