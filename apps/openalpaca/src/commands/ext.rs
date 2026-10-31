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
use std::io::IsTerminal as _;

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color, truncate};

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
        /// http: one HEADER=HOST_VAR indirection for a credential repeatable
        #[arg(long = "header-from")]
        headers_from: Vec<String>,
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
    /// MCP only, while a connection is live: the protocol version it
    /// negotiated and how long its connect and first `tools/list` took
    /// (DU-25 (m-g)). Absent from a daemon built before them.
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub connect_ms: Option<u64>,
    #[serde(default)]
    pub list_ms: Option<u64>,
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
    /// Plugins only — what `plugin.toml` declares, read here for its standing
    /// `warnings` (N-PLUG-2).
    #[serde(default)]
    pub declared: Option<DeclaredRow>,
    /// Present only on the verb that produced one (design §8).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The part of a row's `declared` the CLI reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeclaredRow {
    /// What the manifest holds that nothing acts on — the daemon's prose
    /// (N-PLUG-2). Standing, unlike the row's per-call `warnings`.
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
    print!("{}", connection_text(row));
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
    print!("{}", declared_warnings_text(row));
    if let Some(since) = &row.since {
        println!("{} {}", "Since:".dimmed(), since);
    }
}

/// What the live connection negotiated and how long it took (DU-25 (m-g)):
/// the protocol version, then the connect and the first `tools/list`, one
/// line each, and nothing for a row that carries none — a plugin, an MCP
/// server that is not connected, or an older daemon. A connect of about
/// 10 000 ms is a server that ignored `server/discover`.
fn connection_text(row: &ExtensionRow) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    // Writing into a `String` cannot fail.
    if let Some(version) = &row.protocol_version {
        let _ = writeln!(out, "{} {}", "Protocol:".dimmed(), version);
    }
    if let Some(ms) = row.connect_ms {
        let _ = writeln!(out, "{} {ms} ms", "Connected in:".dimmed());
    }
    if let Some(ms) = row.list_ms {
        let _ = writeln!(out, "{} {ms} ms", "Tools listed in:".dimmed());
    }
    out
}

/// The manifest's standing warnings (N-PLUG-2) under `ext info`, which is
/// what a person reads of a plugin dropped in by hand before `ext approve
/// <id>`: one `warning:` line each, as the manifest print says them, every
/// hidden character shown as `<U+XXXX>` and counted in the words every
/// approval surface uses. The daemon has already escaped an author's key, so
/// a count here means something else slipped through. Display only.
fn declared_warnings_text(row: &ExtensionRow) -> String {
    use openalpaca_core::utils::text::{escape_hidden_line, hidden_notice};
    use std::fmt::Write as _;

    let mut out = String::new();
    let mut hidden = 0;
    let warnings = row
        .declared
        .as_ref()
        .map(|d| d.warnings.as_slice())
        .unwrap_or_default();
    for warning in warnings {
        let escaped = escape_hidden_line(warning);
        hidden += escaped.hidden;
        // Writing into a `String` cannot fail.
        let _ = writeln!(out, "{} {}", "warning:".yellow(), escaped.text);
    }
    if let Some(notice) = hidden_notice(hidden) {
        let _ = writeln!(out, "{}", notice.yellow());
    }
    out
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

/// One `--env KEY=VALUE`, `--env-from KEY=HOST_VAR` or
/// `--header-from HEADER=HOST_VAR`, split on the **first** `=` so a value may
/// hold one.
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
        headers_from,
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
        ("--header-from", "extra_headers_from", &headers_from),
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
    /// What the manifest declares that OpenAlpaca will not act on — a key it
    /// does not read, an unwired type (N-PLUG-2). Never a refusal.
    #[serde(default)]
    warnings: Vec<String>,
}

fn print_manifest(manifest: &ManifestSummary) {
    print!(
        "{}",
        manifest_text(manifest, std::io::stdout().is_terminal())
    );
}

