//! The six memory tools of design §7 / §7a (plan slice 3, row 3.2):
//! `memory_search`, `memory_read`, `memory_list` — the read surface,
//! globally registered under the `memory_read` capability — and
//! `memory_store`, `memory_update`, `memory_forget`, constructed per request
//! for the main conversation's surface (`main_loop.rs`).
//!
//! Every tool validates its own input strictly: the schemas are closed and a
//! JSON object carrying any property the schema does not name — `owner_id`,
//! `workspace_id`, a `path`, `origin`, `pinned`, … — is refused
//! `invalid_input` naming the property, before anything is read or written
//! (design §3 rule 6; the registry's validator accepts unknown keys, F2).
//! Identity, workspace and surface come from the [`ToolContext`] through
//! [`MemoryService::context_for_tool`], never from the arguments.
//!
//! What a tool call may do is the service's decision (design §4, G2): a
//! store records `extracted_user`, `confirm` is refused, a `user` note is
//! never edited, archived or superseded from here. The confirmation prompt
//! is decided by the annotations `annotations_for_builtin` gives the names
//! (`memory_forget` and `memory_update` prompt; `memory_store` does not).

use crate::memory::service::{
    Capabilities, ForgetInput, ListInput, MemoryError, MemoryService, Patch, ReadInput,
    SearchInput, StoreInput, UpdateAction, UpdateInput, wire,
};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::memory_files::{
    NoteId, NoteKind, NoteScope, NoteSource, NoteStatus, ReplacePolicy, SourceType,
};
use serde::Deserialize;
use std::sync::Arc;

use super::annotations_for_builtin;

// ============================================================================
// Shared pieces
// ============================================================================

/// The `type: object … additionalProperties: false` shell of every schema
/// (design §7a: every object is closed).
fn closed_schema(properties: serde_json::Value, required: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn registered(
    definition: ToolDefinition,
    backend: Arc<dyn BuiltInTool>,
    capability: &str,
) -> RegisteredTool {
    RegisteredTool {
        annotations: annotations_for_builtin(&definition.name),
        definition,
        backend: ToolBackend::BuiltIn(backend),
        provides_capabilities: vec![capability.to_string()],
        exempt_from_timeout: false,
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

/// The refusal a tool answers with: the design's class, then the reason.
fn refuse(e: MemoryError) -> String {
    format!("{}: {e}", e.class().trim_start_matches("fixture:"))
}

/// Strict parse: an unknown property is an `invalid_input` naming it.
fn parse<T: for<'de> Deserialize<'de>>(
    tool: &str,
    arguments: &serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(arguments.clone())
        .map_err(|e| format!("invalid_input: {tool} arguments: {e}"))
}

fn parse_id(field: &str, value: &str) -> Result<NoteId, String> {
    NoteId::parse(value).map_err(|_| {
        format!("invalid_input: `{field}` must be a note id (a hyphenated UUID), got {value:?}")
    })
}

fn parse_kind(value: &str) -> Result<NoteKind, String> {
    NoteKind::parse(value).ok_or_else(|| {
        let names: Vec<&str> = NoteKind::ALL.iter().map(|k| k.as_str()).collect();
        format!(
            "invalid_input: `kind` must be one of {}, got {value:?}",
            names.join(", ")
        )
    })
}

fn parse_scope(value: &str) -> Result<NoteScope, String> {
    NoteScope::parse(value).ok_or_else(|| {
        format!("invalid_input: `scope` must be `global` or `project`, got {value:?}")
    })
}

fn parse_status(value: &str) -> Result<NoteStatus, String> {
    NoteStatus::parse(value).ok_or_else(|| {
        format!(
            "invalid_input: `status` must be `active`, `candidate` or `archived`, got {value:?}"
        )
    })
}

fn parse_ts(field: &str, value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| format!("invalid_input: `{field}` is not an RFC 3339 timestamp: {e}"))
}

/// `Some(None)` clears a nullable field; absent leaves it alone.
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Runs a blocking service call on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("io: memory service call did not complete: {e}"))?
}

