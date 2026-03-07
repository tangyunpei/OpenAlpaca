use super::SkillParseError;
use std::collections::HashMap;

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

pub(super) fn parse_body_sections(lines: &[String]) -> (String, HashMap<String, String>) {
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current_section: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();
    let mut full_body = String::new();

    for line in lines {
        // Build full body text
        if !full_body.is_empty() || !line.trim().is_empty() {
            if !full_body.is_empty() {
                full_body.push('\n');
            }
            full_body.push_str(line);
        }

        if let Some(heading) = line.trim().strip_prefix("## ") {
            // Save previous section
            if let Some(ref name) = current_section {
                let content = current_lines.join("\n").trim().to_string();
                if !content.is_empty() {
                    sections.insert(name.clone(), content);
                }
            }
            current_section = Some(heading.trim().to_string());
            current_lines.clear();
        } else if current_section.is_some() {
            current_lines.push(line.to_string());
        }
    }

    // Save last section
    if let Some(ref name) = current_section {
        let content = current_lines.join("\n").trim().to_string();
        if !content.is_empty() {
            sections.insert(name.clone(), content);
        }
    }

    // Trim trailing whitespace from full body
    let body = full_body.trim_end().to_string();
    (body, sections)
}
