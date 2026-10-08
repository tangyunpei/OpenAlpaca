//! `openalpaca memory` — the memory notes from the command line (memory
//! redesign, design §7 *Surfaces*, §7a *CLI syntax*; plan 3.8).
//!
//! ```text
//! openalpaca memory list    [--kind K] [--status S] [--scope global|project] [--history] [--candidates] [--limit N]
//! openalpaca memory search  <query> [--kind K] [--scope S] [--limit N] [--historical] [--as-of RFC3339]
//!                           [--since D] [--until D] [--order relevance|newest|oldest]
//! openalpaca memory timeline <id> | --query Q | --date D  [--before N] [--after N]
//! openalpaca memory show    <id> [--revision <rev>] [--raw]
//! openalpaca memory add     --kind K --title T [--scope S] [--alias A]... [--tag T]... [--fact-key KEY]
//!                           [--supersedes <id>] [BODY | -]       body: the argument, stdin ("-"), or $EDITOR
//! openalpaca memory edit    <id>                                  GET → $EDITOR → POST with the fetched revision
//! openalpaca memory suggestions [--limit N]
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
//! **Layered retrieval** (design 09 §13.1, MD-14): `search` takes the
//! tool's date filters (`--since`/`--until`, RFC 3339 or `YYYY-MM-DD`, both
//! inclusive, on a note's date — when it was observed, else when it was
//! saved) and order (`--order`), and still prints the cited excerpts;
//! `timeline` is `memory_timeline` — the notes around one note, the first
//! hit of `--query`, or `--date`, in time order, superseded, archived and
//! ended ones marked.
//!
//! **Suggestions** (Learning v2, design 09 §3.5): `suggestions` lists the
//! learned changes to notes you saved that wait for you; `confirm` of one
//! also fetches the note it replaces, at that moment, and sends that
//! revision as `target_revision` — so the confirm replaces the note as it
//! stands then, an edit you made after the suggestion was written included;
//! only a change landing between that read and the confirm is the daemon's
//! `409`, and a note already replaced or archived some other way is its
//! `400`; `undo` of one dismisses it (*Keep mine*) — a withdrawal that
//! forgets nothing else you said.
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
use openalpaca_core::utils::text::{
    escape_hidden, escape_hidden_line, hidden_notice, is_hidden, one_line,
};
use openalpaca_storage::memory_files::{
    self as files, Note, NoteId, NoteKind, NoteSource, NoteStatus, Partition, SourceType,
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
use crate::output::{OutputFormat, json_with_hidden_escaped, terminal_reset};

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
        /// Maximum number of results (1-50)
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Include superseded, archived and ended-window notes
        #[arg(long)]
        historical: bool,
        /// Evaluate validity at this instant (RFC 3339)
        #[arg(long)]
        as_of: Option<String>,
        /// Only notes dated at or after this (RFC 3339, or YYYY-MM-DD from its start)
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
        /// Only notes dated at or before this (RFC 3339, or YYYY-MM-DD to its end)
        #[arg(long, value_name = "DATE")]
        until: Option<String>,
        /// Sort by relevance (default), or by date: newest, oldest
        #[arg(long, value_name = "ORDER")]
        order: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Show the notes around one note, a search's best match, or a date, in time order
    #[command(group(
        clap::ArgGroup::new("around")
            .required(true)
            .args(["id", "query", "date"])
    ))]
    Timeline {
        /// The note to draw it around (a unique prefix will do)
        id: Option<String>,
        /// Draw it around the best match of this search
        #[arg(long)]
        query: Option<String>,
        /// Draw it around this instant (RFC 3339, or YYYY-MM-DD from its start)
        #[arg(long)]
        date: Option<String>,
        /// How many notes before it (0-20)
        #[arg(long, default_value = "5")]
        before: usize,
        /// How many notes after it (0-20)
        #[arg(long, default_value = "5")]
        after: usize,
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
    /// List pending suggestions: learned changes to notes you saved, waiting for you
    Suggestions {
        /// Maximum number of suggestions (1-100)
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Confirm a candidate: it becomes active, and yours (a suggestion replaces the note it names)
    Confirm {
        /// The note id (a unique prefix will do)
        id: String,
        #[command(flatten)]
        at: ScopeArgs,
    },
    /// Undo a recently learned note, or dismiss a suggestion and keep your note
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
        /// Rebuild missing or invalid snapshots
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
            since,
            until,
            order,
            format,
            at,
        } => {
            let api = Api::connect(&at)?;
            let path = search_path(&SearchFilters {
                query: &query,
                kind: kind.as_deref(),
                scope: scope.as_deref(),
                limit,
                historical,
                as_of: as_of.as_deref(),
                since: since.as_deref(),
                until: until.as_deref(),
                order: order.as_deref(),
            });
            let found = api.get(&path).await?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&found)?),
                OutputFormat::Table => print_search(&found),
            }
            Ok(())
        }
        MemoryCommands::Timeline {
            id,
            query,
            date,
            before,
            after,
            format,
            at,
        } => {
            let api = Api::connect(&at)?;
            let found = timeline(
                &api,
                id.as_deref(),
                query.as_deref(),
                date.as_deref(),
                before,
                after,
            )
            .await?;
            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&found)?),
                OutputFormat::Table => print_timeline(&found),
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
                print!("{}", note_text(&note, std::io::stdout().is_terminal()));
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
        MemoryCommands::Suggestions { limit, format, at } => {
            let api = Api::connect(&at)?;
            let page = api
                .get(&format!("/v1/memories/suggestions?limit={limit}"))
                .await?;
            print!(
                "{}",
                suggestions_text(&page, format, std::io::stdout().is_terminal())
            );
            Ok(())
        }
        MemoryCommands::Confirm { id, at } => {
            let api = Api::connect(&at)?;
            let id = api.resolve_id(&id).await?;
            let confirmed = confirm_note(&api, id).await?;
            for line in confirmed.lines(id) {
                println!("{line}");
            }
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
            let out: Value = response.json().await?;
            println!("{} {}", "✓".green(), undo_message(id, &current, &out));
            Ok(())
        }
        MemoryCommands::Forget { ids, yes, at } => {
            let api = Api::connect(&at)?;
            forget_verb(&api, &ids, yes, &mut |line| println!("{line}")).await
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
                        matches.push((id.to_string(), shown(&item["title"])));
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

// ── search and timeline (design 09 §13.1) ───────────────────────

/// What `memory search` asks for.
pub(crate) struct SearchFilters<'a> {
    pub query: &'a str,
    pub kind: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub limit: usize,
    pub historical: bool,
    pub as_of: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub order: Option<&'a str>,
}