// ============================================================================
// memory_search
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    kind: Option<String>,
    scope: Option<String>,
    limit: Option<u64>,
    historical: Option<bool>,
    as_of: Option<String>,
}

struct MemorySearchTool {
    service: Arc<MemoryService>,
}

#[async_trait]
impl BuiltInTool for MemorySearchTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_search requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: SearchArgs = parse("memory_search", arguments)?;
        let input = SearchInput {
            query: args.query,
            kind: args.kind.as_deref().map(parse_kind).transpose()?,
            scope: args.scope.as_deref().map(parse_scope).transpose()?,
            limit: args.limit.map(|l| usize::try_from(l).unwrap_or(usize::MAX)),
            historical: args.historical.unwrap_or(false),
            as_of: args
                .as_of
                .as_deref()
                .map(|v| parse_ts("as_of", v))
                .transpose()?,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.search(&access, input).map_err(refuse)).await?;
        Ok(wire::search_output_json(&out).to_string())
    }
}

pub fn memory_search_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_search".to_string(),
        description: "Search the user's memory notes by the words they contain. Matching is \
            lexical: whole words and identifiers, partial words (a note saying `prefers` \
            matches `prefer`), titles, aliases and tags. There is no meaning-based search, \
            so use the words a note would actually contain and, if nothing matches, try one \
            more wording. Returns cited excerpts with each note's id, revision, path, line \
            range, origin, freshness and sources. Active notes only unless `historical` is \
            true; a result flagged `conflict` is a note another note outranks. Use before \
            answering about the user's preferences, decisions or past facts."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "query": { "type": "string", "description": "1–512 characters: the words to look for" },
                "kind": { "type": "string", "enum": NoteKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(), "description": "Only notes of this kind" },
                "scope": { "type": "string", "enum": ["global", "project"], "description": "Only the global or only the current project's notes (default: both)" },
                "limit": { "type": "integer", "description": "1–20, default 8" },
                "historical": { "type": "boolean", "description": "Also include notes that are no longer current: superseded, archived, and those whose validity window has ended (default false). Set it for questions about the past — \"used to\", \"before\", \"previously\", \"以前\" — and with `as_of` to ask what held at a date." },
                "as_of": { "type": "string", "description": "RFC 3339: evaluate each note's validity interval at this instant" }
            }),
            &["query"],
        ),
        strict: Some(true),
        input_examples: Some(vec![
            serde_json::json!({"query": "editor preference"}),
            serde_json::json!({"query": "deploy approval", "kind": "decision", "limit": 5}),
        ]),
    }
}

// ============================================================================
// memory_read
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    id: String,
    expected_revision: Option<String>,
    section: Option<String>,
}

struct MemoryReadTool {
    service: Arc<MemoryService>,
}

#[async_trait]
impl BuiltInTool for MemoryReadTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_read requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: ReadArgs = parse("memory_read", arguments)?;
        let input = ReadInput {
            id: parse_id("id", &args.id)?,
            expected_revision: args.expected_revision,
            section: args.section,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.read(&access, input).map_err(refuse)).await?;
        Ok(wire::read_output_json(&out).to_string())
    }
}

pub fn memory_read_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_read".to_string(),
        description: "Read one memory note by id: its current Markdown (at most 32 KiB), \
            frontmatter, provenance (origin, sources, what it supersedes and what supersedes \
            it) and revision. Pass `expected_revision` to be told if the note changed since \
            you last saw it; pass `section` (a heading) to read that section only. Ids come \
            from memory_search or memory_list — a file path is not an id."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "id": { "type": "string", "description": "The note's id (a hyphenated UUID)" },
                "expected_revision": { "type": "string", "description": "A 64-hex revision; a mismatch is reported as revision_conflict with the current one" },
                "section": { "type": "string", "description": "A heading of the note; returns that section only" }
            }),
            &["id"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "id": "55e811b6-3220-4758-a47b-034bd1023f38"
        })]),
    }
}

// ============================================================================
// memory_list
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    kind: Option<String>,
    status: Option<String>,
    scope: Option<String>,
    cursor: Option<String>,
    limit: Option<u64>,
    include_history: Option<bool>,
    include_candidates: Option<bool>,
}

