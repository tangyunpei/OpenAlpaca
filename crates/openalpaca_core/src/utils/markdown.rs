//! Shared mechanical Markdown parsing; document validation stays with each caller.
use std::collections::HashMap;

pub(crate) enum FrontmatterError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
}

pub(crate) struct PersonaFrontmatter {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub read_when: Vec<String>,
}

pub(crate) fn strip_outer_quotes(value: &str) -> String {
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

pub(crate) fn split_frontmatter(
    input: &str,
) -> Result<(Vec<String>, Vec<String>), FrontmatterError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(FrontmatterError::MissingFrontmatter);
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
        return Err(FrontmatterError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

/// Parse a YAML list of `- "item"` lines starting at `idx + 1`, leaving `idx`
/// on the first line that is not part of the list.
pub(crate) fn parse_yaml_list(lines: &[String], idx: &mut usize) -> Vec<String> {
    let mut items = Vec::new();
    *idx += 1;
    while *idx < lines.len() {
        let item = lines[*idx].trim();
        if item.is_empty() {
            *idx += 1;
            continue;
        }
        if let Some(v) = item.strip_prefix("- ") {
            items.push(strip_outer_quotes(v));
            *idx += 1;
            continue;
        }
        break; // Non-list-item line -> stop
    }
    items
}

pub(crate) fn scan_persona_frontmatter(lines: &[String]) -> PersonaFrontmatter {
    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut read_when: Vec<String> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("title:") {
            title = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("summary:") {
            summary = Some(strip_outer_quotes(rest));
            idx += 1;
            continue;
        }
        if trimmed.starts_with("read_when:") {
            // Repeated keys accumulate, unlike the last-one-wins scalars above.
            read_when.extend(parse_yaml_list(lines, &mut idx));
            continue;
        }

        idx += 1;
    }

    PersonaFrontmatter {
        title,
        summary,
        read_when,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_scan_preserves_quotes_duplicate_keys_and_list_boundaries() {
        let (lines, body) = split_frontmatter("---\ntitle: old\nsummary: ' summary '\nread_when:\n - \"one\"\n\n - two\nunknown: ignored\ntitle: \"new\"\n---\nbody").ok().unwrap();
        let fields = scan_persona_frontmatter(&lines);
        assert_eq!(fields.title.as_deref(), Some("new"));
        assert_eq!(fields.summary.as_deref(), Some("summary"));
        assert_eq!(fields.read_when, ["one", "two"]);
        assert_eq!(body, ["body"]);
    }

    #[test]
    fn body_sections_preserve_existing_duplicate_and_fence_semantics() {
        let lines = "\nintro\n  ## Same\nfirst\n## Empty\n## Same\nsecond\n```\n## InFence\ninside\n```\n## Same\n\n".lines().map(str::to_owned).collect::<Vec<_>>();
        let (body, sections) = parse_body_sections(&lines);
        assert!(body.starts_with("intro\n"));
        assert!(body.ends_with("## Same"));
        assert_eq!(sections["Same"], "second\n```");
        assert_eq!(sections["InFence"], "inside\n```");
        assert!(!sections.contains_key("Empty"));
    }
}
