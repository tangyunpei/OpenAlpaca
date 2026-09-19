//! User profile (USER.md) parsing and rendering.
//!
//! Mirrors the SOUL.md system but with lenient parsing — all sections are optional
//! since a freshly-bootstrapped profile starts empty and fills in organically.

use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFrontmatter {
    pub title: String,
    pub summary: String,
    pub read_when: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDocument {
    pub frontmatter: UserFrontmatter,
    /// Whatever stands between the `# USER.md` title and the first `##`
    /// heading, verbatim (V5).
    ///
    /// The seeded template opens with "Learn about the person you're helping.
    /// Update this as you go." — an instruction to whoever opens the file, in
    /// no section. Parsing dropped it and rendering wrote the canonical
    /// skeleton, so the first automatic extraction to touch the file deleted
    /// it. A rewrite owns the sections it updates and nothing else.
    pub preamble: String,
    /// Key-value pairs from `## Identity` (e.g. "Name" → "Alex").
    pub identity: HashMap<String, String>,
    pub communication_style: String,
    pub expertise: String,
    pub projects: String,
    pub preferences: String,
    pub notes: String,
    /// `## ` sections this module does not know, in the order they appeared,
    /// as (heading, body) (V5). Kept for the same reason as [`Self::preamble`]:
    /// a section somebody added by hand is not the rewriter's to delete.
    pub extra_sections: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserParseError {
    MissingFrontmatter,
    UnterminatedFrontmatter,
    MissingField(&'static str),
}

impl fmt::Display for UserParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => write!(f, "Missing YAML frontmatter"),
            Self::UnterminatedFrontmatter => write!(f, "Unterminated YAML frontmatter"),
            Self::MissingField(field) => write!(f, "Missing frontmatter field '{}'", field),
        }
    }
}

impl std::error::Error for UserParseError {}

// ---------------------------------------------------------------------------
// Frontmatter helpers (shared logic with soul.rs, inlined to avoid coupling)
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

fn split_frontmatter(input: &str) -> Result<(Vec<String>, Vec<String>), UserParseError> {
    let mut lines = input.lines();
    let first = lines.next().unwrap_or_default();
    if first.trim() != "---" {
        return Err(UserParseError::MissingFrontmatter);
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
        return Err(UserParseError::UnterminatedFrontmatter);
    }

    Ok((frontmatter, body))
}

fn parse_frontmatter(lines: &[String]) -> Result<UserFrontmatter, UserParseError> {
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

    let title = title.ok_or(UserParseError::MissingField("title"))?;
    let summary = summary.ok_or(UserParseError::MissingField("summary"))?;
    if read_when.is_empty() {
        return Err(UserParseError::MissingField("read_when"));
    }

    Ok(UserFrontmatter {
        title,
        summary,
        read_when,
    })
}

// ---------------------------------------------------------------------------
// Section parsing
// ---------------------------------------------------------------------------

enum Section {
    Identity,
    CommunicationStyle,
    Expertise,
    Projects,
    Preferences,
    Notes,
    Other,
}

/// Parse `* Key: Value` bullet items into a HashMap.
fn parse_identity_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let stripped = trimmed
        .strip_prefix("* ")
        .or_else(|| trimmed.strip_prefix("- "))?;
    let colon_pos = stripped.find(':')?;
    let key = stripped[..colon_pos].trim().to_string();
    let value = stripped[colon_pos + 1..].trim().to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

fn classify_heading(title: &str) -> Section {
    match title.trim() {
        "Identity" => Section::Identity,
        "Communication Style" => Section::CommunicationStyle,
        "Expertise & Background" => Section::Expertise,
        "Projects & Context" => Section::Projects,
        "Preferences" => Section::Preferences,
        "Notes" => Section::Notes,
        _ => Section::Other,
    }
}

/// Everything [`parse_user_markdown`] reads out of the body.
struct ParsedSections {
    preamble: String,
    identity: HashMap<String, String>,
    communication_style: String,
    expertise: String,
    projects: String,
    preferences: String,
    notes: String,
    extra_sections: Vec<(String, String)>,
}