struct MemoryListTool {
    service: Arc<MemoryService>,
}

#[async_trait]
impl BuiltInTool for MemoryListTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_list requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: ListArgs = parse("memory_list", arguments)?;
        let input = ListInput {
            kind: args.kind.as_deref().map(parse_kind).transpose()?,
            status: args.status.as_deref().map(parse_status).transpose()?,
            scope: args.scope.as_deref().map(parse_scope).transpose()?,
            cursor: args.cursor,
            limit: args.limit.map(|l| usize::try_from(l).unwrap_or(usize::MAX)),
            include_history: args.include_history.unwrap_or(false),
            include_candidates: args.include_candidates.unwrap_or(false),
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.list(&access, input).map_err(refuse)).await?;
        Ok(wire::list_output_json(&out).to_string())
    }
}

pub fn memory_list_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_list".to_string(),
        description: "List the user's memory notes (metadata only: id, revision, title, kind, \
            scope, status, origin, updated_at, pinned), newest first, in pages of up to 100. \
            Active notes by default; `status` selects archived or candidate notes, \
            `include_history` adds superseded ones, `include_candidates` adds unconfirmed \
            extractions. Pass the previous page's `next_cursor` for the next page."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "kind": { "type": "string", "enum": NoteKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>() },
                "status": { "type": "string", "enum": ["active", "candidate", "archived"], "description": "Default active" },
                "scope": { "type": "string", "enum": ["global", "project"] },
                "cursor": { "type": "string", "description": "Opaque, from a previous page with the same filters" },
                "limit": { "type": "integer", "description": "1–100, default 50" },
                "include_history": { "type": "boolean", "description": "Include superseded notes (default false)" },
                "include_candidates": { "type": "boolean", "description": "Include candidates (default false)" }
            }),
            &[],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({"kind": "preference", "limit": 20})]),
    }
}

/// The read surface: `memory_search`, `memory_read` and `memory_list`,
/// each providing the `memory_read` capability (design §7: no template
/// change — the six templates that grant `memory_read` keep working).
/// Registered globally by the daemon.
pub fn memory_read_tools(service: Arc<MemoryService>) -> Vec<RegisteredTool> {
    vec![
        registered(
            memory_search_tool_definition(),
            Arc::new(MemorySearchTool {
                service: service.clone(),
            }),
            "memory_read",
        ),
        registered(
            memory_read_tool_definition(),
            Arc::new(MemoryReadTool {
                service: service.clone(),
            }),
            "memory_read",
        ),
        registered(
            memory_list_tool_definition(),
            Arc::new(MemoryListTool { service }),
            "memory_read",
        ),
    ]
}

