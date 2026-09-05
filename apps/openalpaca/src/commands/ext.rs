//! `openalpaca ext` — the ENABLE axis from the command line (extension design
//! §8, ADR-030).
//!
//! ```text
//! openalpaca ext list [--include-orphaned] [--format table|json]
//! openalpaca ext info    <kind> <id>
//! openalpaca ext enable  <kind> <id>
//! openalpaca ext disable <kind> <id>
//! openalpaca ext reload  <kind> <id>
//! openalpaca ext approve <id>          # plugins — consent, not the toggle
//! openalpaca ext deny    <id>          # plugins — consent, and a full unload
//! openalpaca ext remove  <id>          # plugins — orphaned rows only
//! ```
//!
//! `<kind>` is `mcp` or `plugin`. **MCP gains a CLI surface here for the first
//! time** — before this commit `grep mcp apps/openalpaca/src/` returned
//! nothing, and a server could only be toggled by hand-editing `mcp.toml`.
//!
//! `approve`/`deny`/`remove` take no kind: writing a server into your own
//! `config/mcp.toml` *is* the consent, and there is no MCP `Orphaned`.
//!
//! There is deliberately **no per-tool verb**: ENABLE is per extension, ALLOW
//! is per agent, and nothing in between (S1).
//!
//! *A note for whoever edits `ExtCommands` next:* keep plain `//` comments and
//! apostrophes out of its body. `scripts/gen_api_docs.py` splits the enum with
//! a scanner that strips only `///` and `#[..]` lines and treats a lone quote
//! character as opening a string, so either one grows a phantom variant in
//! `docs/api/apps/openalpaca.md`.

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct ExtArgs {
    #[command(subcommand)]
    pub command: ExtCommands,
}

#[derive(Subcommand)]
pub enum ExtCommands {
    /// List MCP servers and plugins
    List {
        /// Include plugins whose directory is gone
        #[arg(long)]
        include_orphaned: bool,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show one extension in full
    Info {
        /// mcp | plugin
        kind: String,
        /// Server or plugin id
        id: String,
    },
    /// Turn an extension on (writes the bit, then loads)
    Enable {
        /// mcp | plugin
        kind: String,
        /// Server or plugin id
        id: String,
    },
    /// Turn an extension off (writes the bit, then unloads)
    Disable {
        /// mcp | plugin
        kind: String,
        /// Server or plugin id
        id: String,
    },
    /// Re-apply an edited declaration or a rotated credential
    Reload {
        /// mcp | plugin
        kind: String,
        /// Server or plugin id
        id: String,
    },
    /// Record consent for a plugin (does not turn it on)
    Approve {
        /// Plugin id
        id: String,
    },
    /// Refuse a plugin and unload it (leaves the toggle position alone)
    Deny {
        /// Plugin id
        id: String,
    },
    /// Remove the permissions entry of an orphaned plugin
    Remove {
        /// Plugin id
        id: String,
    },
}

// ── The §8 row ───────────────────────────────────────────────────

/// One `GET /v1/extensions` row. Every field is optional or defaulted so a
/// daemon that has not yet grown one does not break the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionRow {
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub transport: Option<String>,
    /// `null` when the disposition store cannot be read (design §4).
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub consent: Option<String>,
    pub state: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub actionable: bool,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub missing_config_keys: Vec<String>,
    #[serde(default)]
    pub added_capabilities: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub skipped_tools: Vec<String>,
    #[serde(default)]
    pub withdrawn_by_server: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub since: Option<String>,
    /// Present only on the verb that produced one (design §8).
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl ExtensionRow {
    /// The toggle column. `-` is the row whose bit nobody can read — it is not
    /// `off`, and saying so would be a lie the owner would act on.
    fn toggle(&self) -> &'static str {
        match self.enabled {
            Some(true) => "on",
            Some(false) => "off",
            None => "-",
        }
    }
}