fn parse_sections(lines: &[String]) -> ParsedSections {
    let mut section = Section::Other;
    let mut identity = HashMap::new();
    let mut preamble_lines: Vec<String> = Vec::new();
    let mut comm_lines: Vec<String> = Vec::new();
    let mut expertise_lines: Vec<String> = Vec::new();
    let mut projects_lines: Vec<String> = Vec::new();
    let mut preferences_lines: Vec<String> = Vec::new();
    let mut notes_lines: Vec<String> = Vec::new();
    // V5: an unrecognised `## ` heading and its body, kept verbatim.
    let mut extra_sections: Vec<(String, String)> = Vec::new();
    let mut extra_lines: Vec<String> = Vec::new();
    let mut extra_title: Option<String> = None;
    let mut seen_heading = false;

    let close_extra =
        |title: &mut Option<String>, body: &mut Vec<String>, out: &mut Vec<(String, String)>| {
            if let Some(name) = title.take() {
                out.push((name, body.join("\n").trim().to_string()));
            }
            body.clear();
        };

    for raw_line in lines {
        let line = raw_line.as_str();
        let trimmed = line.trim();

        // Section heading
        if let Some(title) = trimmed.strip_prefix("## ") {
            close_extra(&mut extra_title, &mut extra_lines, &mut extra_sections);
            seen_heading = true;
            section = classify_heading(title);
            if matches!(section, Section::Other) {
                extra_title = Some(title.trim().to_string());
            }
            continue;
        }

        // Skip top-level heading (# USER.md ...)
        if trimmed.starts_with("# ") {
            continue;
        }

        // V5: the text before the first `## ` belongs to nobody's section and
        // used to be dropped on every rewrite.
        if !seen_heading {
            if !trimmed.is_empty() {
                preamble_lines.push(trimmed.to_string());
            }
            continue;
        }

        match section {
            Section::Identity => {
                if let Some((key, value)) = parse_identity_line(trimmed)
                    && !value.is_empty()
                {
                    identity.insert(key, value);
                }
            }
            Section::CommunicationStyle => {
                if !trimmed.is_empty() {
                    comm_lines.push(trimmed.to_string());
                }
            }
            Section::Expertise => {
                if !trimmed.is_empty() {
                    expertise_lines.push(trimmed.to_string());
                }
            }
            Section::Projects => {
                if !trimmed.is_empty() {
                    projects_lines.push(trimmed.to_string());
                }
            }
            Section::Preferences => {
                if !trimmed.is_empty() {
                    preferences_lines.push(trimmed.to_string());
                }
            }
            Section::Notes => {
                if !trimmed.is_empty() {
                    notes_lines.push(trimmed.to_string());
                }
            }
            Section::Other => extra_lines.push(line.to_string()),
        }
    }
    close_extra(&mut extra_title, &mut extra_lines, &mut extra_sections);

    ParsedSections {
        preamble: preamble_lines.join("\n"),
        identity,
        communication_style: comm_lines.join(" "),
        expertise: expertise_lines.join(" "),
        projects: projects_lines.join(" "),
        preferences: preferences_lines.join(" "),
        notes: notes_lines.join(" "),
        extra_sections,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a USER.md file into a `UserDocument`.
///
/// Unlike `parse_soul_markdown`, all body sections are optional.
/// Only the YAML frontmatter is required.
pub fn parse_user_markdown(input: &str) -> Result<UserDocument, UserParseError> {
    let (frontmatter_lines, body_lines) = split_frontmatter(input)?;
    let frontmatter = parse_frontmatter(&frontmatter_lines)?;
    let sections = parse_sections(&body_lines);

    Ok(UserDocument {
        frontmatter,
        preamble: sections.preamble,
        identity: sections.identity,
        communication_style: sections.communication_style,
        expertise: sections.expertise,
        projects: sections.projects,
        preferences: sections.preferences,
        notes: sections.notes,
        extra_sections: sections.extra_sections,
    })
}

/// Render a `UserDocument` back to valid USER.md markdown.
///
/// Round-trip: `parse_user_markdown(render_user_markdown(doc))` produces a
/// semantically-equal `UserDocument`.
pub fn render_user_markdown(doc: &UserDocument) -> String {
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

    out.push_str("# USER.md -- About Your Human\n\n");

    // V5: whatever stood above the first section stands there still.
    if !doc.preamble.is_empty() {
        out.push_str(&doc.preamble);
        out.push_str("\n\n");
    }

    // -- Identity --
    out.push_str("## Identity\n\n");
    // Use a stable order for known keys, then alphabetical for the rest
    let known_keys = ["Name", "What to call them", "Pronouns", "Timezone"];
    for key in &known_keys {
        if let Some(value) = doc.identity.get(*key) {
            out.push_str(&format!("* {}: {}\n", key, value));
        } else {
            out.push_str(&format!("* {}:\n", key));
        }
    }
    // Any extra keys not in the known set
    let mut extra_keys: Vec<_> = doc
        .identity
        .keys()
        .filter(|k| !known_keys.contains(&k.as_str()))
        .collect();
    extra_keys.sort();
    for key in extra_keys {
        out.push_str(&format!("* {}: {}\n", key, doc.identity[key]));
    }
    out.push('\n');

    // -- Communication Style --
    out.push_str("## Communication Style\n\n");
    if doc.communication_style.is_empty() {
        out.push_str(
            "(How they like to communicate -- terse vs verbose, formal vs casual, etc.)\n",
        );
    } else {
        out.push_str(&doc.communication_style);
        out.push('\n');
    }
    out.push('\n');

    // -- Expertise & Background --
    out.push_str("## Expertise & Background\n\n");
    if doc.expertise.is_empty() {
        out.push_str(
            "(Technical background, domains of expertise, skill level in various areas)\n",
        );
    } else {
        out.push_str(&doc.expertise);
        out.push('\n');
    }
    out.push('\n');

    // -- Projects & Context --
    out.push_str("## Projects & Context\n\n");
    if doc.projects.is_empty() {
        out.push_str("(Current projects, tools they use, stack preferences)\n");
    } else {
        out.push_str(&doc.projects);
        out.push('\n');
    }
    out.push('\n');

    // -- Preferences --
    out.push_str("## Preferences\n\n");
    if doc.preferences.is_empty() {
        out.push_str("(Likes, dislikes, pet peeves, formatting preferences, etc.)\n");
    } else {
        out.push_str(&doc.preferences);
        out.push('\n');
    }
    out.push('\n');

    // -- Notes --
    out.push_str("## Notes\n\n");
    if doc.notes.is_empty() {
        out.push_str("(Anything else. Build this over time.)\n");
    } else {
        out.push_str(&doc.notes);
        out.push('\n');
    }

    // V5: sections this module does not know come last, in the order they
    // were read. A rewrite updates what it understands and keeps the rest.
    for (title, body) in &doc.extra_sections {
        out.push_str(&format!("\n## {title}\n\n"));
        if !body.is_empty() {
            out.push_str(body);
            out.push('\n');
        }
    }

    out
}

/// Whether a parsed section holds nothing the agent has learned (S6).
///
/// This module's own doc comment says "a freshly-bootstrapped profile starts
/// empty and fills in organically", and every caller believed it. It does
/// not: the seeded `USER.md` gives each section a parenthetical hint —
/// `(How they like to communicate -- terse vs verbose, formal vs casual,
/// etc.)` — which parses as that section's content. So the automatic
/// extraction's `set` action, which by contract "fills only empty profile
/// fields", filled nothing on any install, ever: the trait was extracted, the
/// call was billed, and `USER.md` never changed.
///
/// A section is unset when it is blank, or when it is **wholly one
/// parenthesised run** — a prompt to whoever opens the file, not a fact about
/// the person. Text outside the parentheses makes it content, so a real note
/// that happens to contain an aside is safe; a real note that is *only* an
/// aside is not, and may be overwritten by a later `set`.
pub fn section_is_unset(section: &str) -> bool {
    let text = section.trim();
    if text.is_empty() {
        return true;
    }
    if !text.starts_with('(') {
        return false;
    }
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return text[idx + ch.len_utf8()..].trim().is_empty();
                }
            }
            _ => {}
        }
    }
    // Unbalanced: treat as content rather than silently discarding it.
    false
}

