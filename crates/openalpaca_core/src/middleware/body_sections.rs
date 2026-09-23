//! The markdown body sectioning SKILL.md and agent templates share.
//!
//! Each document keeps its own frontmatter parsing; this only reads the body
//! lines after it (`body_sections_tests.rs` pins the table).

use std::collections::HashMap;

/// Split body lines into the whole body and its `## ` sections.
///
/// The body is every line from the first non-blank one on, with trailing
/// whitespace trimmed. A heading is a line that starts with `## ` once
/// trimmed, so an indented one counts; there is no code-fence awareness.
/// A section is the trimmed text up to the next heading; an empty one is not
/// inserted, so a later non-empty duplicate heading overwrites an earlier one
/// and a later empty duplicate does not.
pub(crate) fn parse_body_sections(lines: &[String]) -> (String, HashMap<String, String>) {
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