// ============================================================================
// memory_store
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceArg {
    #[serde(rename = "type")]
    kind: String,
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SupersedesArg {
    id: String,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreArgs {
    title: String,
    body: String,
    kind: String,
    scope: Option<String>,
    aliases: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    fact_key: Option<String>,
    sources: Option<Vec<SourceArg>>,
    valid_from: Option<String>,
    valid_to: Option<String>,
    supersedes: Option<SupersedesArg>,
}

fn parse_sources(sources: Vec<SourceArg>) -> Result<Vec<NoteSource>, String> {
    sources
        .into_iter()
        .map(|s| {
            let kind = SourceType::parse(&s.kind).ok_or_else(|| {
                format!(
                    "invalid_input: `sources[].type` must be `message` or `task`, got {:?}",
                    s.kind
                )
            })?;
            Ok(NoteSource {
                kind,
                id: s.id,
                observed_at: None,
            })
        })
        .collect()
}

/// `memory_store`: constructed per request for the main conversation's
/// surface. A tool-call save records origin `extracted_user` (design §4).
pub struct MemoryStoreTool {
    service: Arc<MemoryService>,
}

impl MemoryStoreTool {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl BuiltInTool for MemoryStoreTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_store requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: StoreArgs = parse("memory_store", arguments)?;
        let input = StoreInput {
            title: args.title,
            body: args.body,
            kind: Some(parse_kind(&args.kind)?),
            scope: args.scope.as_deref().map(parse_scope).transpose()?,
            aliases: args.aliases.unwrap_or_default(),
            tags: args.tags.unwrap_or_default(),
            fact_key: args.fact_key,
            sources: parse_sources(args.sources.unwrap_or_default())?,
            valid_from: args
                .valid_from
                .as_deref()
                .map(|v| parse_ts("valid_from", v))
                .transpose()?,
            valid_to: args
                .valid_to
                .as_deref()
                .map(|v| parse_ts("valid_to", v))
                .transpose()?,
            supersedes: args
                .supersedes
                .map(|s| Ok::<_, String>((parse_id("supersedes.id", &s.id)?, s.expected_revision)))
                .transpose()?,
            // Learning's own (design 09 §2.5): never from a tool call.
            observed_at: None,
            replace_policy: ReplacePolicy::Direct,
            provenance: None,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_WRITE)
            .map_err(refuse)?;
        let out = blocking(move || service.store(&access, input).map_err(refuse)).await?;
        Ok(wire::store_output_json(&out).to_string())
    }
}

pub fn memory_store_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_store".to_string(),
        description: "Save something durable the user told you — a preference, a fact, a \
            decision, a procedure, a reference or an episode — as a memory note in the user's \
            own words, with a short title and a Markdown body. Use when the user asks you to \
            remember something or states a lasting preference. A preference is saved for \
            every project; other kinds go to the current project when there is one. The \
            note is recorded as learned from the user, not as the user's own edit, and the \
            user can review it. To replace an earlier note pass `supersedes` with its id and \
            revision (you cannot replace a note the user saved themselves). An exact \
            duplicate returns the existing note."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "title": { "type": "string", "description": "1–200 characters" },
                "body": { "type": "string", "description": "Markdown, at most 32 KiB; start with a heading, then the knowledge" },
                "kind": { "type": "string", "enum": NoteKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>() },
                "scope": { "type": "string", "enum": ["global", "project"], "description": "Default: global for a preference, the current project for everything else" },
                "aliases": { "type": "array", "items": { "type": "string" }, "description": "Other words or languages a search might use (at most 16)" },
                "tags": { "type": "array", "items": { "type": "string" }, "description": "At most 16" },
                "fact_key": { "type": "string", "description": "A stable key like `editor.preference` ([a-z0-9][a-z0-9._-]{0,63}); notes with the same key resolve to one winner" },
                "sources": { "type": "array", "items": { "type": "object", "properties": { "type": { "type": "string", "enum": ["message", "task"] }, "id": { "type": "string" } }, "required": ["type", "id"], "additionalProperties": false }, "description": "At most 8 database ids the note rests on" },
                "valid_from": { "type": "string", "description": "RFC 3339" },
                "valid_to": { "type": "string", "description": "RFC 3339" },
                "supersedes": { "type": "object", "properties": { "id": { "type": "string" }, "expected_revision": { "type": "string" } }, "required": ["id", "expected_revision"], "additionalProperties": false, "description": "The note this one replaces, at the revision you read" }
            }),
            &["title", "body", "kind"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "title": "Preferred editor",
            "body": "# Preferred editor\n\nThe user prefers vim for editing.\n",
            "kind": "preference",
            "aliases": ["vi", "neovim"],
            "fact_key": "editor.preference"
        })]),
    }
}

