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
//!
//! openalpaca ext install <path> [--dry-run]      # plugins — copy it in
//! openalpaca ext update  <id> <path>            # plugins — replace the tree
//! openalpaca ext uninstall <kind> <id> [--purge-data]
//! openalpaca ext mcp add <name> --transport stdio --command <cmd> [--arg …]
//! openalpaca ext mcp remove <name>              # turn it off first
//! ```
//!
//! `<kind>` is `mcp` or `plugin`. **MCP gains a CLI surface here for the first
//! time** — before this commit `grep mcp apps/openalpaca/src/` returned
//! nothing, and a server could only be toggled by hand-editing `mcp.toml`.
//!
//! `approve`/`deny`/`remove` take no kind: writing a server into your own
//! `config/mcp.toml` *is* the consent, and there is no MCP `Orphaned`.
//!
//! **`install` starts nothing** and says so: the directory lands turned on but
//! unapproved, and `ext approve` is the single action that runs it. `uninstall`
//! deletes nothing either — the directory is moved to `plugins/.trash/`, and the
//! output names where it went.
//!
//! There is deliberately **no per-tool verb**: ENABLE is per extension, ALLOW
//! is per agent, and nothing in between (S1).
//!
//! *A note for whoever edits `ExtCommands` next:* keep plain `//` comments,
//! apostrophes **and commas** out of its body at the variant level.
//! `scripts/gen_api_docs.py` splits the enum with a scanner that strips only
//! `///` and `#[..]` lines *after* splitting on top-level commas and treats a
//! lone quote character as opening a string, so any of the three grows a
//! phantom variant in `docs/api/apps/openalpaca.md` — a comma in a variant's
//! own doc comment produced one called `its`. Inside a variant's braces the
//! same characters are harmless, because the split there is a separate pass.

use std::collections::BTreeMap;

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

