//! The frontmatter grammar SOUL.md, USER.md, IDENTITY.md and BOOTSTRAP.md share.
//!
//! Not YAML, deliberately: the first line must be `---` and the next `---`
//! line closes the block (both compared trimmed); inside it, `title:` and
//! `summary:` are scalars and `read_when:` is a list of `- ` items that ends
//! at the first line that is neither blank nor an item. Anything else is
//! skipped. Each document keeps its own error type and decides which of these
//! fields it requires (`persona_frontmatter_tests.rs` pins the table).

/// The fields the grammar knows. A repeated scalar keeps its last value; a
/// repeated `read_when:` block appends to the list.
pub(super) struct PersonaFrontmatter {
    pub(super) title: Option<String>,
    pub(super) summary: Option<String>,
    pub(super) read_when: Vec<String>,
}

/// Split `input` into its frontmatter and the body lines after it, and scan
/// the frontmatter. `missing` and `unterminated` are the caller's own errors.
pub(super) fn parse<E>(
    input: &str,
    missing: E,
    unterminated: E,
) -> Result<(PersonaFrontmatter, Vec<String>), E> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(missing);
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
        return Err(unterminated);
    }

    Ok((scan(&frontmatter), body))
}

fn scan(lines: &[String]) -> PersonaFrontmatter {
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

    PersonaFrontmatter {
        title,
        summary,
        read_when,
    }
}

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
