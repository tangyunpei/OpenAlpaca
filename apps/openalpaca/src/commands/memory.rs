//! `openalpaca memory` — the memory notes from the command line (memory
//! redesign, design §7 *Surfaces*, §7a *CLI syntax*; plan 3.8).
//!
//! ```text
//! openalpaca memory list    [--kind K] [--status S] [--scope global|project] [--history] [--candidates] [--limit N]
//! openalpaca memory search  <query> [--kind K] [--scope S] [--limit N] [--historical] [--as-of RFC3339]
//! openalpaca memory show    <id> [--revision <rev>] [--raw]
//! openalpaca memory add     --kind K --title T [--scope S] [--alias A]... [--tag T]... [--fact-key KEY]
//!                           [--supersedes <id>] [BODY | -]       body: the argument, stdin ("-"), or $EDITOR
//! openalpaca memory edit    <id>                                  GET → $EDITOR → POST with the fetched revision
//! openalpaca memory confirm <id>
//! openalpaca memory undo    <id>
//! openalpaca memory forget  <id>... [--yes]
//! openalpaca memory sync
//! openalpaca memory doctor  [--repair]
//! openalpaca memory adopt   <owner-key>
//! ```
//!
//! Every daemon-backed verb takes `--global` (send no project) or `--project
//! <path>`; the default is the CLI's own working directory, sent as the
//! `x-workspace-path` header `chat` already sends, which the daemon resolves
//! to a project root by the same marker walk (design §7a, the CLI row). Ids
//! may be abbreviated to a unique prefix. A write through these verbs is a
//! user-directed act and records origin `user` (design §4).
//!
//! **A revision travels in `If-Match`.** `edit`, `confirm`, `undo` and
//! `forget` fetch the note first and send the revision they fetched, so a
//! note that changed underneath them is the daemon's `409` — an outcome
//! these verbs report, never a silent overwrite. On `edit`'s `409` the
//! edited text is **kept** in a named temp file, because an editor session
//! is work.
//!
//! `doctor --repair` and `adopt` are local: they take the daemon's singleton
//! lock the way `config reset --factory` does and refuse while a daemon
//! runs, read the owner id from the database under that lock, and touch
//! the partitions directly. `doctor` without `--repair` asks the daemon when
//! one runs and scans locally, read-only, when none does.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use colored::Colorize;
use openalpaca_core::memory::service::{WorkspaceHint, resolve_project_root, wire};
use openalpaca_storage::memory_files::{
    self as files, ControlSnapshot, MoveSnapshotState, Note, NoteId, NoteKind, NoteSource,
    NoteStatus, Partition, SourceType,
};
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{ConfigRepository, discovery, store};
use reqwest::Method;
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};

use super::persona::open_in_editor;
use crate::chat_stream::encode_header_path;
use crate::client::{DaemonClient, error_from_response};
use crate::output::OutputFormat;

#[derive(Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub command: MemoryCommands,
}

/// Which project a daemon-backed verb addresses (design §7a, the CLI row).
#[derive(Args, Clone, Debug, Default)]
pub struct ScopeArgs {
    /// Address the global partition only: send no project
    #[arg(long, conflicts_with = "project")]
    pub global: bool,
    /// Address the project at this path instead of the current directory
    #[arg(long, value_name = "PATH")]
    pub project: Option<PathBuf>,
}

impl ScopeArgs {
    /// The directory the request stands in: `None` for `--global`, the named
    /// path for `--project`, else the CLI's working directory. Canonical and
    /// absolute, because the daemon resolves it against *its* directory
    /// otherwise (R22).
    fn directory(&self) -> Result<Option<PathBuf>> {
        if self.global {
            return Ok(None);
        }
        let dir = match &self.project {
            Some(p) => std::fs::canonicalize(p).with_context(|| {
                format!("--project {}: not a directory I can read", p.display())
            })?,
            None => std::env::current_dir().context("the current directory is unreadable")?,
        };
        Ok(Some(dir))
    }

    /// The `x-workspace-path` header this call sends, percent-encoded as
    /// `chat` sends it (R81); none for `--global`.
    fn headers(&self) -> Result<Vec<(&'static str, String)>> {
        Ok(self
            .directory()?
            .map(|dir| {
                vec![(
                    "x-workspace-path",
                    encode_header_path(&dir.to_string_lossy()),
                )]
            })
            .unwrap_or_default())
    }

    /// The project root a local verb scans: the same marker walk the daemon
    /// runs on the header (design §3 rule 1, MD-5).
    fn local_project_root(&self) -> Result<Option<PathBuf>> {
        Ok(self
            .directory()?
            .and_then(|dir| resolve_project_root(WorkspaceHint::Path(&dir))))
    }
}

