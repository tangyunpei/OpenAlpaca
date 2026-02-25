use super::prompt::SystemPersona;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoulFrontmatter {
    pub title: String,
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoulDocument {
    pub frontmatter: SoulFrontmatter,
    pub core_truths: Vec<String>,
    pub boundaries: Vec<String>,
    pub vibe: String,
    pub continuity: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoulParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
    MissingSection(&'static str),
    EmptySection(&'static str),
}

impl fmt::Display for SoulParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
            Self::MissingSection(section) => write!(f, "Missing required section '{}'", section),
            Self::EmptySection(section) => write!(f, "Section '{}' is empty", section),
        }
    }
}

impl std::error::Error for SoulParseError {}

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

fn normalize_text(value: &str) -> String {
    value
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), SoulParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(SoulParseError::MissingFrontmatter);
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
        return Err(SoulParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

fn parse_frontmatter(lines: &[String]) -> Result<SoulFrontmatter, SoulParseError> {
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

    let title = title.ok_or(SoulParseError::MissingField("title"))?;
    let summary = summary.ok_or(SoulParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(SoulParseError::MissingField("read_when"));
    }

    Ok(SoulFrontmatter {
        title,
        summary,
        read_when,
    })
}

enum Section {
    CoreTruths,
    Boundaries,
    Vibe,
    Continuity,
    Other,
}

fn push_block(target: &mut Vec<String>, buf: &mut String) {
    if buf.trim().is_empty() {
        buf.clear();
        return;
    }
    target.push(normalize_text(buf));
    buf.clear();
}

fn append_line(buf: &mut String, line: &str) {
    if !buf.is_empty() {
        buf.push(' ');
    }
    buf.push_str(line.trim());
}

#[allow(clippy::type_complexity)]
fn parse_sections(
    lines: &[String],
) -> Result<(Vec<String>, Vec<String>, String, Vec<String>), SoulParseError> {
    let mut section = Section::Other;
    let mut saw_core_truths = false;
    let mut saw_boundaries = false;
    let mut saw_vibe = false;
    let mut saw_continuity = false;

    let mut core_truths = Vec::new();
    let mut boundaries = Vec::new();
    let mut vibe_lines = Vec::new();
    let mut continuity = Vec::new();

    let mut core_buf = String::new();
    let mut continuity_buf = String::new();

    for raw_line in lines {
        let line = raw_line.trim();

        if line == "---" {
            push_block(&mut core_truths, &mut core_buf);
            push_block(&mut continuity, &mut continuity_buf);
            section = Section::Other;
            continue;
        }

        if let Some(title) = line.strip_prefix("## ") {
            push_block(&mut core_truths, &mut core_buf);
            push_block(&mut continuity, &mut continuity_buf);
            section = match title.trim() {
                "Core Truths" => {
                    saw_core_truths = true;
                    Section::CoreTruths
                }
                "Boundaries" => {
                    saw_boundaries = true;
                    Section::Boundaries
                }
                "Vibe" => {
                    saw_vibe = true;
                    Section::Vibe
                }
                "Continuity" => {
                    saw_continuity = true;
                    Section::Continuity
                }
                _ => Section::Other,
            };
            continue;
        }

        match section {
            Section::CoreTruths => {
                if line.is_empty() {
                    push_block(&mut core_truths, &mut core_buf);
                } else {
                    append_line(&mut core_buf, line);
                }
            }
            Section::Boundaries => {
                if let Some(v) = line.strip_prefix("- ") {
                    boundaries.push(normalize_text(v));
                }
            }
            Section::Vibe => {
                if !line.is_empty() {
                    vibe_lines.push(normalize_text(line));
                }
            }
            Section::Continuity => {
                if line.is_empty() {
                    push_block(&mut continuity, &mut continuity_buf);
                } else {
                    append_line(&mut continuity_buf, line);
                }
            }
            Section::Other => {}
        }
    }

    push_block(&mut core_truths, &mut core_buf);
    push_block(&mut continuity, &mut continuity_buf);

    if !saw_core_truths {
        return Err(SoulParseError::MissingSection("Core Truths"));
    }
    if !saw_boundaries {
        return Err(SoulParseError::MissingSection("Boundaries"));
    }
    if !saw_vibe {
        return Err(SoulParseError::MissingSection("Vibe"));
    }
    if !saw_continuity {
        return Err(SoulParseError::MissingSection("Continuity"));
    }

    if core_truths.is_empty() {
        return Err(SoulParseError::EmptySection("Core Truths"));
    }
    if boundaries.is_empty() {
        return Err(SoulParseError::EmptySection("Boundaries"));
    }
    let vibe = normalize_text(&vibe_lines.join(" "));
    if vibe.is_empty() {
        return Err(SoulParseError::EmptySection("Vibe"));
    }
    if continuity.is_empty() {
        return Err(SoulParseError::EmptySection("Continuity"));
    }

    Ok((core_truths, boundaries, vibe, continuity))
}

pub fn parse_soul_markdown(input: &str) -> Result<SoulDocument, SoulParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;
    let (core_truths, boundaries, vibe, continuity) = parse_sections(&body_lines)?;

    Ok(SoulDocument {
        frontmatter,
        core_truths,
        boundaries,
        vibe,
        continuity,
    })
}

/// Render a `SoulDocument` back to valid SOUL markdown.
///
/// The output is designed to round-trip: `parse_soul_markdown(render_soul_markdown(doc))`
/// should produce a semantically-equal `SoulDocument`.
pub fn render_soul_markdown(doc: &SoulDocument) -> String {
    let mut out = String::new();

    // -- Frontmatter --
    out.push_str("---\n");
    out.push_str(&format!("title: \"{}\"\n", doc.frontmatter.title));
    out.push_str(&format!("summary: \"{}\"\n", doc.frontmatter.summary));
    out.push_str("read_when:\n");
    for item in &doc.frontmatter.read_when {
        out.push_str(&format!("  - {}\n", item));
    }
    out.push_str("---\n\n");

    // -- Core Truths --
    out.push_str("## Core Truths\n\n");
    for (i, truth) in doc.core_truths.iter().enumerate() {
        out.push_str(truth);
        out.push('\n');
        if i < doc.core_truths.len() - 1 {
            out.push('\n');
        }
    }
    out.push('\n');

    // -- Boundaries --
    out.push_str("## Boundaries\n\n");
    for boundary in &doc.boundaries {
        out.push_str(&format!("- {}\n", boundary));
    }
    out.push('\n');

    // -- Vibe --
    out.push_str("## Vibe\n\n");
    out.push_str(&doc.vibe);
    out.push_str("\n\n");

    // -- Continuity --
    out.push_str("## Continuity\n\n");
    for (i, line) in doc.continuity.iter().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i < doc.continuity.len() - 1 {
            out.push('\n');
        }
    }

    out
}

pub fn soul_to_system_persona(doc: &SoulDocument) -> SystemPersona {
    let mut base_instructions = String::new();
    base_instructions.push_str("Communication style:\n");
    base_instructions.push_str(&doc.vibe);
    base_instructions.push_str("\n\nContinuity policy:\n");
    for line in &doc.continuity {
        base_instructions.push_str("- ");
        base_instructions.push_str(line);
        base_instructions.push('\n');
    }

    SystemPersona {
        name: "OpenAlpaca".to_string(),
        core_values: doc.core_truths.clone(),
        safety_rules: doc.boundaries.clone(),
        base_instructions: base_instructions.trim().to_string(),
    }
}

#[cfg(test)]
mod tests;