/// Returns true if the document has meaningful content beyond the template defaults.
///
/// Requires identity to be non-empty AND at least one other section to also be
/// populated. This prevents bootstrap from completing when only the user's name
/// has been saved — the agent should gather communication style, expertise,
/// preferences, etc. before bootstrap is considered done.
///
/// "Populated" is [`section_is_unset`]'s answer, not `!is_empty()` (S6): the
/// seeded template's parenthetical hints are not things the agent learned,
/// and counting them let identity-plus-nothing finish onboarding — exactly
/// what the paragraph above says this must not do.
pub fn user_document_has_content(doc: &UserDocument) -> bool {
    let has_identity = !doc.identity.is_empty();
    let other_sections = [
        !section_is_unset(&doc.communication_style),
        !section_is_unset(&doc.expertise),
        !section_is_unset(&doc.projects),
        !section_is_unset(&doc.preferences),
        !section_is_unset(&doc.notes),
    ];
    let has_other = other_sections.iter().any(|&filled| filled);

    // Identity alone isn't enough — require at least one other section
    // to ensure the bootstrap conversation gathered meaningful user info.
    has_identity && has_other
}

/// Strip markdown heading markers that could be used for prompt injection.
fn sanitize_prompt_field(value: &str) -> String {
    value
        .replace("###", "")
        .replace("## ", "")
        .replace("# ", "")
        .lines()
        .next()
        .unwrap_or("")
        .to_string()
}