/// `GET /v1/memories/search` for these filters: the tool's parameters by
/// their route names — `--since` is `date_from`, `--until` `date_to`,
/// `--order` `order_by` — each sent only when given, so the daemon's
/// defaults stay the defaults. The detail is the route's own (snippets),
/// which is what this verb prints.
pub(crate) fn search_path(f: &SearchFilters<'_>) -> String {
    let mut q = vec![
        format!("q={}", urlencoding::encode(f.query)),
        format!("limit={}", f.limit),
    ];
    let optional = [
        ("kind", f.kind),
        ("scope", f.scope),
        ("as_of", f.as_of),
        ("date_from", f.since),
        ("date_to", f.until),
        ("order_by", f.order),
    ];
    for (name, value) in optional {
        if let Some(v) = value {
            q.push(format!("{name}={}", urlencoding::encode(v)));
        }
    }
    if f.historical {
        q.push("historical=true".into());
    }
    format!("/v1/memories/search?{}", q.join("&"))
}

/// `memory timeline`: around a note (its id, or a unique prefix of one),
/// the first hit of a search, or a date — `GET /v1/memories/timeline`.
pub(crate) async fn timeline(
    api: &Api,
    id: Option<&str>,
    query: Option<&str>,
    date: Option<&str>,
    before: usize,
    after: usize,
) -> Result<Value> {
    let mut q = Vec::new();
    match (id, query, date) {
        (Some(id), None, None) => {
            let id = api.resolve_id(id).await?;
            q.push(format!("anchor={id}"));
        }
        (None, Some(text), None) => q.push(format!("q={}", urlencoding::encode(text))),
        (None, None, Some(at)) => q.push(format!("date={}", urlencoding::encode(at))),
        _ => bail!("draw a timeline around exactly one of a note id, --query or --date"),
    }
    q.push(format!("before={before}"));
    q.push(format!("after={after}"));
    api.get(&format!("/v1/memories/timeline?{}", q.join("&")))
        .await
}

// ── suggestions, confirm, undo ───────────────────────────────────

/// Whose note a suggestion would replace, in the owner's words.
fn target_origin_label(origin: &str) -> &str {
    match origin {
        "user" => "saved by you",
        "extracted_user" => "learned",
        "external" => "edited by hand",
        "imported" => "imported",
        "extracted_other" => "inferred",
        other => other,
    }
}