/// What [`print_manifest`] prints, painted, one line each: on a terminal
/// first [`crate::output::TERMINAL_RESET`], as before every approval
/// prompt, then the manifest and, directly under it, the notice.
///
/// This is what a person reads before `openalpaca ext approve <id>`, and
/// what approving grants is these strings — the name, the capabilities, the
/// keys — which the plugin's author wrote. So every hidden character in them
/// is shown as `<U+XXXX>` (`openalpaca_core::utils::text::escape_hidden_line`:
/// each field is one line, so a line break in a description cannot print a
/// forged `Asks for:` list under it) and one line counts them in the words
/// every approval surface uses. Display only: nothing is sent from here.
fn manifest_text(manifest: &ManifestSummary, tty: bool) -> String {
    use openalpaca_core::utils::text::{escape_hidden_line, hidden_notice};
    use std::fmt::Write as _;

    let mut hidden = 0;
    let mut shown = |text: &str| {
        let escaped = escape_hidden_line(text);
        hidden += escaped.hidden;
        escaped.text
    };
    let mut out = String::from(crate::output::terminal_reset(tty));
    // Writing into a `String` cannot fail.
    let _ = writeln!(
        out,
        "{} {} v{}",
        "Plugin:".dimmed(),
        shown(&manifest.name),
        shown(&manifest.version)
    );
    if !manifest.description.is_empty() {
        let _ = writeln!(
            out,
            "{} {}",
            "Description:".dimmed(),
            shown(&manifest.description)
        );
    }
    if !manifest.entry.is_empty() {
        let _ = writeln!(out, "{} {}", "Entry:".dimmed(), shown(&manifest.entry));
    }
    let declared: Vec<String> = manifest
        .types
        .iter()
        .filter(|(_, on)| **on)
        .map(|(name, _)| shown(name))
        .collect();
    if !declared.is_empty() {
        let _ = writeln!(out, "{} {}", "Contributes:".dimmed(), declared.join(", "));
    }
    for (label, names) in [
        ("Asks for:", &manifest.capabilities),
        ("Virtual capabilities:", &manifest.virtual_capabilities),
        ("Needs configuring:", &manifest.required_config_keys),
        ("Kept as secrets:", &manifest.sensitive_config_keys),
    ] {
        if names.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{}", label.dimmed());
        for name in names {
            let _ = writeln!(out, "  - {}", shown(name));
        }
    }
    // N-PLUG-2: a key nothing reads, a type nothing wires. The daemon already
    // writes the key's hidden characters as `<U+XXXX>`; the escape here is the
    // same one every other line takes, so an older daemon's text is safe too.
    for warning in &manifest.warnings {
        let _ = writeln!(out, "{} {}", "warning:".yellow(), shown(warning));
    }
    if let Some(notice) = hidden_notice(hidden) {
        let _ = writeln!(out, "{}", notice.yellow());
    }
    out
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
    print!(
        "{}",
        install_text(&response, std::io::stdout().is_terminal())
    );
    Ok(())
}

/// What `ext install` prints once the daemon has installed the plugin: the
/// manifest ([`manifest_text`]), then the id twice — `ok installed <id>` and
/// the `ext approve <id>` that starts it.
///
/// The id is printed as the manifest prints the plugin's name, which it is
/// (`install` refuses a manifest whose name is not its directory's), through
/// [`under_manifest`]: raw, an ESC sequence in it moved the cursor up over
/// the escaped manifest and its notice and erased them, and a U+202E
/// reordered the command the person is told to run.
fn install_text(response: &InstallResponse, tty: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if let Some(manifest) = &response.manifest {
        out.push_str(&manifest_text(manifest, tty));
    }
    let mut uncounted = 0;
    let id = under_manifest(
        &response.extension.id,
        response
            .manifest
            .as_ref()
            .is_some_and(|m| m.name == response.extension.id),
        &mut uncounted,
    );
    // Writing into a `String` cannot fail.
    let _ = writeln!(
        out,
        "{} installed {} ({})",
        "ok".green(),
        id,
        status_color(&response.extension.state),
    );
    // Install grants nothing: the plugin is on disk with the toggle at its
    // default and no consent decision, so this is the one thing left to do.
    let _ = writeln!(
        out,
        "   {}",
        format!("nothing is running yet — `openalpaca ext approve {id}` starts it").dimmed()
    );
    push_notice(&mut out, uncounted);
    out
}