// ============================================================================
// memory_update
// ============================================================================

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PatchArg {
    title: Option<String>,
    body: Option<String>,
    aliases: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    fact_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    valid_from: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    valid_to: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    review_after: Option<Option<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArgs {
    id: String,
    expected_revision: String,
    action: String,
    patch: Option<PatchArg>,
    reason: Option<String>,
}

fn nullable_ts(
    field: &str,
    value: Option<Option<String>>,
) -> Result<Option<Option<DateTime<Utc>>>, String> {
    match value {
        None => Ok(None),
        Some(None) => Ok(Some(None)),
        Some(Some(v)) => Ok(Some(Some(parse_ts(field, &v)?))),
    }
}

/// `memory_update`: edit or archive by id and expected revision. `confirm`
/// is refused from a tool call (design §4).
pub struct MemoryUpdateTool {
    service: Arc<MemoryService>,
}

impl MemoryUpdateTool {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl BuiltInTool for MemoryUpdateTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_update requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: UpdateArgs = parse("memory_update", arguments)?;
        if let Some(reason) = &args.reason
            && reason.chars().count() > 500
        {
            return Err("invalid_input: `reason` is at most 500 characters".to_string());
        }
        let patch_given = args.patch.is_some();
        let action = match args.action.as_str() {
            "edit" => {
                let p = args.patch.ok_or_else(|| {
                    "invalid_input: `edit` needs a `patch` naming what changes".to_string()
                })?;
                UpdateAction::Edit(Patch {
                    title: p.title,
                    body: p.body,
                    aliases: p.aliases,
                    tags: p.tags,
                    fact_key: p.fact_key,
                    valid_from: nullable_ts("patch.valid_from", p.valid_from)?,
                    valid_to: nullable_ts("patch.valid_to", p.valid_to)?,
                    review_after: nullable_ts("patch.review_after", p.review_after)?,
                })
            }
            "archive" => UpdateAction::Archive,
            "confirm" => UpdateAction::Confirm {
                target_revision: None,
            },
            other => {
                return Err(format!(
                    "invalid_input: `action` must be `edit`, `archive` or `confirm`, got {other:?}"
                ));
            }
        };
        if !matches!(action, UpdateAction::Edit(_)) && patch_given {
            return Err("invalid_input: `patch` goes with `edit` only".to_string());
        }
        let input = UpdateInput {
            id: parse_id("id", &args.id)?,
            expected_revision: args.expected_revision,
            action,
            reason: args.reason,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_WRITE)
            .map_err(refuse)?;
        let out = blocking(move || service.update(&access, input).map_err(refuse)).await?;
        Ok(wire::update_output_json(&out).to_string())
    }

    /// Design 09 §4.1: `confirm` is refused from a tool call, so it is
    /// refused **before** the card — a person is no longer asked to approve
    /// something the service would then refuse. The refusal names
    /// `memory_confirm` for a suggestion from the user's own words on a turn
    /// that offers it (an attended `gui` turn, §4.1), and the Memory view
    /// otherwise. Every other action raises the ordinary card.
    async fn confirmation_preview(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        if arguments.get("action").and_then(|a| a.as_str()) != Some("confirm") {
            return Ok(None);
        }
        let Some(id) = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|v| NoteId::parse(v).ok())
        else {
            return Err("invalid_input: `id` must be a note id (a hyphenated UUID)".to_string());
        };
        let service = self.service.clone();
        let in_chat = ctx.source.as_deref() == Some(IN_CHAT_SOURCE)
            && match service.context_for_tool(ctx, Capabilities::READ_ONLY) {
                Ok(access) => blocking(move || Ok(service.is_in_chat_suggestion(&access, id)))
                    .await
                    .unwrap_or(false),
                Err(_) => false,
            };
        let message = if in_chat {
            format!(
                "Note {id} is a suggestion from the user's own words, and confirming it is the user's act, not a tool call's: call memory_confirm with its id, its revision and the revision of the note it replaces, and the user answers on the card."
            )
        } else {
            format!(
                "Confirming a remembered candidate is done by you, not by me: open the Memory view in the app, or run `openalpaca memory confirm {id}`."
            )
        };
        Err(refuse(MemoryError::ConfirmRefused { id, message }))
    }
}

// ============================================================================
// memory_confirm (design 09 §4, MD-12)
// ============================================================================

/// The derived source of the GUI's and the CLI's chat turns
/// (`chat/service.rs`, `EventSource::Gui`): the only turns `memory_confirm`
/// is offered on (design 09 §4.1, ruling LR13).
pub(crate) const IN_CHAT_SOURCE: &str = "gui";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmArgs {
    id: String,
    expected_revision: String,
    target_revision: String,
}