/// One `openalpaca memory suggestions` line (design 09 §3.5):
/// `<id8>  replace “X” (saved by you) → “B”  from your words, 2026-09-24 — <reason>`
/// — `suggested by the assistant, <date>` for one the assistant saved in
/// chat (owner decision LQ11).
///
/// The line is what a person reads before `memory confirm <id>` makes the
/// suggestion theirs, and the titles and the reason came from the model or
/// a file: every hidden character in them is shown as `<U+XXXX>`
/// (`escape_hidden_line` — each is one line of this one), never dropped,
/// which would name a title the note does not have. [`suggestions_text`]
/// counts them under the line.
pub(crate) fn suggestion_line(item: &Value) -> String {
    let id: String = item["id"].as_str().unwrap_or("").chars().take(8).collect();
    let target = &item["target"];
    let date: String = item["observed_at"]
        .as_str()
        .or(item["created_at"].as_str())
        .unwrap_or("")
        .chars()
        .take(10)
        .collect();
    // A suggestion whose own text changed after learning wrote it says who
    // wrote the words now, and when it was suggested. One the assistant's
    // memory tools saved in chat (owner decision LQ11) has no `observed_at`,
    // which learning stamps on every note it writes, and says so.
    let whence = match item["origin"].as_str() {
        Some("extracted_user") if item["observed_at"].is_null() => {
            format!("suggested by the assistant, {date}")
        }
        Some("extracted_user") => format!("from your words, {date}"),
        Some("extracted_other") => format!("inferred from the conversation, {date}"),
        Some("user") => format!("edited by you, suggested {date}"),
        Some("external") => format!("edited outside OpenAlpaca, suggested {date}"),
        Some("imported") => format!("imported, suggested {date}"),
        Some(other) => format!("{other}, {date}"),
        None => date,
    };
    let mut line = format!(
        "{id}  replace “{}” ({}) → “{}”  {whence}",
        escape_line(&target["title"]).text,
        target_origin_label(target["origin"].as_str().unwrap_or("")),
        escape_line(&item["title"]).text,
    );
    if let Some(reason) = item["reason"].as_str() {
        line.push_str(&format!(" — {}", escape_hidden_line(reason).text));
    }
    match item["older_count"].as_u64() {
        Some(0) | None => {}
        Some(n) => line.push_str(&format!("  (+{n} older)")),
    }
    line
}

/// A string field of the daemon's answer through the one-line escape.
fn escape_line(value: &Value) -> openalpaca_core::utils::text::Escaped {
    escape_hidden_line(value.as_str().unwrap_or(""))
}

/// How many hidden characters one suggestion holds: the two titles, the
/// reason and the changed values its lines show, and both notes' texts,
/// which they do not — core's one count, which the Memory view's
/// Suggestions card and the chat preview read too.
fn suggestion_hidden(item: &Value) -> usize {
    let text = |value: &Value| value.as_str().unwrap_or("").to_string();
    openalpaca_core::memory::service::suggestion_hidden(
        &text(&item["target"]["title"]),
        &text(&item["title"]),
        item["reason"].as_str(),
        &text(&item["target"]["excerpt"]),
        &text(&item["knowledge"]),
        &change_values(item),
    )
}

/// Every value a suggestion's `changes` show — each change's `from`, then
/// its `to` — as core counts them.
fn change_values(item: &Value) -> Vec<&str> {
    item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| {
            ["from", "to"].into_iter().flat_map(move |side| {
                c[side]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
            })
        })
        .collect()
}

/// The line under a suggestion's that says what else it changes of the
/// note (the L8 close-out's A4) — `changes: tags none → tools; valid until
/// none → 2026-12-31T00:00:00Z` — each value through the one-line escape,
/// as the titles are. `None` when it changes only the title or the text.
pub(crate) fn suggestion_changes_line(item: &Value) -> Option<String> {
    let changes: Vec<String> = item["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let values = |side: &str| {
                let listed: Vec<String> = c[side]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| escape_line(v).text)
                    .collect();
                if listed.is_empty() {
                    "none".to_string()
                } else {
                    listed.join(", ")
                }
            };
            let label = match c["field"].as_str().unwrap_or("") {
                "fact_key" => "key",
                "valid_from" => "valid from",
                "valid_to" => "valid until",
                other => other,
            };
            format!("{} {} → {}", escape_hidden_line(label).text, values("from"), values("to"))
        })
        .collect();
    (!changes.is_empty()).then(|| format!("  changes: {}", changes.join("; ")))
}

/// Under the lines, once, when any suggestion was counted: its notice counts
/// both notes' texts, which the list does not print, so this says where
/// they are shown.
const SUGGESTION_COUNTS_UNPRINTED: &str = "Each count includes both notes' texts, which this \
     list does not print: `openalpaca memory show <id>` shows a suggestion's own with every \
     hidden character as <U+XXXX>, and --format json prints both, each as a JSON \\u escape.";