// `mcp add` carries thirteen options, so the `Mcp` variant is much larger than
// the two-string ones. clap builds exactly one of these per process, from argv,
// and its derive cannot take a boxed subcommand field — so the size is the
// argument surface, not a cost anything pays.
#[allow(clippy::large_enum_variant)]
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
    /// Copy a plugin directory into the plugins root (it starts nothing)
    Install {
        /// Absolute path to the plugin directory
        path: String,
        /// Report what would be installed, and copy nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Replace an installed plugin with the tree at PATH
    Update {
        /// Plugin id
        id: String,
        /// Absolute path to the replacement directory
        path: String,
    },
    /// Remove an extension for good - its files and its row alike
    Uninstall {
        /// mcp | plugin
        kind: String,
        /// Server or plugin id
        id: String,
        /// Send the plugin data directory to the trash along with it
        #[arg(long)]
        purge_data: bool,
    },
    /// Declare or undeclare an MCP server in config/mcp.toml
    Mcp {
        #[command(subcommand)]
        command: ExtMcpCommands,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum ExtMcpCommands {
    /// Write a servers block into config/mcp.toml and connect it
    Add {
        /// Server name, which is its extension id
        name: String,
        /// stdio | http
        #[arg(long, default_value = "stdio")]
        transport: String,
        /// stdio: the program to run
        #[arg(long)]
        command: Option<String>,
        /// stdio: one argument, repeatable (a leading dash is fine)
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
        /// stdio: one KEY=VALUE environment entry (never a secret) repeatable
        #[arg(long = "env")]
        envs: Vec<String>,
        /// stdio: one KEY=HOST_VAR indirection for a secret repeatable
        #[arg(long = "env-from")]
        envs_from: Vec<String>,
        /// stdio: the working directory for the child
        #[arg(long)]
        cwd: Option<String>,
        /// http: the endpoint to connect to
        #[arg(long)]
        url: Option<String>,
        /// http: the environment variable holding the bearer token
        #[arg(long)]
        bearer_env: Option<String>,
        /// http: the header an API key is sent in
        #[arg(long)]
        api_key_header: Option<String>,
        /// http: the environment variable holding the API key
        #[arg(long)]
        api_key_env: Option<String>,
        /// Seconds to wait for the connection
        #[arg(long)]
        connect_timeout_secs: Option<u64>,
        /// Seconds to wait for one request
        #[arg(long)]
        request_timeout_secs: Option<u64>,
        /// Declare it turned off, so nothing is spawned
        #[arg(long)]
        disabled: bool,
    },
    /// Remove a servers block. The server must be turned off first
    Remove {
        /// Server name
        name: String,
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

/// Fit a cell to `max` **characters**, ellipsis included.
///
/// Char-safe by construction: byte-slicing `&s[..max - 3]` panics whenever the
/// cut lands inside a multibyte character, and every string here — a server id,
/// a plugin directory name, a daemon-generated `reason` — can hold one. `ext`
/// is the surface an operator reaches for when something is already wrong, so
/// it must not be the thing that panics.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let head: String = s.chars().take(keep).collect();
    format!("{head}...")
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
        ExtCommands::Install { path, dry_run } => install(&path, dry_run).await,
        ExtCommands::Update { id, path } => update(&id, &path).await,
        ExtCommands::Uninstall {
            kind,
            id,
            purge_data,
        } => uninstall(&kind, &id, !purge_data).await,
        ExtCommands::Mcp { command } => match command {
            ExtMcpCommands::Add { .. } => {
                let body = mcp_add_body(command)?;
                mcp_add(body).await
            }
            ExtMcpCommands::Remove { name } => uninstall("mcp", &name, true).await,
        },
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

// ── GAP-24 ───────────────────────────────────────────────────────

/// The install request body. `source` is always `path`: installing from a URL
/// is declined, and stays declined, until it has had its own security review.
pub(crate) fn source_body(path: &str) -> serde_json::Value {
    serde_json::json!({ "source": "path", "path": path })
}

/// The uninstall path. `uninstall=true` is what separates the real removal from
/// the orphan-row DELETE, so it is never defaulted on.
pub(crate) fn uninstall_path(kind: &str, id: &str, keep_data: bool) -> String {
    format!("/v1/extensions/{kind}/{id}?uninstall=true&keep_data={keep_data}")
}

/// One `--env KEY=VALUE` or `--env-from KEY=HOST_VAR`, split on the **first**
/// `=` so a value may hold one.
fn env_pair(flag: &str, entry: &str) -> Result<(String, String)> {
    match entry.split_once('=') {
        Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
        _ => bail!("{flag} expects KEY=VALUE, got '{entry}'"),
    }
}

/// The `POST /v1/extensions/mcp` body. Only the keys the caller actually gave
/// are sent: the daemon's own defaults are the ones `config/mcp.toml` documents,
/// and a CLI that filled them in would freeze today's values into every block
/// it writes.
pub(crate) fn mcp_add_body(command: ExtMcpCommands) -> Result<serde_json::Value> {
    let ExtMcpCommands::Add {
        name,
        transport,
        command,
        args,
        envs,
        envs_from,
        cwd,
        url,
        bearer_env,
        api_key_header,
        api_key_env,
        connect_timeout_secs,
        request_timeout_secs,
        disabled,
    } = command
    else {
        bail!("not an add command");
    };

    let mut body = serde_json::Map::new();
    body.insert("name".into(), name.into());
    body.insert("transport".into(), transport.into());
    body.insert("enabled".into(), (!disabled).into());

    for (key, value) in [
        ("command", command),
        ("cwd", cwd),
        ("url", url),
        ("bearer_env", bearer_env),
        ("api_key_header", api_key_header),
        ("api_key_env", api_key_env),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), value.into());
        }
    }
    for (key, value) in [
        ("connect_timeout_secs", connect_timeout_secs),
        ("request_timeout_secs", request_timeout_secs),
    ] {
        if let Some(value) = value {
            body.insert(key.into(), value.into());
        }
    }
    if !args.is_empty() {
        body.insert("args".into(), serde_json::json!(args));
    }
    for (flag, field, entries) in [
        ("--env", "env", &envs),
        ("--env-from", "env_from", &envs_from),
    ] {
        if entries.is_empty() {
            continue;
        }
        let mut map = serde_json::Map::new();
        for entry in entries {
            let (key, value) = env_pair(flag, entry)?;
            map.insert(key, value.into());
        }
        body.insert(field.into(), serde_json::Value::Object(map));
    }

    Ok(serde_json::Value::Object(body))
}

/// What an install or an update answers with: the row, plus the manifest the
/// owner has to look at before approving.
#[derive(Debug, Deserialize)]
struct InstallResponse {
    extension: ExtensionRow,
    #[serde(default)]
    manifest: Option<ManifestSummary>,
    #[serde(default)]
    added_capabilities: Vec<String>,
    #[serde(default)]
    consent_reset: bool,
}

#[derive(Debug, Deserialize)]
struct ValidateResponse {
    manifest: ManifestSummary,
    #[serde(default)]
    installed: bool,
}

/// The manifest summary, as the daemon reads it off `plugin.toml`.
#[derive(Debug, Deserialize)]
struct ManifestSummary {
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    entry: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    virtual_capabilities: Vec<String>,
    #[serde(default)]
    types: BTreeMap<String, bool>,
    #[serde(default)]
    required_config_keys: Vec<String>,
    #[serde(default)]
    sensitive_config_keys: Vec<String>,
}

fn print_manifest(manifest: &ManifestSummary) {
    println!("{} {} v{}", "Plugin:".dimmed(), manifest.name, manifest.version);
    if !manifest.description.is_empty() {
        println!("{} {}", "Description:".dimmed(), manifest.description);
    }
    if !manifest.entry.is_empty() {
        println!("{} {}", "Entry:".dimmed(), manifest.entry);
    }
    let declared: Vec<&str> = manifest
        .types
        .iter()
        .filter(|(_, on)| **on)
        .map(|(name, _)| name.as_str())
        .collect();
    if !declared.is_empty() {
        println!("{} {}", "Contributes:".dimmed(), declared.join(", "));
    }
    print_names("Asks for:", &manifest.capabilities);
    print_names("Virtual capabilities:", &manifest.virtual_capabilities);
    print_names("Needs configuring:", &manifest.required_config_keys);
    print_names("Kept as secrets:", &manifest.sensitive_config_keys);
}

async fn install(path: &str, dry_run: bool) -> Result<()> {
    let client = DaemonClient::connect()?;
    if dry_run {
        let response: ValidateResponse = client
            .post("/v1/extensions/plugin/validate", &source_body(path))
            .await?;
        print_manifest(&response.manifest);
        println!(
            "{}",
            if response.installed {
                "A plugin of this name is already installed; `ext update` would replace it.".dimmed()
            } else {
                "Nothing was copied. `ext install` would land this, turned on but unapproved.".dimmed()
            }
        );
        return Ok(());
    }

    let response: InstallResponse = client
        .post("/v1/extensions/plugin", &source_body(path))
        .await?;
    if let Some(manifest) = &response.manifest {
        print_manifest(manifest);
    }
    println!(
        "{} installed {} ({})",
        "ok".green(),
        response.extension.id,
        status_color(&response.extension.state),
    );
    // Install grants nothing: the plugin is on disk with the toggle at its
    // default and no consent decision, so this is the one thing left to do.
    println!(
        "   {}",
        format!(
            "nothing is running yet — `openalpaca ext approve {}` starts it",
            response.extension.id
        )
        .dimmed()
    );
    Ok(())
}

async fn update(id: &str, path: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let response: InstallResponse = client
        .put(&format!("/v1/extensions/plugin/{id}"), &source_body(path))
        .await?;

    if let Some(manifest) = &response.manifest {
        print_manifest(manifest);
    }
    println!(
        "{} updated {} -> {} (enabled: {})",
        "ok".green(),
        response.extension.id,
        status_color(&response.extension.state),
        response.extension.toggle(),
    );
    if response.consent_reset {
        if !response.added_capabilities.is_empty() {
            println!(
                "   {} {}",
                "now also asks for:".yellow(),
                response.added_capabilities.join(", ")
            );
        }
        println!(
            "   {}",
            format!(
                "consent was withdrawn — `openalpaca ext approve {}` starts it again",
                response.extension.id
            )
            .yellow()
        );
    }
    for warning in &response.extension.warnings {
        println!("   {} {}", "warning:".yellow(), warning);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct UninstallResponse {
    removed: String,
    #[serde(default)]
    trashed: Option<String>,
    #[serde(default)]
    data_trashed: Option<String>,
}

async fn uninstall(kind: &str, id: &str, keep_data: bool) -> Result<()> {
    check_kind(kind)?;
    let client = DaemonClient::connect()?;
    let response: UninstallResponse = client
        .delete_req(&uninstall_path(kind, id, keep_data))
        .await?;

    println!("{} uninstalled {}:{}", "ok".green(), kind, response.removed);
    // Nothing was deleted, so say where it went: that is the difference
    // between this and a command an owner would be right to fear.
    if let Some(trashed) = &response.trashed {
        println!("   {} {}", "kept at".dimmed(), trashed);
    }
    if let Some(trashed) = &response.data_trashed {
        println!("   {} {}", "data kept at".dimmed(), trashed);
    }
    Ok(())
}

async fn mcp_add(body: serde_json::Value) -> Result<()> {
    let client = DaemonClient::connect()?;
    let response: InstallResponse = client.post("/v1/extensions/mcp", &body).await?;
    println!(
        "{} declared mcp:{} -> {} (enabled: {})",
        "ok".green(),
        response.extension.id,
        status_color(&response.extension.state),
        response.extension.toggle(),
    );
    if let Some(detail) = &response.extension.detail {
        println!("   {}", detail.dimmed());
    }
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

    // ── GAP-24 ───────────────────────────────────────────────────

    /// The five new verbs, and the shapes their arguments take.
    #[test]
    fn the_install_verbs_parse_with_their_arguments() {
        assert!(matches!(
            parse(&["ext", "install", "/src/notion"]),
            ExtCommands::Install { path, dry_run: false } if path == "/src/notion"
        ));
        assert!(matches!(
            parse(&["ext", "install", "/src/notion", "--dry-run"]),
            ExtCommands::Install { dry_run: true, .. }
        ));
        assert!(matches!(
            parse(&["ext", "update", "notion", "/src/notion"]),
            ExtCommands::Update { id, path } if id == "notion" && path == "/src/notion"
        ));
        assert!(matches!(
            parse(&["ext", "uninstall", "plugin", "notion"]),
            ExtCommands::Uninstall { kind, id, purge_data: false } if kind == "plugin" && id == "notion"
        ));
        assert!(matches!(
            parse(&["ext", "uninstall", "mcp", "github", "--purge-data"]),
            ExtCommands::Uninstall { purge_data: true, .. }
        ));
        assert!(matches!(
            parse(&["ext", "mcp", "remove", "github"]),
            ExtCommands::Mcp {
                command: ExtMcpCommands::Remove { name }
            } if name == "github"
        ));
    }

    /// The install body is always `source: "path"`. A URL install is declined
    /// and stays declined, so the CLI has no flag that could send one.
    #[test]
    fn the_install_body_only_ever_names_a_path() {
        assert_eq!(
            source_body("/src/notion"),
            serde_json::json!({ "source": "path", "path": "/src/notion" })
        );
    }

    /// `uninstall=true` is what separates the real removal from the orphan-row
    /// DELETE, so the CLI always says it, and `keep_data` is on unless the
    /// caller asked otherwise.
    #[test]
    fn the_uninstall_request_always_asks_for_the_uninstall() {
        assert_eq!(
            uninstall_path("plugin", "notion", true),
            "/v1/extensions/plugin/notion?uninstall=true&keep_data=true"
        );
        assert_eq!(
            uninstall_path("mcp", "github", false),
            "/v1/extensions/mcp/github?uninstall=true&keep_data=false"
        );
    }

    /// Only the keys the caller gave are sent: a CLI that filled in the
    /// daemon's defaults would freeze today's values into every block it writes.
    #[test]
    fn an_mcp_add_body_carries_only_what_was_asked_for() {
        let body = mcp_add_body(parse_mcp(&[
            "ext", "mcp", "add", "github", "--command", "npx", "--arg", "-y", "--arg",
            "@modelcontextprotocol/server-github", "--env", "RUST_LOG=debug=1", "--env-from",
            "GITHUB_TOKEN=GH_PAT",
        ]))
        .expect("a stdio declaration");

        assert_eq!(
            body,
            serde_json::json!({
                "name": "github",
                "transport": "stdio",
                "enabled": true,
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"],
                "env": { "RUST_LOG": "debug=1" },
                "env_from": { "GITHUB_TOKEN": "GH_PAT" },
            }),
            "an unasked-for key must not appear: {body}"
        );

        let body = mcp_add_body(parse_mcp(&[
            "ext", "mcp", "add", "remote", "--transport", "http", "--url",
            "https://example.com/mcp", "--bearer-env", "TOKEN", "--disabled",
        ]))
        .expect("an http declaration");
        assert_eq!(
            body,
            serde_json::json!({
                "name": "remote",
                "transport": "http",
                "enabled": false,
                "url": "https://example.com/mcp",
                "bearer_env": "TOKEN",
            })
        );
    }

    /// `--env` splits on the first `=`, so a value may hold one; anything else
    /// is refused before the request is sent — and the refusal names the flag
    /// the caller actually typed.
    #[test]
    fn a_malformed_env_entry_is_refused_locally() {
        let error = mcp_add_body(parse_mcp(&[
            "ext", "mcp", "add", "srv", "--command", "x", "--env", "NOPE",
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("--env expects KEY=VALUE"), "got: {error}");

        let error = mcp_add_body(parse_mcp(&[
            "ext", "mcp", "add", "srv", "--command", "x", "--env-from", "NOPE",
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("--env-from expects KEY=VALUE"), "got: {error}");
    }

    fn parse_mcp(argv: &[&str]) -> ExtMcpCommands {
        match parse(argv) {
            ExtCommands::Mcp { command } => command,
            _ => panic!("{argv:?} did not parse as an mcp subcommand"),
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

    /// A cell whose cut lands inside a multibyte character used to panic —
    /// `ext list` is the surface an operator reaches for when an extension has
    /// already gone wrong, and both the id and the daemon-generated `reason`
    /// can hold one.
    #[test]
    fn truncate_cuts_on_character_boundaries() {
        assert_eq!(truncate("short", 21), "short");
        // Exactly the width: untouched.
        assert_eq!(truncate("abcde", 5), "abcde");
        // Over the width: 3 chars of ellipsis, `max - 3` chars kept.
        assert_eq!(truncate("abcdefgh", 5), "ab...");
        // Multibyte, cut mid-character under the old byte slice.
        assert_eq!(truncate("ünïcödé-server-name", 8), "ünïcö...");
        assert_eq!(truncate("日本語のサーバー", 6), "日本語...");
        // Every emoji is 4 bytes; the old code panicked on all of these.
        assert_eq!(truncate("🙂🙂🙂🙂🙂🙂", 4), "🙂...");
    }
}