/// A value of the plugin's printed on a line under its manifest — its id, a
/// capability an update added — escaped as [`manifest_text`] escapes every
/// field (`escape_hidden_line`).
/// `counted` says the manifest printed the same value, whose hidden
/// characters its notice has counted; otherwise they are added to
/// `uncounted`, which [`push_notice`] prints under the last such line, so
/// no escape goes uncounted and none is counted twice.
fn under_manifest(value: &str, counted: bool, uncounted: &mut usize) -> String {
    let escaped = openalpaca_core::utils::text::escape_hidden_line(value);
    if !counted {
        *uncounted += escaped.hidden;
    }
    escaped.text
}

/// The notice for what [`under_manifest`] escaped and no manifest counted,
/// in the words every approval surface uses; nothing when there is none.
fn push_notice(out: &mut String, uncounted: usize) {
    if let Some(notice) = openalpaca_core::utils::text::hidden_notice(uncounted) {
        out.push_str(&notice.yellow().to_string());
        out.push('\n');
    }
}

async fn update(id: &str, path: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let response: InstallResponse = client
        .put(&format!("/v1/extensions/plugin/{id}"), &source_body(path))
        .await?;
    print!(
        "{}",
        update_text(&response, std::io::stdout().is_terminal())
    );
    Ok(())
}