/// What `openalpaca memory suggestions` prints.
///
/// `--format json`: the daemon's page with every hidden character written
/// as JSON's own `\u` escape ([`json_with_hidden_escaped`]), which reads
/// back as exactly the daemon's values; no notice and no reset, as a
/// machine format. The list: on a terminal first
/// [`crate::output::TERMINAL_RESET`], as before an approval prompt — this
/// is what a person reads before `memory confirm <id>` — then each
/// [`suggestion_line`], the line saying what else it changes when it
/// changes the note's metadata ([`suggestion_changes_line`]) and, directly
/// under one that holds any hidden character, the notice counting them
/// ([`suggestion_hidden`]).
pub(crate) fn suggestions_text(page: &Value, format: OutputFormat, tty: bool) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    // Writing into a `String` cannot fail.
    if matches!(format, OutputFormat::Json) {
        let _ = writeln!(
            out,
            "{}",
            json_with_hidden_escaped(&serde_json::to_string_pretty(page).unwrap_or_default())
        );
        return out;
    }
    out.push_str(terminal_reset(tty));
    let items = page["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        let _ = writeln!(out, "{}", "No suggestions waiting.".dimmed());
        return out;
    }
    let mut counted = false;
    for item in &items {
        let _ = writeln!(out, "{}", suggestion_line(item));
        if let Some(changes) = suggestion_changes_line(item) {
            let _ = writeln!(out, "{changes}");
        }
        if let Some(notice) = hidden_notice(suggestion_hidden(item)) {
            let _ = writeln!(out, "  {}", notice.yellow());
            counted = true;
        }
    }
    if counted {
        let _ = writeln!(out, "{}", SUGGESTION_COUNTS_UNPRINTED.dimmed());
    }
    let _ = writeln!(
        out,
        "{}",
        "Replace: openalpaca memory confirm <id>   Keep yours: openalpaca memory undo <id>"
            .dimmed()
    );
    out
}

/// What `confirm` did: the daemon's answer and, for a suggestion, the
/// title of the note it replaced.
#[derive(Debug)]
pub(crate) struct Confirmed {
    pub out: Value,
    pub replaced_title: Option<String>,
}

