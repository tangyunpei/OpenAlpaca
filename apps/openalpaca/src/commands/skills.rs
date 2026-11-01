//! Skills command — the skill catalog and what its last scan found
//! (N-SKILL-2, audit Z3-03).
//!
//! `openalpaca skills list` prints the skills the daemon loaded
//! (`GET /v1/skills`), then every problem its last scan of the skills
//! directory found (`GET /v1/skills/diagnostics`): a SKILL.md it did not load
//! (`error`: unreadable, not a regular file, too large, no frontmatter,
//! invalid YAML, a missing field — at boot the skill is missing; after a hot
//! edit the skill stays listed but does not run until the file is fixed, an
//! invocation failing with the same error) or read in spite of a problem
//! (`warning`: an unknown key, a deprecated field, an id that is not the
//! directory's name, a slash command another skill also claims).
//! `openalpaca skills info <skill>` shows one skill — by id or slash command —
//! and the problems found in its file, including a skill that was skipped and
//! so is not in the catalog.
//!
//! Both are reads; there is no per-skill switch to write (S1). Every string
//! the daemon serves is printed with its hidden characters escaped, since a
//! plugin's skill carries the plugin's own text.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, table_header_lines, truncate};

#[derive(Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub command: SkillsCommands,
}

#[derive(Subcommand)]
pub enum SkillsCommands {
    /// List the loaded skills, then every problem the last scan found in a SKILL.md
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show one skill — by id or slash command — and the problems found in its SKILL.md
    Info {
        /// The skill's id (its directory name) or its slash command
        skill: String,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
}

/// One row of `GET /v1/skills`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requires_capabilities: Vec<String>,
    #[serde(default)]
    pub triggers: Triggers,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub invocations_today: i64,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: String,
}

/// How a skill fires: its slash command (without the `/`) and its keywords.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Triggers {
    #[serde(default)]
    pub slash: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// One row of `GET /v1/skills/diagnostics`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticRow {
    pub code: String,
    pub severity: String,
    pub path: String,
    #[serde(default)]
    pub skill_id: Option<String>,
    pub message: String,
}

/// A daemon-served string as it is printed: on one line, every hidden
/// character written as `<U+XXXX>`.
fn shown(text: &str) -> String {
    openalpaca_core::utils::text::escape_hidden_line(text).text
}

impl TableRow for SkillRow {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("ID", 24),
            ("SLASH", 16),
            ("SOURCE", 16),
            ("TODAY", 5),
            ("DESCRIPTION", 44),
        ]
    }

    fn table_row(&self) -> String {
        let slash = self
            .triggers
            .slash
            .as_deref()
            .map(|s| format!("/{s}"))
            .unwrap_or_else(|| "-".to_string());
        format!(
            "{:<24} {:<16} {:<16} {:<5} {}",
            truncate(&shown(&self.id), 24),
            truncate(&shown(&slash), 16),
            truncate(&shown(&self.author), 16),
            self.invocations_today,
            truncate(&shown(&self.description), 44),
        )
    }
}

/// The lines that report what the last scan found, empty when it found
/// nothing: a count, then per problem its severity, code and file, and the
/// message under them.
pub(crate) fn diagnostic_lines(diagnostics: &[DiagnosticRow]) -> Vec<String> {
    if diagnostics.is_empty() {
        return Vec::new();
    }
    let errors = diagnostics.iter().filter(|d| d.severity == "error").count();
    let warnings = diagnostics.len() - errors;
    let mut lines = vec![format!(
        "{} in the skill files ({errors} not loaded, {}):",
        plural(diagnostics.len(), "problem"),
        plural(warnings, "warning")
    )];
    for d in diagnostics {
        let severity = if d.severity == "error" {
            format!("{:<7}", d.severity).red().to_string()
        } else {
            format!("{:<7}", shown(&d.severity)).yellow().to_string()
        };
        lines.push(format!(
            "  {severity} {:<18} {}",
            shown(&d.code),
            shown(&d.path)
        ));
        lines.push(format!("          {}", shown(&d.message)));
    }
    lines
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Said of a listed skill whose file holds an error: a hot edit that could
/// not be loaded. The catalog keeps the entry shown above, but every
/// invocation reads the file again and fails until it is fixed.
const EDIT_NOT_LOADED: &str = "Its SKILL.md as edited could not be loaded: the skill stays listed, \
     but invoking it fails with the error below until the file is fixed.";

/// What `skills list` prints as a table: the catalog, then the problems.
pub(crate) fn list_lines(skills: &[SkillRow], diagnostics: &[DiagnosticRow]) -> Vec<String> {
    let mut lines = Vec::new();
    if skills.is_empty() {
        lines.push("No skills loaded.".dimmed().to_string());
    } else {
        let (header, rule) = table_header_lines(&SkillRow::headers());
        lines.push(header.dimmed().to_string());
        lines.push(rule.dimmed().to_string());
        lines.extend(skills.iter().map(TableRow::table_row));
    }
    let problems = diagnostic_lines(diagnostics);
    if !problems.is_empty() {
        lines.push(String::new());
        lines.extend(problems);
    }
    lines
}

/// The catalog row `wanted` names — its id, or its slash command with or
/// without the `/` — case-insensitively.
fn find_skill<'a>(skills: &'a [SkillRow], wanted: &str) -> Option<&'a SkillRow> {
    let wanted = wanted.trim();
    let bare = wanted.strip_prefix('/').unwrap_or(wanted).to_lowercase();
    skills
        .iter()
        .find(|s| s.id.to_lowercase() == bare)
        .or_else(|| {
            skills.iter().find(|s| {
                s.triggers
                    .slash
                    .as_deref()
                    .is_some_and(|slash| slash.to_lowercase() == bare)
            })
        })
}

