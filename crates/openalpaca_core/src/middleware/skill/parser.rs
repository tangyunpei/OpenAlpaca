use super::SkillParseError;

/// Extract the raw YAML string between `---` delimiters.
/// Returns (yaml_str, body_lines) on success.
pub(super) fn extract_frontmatter_str(input: &str) -> Result<(&str, Vec<String>), SkillParseError> {
    let trimmed = input.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::MissingFrontmatter);
    }

    // Find the opening ---
    let after_first = &trimmed[3..];
    // Skip the rest of the opening line (should be just newline)
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    // Find the closing ---
    let close_pos = after_first.find("\n---");
    match close_pos {
        Some(pos) => {
            let yaml_str = &after_first[..pos];
            let remainder = &after_first[pos + 4..]; // skip "\n---"
            // Skip the rest of the closing line
            let remainder = remainder.strip_prefix('\n').unwrap_or(remainder);
            let body_lines: Vec<String> = remainder.lines().map(|l| l.to_string()).collect();
            Ok((yaml_str, body_lines))
        }
        None => Err(SkillParseError::UnterminatedFrontmatter),
    }
}