impl Confirmed {
    /// What `openalpaca memory confirm` prints.
    pub(crate) fn lines(&self, id: NoteId) -> Vec<String> {
        let revision = short(self.out["revision"].as_str().unwrap_or(""));
        let mut lines = vec![match &self.replaced_title {
            Some(title) => format!(
                "{} Note {id} confirmed (revision {revision}): it now replaces “{}”, which is kept as history.",
                "✓".green(),
                one_line(title)
            ),
            None => format!(
                "{} Note {id} confirmed: {} (revision {revision}).",
                "✓".green(),
                self.out["status"].as_str().unwrap_or("active"),
            ),
        }];
        let dismissed: Vec<String> = self.out["dismissed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|d| d.chars().take(8).collect())
            .collect();
        if !dismissed.is_empty() {
            lines.push(format!(
                "  Dismissed {} other suggestion(s) for the same note: {}",
                dismissed.len(),
                dismissed.join(", ")
            ));
        }
        lines
    }
}

/// `confirm` (design 09 §3.5, S5): the candidate's revision as `If-Match`
/// and, when it names a note it would replace, that note's current revision
/// as `target_revision` — fetched now, right before the confirm, so what is
/// retired is the note as it stands at this moment (an edit made since the
/// suggestion or the listing included; the title is printed after). Only a
/// change landing between this read and the POST is the daemon's `409`; a
/// target already replaced or archived another way is its `400`, and a gone
/// one its `404`. The app's *Replace* sends the revision it displayed.
pub(crate) async fn confirm_note(api: &Api, id: NoteId) -> Result<Confirmed> {
    let current = api.get(&format!("/v1/memories/{id}")).await?;
    let revision = revision_of(&current)?;
    let mut body = json!({ "action": "confirm" });
    let mut replaced_title = None;
    if let Some(target) = current["supersedes"]
        .as_array()
        .and_then(|t| t.first())
        .and_then(Value::as_str)
    {
        let target = api.get(&format!("/v1/memories/{target}")).await?;
        body["target_revision"] = Value::String(revision_of(&target)?);
        replaced_title = target["title"].as_str().map(str::to_string);
    }
    let response = api
        .send(
            Method::POST,
            &format!("/v1/memories/{id}"),
            Some(&revision),
            Some(&body),
        )
        .await?;
    if response.status().as_u16() != 200 {
        return Err(error_from_response(response).await);
    }
    let out: Value = response
        .json()
        .await
        .context("POST /v1/memories/{id}: unreadable body")?;
    Ok(Confirmed {
        out,
        replaced_title,
    })
}

/// One note `memory forget` is about to forget, before its prompt.
fn forget_line(id: NoteId, current: &Value, revision: &str) -> String {
    format!(
        "  {}  {}  (revision {})",
        id,
        shown(&current["title"]),
        short(revision)
    )
}

/// What `undo` says (design 09 §3.3): a suggestion dismissed with the
/// owner's note kept, or a learned note withdrawn and what came back.
pub(crate) fn undo_message(id: NoteId, before: &Value, out: &Value) -> String {
    let title = shown(&before["title"]);
    let names_a_note = before["supersedes"]
        .as_array()
        .is_some_and(|t| !t.is_empty());
    if before["status"] == "candidate" {
        if names_a_note {
            format!("Suggestion {id} ({title}) dismissed; your note is kept as it is.")
        } else {
            format!("Candidate {id} ({title}) dismissed.")
        }
    } else {
        let restored = out["restored"].as_array().map(Vec::len).unwrap_or(0);
        if restored > 0 {
            format!("Note {id} ({title}) undone; the note it replaced is back.")
        } else {
            format!("Note {id} ({title}) undone.")
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

/// `memory forget` once connected: each id resolved and read — its line
/// printed, its revision and whether it is a pending suggestion kept — then
/// the question (skipped by `--yes`), then [`forget_notes`] over the ids in
/// [`forget_order`]. Every line goes to `emit`.
pub(crate) async fn forget_verb(
    api: &Api,
    ids: &[String],
    yes: bool,
    emit: &mut dyn FnMut(String),
) -> Result<()> {
    let mut read = Vec::new();
    for arg in ids {
        let id = api.resolve_id(arg).await?;
        let current = api.get(&format!("/v1/memories/{id}")).await?;
        let revision = revision_of(&current)?;
        emit(forget_line(id, &current, &revision));
        read.push((id, revision, is_pending_suggestion(&current)));
    }
    if !yes && !confirm_forget(read.len())? {
        emit("Cancelled. Nothing was forgotten.".to_string());
        return Ok(());
    }
    forget_notes(api, &forget_order(read), emit).await
}

/// `memory forget`'s requests, and the lines it prints: every id in one
/// `POST /v1/memories/forget` — in requests of up to 20 ids, the route's
/// most — with the revision each was read at, not one `DELETE` per id. A
/// forgotten note's pending suggestions are withdrawn once its request's
/// targets have all had their turn, so a suggestion named with the note it
/// would replace is forgotten as asked, in either order (the L8 close-out's
/// A3); the caller sends the pending suggestions first ([`forget_order`]),
/// so this holds across requests too. Each target's outcome is one line,
/// and what a forget keeps is said once per request — each request's lines
/// handed to `emit` as it returns. A refused request ends the command with
/// its refusal, but not before saying what landed (the close-out's repair
/// round 1): the service forgets its targets in turn — the pending
/// suggestions first, each of them even after one fails (its repair round
/// 3) — so the ids before the one that failed, and every pending suggestion,
/// may be gone; each id of that request is read back and reported forgotten
/// or not, and the ids after that request were never sent.
pub(crate) async fn forget_notes(
    api: &Api,
    targets: &[(NoteId, String)],
    emit: &mut dyn FnMut(String),
) -> Result<()> {
    let mut sent = 0;
    for batch in targets.chunks(20) {
        let body = json!({
            "targets": batch
                .iter()
                .map(|(id, revision)| json!({ "id": id.to_string(), "expected_revision": revision }))
                .collect::<Vec<_>>(),
        });
        let response = api
            .send(Method::POST, "/v1/memories/forget", None, Some(&body))
            .await?;
        sent += batch.len();
        if response.status().as_u16() != 200 {
            let refusal = error_from_response(response).await;
            for (id, _) in batch {
                emit(read_back(api, *id).await);
            }
            let unsent = targets.len() - sent;
            if unsent > 0 {
                emit(format!(
                    "{} {unsent} more id(s) not sent; none of them was forgotten",
                    "✗".red()
                ));
            }
            return Err(refusal);
        }
        let out: Value = response.json().await?;
        let mut forgot = false;
        for result in out["results"].as_array().into_iter().flatten() {
            let id = result["id"].as_str().unwrap_or("?");
            emit(match result["status"].as_str() {
                Some("forgotten") => {
                    forgot = true;
                    format!("{} {id} forgotten.", "✓".green())
                }
                Some("revision_conflict") => format!(
                    "{} {id}: changed since it was read (current revision {}); nothing was forgotten",
                    "✗".red(),
                    short(result["current_revision"].as_str().unwrap_or("?"))
                ),
                Some("not_found") => format!("{} {id}: not found", "✗".red()),
                other => format!("{} {id}: {}", "✗".red(), other.unwrap_or("no answer")),
            });
        }
        if forgot && let Some(note) = out["retained"]["note"].as_str() {
            emit(format!("  {}", note.dimmed()));
        }
    }
    Ok(())
}

/// After a refused forget request, whether one of its ids landed: every id
/// was read just before the request, so one that is gone now was forgotten
/// by it (or, a suggestion, withdrawn with the note it named).
async fn read_back(api: &Api, id: NoteId) -> String {
    let answer = api
        .send::<Value>(Method::GET, &format!("/v1/memories/{id}"), None, None)
        .await
        .map(|r| r.status().as_u16());
    match answer {
        Ok(404) => format!("{} {id} forgotten.", "✓".green()),
        Ok(200) => format!("{} {id}: not forgotten", "✗".red()),
        _ => format!("{} {id}: could not be read back to tell", "?".yellow()),
    }
}

/// The order `memory forget` sends its targets in: the pending suggestions
/// first, everything else as given. A request's withdrawals run before the
/// next request is sent, so a note in one request and its suggestion in a
/// later one would forget the note and withdraw the suggestion — answered
/// `not found`, its row never suppressed — where one request forgets both
/// (the close-out's repair round 1); the service orders one request's
/// targets the same way (its repair round 2). A suggestion never names
/// another suggestion, so this one order keeps every request apart.
pub(crate) fn forget_order(read: Vec<(NoteId, String, bool)>) -> Vec<(NoteId, String)> {
    let (first, rest): (Vec<_>, Vec<_>) = read.into_iter().partition(|(_, _, s)| *s);
    first
        .into_iter()
        .chain(rest)
        .map(|(id, revision, _)| (id, revision))
        .collect()
}

/// Whether a note as read is a pending suggestion: a candidate naming the
/// note it would replace.
fn is_pending_suggestion(current: &Value) -> bool {
    current["status"] == "candidate"
        && current["supersedes"]
            .as_array()
            .is_some_and(|t| !t.is_empty())
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
/// partition and of the named project's. Never rewrites, moves or deletes a
/// note file.
pub(crate) fn repair_locally(at: &ScopeArgs) -> Result<()> {
    let _lock = claim_store()?;
    let (_db, owner_id) = open_owner_database()?;
    let owner_key = files::owner_key(&owner_id);
    let mut scopes = vec![StoreScope::Home];
    if let Some(root) = at.local_project_root()? {
        if store::projects::lookup(&root)?.is_some() {
            scopes.push(StoreScope::Project(root));
        } else {
            // R12: a folder no turn has registered has no notes to repair.
            println!(
                "{} {}: not a registered project, so it has no notes to repair; answered from the home notes.",
                "=".dimmed(),
                root.display()
            );
        }
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
    Ok(())
}

/// `adopt <owner-key>` (design §3 *Resets*, §6): under the store lock,
/// re-bind every partition of an orphaned owner key — the home one and
/// every project partition, found by the project id its snapshot carries
/// (storage design R20) — to the current owner in one command. A project
/// snapshot with no id (an earlier build's, R22) is left as it is.
pub(crate) fn adopt_locally(old_owner_key: &str) -> Result<()> {
    let old_owner_key = old_owner_key.trim().to_ascii_lowercase();
    if !files::is_owner_key(&old_owner_key) {
        bail!(
            "`{old_owner_key}` is not an owner key (32 lowercase hex characters, as `openalpaca memory doctor` lists them)"
        );
    }
    let _lock = claim_store()?;
    let (_db, owner_id) = open_owner_database()?;
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
//
// A note's words come from the model, from learning or from a hand-edited
// file, and they are printed raw at a terminal: an ESC sequence in a title
// can clear or rewrite the screen (or, through OSC, the window title and the
// clipboard), and a bidi override reorders a line the person decides from.
// So in a listing every title, heading, alias, tag and reason passes through
// `shown` / `one_line` (the daemon's preview rule: the escape's characters
// dropped), an excerpt line through `excerpt_line`, and `memory show`'s body
// through `body_text`. Where a person decides — a suggestion's line in
// `memory suggestions` and a candidate's `memory show` — nothing is dropped:
// the titles, the reason and a candidate's fields go through
// `escape_hidden_line`, the texts through `escape_hidden`, and a notice
// counts them. `--raw` prints the note file as it is, for redirecting —
// unchanged.

/// A note's words on one terminal line: whitespace runs as one space, and
/// none of the characters an approval's escape shows
/// (`openalpaca_core::utils::text::one_line`, which drops `needs_escape`).
fn shown(value: &Value) -> String {
    one_line(value.as_str().unwrap_or(""))
}

/// One line of a search excerpt: its own spacing and tabs kept, every other
/// control or format character dropped.
fn excerpt_line(line: &str) -> String {
    line.chars()
        .filter(|c| *c == '\t' || !is_hidden(*c))
        .collect()
}

/// `memory show`'s body: the note's whole text, which the person asked to
/// read, so its lines, tabs and format characters stay (a joiner or a
/// right-to-left mark can be content) — only control characters go, C0 and
/// C1 alike (ESC, CSI, BEL, CR, …), so nothing in it drives the terminal.
fn body_text(body: &str) -> String {
    body.chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

/// The first 12 characters of a revision — enough to tell two apart.
fn short(revision: &str) -> String {
    revision.chars().take(12).collect()
}

/// The first 8 characters of an id — a unique prefix in any real store.
fn short_id(id: &NoteId) -> String {
    id.to_string().chars().take(8).collect()
}

fn print_list(page: &Value) {
    for line in list_lines(page) {
        println!("{line}");
    }
}

/// What `memory list` prints, line by line.
fn list_lines(page: &Value) -> Vec<String> {
    let items = page["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return vec!["No notes.".dimmed().to_string()];
    }
    let mut lines = vec![
        format!(
            "{:<8} {:<10} {:<7} {:<10} {:<15} {:<20} TITLE",
            "ID", "KIND", "SCOPE", "STATUS", "ORIGIN", "UPDATED"
        )
        .dimmed()
        .to_string(),
    ];
    for item in &items {
        let status = item["effective_status"]
            .as_str()
            .or(item["status"].as_str())
            .unwrap_or("");
        lines.push(format!(
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
            shown(&item["title"]),
            if item["pinned"].as_bool().unwrap_or(false) {
                "  (pinned)"
            } else {
                ""
            }
        ));
    }
    if let Some(hint) = list_more_hint(page) {
        lines.push(hint.dimmed().to_string());
    }
    lines
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
    for line in search_lines(found) {
        println!("{line}");
    }
}

/// What `memory search` prints, line by line.
fn search_lines(found: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let results = found["results"].as_array().cloned().unwrap_or_default();
    if results.is_empty() {
        lines.push("Nothing matched.".dimmed().to_string());
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
        lines.push(format!(
            "{}  {}  [{}/{}/{}/{}] {}{}",
            r["id"]
                .as_str()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_default()
                .bold(),
            shown(&r["title"]),
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
        ));
        let matched: Vec<&str> = r["match"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        lines.push(
            format!(
                "    {}lines {}-{}, matched on {}",
                r["heading"]
                    .as_str()
                    .map(|h| format!("{} · ", one_line(h)))
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
            .to_string(),
        );
        for line in r["excerpt"].as_str().unwrap_or("").lines() {
            lines.push(format!("    {}", excerpt_line(line)));
        }
    }
    if found["partial"].as_bool().unwrap_or(false) {
        lines.push("(partial: a scan cap was hit)".dimmed().to_string());
    }
    if found["index_state"] == "rebuilding" {
        lines.push("(the search index is rebuilding)".dimmed().to_string());
    }
    lines
}

fn print_timeline(found: &Value) {
    for line in timeline_lines(found) {
        println!("{line}");
    }
}

/// One row per note, oldest first: date, id, kind, scope, status, title —
/// the anchor in bold, a note out of recall with its status in yellow.
fn timeline_lines(found: &Value) -> Vec<String> {
    let rows = found["results"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return vec!["No notes to show.".dimmed().to_string()];
    }
    let mut lines = Vec::new();
    for r in &rows {
        let status = r["status"].as_str().unwrap_or("");
        let line = format!(
            "{:<20} {:<8} {:<10} {:<7} {:<10} {}",
            r["date"].as_str().unwrap_or(""),
            r["id"]
                .as_str()
                .map(|s| s.chars().take(8).collect::<String>())
                .unwrap_or_default(),
            r["kind"].as_str().unwrap_or(""),
            r["scope"].as_str().unwrap_or(""),
            status,
            shown(&r["title"]),
        );
        if r["reason"] == "anchor" {
            lines.push(line.bold().to_string());
        } else if status != "active" {
            lines.push(line.yellow().to_string());
        } else {
            lines.push(line);
        }
    }
    if found["index_state"] == "rebuilding" {
        lines.push("(the search index is rebuilding)".dimmed().to_string());
    }
    lines
}

/// What `memory show` prints: the header lines, a blank line, the body.
///
/// A **candidate** — a note waiting for `memory confirm <id>` — is text the
/// model wrote that the person is deciding to keep, so on one every hidden
/// character in the title, the aliases, the tags, the fact key and the body
/// is shown as `<U+XXXX>` (the fields through `escape_hidden_line`, the body
/// through `escape_hidden`, which keeps its lines and tabs), one line
/// directly under the body counts them, and on a terminal the text starts
/// with [`crate::output::TERMINAL_RESET`], as an approval prompt does. Any
/// other note is the person's own reading: its fields go through `one_line`
/// and its body through [`body_text`], as before.
fn note_text(note: &Value, tty: bool) -> String {
    let deciding = note["effective_status"]
        .as_str()
        .or(note["status"].as_str())
        == Some("candidate");
    let mut hidden = 0;
    let mut field = |text: &str| -> String {
        if deciding {
            let escaped = escape_hidden_line(text);
            hidden += escaped.hidden;
            escaped.text
        } else {
            one_line(text)
        }
    };
    let mut out = String::from(if deciding { terminal_reset(tty) } else { "" });
    let title = field(note["title"].as_str().unwrap_or(""));
    let fm = &note["frontmatter"];
    let strings = |key: &str| -> Vec<String> {
        fm[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let aliases: Vec<String> = strings("aliases").iter().map(|a| field(a)).collect();
    let tags: Vec<String> = strings("tags").iter().map(|t| field(t)).collect();
    let fact_key = fm["fact_key"].as_str().map(&mut field);
    let valid_from = fm["valid_from"].as_str().map(&mut field);
    let valid_to = fm["valid_to"].as_str().map(&mut field);
    let supersedes: Vec<String> = strings("supersedes").iter().map(|s| field(s)).collect();
    let raw_body = note["body"].as_str().unwrap_or("");
    let body = if deciding {
        let escaped = escape_hidden(raw_body);
        hidden += escaped.hidden;
        escaped.text
    } else {
        body_text(raw_body)
    };

    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    line(title.bold().to_string());
    line(
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
        .to_string(),
    );
    let mut facts = Vec::new();
    if let Some(k) = fact_key {
        facts.push(format!("fact_key {k}"));
    }
    if !aliases.is_empty() {
        facts.push(format!("aliases {}", aliases.join(", ")));
    }
    if !tags.is_empty() {
        facts.push(format!("tags {}", tags.join(", ")));
    }
    // A suggestion that bounds a note in time differs from it here alone
    // (the L8 close-out's A4).
    if let Some(from) = valid_from {
        facts.push(format!("valid from {from}"));
    }
    if let Some(to) = valid_to {
        facts.push(format!("valid until {to}"));
    }
    if fm["pinned"].as_bool().unwrap_or(false) {
        facts.push("pinned".into());
    }
    if !facts.is_empty() {
        line(facts.join("  ").dimmed().to_string());
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
        line(
            format!(
                "sources {} ({})",
                sources.join(", "),
                note["sources_status"].as_str().unwrap_or("")
            )
            .dimmed()
            .to_string(),
        );
    }
    if !supersedes.is_empty() {
        line(
            format!("supersedes {}", supersedes.join(", "))
                .dimmed()
                .to_string(),
        );
    }
    let superseded_by: Vec<&str> = note["superseded_by"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !superseded_by.is_empty() {
        line(
            format!("superseded by {}", superseded_by.join(", "))
                .yellow()
                .to_string(),
        );
    }
    if note["conflict"].as_bool().unwrap_or(false) {
        line(
            "conflict: see `openalpaca memory doctor`"
                .yellow()
                .to_string(),
        );
    }
    line(String::new());
    out.push_str(&body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    if let Some(notice) = hidden_notice(hidden) {
        out.push_str(&notice.yellow().to_string());
        out.push('\n');
    }
    if note["truncated"].as_bool().unwrap_or(false) {
        out.push_str(&"(truncated at 32 KiB)".dimmed().to_string());
        out.push('\n');
    }
    out
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
    for s in report["idless_project_snapshots"]
        .as_array()
        .into_iter()
        .flatten()
    {
        println!("{} {}", "!".yellow(), idless_snapshot_line(s));
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

/// One `doctor` line for a project control snapshot that carries no project
/// id (storage design R22): an earlier build's, which this build never
/// opens or adopts, and which nothing deletes.
fn idless_snapshot_line(entry: &Value) -> String {
    format!(
        "project snapshot {} carries no project id: an earlier build wrote it for {}. \
         It is never opened or adopted, and nothing deletes it.",
        entry["partition_key"].as_str().unwrap_or(""),
        entry["root"].as_str().unwrap_or("")
    )
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