/// One skill as `skills info` finds it: its catalog row, if it loaded, and
/// the problems found in its file, by its id.
pub(crate) struct Found<'a> {
    pub skill: Option<&'a SkillRow>,
    pub id: String,
    pub problems: Vec<DiagnosticRow>,
}

/// Look `wanted` up in both reads. A skill the scan skipped has no catalog
/// row; its problems are found by the id the diagnostics carry.
pub(crate) fn lookup<'a>(
    skills: &'a [SkillRow],
    diagnostics: &[DiagnosticRow],
    wanted: &str,
) -> Found<'a> {
    let skill = find_skill(skills, wanted);
    let id = match skill {
        Some(skill) => skill.id.to_lowercase(),
        None => wanted.trim().trim_start_matches('/').to_lowercase(),
    };
    let problems = diagnostics
        .iter()
        .filter(|d| {
            d.skill_id
                .as_deref()
                .is_some_and(|skill_id| skill_id.to_lowercase() == id)
        })
        .cloned()
        .collect();
    Found {
        skill,
        id,
        problems,
    }
}

/// What `skills info <skill>` prints: the skill, then the problems in its
/// file. A skill the scan skipped is not in the catalog; its problems are
/// still shown, under a line saying it is not loaded.
pub(crate) fn info_lines(found: &Found<'_>, wanted: &str) -> Result<Vec<String>> {
    let (id, own) = (&found.id, &found.problems);
    let mut lines = Vec::new();
    match found.skill {
        Some(s) => {
            lines.push(format!("{} ({})", shown(&s.name).bold(), shown(&s.id)));
            lines.push(format!("  description: {}", shown(&s.description)));
            let slash = s
                .triggers
                .slash
                .as_deref()
                .map(|slash| format!("/{}", shown(slash)))
                .unwrap_or_else(|| "-".to_string());
            lines.push(format!("  slash:       {slash}"));
            lines.push(format!("  keywords:    {}", joined(&s.triggers.keywords)));
            lines.push(format!(
                "  requires:    {}",
                joined(&s.requires_capabilities)
            ));
            lines.push(format!(
                "  schedule:    {}",
                s.schedule
                    .as_deref()
                    .map(shown)
                    .unwrap_or_else(|| "-".to_string())
            ));
            lines.push(format!("  source:      {}", shown(&s.author)));
            lines.push(format!(
                "  version:     {}",
                s.version
                    .as_deref()
                    .map(shown)
                    .unwrap_or_else(|| "-".to_string())
            ));
            lines.push(format!(
                "  today:       {} invocations",
                s.invocations_today
            ));
            if !own.is_empty() {
                lines.push(String::new());
                if own.iter().any(|d| d.severity == "error") {
                    lines.push(EDIT_NOT_LOADED.to_string());
                }
                lines.extend(diagnostic_lines(own));
            }
        }
        None if !own.is_empty() => {
            lines.push(format!(
                "Skill '{}' is not loaded — the daemon skipped its SKILL.md.",
                shown(id)
            ));
            lines.push(String::new());
            lines.extend(diagnostic_lines(own));
        }
        None => bail!(
            "No skill '{}' — `openalpaca skills list` shows the skills loaded and the problems found",
            shown(wanted)
        ),
    }
    Ok(lines)
}

fn joined(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items
            .iter()
            .map(|i| shown(i))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Both reads: the catalog and what its last scan found.
pub(crate) async fn fetch(client: &DaemonClient) -> Result<(Vec<SkillRow>, Vec<DiagnosticRow>)> {
    let skills: Vec<SkillRow> = client.get("/v1/skills").await?;
    let diagnostics: Vec<DiagnosticRow> = client.get("/v1/skills/diagnostics").await?;
    Ok((skills, diagnostics))
}

/// The JSON a `--format json` prints: both arrays, hidden characters escaped
/// the lossless JSON way.
fn json_document(value: &serde_json::Value) -> String {
    crate::output::json_with_hidden_escaped(
        &serde_json::to_string_pretty(value).unwrap_or_default(),
    )
}

pub async fn run(args: SkillsArgs) -> Result<()> {
    let client = DaemonClient::connect()?;
    match args.command {
        SkillsCommands::List { format } => {
            let (skills, diagnostics) = fetch(&client).await?;
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    json_document(&serde_json::json!({
                        "skills": skills,
                        "diagnostics": diagnostics,
                    }))
                ),
                OutputFormat::Table => {
                    for line in list_lines(&skills, &diagnostics) {
                        println!("{line}");
                    }
                }
            }
        }
        SkillsCommands::Info { skill, format } => {
            let (skills, diagnostics) = fetch(&client).await?;
            let found = lookup(&skills, &diagnostics, &skill);
            match format {
                OutputFormat::Json => {
                    if found.skill.is_none() && found.problems.is_empty() {
                        bail!("No skill '{}'", shown(&skill));
                    }
                    println!(
                        "{}",
                        json_document(&serde_json::json!({
                            "skill": found.skill,
                            "diagnostics": found.problems,
                        }))
                    );
                }
                OutputFormat::Table => {
                    for line in info_lines(&found, &skill)? {
                        println!("{line}");
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