/// `memory_confirm`: the assistant raises one suggestion from the user's own
/// words, and the person answers on the card (design 09 §4). The model's
/// call alone changes nothing: the confirm runs only with the approval fact
/// the sandbox makes when a person clicked Approve for **this** call (§4.2
/// rule 1), and a Deny dismisses the suggestion for good (§4.3, LQ2).
/// Constructed per request, only on attended `gui` main-loop turns.
pub struct MemoryConfirmTool {
    service: Arc<MemoryService>,
}

impl MemoryConfirmTool {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }

    fn parse(arguments: &serde_json::Value) -> Result<(NoteId, String, String), String> {
        let args: ConfirmArgs = parse("memory_confirm", arguments)?;
        Ok((
            parse_id("id", &args.id)?,
            args.expected_revision,
            args.target_revision,
        ))
    }
}

#[async_trait]
impl BuiltInTool for MemoryConfirmTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_confirm requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let (id, expected_revision, target_revision) = Self::parse(arguments)?;
        // Rule 6: the model saying yes changes nothing.
        let Some(approval) = ctx.human_approval.clone() else {
            return Err(refuse(MemoryError::ApprovalRequired { id }));
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_WRITE)
            .map_err(refuse)?;
        let out = blocking(move || {
            service
                .confirm_suggestion(&access, &approval, id, &expected_revision, &target_revision)
                .map_err(refuse)
        })
        .await?;
        Ok(wire::update_output_json(&out).to_string())
    }

    /// The card's text (design 09 §4.4), read from the notes at the named
    /// revisions — never the model's words. A suggestion that is not from
    /// the user's own words, or whose notes moved, is refused here, before
    /// any card. A preview that renders marks the suggestion raised on this
    /// lane (§4.5).
    async fn confirmation_preview(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<Option<String>, String> {
        let (id, expected_revision, target_revision) = Self::parse(arguments)?;
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let lane = ctx.lane_key.clone();
        let text = blocking(move || {
            let text = service
                .suggestion_preview(&access, id, &expected_revision, &target_revision)
                .map_err(refuse)?;
            if let Some(lane) = &lane {
                service.mark_raised(lane, id);
            }
            Ok(text)
        })
        .await?;
        Ok(Some(text))
    }

    /// A Deny (design 09 §4.3, LQ2): the suggestion is withdrawn, as *Keep
    /// mine* withdraws it; the note it would have replaced is untouched.
    async fn on_confirmation_denied(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Option<String> {
        let (id, expected_revision, _) = Self::parse(arguments).ok()?;
        let denial = ctx.human_denial.clone()?;
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_WRITE)
            .ok()?;
        let dismissed = blocking(move || {
            service
                .dismiss_suggestion(&access, &denial, id, &expected_revision)
                .map_err(refuse)
        })
        .await;
        Some(match dismissed {
            Ok(_) => {
                "The user declined; the suggestion was dismissed. Do not raise it again."
                    .to_string()
            }
            Err(e) => {
                tracing::warn!(note = %id, "memory: a denied suggestion could not be dismissed: {e}");
                format!(
                    "The user declined; the suggestion was left as it is ({e}). Do not raise it again."
                )
            }
        })
    }
}

pub fn memory_confirm_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_confirm".to_string(),
        description: "Ask the user whether one pending suggestion should replace the note it \
            names: the user answers Approve or Deny on a card that shows both texts, written \
            by OpenAlpaca, not by you. Call it for one suggestion listed in \
            <memory_suggestions>, with that line's id, revision and replaces_revision, when \
            the conversation reaches its topic or comes to a natural pause — at most once per \
            turn, and never for a suggestion already raised in this conversation. Your call \
            alone changes nothing: only the user's Approve applies it, and a Deny dismisses it \
            for good. A suggestion inferred from the conversation rather than from the user's \
            own words is refused as not_a_suggestion; the user confirms that kind in the \
            app's Memory view or with `openalpaca memory confirm <id>`."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "id": { "type": "string", "description": "The suggestion's id, from <memory_suggestions>" },
                "expected_revision": { "type": "string", "description": "The suggestion's 64-hex revision, as listed" },
                "target_revision": { "type": "string", "description": "The 64-hex revision of the note it replaces (replaces_revision), as listed" }
            }),
            &["id", "expected_revision", "target_revision"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "id": "55e811b6-3220-4758-a47b-034bd1023f38",
            "expected_revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "target_revision": "2c26b46b68ffc68ff99b453c1d30413413422d706483bfa0f98a5e886266e7ae"
        })]),
    }
}