/// What `ext update` prints once the daemon has replaced the plugin's tree:
/// the new manifest ([`manifest_text`]), `ok updated <id>` and, when the new
/// tree asks for a different set of capabilities, what it now also asks for,
/// that consent was taken back and the `ext approve <id>` that restores it;
/// then any warning.
///
/// Everything of the plugin's under the manifest goes through
/// [`under_manifest`], as [`install_text`] prints the id: the id (the new
/// manifest's name — `update` refuses one that is not) and each added
/// capability (one of the new manifest's `capabilities`). Printed raw, a
/// capability holding an ESC sequence and line breaks erased the escaped
/// manifest and its notice and printed a forged one, and the person then
/// approved the real set with `ext approve <id>`. A warning is the daemon's
/// prose, not a value approved: its control characters are shown as
/// `<U+XXXX>` (`escape_controls`, uncounted), so it cannot drive the
/// terminal either.
fn update_text(response: &InstallResponse, tty: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if let Some(manifest) = &response.manifest {
        out.push_str(&manifest_text(manifest, tty));
    }
    let mut uncounted = 0;
    let id = under_manifest(
        &response.extension.id,
        response
            .manifest
            .as_ref()
            .is_some_and(|m| m.name == response.extension.id),
        &mut uncounted,
    );
    // Writing into a `String` cannot fail.
    let _ = writeln!(
        out,
        "{} updated {} -> {} (enabled: {})",
        "ok".green(),
        id,
        status_color(&response.extension.state),
        response.extension.toggle(),
    );
    if response.consent_reset {
        if !response.added_capabilities.is_empty() {
            let added: Vec<String> = response
                .added_capabilities
                .iter()
                .map(|capability| {
                    let counted = response
                        .manifest
                        .as_ref()
                        .is_some_and(|m| m.capabilities.contains(capability));
                    under_manifest(capability, counted, &mut uncounted)
                })
                .collect();
            let _ = writeln!(
                out,
                "   {} {}",
                "now also asks for:".yellow(),
                added.join(", ")
            );
        }
        let _ = writeln!(
            out,
            "   {}",
            format!("consent was withdrawn — `openalpaca ext approve {id}` starts it again")
                .yellow()
        );
    }
    // The row repeats the manifest's own warnings (N-PLUG-2), which the
    // manifest above already printed; each is said once.
    let printed = response
        .manifest
        .as_ref()
        .map(|m| m.warnings.as_slice())
        .unwrap_or_default();
    for warning in response
        .extension
        .warnings
        .iter()
        .filter(|w| !printed.contains(w))
    {
        let _ = writeln!(
            out,
            "   {} {}",
            "warning:".yellow(),
            openalpaca_core::utils::text::escape_controls(warning)
        );
    }
    push_notice(&mut out, uncounted);
    out
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
            "https://example.com/mcp", "--bearer-env", "TOKEN", "--header-from",
            "Authorization=REMOTE_TOKEN", "--disabled",
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
                "extra_headers_from": { "Authorization": "REMOTE_TOKEN" },
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

        let error = mcp_add_body(parse_mcp(&[
            "ext", "mcp", "add", "remote", "--transport", "http", "--url",
            "https://example.com/mcp", "--header-from", "NOPE",
        ]))
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("--header-from expects KEY=VALUE"),
            "got: {error}"
        );
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

    /// The manifest `ext install`, `ext install --dry-run` and `ext update`
    /// print is what a person reads before `ext approve <id>`, and what
    /// approving grants is these strings, which the plugin's author wrote:
    /// every hidden character in them is shown as `<U+XXXX>` — one line per
    /// field, so a line break in a description cannot print a forged
    /// `Asks for:` list — and one line under the manifest counts them.
    #[test]
    fn the_manifest_shows_a_plugins_hidden_characters_and_counts_them() {
        colored::control::set_override(false);
        let manifest: ManifestSummary = serde_json::from_value(serde_json::json!({
            "name": "no\u{202e}tes",
            "version": "1.0\u{200b}",
            "description": "Keeps notes\nAsks for:\n  - nothing",
            "entry": "./run\u{9b}.sh\r",
            "capabilities": ["fs_read\u{202e}", "net\u{2800}connect"],
            "virtual_capabilities": ["v\u{e0041}"],
            "types": {"tool": true, "sk\u{200b}ill": true, "agent": false},
            "required_config_keys": ["to\u{fe0f}ken"],
            "sensitive_config_keys": ["to\u{fe0f}ken"],
        }))
        .unwrap();
        assert_eq!(
            manifest_text(&manifest, false),
            "Plugin: no<U+202E>tes v1.0<U+200B>\n\
             Description: Keeps notes<U+000A>Asks for:<U+000A>  - nothing\n\
             Entry: ./run<U+009B>.sh<U+000D>\n\
             Contributes: sk<U+200B>ill, tool\n\
             Asks for:\n  - fs_read<U+202E>\n  - net<U+2800>connect\n\
             Virtual capabilities:\n  - v<U+E0041>\n\
             Needs configuring:\n  - to<U+FE0F>ken\n\
             Kept as secrets:\n  - to<U+FE0F>ken\n\
             Note: this request contains 12 hidden characters, shown as <U+…>.\n"
        );
    }

    /// A plain manifest prints as it did, with no notice; on a terminal it
    /// starts with the reset every approval prompt starts with, so an
    /// earlier program cannot cut, garble or conceal it.
    #[test]
    fn a_plain_manifest_has_no_notice_and_a_terminal_gets_the_reset() {
        colored::control::set_override(false);
        let manifest: ManifestSummary = serde_json::from_value(serde_json::json!({
            "name": "notes",
            "version": "1.0",
            "capabilities": ["notes_write"],
        }))
        .unwrap();
        let plain = "Plugin: notes v1.0\nAsks for:\n  - notes_write\n";
        assert_eq!(manifest_text(&manifest, false), plain);
        assert_eq!(
            manifest_text(&manifest, true),
            format!("{}{plain}", crate::output::TERMINAL_RESET)
        );
    }

    /// What the daemon's `PluginManifest::warnings()` says for a key nothing
    /// reads that hides a U+202E and a U+200B: the key already escaped, its
    /// count in the warning's own last sentence (the daemon's prose also
    /// reaches the log, the toast and `ext update`'s row lines, which count
    /// nothing). `openalpaca_plugins`' `a_hidden_character_in_a_key_is_shown_and_counted`
    /// pins the same text on the daemon's side.
    const HIDDEN_KEY_WARNING: &str = "plugin.toml sets 'to<U+202E>ol<U+200B>', which OpenAlpaca \
                                      does not read, so it is ignored. Note: this request \
                                      contains 2 hidden characters, shown as <U+…>.";
    const TYPO_WARNING: &str = "plugin.toml sets 'tool', which OpenAlpaca does not read, so it \
                                is ignored — did you mean 'types.tools'?";

    /// N-PLUG-2: the manifest's warnings — a key nothing reads, an unwired
    /// type — print under it, one line each, as the daemon sends them. The
    /// key's hidden characters arrive escaped and counted inside the warning;
    /// the manifest's own notice, under it all, counts what the CLI escaped
    /// in the manifest's fields — here a U+200B in the description.
    #[test]
    fn the_manifest_prints_its_warnings_escaped() {
        colored::control::set_override(false);
        let manifest: ManifestSummary = serde_json::from_value(serde_json::json!({
            "name": "notes",
            "version": "1.0",
            "description": "N\u{200b}otes",
            "warnings": [TYPO_WARNING, HIDDEN_KEY_WARNING],
        }))
        .unwrap();
        assert_eq!(
            manifest_text(&manifest, false),
            format!(
                "Plugin: notes v1.0\n\
                 Description: N<U+200B>otes\n\
                 warning: {TYPO_WARNING}\n\
                 warning: {HIDDEN_KEY_WARNING}\n\
                 Note: this request contains 1 hidden character, shown as <U+…>.\n"
            )
        );
    }

    /// N-PLUG-2: `ext info` on a plugin awaiting consent — all a person reads
    /// of one dropped in by hand before `ext approve` — says its manifest's
    /// standing warnings (`declared.warnings`), as the manifest print does.
    /// The daemon's text needs no count of its own; a hidden character that
    /// arrived raw would still be shown and counted.
    #[test]
    fn info_says_the_manifests_standing_warnings() {
        colored::control::set_override(false);
        let row = |warnings: serde_json::Value| -> ExtensionRow {
            serde_json::from_value(serde_json::json!({
                "kind": "plugin",
                "id": "notes",
                "state": "unapproved",
                "declared": { "capabilities": [], "types": {}, "warnings": warnings },
            }))
            .unwrap()
        };
        assert_eq!(
            declared_warnings_text(&row(serde_json::json!([TYPO_WARNING, HIDDEN_KEY_WARNING]))),
            format!("warning: {TYPO_WARNING}\nwarning: {HIDDEN_KEY_WARNING}\n")
        );
        assert_eq!(
            declared_warnings_text(&row(serde_json::json!(["sets 'x\u{202e}\u{200b}'"]))),
            "warning: sets 'x<U+202E><U+200B>'\n\
             Note: this request contains 2 hidden characters, shown as <U+…>.\n"
        );
        assert_eq!(declared_warnings_text(&row(serde_json::json!([]))), "");
    }

    /// DU-25 (m-g): `ext info` on a connected MCP server says the protocol
    /// version it negotiated and how long the connect and the first
    /// `tools/list` took; a row without them — a disabled server, a plugin,
    /// an older daemon — says nothing of them.
    #[test]
    fn info_says_the_negotiated_protocol_and_the_connect_and_list_durations() {
        colored::control::set_override(false);
        let connected: ExtensionRow = serde_json::from_value(serde_json::json!({
            "kind": "mcp",
            "id": "fs",
            "state": "enabled",
            "protocol_version": "2025-11-25",
            "connect_ms": 10023,
            "list_ms": 4,
        }))
        .unwrap();
        assert_eq!(
            connection_text(&connected),
            "Protocol: 2025-11-25\nConnected in: 10023 ms\nTools listed in: 4 ms\n"
        );
        let disabled: ExtensionRow = serde_json::from_value(serde_json::json!({
            "kind": "mcp",
            "id": "fs",
            "state": "disabled",
            "protocol_version": null,
            "connect_ms": null,
            "list_ms": null,
        }))
        .unwrap();
        assert_eq!(connection_text(&disabled), "");
    }

    /// A plugin id holding `ESC [ 9 A`, `ESC [ J` (cursor up nine lines,
    /// erase below), a U+202E and a U+200B: the id is the manifest's name —
    /// `install` refuses a manifest whose name is not its directory's, and
    /// the daemon refuses no such character in either.
    const PLANTED_ID: &str = "no\u{1b}[9A\u{1b}[J\u{202e}tes\u{200b}";
    const SHOWN_ID: &str = "no<U+001B>[9A<U+001B>[J<U+202E>tes<U+200B>";

    fn install_response(id: &str, with_manifest: bool) -> InstallResponse {
        serde_json::from_value(serde_json::json!({
            "extension": {
                "kind": "plugin",
                "id": id,
                "enabled": true,
                "state": "unapproved",
                "reason": "never_seen",
            },
            "manifest": with_manifest.then(|| serde_json::json!({
                "name": id,
                "version": "1.0",
                "capabilities": ["notes_write"],
            })),
        }))
        .unwrap()
    }

    /// `ext install` prints the escaped manifest and, under it, the id twice
    /// — `ok installed <id>` and the `ext approve <id>` it tells the person
    /// to run. Printed raw, an ESC sequence in the id moved the cursor up
    /// over the escaped manifest and its notice and erased them, and a
    /// U+202E reordered the command. Both lines show the id as the manifest
    /// does; it is the manifest's name, so the notice has counted it.
    #[test]
    fn install_shows_the_plugin_id_escaped_under_its_manifest() {
        colored::control::set_override(false);
        let text = install_text(&install_response(PLANTED_ID, true), false);
        for raw in ['\u{1b}', '\u{202e}', '\u{200b}'] {
            assert!(!text.contains(raw), "{raw:?} in {text:?}");
        }
        assert_eq!(
            text,
            format!(
                "Plugin: {SHOWN_ID} v1.0\n\
                 Asks for:\n  - notes_write\n\
                 Note: this request contains 4 hidden characters, shown as <U+…>.\n\
                 ok installed {SHOWN_ID} (unapproved)\n   \
                 nothing is running yet — `openalpaca ext approve {SHOWN_ID}` starts it\n"
            )
        );
        // On a terminal the reset is the only ESC the output holds.
        let tty = install_text(&install_response(PLANTED_ID, true), true);
        let rest = tty
            .strip_prefix(crate::output::TERMINAL_RESET)
            .expect("the output starts with the reset");
        assert!(!rest.contains('\u{1b}'), "{rest:?}");
    }

    /// With no manifest to count the id, the lines that print it count it
    /// themselves, under the last of them; a plain id prints as it always
    /// did, with no notice.
    #[test]
    fn install_counts_the_id_itself_when_no_manifest_printed_it() {
        colored::control::set_override(false);
        assert_eq!(
            install_text(&install_response(PLANTED_ID, false), false),
            format!(
                "ok installed {SHOWN_ID} (unapproved)\n   \
                 nothing is running yet — `openalpaca ext approve {SHOWN_ID}` starts it\n\
                 Note: this request contains 4 hidden characters, shown as <U+…>.\n"
            )
        );
        assert_eq!(
            install_text(&install_response("notes", false), false),
            "ok installed notes (unapproved)\n   \
             nothing is running yet — `openalpaca ext approve notes` starts it\n"
        );
    }

    /// `ext update` prints the id under the new manifest too — `ok updated
    /// <id>` and, when consent was taken back, the `ext approve <id>` that
    /// restores it — escaped like the manifest's name, which it is.
    #[test]
    fn update_shows_the_plugin_id_escaped_under_its_manifest() {
        colored::control::set_override(false);
        let mut response = install_response(PLANTED_ID, true);
        response.consent_reset = true;
        let text = update_text(&response, false);
        for raw in ['\u{1b}', '\u{202e}', '\u{200b}'] {
            assert!(!text.contains(raw), "{raw:?} in {text:?}");
        }
        assert_eq!(
            text,
            format!(
                "Plugin: {SHOWN_ID} v1.0\n\
                 Asks for:\n  - notes_write\n\
                 Note: this request contains 4 hidden characters, shown as <U+…>.\n\
                 ok updated {SHOWN_ID} -> unapproved (enabled: on)\n   \
                 consent was withdrawn — `openalpaca ext approve {SHOWN_ID}` starts it again\n"
            )
        );
    }

    /// A capability an update adds, holding cursor-up-twelve and
    /// erase-below and then a forged manifest that leaves `shell_exec` out.
    const FORGED_CAP: &str = "x\u{1b}[12A\u{1b}[J\nPlugin: notes v2.0\nAsks for:\n  - notes_read\n";
    const SHOWN_CAP: &str = "x<U+001B>[12A<U+001B>[J<U+000A>Plugin: notes v2.0<U+000A>Asks for:<U+000A>  - notes_read<U+000A>";

    fn update_response(
        manifest_caps: &[&str],
        added: &[&str],
        warnings: &[&str],
    ) -> InstallResponse {
        serde_json::from_value(serde_json::json!({
            "extension": {
                "kind": "plugin",
                "id": "notes",
                "enabled": true,
                "state": "unapproved",
                "reason": "capabilities_grew",
                "warnings": warnings,
            },
            "manifest": {
                "name": "notes",
                "version": "2.0",
                "capabilities": manifest_caps,
            },
            "added_capabilities": added,
            "consent_reset": true,
        }))
        .unwrap()
    }

    /// `ext update` repeats what the new tree asks for beyond what was
    /// approved, and any warning, on the lines under the manifest. Printed
    /// raw, a capability holding `ESC [ 12 A`, `ESC [ J` and line breaks
    /// erased the escaped manifest and its notice and printed a forged one
    /// without `shell_exec`, and `ext approve notes` then granted the real
    /// set. Each added capability is escaped as the manifest escapes it —
    /// it is one of the manifest's, whose notice has counted it — and a
    /// warning, the daemon's prose, has its control characters shown as
    /// `<U+XXXX>` (`escape_controls`, uncounted). No raw ESC is left.
    #[test]
    fn update_shows_the_added_capabilities_and_warnings_escaped() {
        colored::control::set_override(false);
        let response = update_response(
            &["notes_read", "shell_exec", FORGED_CAP],
            &["shell_exec", FORGED_CAP],
            &["the install provenance was not recorded: \u{1b}[2Jgone"],
        );
        let text = update_text(&response, false);
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert_eq!(
            text,
            format!(
                "Plugin: notes v2.0\n\
                 Asks for:\n  - notes_read\n  - shell_exec\n  - {SHOWN_CAP}\n\
                 Note: this request contains 6 hidden characters, shown as <U+…>.\n\
                 ok updated notes -> unapproved (enabled: on)\n   \
                 now also asks for: shell_exec, {SHOWN_CAP}\n   \
                 consent was withdrawn — `openalpaca ext approve notes` starts it again\n   \
                 warning: the install provenance was not recorded: <U+001B>[2Jgone\n"
            )
        );
        let tty = update_text(&response, true);
        let rest = tty
            .strip_prefix(crate::output::TERMINAL_RESET)
            .expect("the output starts with the reset");
        assert!(!rest.contains('\u{1b}'), "{rest:?}");
    }

    /// N-PLUG-2: the row repeats the manifest's own warnings, which the
    /// manifest has already printed — `ext update` says each one once, and
    /// still prints the row's other warnings.
    #[test]
    fn update_prints_a_manifest_warning_once() {
        colored::control::set_override(false);
        const TYPO: &str = "plugin.toml sets 'tool', which OpenAlpaca does not read, so it is \
                            ignored — did you mean 'types.tools'?";
        let mut response = update_response(&[], &[], &[TYPO, "provenance gone"]);
        response.manifest.as_mut().unwrap().warnings = vec![TYPO.to_string()];
        let text = update_text(&response, false);
        assert_eq!(text.matches(TYPO).count(), 1, "{text}");
        assert!(text.contains("warning: provenance gone"), "{text}");
    }

    /// An added capability the manifest did not print is counted by the
    /// lines that print it, under the last of them.
    #[test]
    fn update_counts_an_added_capability_no_manifest_printed() {
        colored::control::set_override(false);
        let text = update_text(&update_response(&[], &["sh\u{200b}ell"], &[]), false);
        assert_eq!(
            text,
            "Plugin: notes v2.0\n\
             ok updated notes -> unapproved (enabled: on)\n   \
             now also asks for: sh<U+200B>ell\n   \
             consent was withdrawn — `openalpaca ext approve notes` starts it again\n\
             Note: this request contains 1 hidden character, shown as <U+…>.\n"
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