impl TableRow for ExtensionRow {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("KIND", 8),
            ("ID", 22),
            ("ENABLED", 8),
            ("STATE", 12),
            ("REASON", 20),
            ("TOOLS", 6),
        ]
    }

    fn table_row(&self) -> String {
        format!(
            "{:<8} {:<22} {:<8} {:<12} {:<20} {:<6}",
            truncate(&self.kind, 7),
            truncate(&self.id, 21),
            self.toggle(),
            status_color(&self.state),
            truncate(self.reason.as_deref().unwrap_or("-"), 19),
            self.tools.len(),
        )
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

/// `mcp` and `plugin` are the only kinds. Refused here rather than sent, so the
/// error names the two words instead of reading as a missing extension.
fn check_kind(kind: &str) -> Result<()> {
    if kind == "mcp" || kind == "plugin" {
        Ok(())
    } else {
        bail!("unknown extension kind '{kind}' (expected 'mcp' or 'plugin')")
    }
}

// ── Command runner ───────────────────────────────────────────────

pub async fn run(args: ExtArgs) -> Result<()> {
    match args.command {
        ExtCommands::List {
            include_orphaned,
            format,
        } => list(include_orphaned, format).await,
        ExtCommands::Info { kind, id } => info(&kind, &id).await,
        ExtCommands::Enable { kind, id } => verb(&kind, &id, "enable").await,
        ExtCommands::Disable { kind, id } => verb(&kind, &id, "disable").await,
        ExtCommands::Reload { kind, id } => verb(&kind, &id, "reload").await,
        ExtCommands::Approve { id } => verb("plugin", &id, "approve").await,
        ExtCommands::Deny { id } => verb("plugin", &id, "deny").await,
        ExtCommands::Remove { id } => remove(&id).await,
    }
}

pub(crate) async fn fetch_rows(include_orphaned: bool) -> Result<Vec<ExtensionRow>> {
    let client = DaemonClient::connect()?;
    let path = if include_orphaned {
        "/v1/extensions?include_orphaned=true"
    } else {
        "/v1/extensions"
    };
    client.get(path).await
}

async fn list(include_orphaned: bool, format: OutputFormat) -> Result<()> {
    let rows = fetch_rows(include_orphaned).await?;
    print_list(&rows, format);
    Ok(())
}

async fn info(kind: &str, id: &str) -> Result<()> {
    check_kind(kind)?;
    let rows = fetch_rows(true).await?;
    match rows.iter().find(|r| r.kind == kind && r.id == id) {
        Some(row) => {
            print_row(row);
            Ok(())
        }
        None => bail!("no {kind} extension named '{id}'"),
    }
}

pub(crate) fn print_row(row: &ExtensionRow) {
    println!("{} {}:{}", "Extension:".dimmed(), row.kind, row.id);
    if let Some(version) = &row.version {
        println!("{} {}", "Version:".dimmed(), version);
    }
    if let Some(transport) = &row.transport {
        println!("{} {}", "Transport:".dimmed(), transport);
    }
    println!("{} {}", "Enabled:".dimmed(), row.toggle());
    if let Some(consent) = &row.consent {
        println!("{} {}", "Consent:".dimmed(), consent);
    }
    println!("{} {}", "State:".dimmed(), status_color(&row.state));
    if let Some(reason) = &row.reason {
        let actionable = if row.actionable {
            " (actionable)"
        } else {
            ""
        };
        println!("{} {}{}", "Reason:".dimmed(), reason, actionable);
    }
    if let Some(detail) = &row.detail {
        println!("{} {}", "Detail:".dimmed(), detail);
    }
    if let Some(hint) = &row.hint {
        println!("{} {}", "Hint:".dimmed(), hint);
    }
    print_names("Missing config:", &row.missing_config_keys);
    print_names("Added capabilities:", &row.added_capabilities);
    print_names("Tools:", &row.tools);
    print_names("Skipped (name in use):", &row.skipped_tools);
    print_names("Withdrawn by the server:", &row.withdrawn_by_server);
    print_names("Skills:", &row.skills);
    print_names("Agents:", &row.agents);
    if let Some(since) = &row.since {
        println!("{} {}", "Since:".dimmed(), since);
    }
}

fn print_names(label: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    println!("{}", label.dimmed());
    for name in names {
        println!("  - {name}");
    }
}

pub(crate) async fn verb(kind: &str, id: &str, verb: &str) -> Result<()> {
    check_kind(kind)?;
    let client = DaemonClient::connect()?;
    let row: ExtensionRow = client
        .post(
            &format!("/v1/extensions/{kind}/{id}/{verb}"),
            &serde_json::json!({}),
        )
        .await?;

    println!(
        "{} {}:{} -> {} (enabled: {})",
        "ok".green(),
        row.kind,
        row.id,
        status_color(&row.state),
        row.toggle(),
    );
    // A `200` whose row reads `failed` is the design's own answer: the write
    // succeeded and the intent is durable; the connection outcome is a separate
    // fact in the body. Say both rather than implying the verb failed.
    if let Some(detail) = &row.detail {
        println!("   {}", detail.dimmed());
    }
    for warning in &row.warnings {
        println!("   {} {}", "warning:".yellow(), warning);
    }
    Ok(())
}

async fn remove(id: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let _: serde_json::Value = client
        .delete_req(&format!("/v1/extensions/plugin/{id}"))
        .await?;
    println!("{} removed the permissions entry for '{}'", "ok".green(), id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: ExtArgs,
    }

    fn parse(argv: &[&str]) -> ExtCommands {
        Harness::try_parse_from(argv)
            .unwrap_or_else(|e| panic!("{argv:?} did not parse: {e}"))
            .args
            .command
    }

    #[test]
    fn every_verb_parses_with_its_arguments() {
        assert!(matches!(
            parse(&["ext", "list"]),
            ExtCommands::List {
                include_orphaned: false,
                ..
            }
        ));
        assert!(matches!(
            parse(&["ext", "list", "--include-orphaned"]),
            ExtCommands::List {
                include_orphaned: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&["ext", "enable", "mcp", "github"]),
            ExtCommands::Enable { kind, id } if kind == "mcp" && id == "github"
        ));
        assert!(matches!(
            parse(&["ext", "disable", "plugin", "notion"]),
            ExtCommands::Disable { kind, id } if kind == "plugin" && id == "notion"
        ));
        assert!(matches!(
            parse(&["ext", "reload", "mcp", "github"]),
            ExtCommands::Reload { kind, id } if kind == "mcp" && id == "github"
        ));
        assert!(matches!(
            parse(&["ext", "info", "mcp", "github"]),
            ExtCommands::Info { kind, id } if kind == "mcp" && id == "github"
        ));
    }

    /// The three consent verbs are plugin-only, so they take **no** kind: a
    /// `kind` argument there would invite `ext approve mcp github`, which the
    /// daemon can only answer `409 unsupported_for_kind`.
    #[test]
    fn the_consent_verbs_take_only_an_id() {
        assert!(matches!(
            parse(&["ext", "approve", "notion"]),
            ExtCommands::Approve { id } if id == "notion"
        ));
        assert!(matches!(
            parse(&["ext", "deny", "notion"]),
            ExtCommands::Deny { id } if id == "notion"
        ));
        assert!(matches!(
            parse(&["ext", "remove", "notion"]),
            ExtCommands::Remove { id } if id == "notion"
        ));
        assert!(
            Harness::try_parse_from(["ext", "approve", "plugin", "notion"]).is_err(),
            "approve must not accept a kind argument"
        );
    }

    #[test]
    fn a_missing_argument_is_a_parse_error() {
        for argv in [
            vec!["ext", "enable"],
            vec!["ext", "enable", "mcp"],
            vec!["ext", "approve"],
            vec!["ext", "banana"],
        ] {
            assert!(
                Harness::try_parse_from(&argv).is_err(),
                "{argv:?} should not have parsed"
            );
        }
    }

    /// `check_kind` is what keeps a typo from being reported as a missing
    /// extension: the two words are named up front.
    #[test]
    fn only_the_two_kinds_are_accepted() {
        assert!(check_kind("mcp").is_ok());
        assert!(check_kind("plugin").is_ok());
        let error = check_kind("Plugin").unwrap_err().to_string();
        assert!(
            error.contains("'mcp'") && error.contains("'plugin'"),
            "the refusal should name both kinds, got: {error}"
        );
    }

    /// §4's two unreadable rows report `enabled: null`, and the CLI must not
    /// render that as `off`.
    #[test]
    fn an_unreadable_disposition_is_neither_on_nor_off() {
        let row: ExtensionRow = serde_json::from_value(serde_json::json!({
            "kind": "mcp",
            "id": "config/mcp.toml",
            "enabled": null,
            "state": "failed",
            "reason": "config_invalid",
        }))
        .expect("the §8 row should deserialize from its minimum fields");
        assert_eq!(row.toggle(), "-");
        assert_eq!(row.enabled, None);
    }

    #[test]
    fn a_full_row_round_trips() {
        let row: ExtensionRow = serde_json::from_value(serde_json::json!({
            "kind": "plugin",
            "id": "notion",
            "version": "1.4.0",
            "transport": null,
            "enabled": true,
            "consent": "approved",
            "state": "enabled",
            "reason": null,
            "actionable": false,
            "detail": null,
            "hint": null,
            "missing_config_keys": [],
            "added_capabilities": [],
            "tools": ["notion::create_page"],
            "skipped_tools": [],
            "withdrawn_by_server": [],
            "tools_changed_at": null,
            "declared": {"capabilities": [], "virtual_capabilities": [], "types": {}},
            "skills": ["daily-digest"],
            "agents": [],
            "connector": null,
            "provider": null,
            "since": "2026-09-01T10:04:00+00:00",
        }))
        .expect("the §8 row should deserialize in full");
        assert_eq!(row.toggle(), "on");
        assert_eq!(row.tools, vec!["notion::create_page".to_string()]);
        assert_eq!(row.skills, vec!["daily-digest".to_string()]);
    }
}