pub fn memory_update_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_update".to_string(),
        description: "Edit or archive one memory note by id and the revision you read \
            (memory_search or memory_read gives both); a changed revision is a conflict and \
            nothing is written. `edit` applies a `patch` to the named fields and keeps the \
            rest byte for byte; `archive` retires the note from recall. A note the user saved \
            themselves cannot be edited or archived from here. `confirm` (making a candidate \
            active) is the user's own act and is refused here — tell the user to confirm it \
            in the app's Memory view or with `openalpaca memory confirm <id>`. Asks the user \
            for confirmation before it runs."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "id": { "type": "string", "description": "The note's id" },
                "expected_revision": { "type": "string", "description": "The 64-hex revision you read" },
                "action": { "type": "string", "enum": ["edit", "archive", "confirm"] },
                "patch": { "type": "object", "properties": {
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "aliases": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "fact_key": { "type": ["string", "null"] },
                    "valid_from": { "type": ["string", "null"] },
                    "valid_to": { "type": ["string", "null"] },
                    "review_after": { "type": ["string", "null"] }
                }, "additionalProperties": false, "description": "With `edit`: only the fields named change; null clears a nullable one" },
                "reason": { "type": "string", "description": "At most 500 characters" }
            }),
            &["id", "expected_revision", "action"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "id": "55e811b6-3220-4758-a47b-034bd1023f38",
            "expected_revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "action": "edit",
            "patch": { "aliases": ["vi", "neovim"] }
        })]),
    }
}

// ============================================================================
// memory_forget
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetArg {
    id: String,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgetArgs {
    targets: Vec<TargetArg>,
    reason: Option<String>,
}

/// `memory_forget`: explicit ids with revisions, never "the best match"
/// (design §6, *Forget contract*). Prompted on an attended turn, refused
/// unattended, by its annotations.
pub struct MemoryForgetTool {
    service: Arc<MemoryService>,
}

impl MemoryForgetTool {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl BuiltInTool for MemoryForgetTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_forget requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: ForgetArgs = parse("memory_forget", arguments)?;
        if args.targets.is_empty() || args.targets.len() > 20 {
            return Err("invalid_input: `targets` takes 1 to 20 entries".to_string());
        }
        if let Some(reason) = &args.reason
            && reason.chars().count() > 500
        {
            return Err("invalid_input: `reason` is at most 500 characters".to_string());
        }
        let targets = args
            .targets
            .into_iter()
            .map(|t| Ok::<_, String>((parse_id("targets[].id", &t.id)?, t.expected_revision)))
            .collect::<Result<Vec<_>, _>>()?;
        let input = ForgetInput {
            targets,
            reason: args.reason,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::ALL)
            .map_err(refuse)?;
        let out = blocking(move || service.forget(&access, input).map_err(refuse)).await?;
        Ok(wire::forget_output_json(&out).to_string())
    }
}

pub fn memory_forget_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_forget".to_string(),
        description: "Forget memory notes the user asked to forget: each target is a note id \
            with the revision you read (from memory_search or memory_read), never a search \
            phrase — find and name the exact notes first, and resolve any ambiguity with the \
            user before calling. Removes the notes, their history and their index rows, and \
            says what remains elsewhere (the transcript messages they cited are not \
            erased). Asks the user for confirmation before it runs."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "targets": { "type": "array", "items": { "type": "object", "properties": { "id": { "type": "string" }, "expected_revision": { "type": "string" } }, "required": ["id", "expected_revision"], "additionalProperties": false }, "description": "1–20 notes, each at the revision you read" },
                "reason": { "type": "string", "description": "At most 500 characters" }
            }),
            &["targets"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "targets": [{
                "id": "55e811b6-3220-4758-a47b-034bd1023f38",
                "expected_revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
            }],
            "reason": "the user asked to forget their old address"
        })]),
    }
}

#[cfg(test)]
mod tests;