#[derive(Subcommand, Debug)]
pub enum MemoryCommands {
    /// List notes (active by default)
    List {
        /// Filter by kind (fact, preference, decision, procedure, reference, episode)
        #[arg(long)]
        kind: Option<String>,
        /// Filter by status (active, candidate, archived)
        #[arg(long)]
        status: Option<String>,
        /// Filter by scope (global, project)
        #[arg(long)]
        scope: Option<String>,
        /// Include superseded notes
        #[arg(long)]
        history: bool,
        /// Include candidates
        #[arg(long)]
        candidates: bool,
        /// Maximum number of results (1-100)
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Search notes by literal terms
    Search {
        /// The query
        query: String,
        /// Filter by kind
        #[arg(long)]
        kind: Option<String>,
        /// Filter by scope (global, project)
        #[arg(long)]
        scope: Option<String>,
        /// Maximum number of results (1-20)
        #[arg(long, default_value = "8")]
        limit: usize,
        /// Include superseded and archived notes
        #[arg(long)]
        historical: bool,
        /// Evaluate validity at this instant (RFC 3339)
        #[arg(long)]
        as_of: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Print one note
    Show {
        /// The note id (a unique prefix will do)
        id: String,
        /// A kept revision instead of the current one
        #[arg(long)]
        revision: Option<String>,
        /// Print the note file as the daemon serves it, frontmatter and all
        #[arg(long)]
        raw: bool,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Save a note
    Add {
        /// The kind (fact, preference, decision, procedure, reference, episode)
        #[arg(long)]
        kind: String,
        /// The title
        #[arg(long)]
        title: String,
        /// Where it lives (global, project); by kind when omitted
        #[arg(long)]
        scope: Option<String>,
        /// An alias (repeatable)
        #[arg(long = "alias", value_name = "ALIAS")]
        aliases: Vec<String>,
        /// A tag (repeatable)
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
        /// The fact key this note answers for
        #[arg(long)]
        fact_key: Option<String>,
        /// The note this one replaces (its current revision is fetched first)
        #[arg(long)]
        supersedes: Option<String>,
        /// The body: the argument, "-" for stdin, or $EDITOR when omitted
        body: Option<String>,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Edit a note in $EDITOR
    Edit {
        /// The note id (a unique prefix will do)
        id: String,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Confirm a candidate: it becomes active, and yours
    Confirm {
        /// The note id (a unique prefix will do)
        id: String,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Undo a recently learned note (one the extractor wrote from your words)
    Undo {
        /// The note id (a unique prefix will do)
        id: String,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Forget notes by id
    Forget {
        /// The note ids (unique prefixes will do)
        #[arg(required = true)]
        ids: Vec<String>,
        /// Do not ask
        #[arg(short = 'y', long = "yes")]
        yes: bool,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Run one reconciliation scan now and report what changed on disk
    Sync {
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Report the partitions' health, or repair a lost snapshot (local, daemon stopped)
    Doctor {
        /// Rebuild missing or invalid snapshots and delete stale move snapshots
        #[arg(long)]
        repair: bool,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Re-bind an orphaned owner key's partitions to the current owner (local, daemon stopped)
    Adopt {
        /// The orphaned owner key (32 hex characters, as `doctor` lists it)
        owner_key: String,
    },
}

pub async fn run(args: MemoryArgs) -> Result<()> {
    match args.command {
        MemoryCommands::List {
            kind,
            status,
            scope,
            history,
            candidates,
            limit,
            format,
            at,
        } => {
            let api = Api::connect(&at)?;
            let mut query = vec![format!("limit={limit}")];
            if let Some(k) = kind {
                query.push(format!("kind={}", urlencoding::encode(&k)));
            }
            if let Some(s) = status {
                query.push(format!("status={}", urlencoding::encode(&s)));
            }
            if let Some(s) = scope {
                query.push(format!("scope={}", urlencoding::encode(&s)));
            }
            if history {
                query.push("include_history=true".into());
            }
            if candidates {
                query.push("include_candidates=true".into());
            }
            let page = api
                .get(&format!("/v1/memories?{}", query.join("&")))
                .await?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&page)?),
                OutputFormat::Table => print_list(&page),
            }
            Ok(())
        }
        MemoryCommands::Search {
            query,
            kind,
            scope,
            limit,
            historical,
            as_of,
            format,
            at,
        } => {
            let api = Api::connect(&at)?;
            let mut q = vec![
                format!("q={}", urlencoding::encode(&query)),
                format!("limit={limit}"),
            ];
            if let Some(k) = kind {
                q.push(format!("kind={}", urlencoding::encode(&k)));
            }
            if let Some(s) = scope {
                q.push(format!("scope={}", urlencoding::encode(&s)));
            }
            if historical {
                q.push("historical=true".into());
            }
            if let Some(t) = as_of {
                q.push(format!("as_of={}", urlencoding::encode(&t)));
            }
            let found = api
                .get(&format!("/v1/memories/search?{}", q.join("&")))
                .await?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&found)?),
                OutputFormat::Table => print_search(&found),
            }
            Ok(())
        }
        MemoryCommands::Show {
            id,
            revision,
            raw,
            at,
        } => {
            let api = Api::connect(&at)?;
            let id = api.resolve_id(&id).await?;
            let path = match &revision {
                Some(rev) => format!("/v1/memories/{id}?revision={}", urlencoding::encode(rev)),
                None => format!("/v1/memories/{id}"),
            };
            let note = api.get(&path).await?;
            if raw {
                print!("{}", note_from_read(&note)?.serialize());
            } else {
                print_note(&note);
            }
            Ok(())
        }
        MemoryCommands::Add {
            kind,
            title,
            scope,
            aliases,
            tags,
            fact_key,
            supersedes,
            body,
            at,
        } => {
            let api = Api::connect(&at)?;
            let body = match body.as_deref() {
                Some("-") => {
                    let mut text = String::new();
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .context("could not read the body from stdin")?;
                    text
                }
                Some(text) => text.to_string(),
                None => body_from_editor(&title)?,
            };
            if body.trim().is_empty() {
                bail!("the body is empty; nothing was saved");
            }
            let mut request = json!({
                "title": title,
                "body": body,
                "kind": kind,
                "aliases": aliases,
                "tags": tags,
            });
            if let Some(s) = scope {
                request["scope"] = Value::String(s);
            }
            if let Some(k) = fact_key {
                request["fact_key"] = Value::String(k);
            }
            if let Some(target) = supersedes {
                // The target's current revision, so a race is a visible 409.
                let target = api.resolve_id(&target).await?;
                let current = api.get(&format!("/v1/memories/{target}")).await?;
                request["supersedes"] = json!({
                    "id": target.to_string(),
                    "expected_revision": current["revision"],
                });
            }
            let response = api
                .send(Method::POST, "/v1/memories", None, Some(&request))
                .await?;
            let status = response.status().as_u16();
            if status != 200 && status != 201 {
                return Err(error_from_response(response).await);
            }
            let saved: Value = response
                .json()
                .await
                .context("POST /v1/memories: unreadable body")?;
            let id = saved["id"].as_str().unwrap_or("?");
            if saved["dedup"] == "existing" {
                println!(
                    "{} Already remembered as {} (revision {}).",
                    "=".dimmed(),
                    id,
                    short(saved["revision"].as_str().unwrap_or(""))
                );
            } else {
                println!(
                    "{} Saved {} in the {} partition (revision {}).",
                    "✓".green(),
                    id,
                    saved["scope"].as_str().unwrap_or("?"),
                    short(saved["revision"].as_str().unwrap_or(""))
                );
                let superseded = saved["superseded"].as_array().map(Vec::len).unwrap_or(0);
                if superseded > 0 {
                    println!("  It replaces {superseded} note(s).");
                }
            }
            Ok(())
        }
        MemoryCommands::Edit { id, at } => {
            let api = Api::connect(&at)?;
            let id = api.resolve_id(&id).await?;
            match edit_note(&api, id, &open_in_editor).await? {
                EditOutcome::Unchanged => {
                    println!("No changes.");
                    Ok(())
                }
                EditOutcome::Saved { revision } => {
                    println!(
                        "{} Note {} saved (revision {}).",
                        "✓".green(),
                        id,
                        short(&revision)
                    );
                    Ok(())
                }
                EditOutcome::Conflict {
                    ours,
                    theirs,
                    kept_at,
                } => {
                    eprintln!(
                        "{} Note {} changed while you were editing it: you edited revision {}, the daemon now holds {}.",
                        "✗".red(),
                        id,
                        short(&ours),
                        short(&theirs)
                    );
                    eprintln!(
                        "Your text is kept at {} — run `openalpaca memory show {}` to see the current version, then merge and edit again.",
                        kept_at.display(),
                        short_id(&id)
                    );
                    std::process::exit(1);
                }
                EditOutcome::Refused { reason, kept_at } => {
                    eprintln!("{} Nothing was saved: {reason}", "✗".red());
                    eprintln!("Your text is kept at {}.", kept_at.display());
                    std::process::exit(1);
                }
                EditOutcome::TooLong { path } => {
                    eprintln!("{}", too_long_to_edit(&path));
                    std::process::exit(1);
                }
            }
        }
        MemoryCommands::Confirm { id, at } => {
            let api = Api::connect(&at)?;
            let id = api.resolve_id(&id).await?;
            let current = api.get(&format!("/v1/memories/{id}")).await?;
            let revision = revision_of(&current)?;
            let response = api
                .send(
                    Method::POST,
                    &format!("/v1/memories/{id}"),
                    Some(&revision),
                    Some(&json!({ "action": "confirm" })),
                )
                .await?;
            if response.status().as_u16() != 200 {
                return Err(error_from_response(response).await);
            }
            let out: Value = response.json().await?;
            println!(
                "{} Note {} confirmed: {} (revision {}).",
                "✓".green(),
                id,
                out["status"].as_str().unwrap_or("active"),
                short(out["revision"].as_str().unwrap_or(""))
            );
            Ok(())
        }
        MemoryCommands::Undo { id, at } => {
            let api = Api::connect(&at)?;
            let id = api.resolve_id(&id).await?;
            let current = api.get(&format!("/v1/memories/{id}")).await?;
            let revision = revision_of(&current)?;
            let response = api
                .send::<Value>(
                    Method::POST,
                    &format!("/v1/memories/{id}/undo"),
                    Some(&revision),
                    None,
                )
                .await?;
            if response.status().as_u16() != 200 {
                return Err(error_from_response(response).await);
            }
            println!(
                "{} Note {} ({}) forgotten.",
                "✓".green(),
                id,
                current["title"].as_str().unwrap_or("")
            );
            Ok(())
        }
        MemoryCommands::Forget { ids, yes, at } => {
            let api = Api::connect(&at)?;
            let mut targets = Vec::new();
            for arg in &ids {
                let id = api.resolve_id(arg).await?;
                let current = api.get(&format!("/v1/memories/{id}")).await?;
                let revision = revision_of(&current)?;
                println!(
                    "  {}  {}  (revision {})",
                    id,
                    current["title"].as_str().unwrap_or(""),
                    short(&revision)
                );
                targets.push((id, revision));
            }
            if !yes && !confirm_forget(targets.len())? {
                println!("Cancelled. Nothing was forgotten.");
                return Ok(());
            }
            for (id, revision) in targets {
                let response = api
                    .send::<Value>(
                        Method::DELETE,
                        &format!("/v1/memories/{id}"),
                        Some(&revision),
                        None,
                    )
                    .await?;
                if response.status().as_u16() != 200 {
                    eprintln!(
                        "{} {}: {}",
                        "✗".red(),
                        id,
                        error_from_response(response).await
                    );
                    continue;
                }
                let out: Value = response.json().await?;
                println!("{} {} forgotten.", "✓".green(), id);
                if let Some(note) = out["retained"]["note"].as_str() {
                    println!("  {}", note.dimmed());
                }
            }
            Ok(())
        }
        MemoryCommands::Sync { at } => {
            let api = Api::connect(&at)?;
            let response = api
                .send::<Value>(Method::POST, "/v1/memories/sync", None, None)
                .await?;
            if response.status().as_u16() != 200 {
                return Err(error_from_response(response).await);
            }
            let report: Value = response.json().await?;
            println!(
                "Scanned {} note file(s); {} changed on disk; index {}.",
                report["scanned"],
                report["changed"],
                report["index_state"].as_str().unwrap_or("?")
            );
            for change in report["changes"].as_array().into_iter().flatten() {
                println!(
                    "  external  {}  {}",
                    change["note_id"].as_str().unwrap_or("?"),
                    change["revision"]
                        .as_str()
                        .map(short)
                        .unwrap_or_else(|| "(removed)".to_string())
                );
            }
            Ok(())
        }
        MemoryCommands::Doctor { repair, at } => {
            if repair {
                return repair_locally(&at);
            }
            let report = if crate::manager::is_daemon_running() {
                Api::connect(&at)?.get("/v1/memories/doctor").await?
            } else {
                println!("{}", "No daemon is running; scanning locally.".dimmed());
                let (_db, owner_id) = open_owner_database()?;
                let report = files::doctor(&owner_id, at.local_project_root()?.as_deref())?;
                wire::doctor_report_json(&report)
            };
            print_doctor(&report);
            Ok(())
        }
        MemoryCommands::Adopt { owner_key } => adopt_locally(&owner_key),
    }
}

// ── The wire ─────────────────────────────────────────────────────

/// The daemon client plus the project header every call carries.
pub(crate) struct Api {
    client: DaemonClient,
    headers: Vec<(&'static str, String)>,
}

impl Api {
    fn connect(at: &ScopeArgs) -> Result<Self> {
        Ok(Self {
            client: DaemonClient::connect()?,
            headers: at.headers()?,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_tests(client: DaemonClient) -> Self {
        Self {
            client,
            headers: Vec::new(),
        }
    }

    /// A checked GET: a non-2xx is the daemon's refusal as an error.
    async fn get(&self, path: &str) -> Result<Value> {
        let response = self
            .client
            .request_raw::<Value>(Method::GET, path, &self.headers, None)
            .await?;
        if !response.status().is_success() {
            return Err(error_from_response(response).await);
        }
        response
            .json()
            .await
            .with_context(|| format!("GET {path}: unreadable body"))
    }

    /// An unchecked request with the project header and, for a single-note
    /// mutation, the revision as `If-Match`.
    async fn send<B: serde::Serialize>(
        &self,
        method: Method,
        path: &str,
        if_match: Option<&str>,
        body: Option<&B>,
    ) -> Result<reqwest::Response> {
        let mut headers = self.headers.clone();
        if let Some(revision) = if_match {
            headers.push(("If-Match", format!("\"{revision}\"")));
        }
        self.client.request_raw(method, path, &headers, body).await
    }

    /// A full id, or a unique prefix of one of the owner's notes — every
    /// status, superseded and candidates included.
    async fn resolve_id(&self, arg: &str) -> Result<NoteId> {
        if let Ok(id) = NoteId::parse(arg) {
            return Ok(id);
        }
        let prefix = arg.trim().to_ascii_lowercase();
        if prefix.len() < 4 || !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            bail!("`{arg}` is neither a note id nor a prefix of one (at least 4 hex characters)");
        }
        let mut matches: Vec<(String, String)> = Vec::new();
        for status in ["active", "candidate", "archived"] {
            let mut cursor: Option<String> = None;
            loop {
                let mut path = format!(
                    "/v1/memories?limit=100&status={status}&include_history=true&include_candidates=true"
                );
                if let Some(c) = &cursor {
                    path.push_str(&format!("&cursor={}", urlencoding::encode(c)));
                }
                let page = self.get(&path).await?;
                for item in page["items"].as_array().into_iter().flatten() {
                    let id = item["id"].as_str().unwrap_or("");
                    if id.starts_with(&prefix) && !matches.iter().any(|(m, _)| m == id) {
                        matches.push((
                            id.to_string(),
                            item["title"].as_str().unwrap_or("").to_string(),
                        ));
                    }
                }
                cursor = page["next_cursor"].as_str().map(str::to_string);
                if cursor.is_none() {
                    break;
                }
            }
        }
        match matches.as_slice() {
            [] => bail!("no note of yours has an id starting with `{arg}`"),
            [(id, _)] => NoteId::parse(id).map_err(|e| anyhow::anyhow!(e)),
            many => {
                let listed: Vec<String> = many
                    .iter()
                    .map(|(id, title)| format!("  {id}  {title}"))
                    .collect();
                bail!(
                    "`{arg}` matches {} notes; give more of the id:\n{}",
                    many.len(),
                    listed.join("\n")
                )
            }
        }
    }
}

/// The revision a read answered — the one the next mutation sends back.
fn revision_of(note: &Value) -> Result<String> {
    note["revision"]
        .as_str()
        .map(str::to_string)
        .context("the daemon's answer carries no revision")
}

// ── edit ─────────────────────────────────────────────────────────

/// How an edit ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditOutcome {
    /// The editor closed on a note that says the same: nothing was sent.
    Unchanged,
    /// Saved; `revision` is the new one.
    Saved { revision: String },
    /// The daemon's `409`: `ours` is the revision that was edited, `theirs`
    /// the one the daemon holds now, and `kept_at` the file holding the
    /// edited text.
    Conflict {
        ours: String,
        theirs: String,
        kept_at: PathBuf,
    },
    /// The edited file could not be sent — it does not parse as a note, or
    /// it changes a field an edit cannot (`kind`, `status`, the sources…);
    /// nothing was sent and the text is kept.
    Refused { reason: String, kept_at: PathBuf },
    /// The read came back `truncated`: the note's body is longer than the
    /// 32 KiB a read serves, so the editor would hold only the first 32 KiB
    /// and a save would replace the whole file with them. Refused before
    /// the editor opens; nothing was written and nothing sent. `path` is
    /// the read's own `path` (the note's file, relative to its partition),
    /// or the id when the answer carries none.
    TooLong { path: String },
}

/// What `edit` prints, verbatim, when it refuses a note cut at 32 KiB.
pub(crate) fn too_long_to_edit(path: &str) -> String {
    format!(
        "This note is longer than 32 KiB, so `openalpaca memory edit` would save only its first 32 KiB. Edit the file itself: {path}"
    )
}

/// `GET` → the note as a file → `open_editor` → the changed fields as a
/// patch → `POST` with the fetched revision as `If-Match`.
///
/// The editor sees the note exactly as the service writes it (design §4's
/// canon) — title, aliases, tags, `fact_key`, the validity bounds and the
/// body are the patchable fields; a change to any other is refused before
/// anything is sent, with the text kept. A read marked `truncated` is
/// [`EditOutcome::TooLong`] before the editor opens: the editor would hold
/// a cut body, and saving it would replace the whole file.
pub(crate) async fn edit_note(
    api: &Api,
    id: NoteId,
    open_editor: &dyn Fn(&Path) -> Result<()>,
) -> Result<EditOutcome> {
    let read = api.get(&format!("/v1/memories/{id}")).await?;
    if read["truncated"].as_bool().unwrap_or(false) {
        return Ok(EditOutcome::TooLong {
            path: read["path"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| id.to_string()),
        });
    }
    let before = note_from_read(&read)?;
    let revision = revision_of(&read)?;
    let text = before.serialize();

    let file = tempfile::Builder::new()
        .prefix(&format!("openalpaca-memory-{}-", short_id(&id)))
        .suffix(".md")
        .tempfile()
        .context("could not create a temp file for the editor")?;
    std::fs::write(file.path(), &text)
        .with_context(|| format!("could not write {}", file.path().display()))?;

    open_editor(file.path())?;

    let edited = std::fs::read_to_string(file.path())
        .with_context(|| format!("could not read {} back", file.path().display()))?;
    if edited == text {
        return Ok(EditOutcome::Unchanged);
    }
    let keep = |file: tempfile::NamedTempFile| -> Result<PathBuf> {
        file.into_temp_path()
            .keep()
            .context("could not keep the edited text")
    };
    let after = match Note::parse(edited.as_bytes()) {
        Ok(note) => note,
        Err(e) => {
            return Ok(EditOutcome::Refused {
                reason: format!("the edited file is not a note: {}", e.reason),
                kept_at: keep(file)?,
            });
        }
    };
    let patch = match patch_between(&before, &after) {
        Ok(p) => p,
        Err(reason) => {
            return Ok(EditOutcome::Refused {
                reason,
                kept_at: keep(file)?,
            });
        }
    };
    if patch.as_object().is_some_and(|p| p.is_empty()) {
        return Ok(EditOutcome::Unchanged);
    }

    let response = api
        .send(
            Method::POST,
            &format!("/v1/memories/{id}"),
            Some(&revision),
            Some(&json!({ "action": "edit", "patch": patch })),
        )
        .await?;
    match response.status().as_u16() {
        200 => {
            let out: Value = response
                .json()
                .await
                .context("POST /v1/memories/{id}: unreadable body")?;
            Ok(EditOutcome::Saved {
                revision: revision_of(&out)?,
            })
        }
        409 => {
            let theirs = etag_of(&response).unwrap_or_else(|| "unknown".to_string());
            Ok(EditOutcome::Conflict {
                ours: revision,
                theirs,
                kept_at: keep(file)?,
            })
        }
        _ => {
            let kept_at = keep(file)?;
            Err(error_from_response(response)
                .await
                .context(format!("your text is kept at {}", kept_at.display())))
        }
    }
}

/// The `memory_update` patch that turns `before` into `after` — only the
/// patchable fields of design §7a. A change to any other field is refused
/// by name: an edit cannot change what a note *is* (its id, kind, status,
/// timestamps, pin, sources or links); `add --supersedes` and `forget` are
/// the verbs for those.
pub(crate) fn patch_between(before: &Note, after: &Note) -> Result<Value, String> {
    let fixed = |name: &str| format!("`{name}` cannot be changed by an edit");
    if after.id != before.id {
        return Err(fixed("id"));
    }
    if after.kind != before.kind {
        return Err(format!(
            "{}; save a replacement with `openalpaca memory add --kind {} --supersedes {}`",
            fixed("kind"),
            after.kind,
            short_id(&before.id)
        ));
    }
    if after.status != before.status {
        return Err(format!(
            "{}; `confirm` makes a candidate active and `forget` retires a note",
            fixed("status")
        ));
    }
    if after.created_at != before.created_at || after.updated_at != before.updated_at {
        return Err(format!(
            "{} (the service stamps them)",
            fixed("created_at`/`updated_at")
        ));
    }
    if after.observed_at != before.observed_at {
        return Err(fixed("observed_at"));
    }
    if after.pinned != before.pinned {
        return Err(fixed("pinned"));
    }
    if after.sources != before.sources {
        return Err(fixed("sources"));
    }
    if after.supersedes != before.supersedes {
        return Err(format!(
            "{}; a replacement is `openalpaca memory add --supersedes <id>`",
            fixed("supersedes")
        ));
    }
    if !after.unknown.is_empty() {
        let keys: Vec<&str> = after.unknown.iter().map(|u| u.key.as_str()).collect();
        return Err(format!(
            "unknown frontmatter key(s) {}: an edit takes only the fields the note already has",
            keys.join(", ")
        ));
    }
    let mut patch = serde_json::Map::new();
    if after.title != before.title {
        patch.insert("title".into(), Value::String(after.title.clone()));
    }
    if after.body != before.body {
        patch.insert("body".into(), Value::String(after.body.clone()));
    }
    if after.aliases != before.aliases {
        patch.insert("aliases".into(), json!(after.aliases));
    }
    if after.tags != before.tags {
        patch.insert("tags".into(), json!(after.tags));
    }
    if after.fact_key != before.fact_key {
        patch.insert("fact_key".into(), json!(after.fact_key));
    }
    let ts =
        |t: Option<DateTime<Utc>>| json!(t.map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string()));
    if after.valid_from != before.valid_from {
        patch.insert("valid_from".into(), ts(after.valid_from));
    }
    if after.valid_to != before.valid_to {
        patch.insert("valid_to".into(), ts(after.valid_to));
    }
    if after.review_after != before.review_after {
        patch.insert("review_after".into(), ts(after.review_after));
    }
    Ok(Value::Object(patch))
}

/// The note file behind a `memory_read` result: the canon of design §4,
/// rebuilt from `frontmatter` and `body`, so `show --raw` prints what the
/// daemon holds and `edit` opens it.
pub(crate) fn note_from_read(read: &Value) -> Result<Note> {
    let fm = &read["frontmatter"];
    let text = |key: &str| -> Result<String> {
        fm[key]
            .as_str()
            .map(str::to_string)
            .with_context(|| format!("the note's frontmatter has no `{key}`"))
    };
    let ts = |key: &str| -> Result<DateTime<Utc>> {
        let raw = text(key)?;
        DateTime::parse_from_rfc3339(&raw)
            .map(|t| t.with_timezone(&Utc))
            .with_context(|| format!("`{key}` is not RFC 3339: {raw}"))
    };
    let opt_ts = |key: &str| -> Result<Option<DateTime<Utc>>> {
        match fm[key].as_str() {
            None => Ok(None),
            Some(raw) => DateTime::parse_from_rfc3339(raw)
                .map(|t| Some(t.with_timezone(&Utc)))
                .with_context(|| format!("`{key}` is not RFC 3339: {raw}")),
        }
    };
    let strings = |key: &str| -> Vec<String> {
        fm[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let kind_word = text("kind")?;
    let status_word = text("status")?;
    let sources = fm["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            let kind_word = s["type"].as_str().unwrap_or("");
            Ok(NoteSource {
                kind: SourceType::parse(kind_word)
                    .with_context(|| format!("unknown source type `{kind_word}`"))?,
                id: s["id"].as_str().unwrap_or("").to_string(),
                observed_at: match s["observed_at"].as_str() {
                    None => None,
                    Some(raw) => Some(
                        DateTime::parse_from_rfc3339(raw)
                            .map(|t| t.with_timezone(&Utc))
                            .with_context(|| {
                                format!("a source's observed_at is not RFC 3339: {raw}")
                            })?,
                    ),
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let supersedes = strings("supersedes")
        .iter()
        .map(|id| NoteId::parse(id).map_err(|e| anyhow::anyhow!("supersedes: {e}")))
        .collect::<Result<Vec<_>>>()?;
    Ok(Note {
        id: NoteId::parse(read["id"].as_str().unwrap_or("")).map_err(|e| anyhow::anyhow!(e))?,
        title: text("title")?,
        kind: NoteKind::parse(&kind_word).with_context(|| format!("unknown kind `{kind_word}`"))?,
        status: NoteStatus::parse(&status_word)
            .with_context(|| format!("unknown status `{status_word}`"))?,
        fact_key: fm["fact_key"].as_str().map(str::to_string),
        aliases: strings("aliases"),
        tags: strings("tags"),
        created_at: ts("created_at")?,
        updated_at: ts("updated_at")?,
        observed_at: opt_ts("observed_at")?,
        valid_from: opt_ts("valid_from")?,
        valid_to: opt_ts("valid_to")?,
        review_after: opt_ts("review_after")?,
        pinned: fm["pinned"].as_bool().unwrap_or(false),
        sources,
        supersedes,
        unknown: Vec::new(),
        body: read["body"].as_str().unwrap_or("").to_string(),
    })
}

/// `add` with no body and no `-`: an empty file in `$EDITOR`, its contents
/// the body.
fn body_from_editor(title: &str) -> Result<String> {
    let file = tempfile::Builder::new()
        .prefix("openalpaca-memory-new-")
        .suffix(".md")
        .tempfile()
        .context("could not create a temp file for the editor")?;
    std::fs::write(file.path(), format!("# {title}\n\n"))
        .with_context(|| format!("could not write {}", file.path().display()))?;
    open_in_editor(file.path())?;
    std::fs::read_to_string(file.path())
        .with_context(|| format!("could not read {} back", file.path().display()))
}

/// `forget` without `--yes`: a terminal question. Off a terminal there is no
/// one to ask, so the answer is no and the message says which flag to pass.
fn confirm_forget(count: usize) -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!(
            "not a terminal, so nobody can answer the question; pass --yes to forget without one"
        );
    }
    Ok(
        dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt(format!("Forget {count} note(s)? This cannot be undone"))
            .default(false)
            .interact()?,
    )
}

// ── The local verbs ──────────────────────────────────────────────

/// Why `doctor --repair` and `adopt` will not run under a live daemon.
const DAEMON_RUNNING_REFUSAL: &str = "\
A daemon is running against this store. `openalpaca memory doctor --repair` and \
`openalpaca memory adopt` rewrite control snapshots the daemon holds open, so they \
take the store lock and refuse while it runs. Stop it first:

    openalpaca daemon stop

then run the command again. (`openalpaca memory doctor` without --repair asks the \
running daemon instead.)";

/// The daemon's singleton lock, held for the length of a local verb — the
/// same acquisition `config reset --factory` makes, so exactly one of the
/// two can hold the store. The lock decides; the liveness check only
/// chooses the words.
fn claim_store() -> Result<impl Sized> {
    discovery::acquire_single_instance_lock(false).map_err(|e| {
        if crate::manager::is_daemon_running() {
            e.context(DAEMON_RUNNING_REFUSAL)
        } else {
            let lock = store::lock_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "state/openalpacad.lock".to_string());
            e.context(format!(
                "The store's lock ({lock}) is held or cannot be taken, and no running daemon was \
                 found. A daemon may still be starting: wait a few seconds and try again. If \
                 none is starting, check that you can write that file. Nothing was changed."
            ))
        }
    })
}

/// The store database and the owner id it records (`identity.local_user_id`,
/// the row the daemon mints at its first start). No database means no
/// owner yet, and nothing to repair or adopt *for*.
fn open_owner_database() -> Result<(openalpaca_storage::Database, String)> {
    let path = store::database_path()?;
    if !path.exists() {
        bail!(
            "no database at {}: start the daemon once so it creates the store and mints an owner id, then try again",
            path.display()
        );
    }
    let db = store::open_home_database()?;
    let owner_id = ConfigRepository::new(&db)
        .get("identity.local_user_id")?
        .context("the database records no owner yet (identity.local_user_id): start the daemon once, then try again")?;
    Ok((db, owner_id))
}

/// `doctor --repair` (design §6, *Management operations*): under the store
/// lock, rebuild the missing or invalid snapshot of the owner's home
/// partition and of the named project's, then delete stale and abandoned
/// move snapshots. Never rewrites, moves or deletes a note file.
pub(crate) fn repair_locally(at: &ScopeArgs) -> Result<()> {
    let _lock = claim_store()?;
    let (_db, owner_id) = open_owner_database()?;
    let owner_key = files::owner_key(&owner_id);
    let mut scopes = vec![StoreScope::Home];
    if let Some(root) = at.local_project_root()? {
        scopes.push(StoreScope::Project(root));
    }
    for scope in &scopes {
        let label = match scope {
            StoreScope::Home => "global".to_string(),
            StoreScope::Project(root) => format!("project {}", root.display()),
        };
        let partition = Partition::open(scope, &owner_id)
            .with_context(|| format!("could not open the {label} partition"))?;
        let report = partition
            .repair(&owner_key)
            .with_context(|| format!("could not repair the {label} partition"))?;
        if report.rebuilt {
            println!(
                "{} {}: snapshot rebuilt, {} note(s) recorded as external.",
                "✓".green(),
                report.partition_key,
                report.notes_recorded
            );
        } else {
            println!(
                "{} {}: not quarantined, nothing rebuilt.",
                "=".dimmed(),
                report.partition_key
            );
        }
        for m in &report.malformed {
            println!("  malformed  {}  {}", m.path.display(), m.reason);
        }
    }
    let snapshots: Vec<ControlSnapshot> = files::list_snapshots()?
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .collect();
    for (key, state) in files::classify_move_snapshots(&snapshots) {
        match state {
            MoveSnapshotState::Stale | MoveSnapshotState::Abandoned => {
                let path = files::snapshot_path_for(&key)?;
                std::fs::remove_file(&path)
                    .with_context(|| format!("could not delete {}", path.display()))?;
                println!(
                    "{} {key}: {} move snapshot deleted.",
                    "✓".green(),
                    match state {
                        MoveSnapshotState::Stale => "stale",
                        _ => "abandoned",
                    }
                );
            }
            MoveSnapshotState::Orphaned => {
                println!(
                    "{} {key}: orphaned (its directory is gone); left in place.",
                    "!".yellow()
                );
            }
        }
    }
    Ok(())
}

/// `adopt <owner-key>` (design §3 *Resets*, §6): under the store lock,
/// re-bind every partition of an orphaned owner key — the home one and
/// every `proj-<key>-*` snapshot — to the current owner in one command.
/// A stale or abandoned move snapshot of that key is `doctor --repair`'s to
/// settle first (§7c Q6), so it refuses rather than adopt a directory that
/// does not exist.
pub(crate) fn adopt_locally(old_owner_key: &str) -> Result<()> {
    let old_owner_key = old_owner_key.trim().to_ascii_lowercase();
    if !files::is_owner_key(&old_owner_key) {
        bail!(
            "`{old_owner_key}` is not an owner key (32 lowercase hex characters, as `openalpaca memory doctor` lists them)"
        );
    }
    let _lock = claim_store()?;
    let (_db, owner_id) = open_owner_database()?;
    let snapshots: Vec<ControlSnapshot> = files::list_snapshots()?
        .into_iter()
        .filter_map(|(_, parsed)| parsed.ok())
        .collect();
    let prefix = format!("proj-{old_owner_key}-");
    let stuck: Vec<String> = files::classify_move_snapshots(&snapshots)
        .into_iter()
        .filter(|(key, state)| {
            key.starts_with(&prefix)
                && matches!(
                    state,
                    MoveSnapshotState::Stale | MoveSnapshotState::Abandoned
                )
        })
        .map(|(key, state)| format!("  {key}  ({state:?})"))
        .collect();
    if !stuck.is_empty() {
        bail!(
            "{} move snapshot(s) of {old_owner_key} are stale or abandoned; run `openalpaca memory doctor --repair` first:\n{}",
            stuck.len(),
            stuck.join("\n")
        );
    }
    let adopted = files::adopt(&old_owner_key, &owner_id, Utc::now())?;
    println!(
        "{} Adopted {} partition(s) of {old_owner_key} as the current owner's:",
        "✓".green(),
        adopted.len()
    );
    for key in adopted {
        println!("  {key}");
    }
    println!("The daemon rebuilds its search index for them at its next start.");
    Ok(())
}

// ── Printing ─────────────────────────────────────────────────────

/// The first 12 characters of a revision — enough to tell two apart.
fn short(revision: &str) -> String {
    revision.chars().take(12).collect()
}

/// The first 8 characters of an id — a unique prefix in any real store.
fn short_id(id: &NoteId) -> String {
    id.to_string().chars().take(8).collect()
}

fn print_list(page: &Value) {
    let items = page["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        println!("{}", "No notes.".dimmed());
        return;
    }
    println!(
        "{}",
        format!(
            "{:<8} {:<10} {:<7} {:<10} {:<15} {:<20} TITLE",
            "ID", "KIND", "SCOPE", "STATUS", "ORIGIN", "UPDATED"
        )
        .dimmed()
    );
    for item in &items {
        let status = item["effective_status"]
            .as_str()
            .or(item["status"].as_str())
            .unwrap_or("");
        println!(
            "{:<8} {:<10} {:<7} {:<10} {:<15} {:<20} {}{}",
            item["id"]
                .as_str()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_default(),
            item["kind"].as_str().unwrap_or(""),
            item["scope"].as_str().unwrap_or(""),
            status,
            item["origin"].as_str().unwrap_or(""),
            item["updated_at"].as_str().unwrap_or(""),
            item["title"].as_str().unwrap_or(""),
            if item["pinned"].as_bool().unwrap_or(false) {
                "  (pinned)"
            } else {
                ""
            }
        );
    }
    if let Some(hint) = list_more_hint(page) {
        println!("{}", hint.dimmed());
    }
}

/// The line under a list page that has more: names only what `list` can
/// take (§7a gives it no `--cursor`), so the hint is never a continuation
/// the verb refuses.
fn list_more_hint(page: &Value) -> Option<String> {
    page["next_cursor"]
        .as_str()
        .map(|_| "More: raise --limit (up to 100)".to_string())
}

fn print_search(found: &Value) {
    let results = found["results"].as_array().cloned().unwrap_or_default();
    if results.is_empty() {
        println!("{}", "Nothing matched.".dimmed());
    }
    for r in &results {
        let flags: Vec<&str> = [
            r["conflict"]
                .as_bool()
                .unwrap_or(false)
                .then_some("conflict"),
            r["same_key_loser"]
                .as_bool()
                .unwrap_or(false)
                .then_some("same-key loser"),
            (r["sources_status"] == "source_forgotten").then_some("source forgotten"),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!(
            "{}  {}  [{}/{}/{}/{}] {}{}",
            r["id"]
                .as_str()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_default()
                .bold(),
            r["title"].as_str().unwrap_or(""),
            r["kind"].as_str().unwrap_or(""),
            r["scope"].as_str().unwrap_or(""),
            r["effective_status"].as_str().unwrap_or(""),
            r["origin"].as_str().unwrap_or(""),
            r["freshness"].as_str().unwrap_or(""),
            if flags.is_empty() {
                String::new()
            } else {
                format!("  {}", flags.join(", ").yellow())
            }
        );
        let matched: Vec<&str> = r["match"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        println!(
            "{}",
            format!(
                "    {}lines {}-{}, matched on {}",
                r["heading"]
                    .as_str()
                    .map(|h| format!("{h} · "))
                    .unwrap_or_default(),
                r["line_range"][0],
                r["line_range"][1],
                if matched.is_empty() {
                    "-".to_string()
                } else {
                    matched.join(", ")
                }
            )
            .dimmed()
        );
        for line in r["excerpt"].as_str().unwrap_or("").lines() {
            println!("    {line}");
        }
    }
    if found["partial"].as_bool().unwrap_or(false) {
        println!("{}", "(partial: a scan cap was hit)".dimmed());
    }
    if found["index_state"] == "rebuilding" {
        println!("{}", "(the search index is rebuilding)".dimmed());
    }
}

fn print_note(note: &Value) {
    println!("{}", note["title"].as_str().unwrap_or("").bold());
    println!(
        "{}",
        format!(
            "id {}  revision {}  {}/{}/{}  origin {}  freshness {}",
            note["id"].as_str().unwrap_or(""),
            short(note["revision"].as_str().unwrap_or("")),
            note["kind"].as_str().unwrap_or(""),
            note["scope"].as_str().unwrap_or(""),
            note["effective_status"].as_str().unwrap_or(""),
            note["origin"].as_str().unwrap_or(""),
            note["freshness"].as_str().unwrap_or(""),
        )
        .dimmed()
    );
    let fm = &note["frontmatter"];
    let list = |key: &str| -> Vec<&str> {
        fm[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    let aliases = list("aliases");
    let tags = list("tags");
    let mut facts = Vec::new();
    if let Some(k) = fm["fact_key"].as_str() {
        facts.push(format!("fact_key {k}"));
    }
    if !aliases.is_empty() {
        facts.push(format!("aliases {}", aliases.join(", ")));
    }
    if !tags.is_empty() {
        facts.push(format!("tags {}", tags.join(", ")));
    }
    if fm["pinned"].as_bool().unwrap_or(false) {
        facts.push("pinned".into());
    }
    if !facts.is_empty() {
        println!("{}", facts.join("  ").dimmed());
    }
    let sources: Vec<String> = note["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| {
            format!(
                "{}:{}",
                s["type"].as_str().unwrap_or(""),
                s["id"].as_str().unwrap_or("")
            )
        })
        .collect();
    if !sources.is_empty() {
        println!(
            "{}",
            format!(
                "sources {} ({})",
                sources.join(", "),
                note["sources_status"].as_str().unwrap_or("")
            )
            .dimmed()
        );
    }
    let supersedes = list("supersedes");
    if !supersedes.is_empty() {
        println!(
            "{}",
            format!("supersedes {}", supersedes.join(", ")).dimmed()
        );
    }
    let superseded_by: Vec<&str> = note["superseded_by"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !superseded_by.is_empty() {
        println!(
            "{}",
            format!("superseded by {}", superseded_by.join(", ")).yellow()
        );
    }
    if note["conflict"].as_bool().unwrap_or(false) {
        println!("{}", "conflict: see `openalpaca memory doctor`".yellow());
    }
    println!();
    print!("{}", note["body"].as_str().unwrap_or(""));
    if !note["body"].as_str().unwrap_or("").ends_with('\n') {
        println!();
    }
    if note["truncated"].as_bool().unwrap_or(false) {
        println!("{}", "(truncated at 32 KiB)".dimmed());
    }
}

fn print_doctor(report: &Value) {
    println!(
        "{}",
        format!("owner key {}", report["owner_key"].as_str().unwrap_or("?")).dimmed()
    );
    let partitions = report["partitions"].as_array().cloned().unwrap_or_default();
    if partitions.is_empty() {
        println!("No memory partitions.");
    }
    for p in &partitions {
        let health = p["health"].as_str().unwrap_or("?");
        let mark = match health {
            "ready" => "✓".green(),
            "absent" => "·".dimmed(),
            _ => "✗".red(),
        };
        println!(
            "{} {} {}{}  {}",
            mark,
            p["scope"].as_str().unwrap_or(""),
            p["partition_key"].as_str().unwrap_or(""),
            if p["orphan"].as_bool().unwrap_or(false) {
                "  (orphan: another owner's — `openalpaca memory adopt <owner-key>`)"
                    .yellow()
                    .to_string()
            } else {
                String::new()
            },
            health
        );
        println!("    {}", p["root"].as_str().unwrap_or("").dimmed());
        if let Some(reason) = p["quarantine_reason"].as_str() {
            println!("    quarantined: {reason}");
            if !p["orphan"].as_bool().unwrap_or(false) {
                println!("    run `openalpaca memory doctor --repair` with the daemon stopped");
            }
        }
        println!(
            "    {} note(s), {} unverified, {} tombstone(s){}",
            p["notes"],
            p["unverified"],
            p["tombstones"],
            p["pending"]
                .as_str()
                .map(|id| format!(", a pending write on {id}"))
                .unwrap_or_default()
        );
        for c in p["conflicts"].as_array().into_iter().flatten() {
            println!(
                "    conflict {}: {} vs {}",
                c["kind"].as_str().unwrap_or(""),
                c["note"].as_str().unwrap_or(""),
                c["target"].as_str().unwrap_or("")
            );
        }
        for m in p["malformed"].as_array().into_iter().flatten() {
            println!(
                "    malformed {}: {}",
                m["path"].as_str().unwrap_or(""),
                m["reason"].as_str().unwrap_or("")
            );
        }
        for d in p["duplicates"].as_array().into_iter().flatten() {
            let files: Vec<&str> = d["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            println!(
                "    duplicate id {}: {}",
                d["id"].as_str().unwrap_or(""),
                files.join(", ")
            );
        }
    }
    for m in report["move_snapshots"].as_array().into_iter().flatten() {
        println!(
            "{} move snapshot {} is {} (bound to {}{})",
            "!".yellow(),
            m["partition_key"].as_str().unwrap_or(""),
            m["state"].as_str().unwrap_or(""),
            m["root"].as_str().unwrap_or(""),
            m["moved_from"]
                .as_str()
                .map(|f| format!(", moved from {f}"))
                .unwrap_or_default()
        );
    }
    for r in report["refused"].as_array().into_iter().flatten() {
        println!(
            "{} {} partition of {} refused: {} is {}",
            "✗".red(),
            r["scope"].as_str().unwrap_or(""),
            r["owner_key"].as_str().unwrap_or(""),
            r["component"].as_str().unwrap_or(""),
            r["reason"].as_str().unwrap_or("")
        );
    }
    for u in report["unreadable_snapshots"]
        .as_array()
        .into_iter()
        .flatten()
    {
        println!(
            "{} unreadable snapshot {}: {}",
            "✗".red(),
            u["path"].as_str().unwrap_or(""),
            u["reason"].as_str().unwrap_or("")
        );
    }
}

/// The unquoted `ETag`, if the response carries one.
fn etag_of(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let v = v.trim();
            let v = v.strip_prefix("W/").unwrap_or(v);
            v.trim_matches('"').to_string()
        })
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests;