/// Default character budget for the user profile prompt block.
const USER_PROFILE_BUDGET: usize = 1000;

/// Render a `UserDocument` into a `### USER PROFILE ###` prompt block.
///
/// Returns an empty string if the document has no meaningful content.
///
/// The `budget` parameter controls the maximum character budget for the profile
/// block. Pass `None` to use the compiled default (1000 chars).
pub fn user_to_prompt_block(doc: &UserDocument, budget: Option<usize>) -> String {
    if !user_document_has_content(doc) {
        return String::new();
    }

    let mut block = String::from("### USER PROFILE ###\n");
    let mut budget = budget.unwrap_or(USER_PROFILE_BUDGET);

    // Identity line (compact: "Name: Alex | Timezone: PST")
    if !doc.identity.is_empty() {
        let known_keys = ["Name", "What to call them", "Pronouns", "Timezone"];
        let mut parts = Vec::new();
        for key in &known_keys {
            if let Some(value) = doc.identity.get(*key)
                && !value.is_empty()
            {
                parts.push(format!("{}: {}", key, sanitize_prompt_field(value)));
            }
        }
        // Extra keys
        let mut extra_keys: Vec<_> = doc
            .identity
            .keys()
            .filter(|k| !known_keys.contains(&k.as_str()))
            .collect();
        extra_keys.sort();
        for key in extra_keys {
            let val = &doc.identity[key];
            if !val.is_empty() {
                parts.push(format!("{}: {}", key, sanitize_prompt_field(val)));
            }
        }
        if !parts.is_empty() {
            let line = parts.join(" | ");
            let entry = format!("{}\n", line);
            if entry.len() <= budget {
                block.push_str(&entry);
                budget -= entry.len();
            }
        }
    }

    // Remaining sections as labeled lines
    let sections = [
        ("Style", &doc.communication_style),
        ("Background", &doc.expertise),
        ("Working on", &doc.projects),
        ("Preferences", &doc.preferences),
        ("Notes", &doc.notes),
    ];

    for (label, content) in &sections {
        if content.is_empty() {
            continue;
        }
        let sanitized = sanitize_prompt_field(content);
        let truncated: String = sanitized.chars().take(200).collect();
        let entry = format!("{}: {}\n", label, truncated);
        if entry.len() > budget {
            break;
        }
        block.push_str(&entry);
        budget -= entry.len();
    }

    block
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
