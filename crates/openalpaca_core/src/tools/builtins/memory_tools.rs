//! The memory tools of design §7 / §7a (plan slice 3, row 3.2):
//! `memory_search`, `memory_read`, `memory_list` and `memory_timeline` —
//! the read surface, globally registered under the `memory_read`
//! capability — and `memory_store`, `memory_update`, `memory_forget`,
//! constructed per request for the main conversation's surface
//! (`main_loop.rs`).
//!
//! **The read surface is layered** (design 09 §13.1, MD-14, after
//! claude-mem's "filter first, fetch later"): `memory_search` answers an
//! index by default — one short row per note, no body — `memory_timeline`
//! shows what surrounds a note in time in the same rows, and `memory_read`
//! fetches the notes picked, up to ten in one call.
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
    Capabilities, DayEdge, ForgetInput, ListInput, MemoryError, MemoryService, Patch, READ_MAX_IDS,
    ReadManyInput, SEARCH_MAX_LIMIT, SearchDetail, SearchInput, SearchOrder, StoreInput,
    TIMELINE_MAX_SIDE, TimelineAnchor, TimelineInput, UpdateAction, UpdateInput, parse_date_bound,
    wire,
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

/// A date filter or a timeline's instant: RFC 3339, or `YYYY-MM-DD` taken
/// as that whole UTC day (design 09 §13.1).
fn parse_date(field: &str, value: &str, edge: DayEdge) -> Result<DateTime<Utc>, String> {
    parse_date_bound(value, edge).ok_or_else(|| {
        format!(
            "invalid_input: `{field}` must be an RFC 3339 timestamp or a date (YYYY-MM-DD), got {value:?}"
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
    detail: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    order_by: Option<String>,
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
        let detail = match args.detail.as_deref() {
            None => SearchDetail::Index,
            Some(v) => SearchDetail::parse(v).ok_or_else(|| {
                format!("invalid_input: `detail` must be `index` or `snippets`, got {v:?}")
            })?,
        };
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
            date_from: args
                .date_from
                .as_deref()
                .map(|v| parse_date("date_from", v, DayEdge::Start))
                .transpose()?,
            date_to: args
                .date_to
                .as_deref()
                .map(|v| parse_date("date_to", v, DayEdge::End))
                .transpose()?,
            order: match args.order_by.as_deref() {
                None => SearchOrder::Relevance,
                Some(v) => SearchOrder::parse(v).ok_or_else(|| {
                    format!(
                        "invalid_input: `order_by` must be `relevance`, `newest` or `oldest`, got {v:?}"
                    )
                })?,
            },
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.search(&access, input).map_err(refuse)).await?;
        Ok(wire::search_json(&out, detail).to_string())
    }
}

pub fn memory_search_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_search".to_string(),
        description: "Search the user's memory notes by the words they contain. It returns an \
            index — one short row per note (id, date, kind, scope, status, title, why it \
            matched, size in tokens) and no body — so pick the ids that answer the question \
            and read only those with memory_read, several at once. Matching is lexical: whole \
            words and identifiers, the beginning of a word (a note saying `prefers` matches \
            `prefer`), and part of a word in a title, an alias or a tag, or in Chinese, \
            Japanese, Korean, Thai, Hindi, Tamil and similar scripts. \
            There is no meaning-based search, so use the words a note would actually \
            contain and, if nothing matches, try one more wording. \
            `detail: \"snippets\"` returns cited excerpts instead. Active notes only unless \
            `historical` is true; `date_from`/`date_to` keep notes dated in a range (a note's \
            date is when it was observed, else when it was saved), and `order_by` sorts by \
            relevance, newest or oldest. A result flagged `conflict` is a note another note \
            outranks. Use before answering about the user's preferences, decisions or past \
            facts."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "query": { "type": "string", "description": "1–512 characters: the words to look for" },
                "kind": { "type": "string", "enum": NoteKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(), "description": "Only notes of this kind" },
                "scope": { "type": "string", "enum": ["global", "project"], "description": "Only the global or only the current project's notes (default: both)" },
                "limit": { "type": "integer", "description": format!("1–{SEARCH_MAX_LIMIT}, default 10") },
                "detail": { "type": "string", "enum": ["index", "snippets"], "description": "`index` (default): one row per note and no body; `snippets`: the matching excerpt of each note, with its revision, lines and sources" },
                "date_from": { "type": "string", "description": "Only notes dated at or after this: RFC 3339, or a date YYYY-MM-DD (from its start)" },
                "date_to": { "type": "string", "description": "Only notes dated at or before this: RFC 3339, or a date YYYY-MM-DD (to its end)" },
                "order_by": { "type": "string", "enum": ["relevance", "newest", "oldest"], "description": "Default relevance; newest or oldest sort by the notes' dates" },
                "historical": { "type": "boolean", "description": "Also include notes that are no longer current: superseded, archived, and those whose validity window has ended (default false). Set it for questions about the past — \"used to\", \"before\", \"previously\", \"以前\" — and with `as_of` to ask what held at a date." },
                "as_of": { "type": "string", "description": "RFC 3339: evaluate each note's validity interval at this instant" }
            }),
            &["query"],
        ),
        strict: Some(true),
        input_examples: Some(vec![
            serde_json::json!({"query": "editor preference"}),
            serde_json::json!({"query": "deploy approval", "kind": "decision", "limit": 5}),
            serde_json::json!({"query": "office move", "date_from": "2026-06-01", "order_by": "newest"}),
        ]),
    }
}

// ============================================================================
// memory_read
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    ids: Vec<String>,
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
        if args.ids.is_empty() || args.ids.len() > READ_MAX_IDS {
            return Err(format!(
                "invalid_input: `ids` takes 1 to {READ_MAX_IDS} note ids, got {}",
                args.ids.len()
            ));
        }
        let input = ReadManyInput {
            ids: args
                .ids
                .iter()
                .map(|id| parse_id("ids", id))
                .collect::<Result<_, _>>()?,
            expected_revision: args.expected_revision,
            section: args.section,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.read_many(&access, input).map_err(refuse)).await?;
        Ok(wire::read_many_json(&out).to_string())
    }
}

pub fn memory_read_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_read".to_string(),
        description: "Read the memory notes you picked from a memory_search or \
            memory_timeline index — several at once, up to 10 ids in one call, rather than one \
            call per note. Each comes back with its current Markdown, frontmatter, provenance \
            (origin, sources, what it supersedes and what supersedes it) and revision; the \
            bodies share 32 KiB, so a cut body says `truncated` (read it alone for more). An \
            id no note has answers `not_found` in its place. With a single id, pass \
            `expected_revision` to be told if the note changed since you last saw it, or \
            `section` (a heading) to read that section only. Ids come from memory_search, \
            memory_timeline or memory_list — a file path is not an id."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": READ_MAX_IDS, "description": "1–10 note ids (hyphenated UUIDs), each once" },
                "expected_revision": { "type": "string", "description": "With a single id: a 64-hex revision; a mismatch is reported as revision_conflict with the current one" },
                "section": { "type": "string", "description": "With a single id: a heading of the note; returns that section only" }
            }),
            &["ids"],
        ),
        strict: Some(true),
        input_examples: Some(vec![serde_json::json!({
            "ids": [
                "55e811b6-3220-4758-a47b-034bd1023f38",
                "0c7b2e4a-9d13-4c8e-b1f5-6a2d3e4f5a6b"
            ]
        })]),
    }
}

// ============================================================================
// memory_timeline (design 09 §13.1, MD-14)
// ============================================================================

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineArgs {
    anchor: Option<String>,
    query: Option<String>,
    date: Option<String>,
    before: Option<u64>,
    after: Option<u64>,
}

struct MemoryTimelineTool {
    service: Arc<MemoryService>,
}

#[async_trait]
impl BuiltInTool for MemoryTimelineTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("memory_timeline requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let args: TimelineArgs = parse("memory_timeline", arguments)?;
        let anchor = match (args.anchor, args.query, args.date) {
            (Some(id), None, None) => TimelineAnchor::Note(parse_id("anchor", &id)?),
            (None, Some(query), None) => TimelineAnchor::Query(query),
            (None, None, Some(date)) => {
                TimelineAnchor::Date(parse_date("date", &date, DayEdge::Start)?)
            }
            _ => {
                return Err(
                    "invalid_input: give exactly one of `anchor` (a note id), `query` or `date`"
                        .to_string(),
                );
            }
        };
        let side = |name: &str, value: Option<u64>| -> Result<Option<usize>, String> {
            match value {
                Some(n) if n > TIMELINE_MAX_SIDE as u64 => Err(format!(
                    "invalid_input: `{name}` must be between 0 and {TIMELINE_MAX_SIDE}"
                )),
                Some(n) => Ok(Some(n as usize)),
                None => Ok(None),
            }
        };
        let input = TimelineInput {
            anchor,
            before: side("before", args.before)?,
            after: side("after", args.after)?,
        };
        let service = self.service.clone();
        let access = service
            .context_for_tool(ctx, Capabilities::READ_ONLY)
            .map_err(refuse)?;
        let out = blocking(move || service.timeline(&access, input).map_err(refuse)).await?;
        Ok(wire::timeline_json(&out).to_string())
    }
}

pub fn memory_timeline_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "memory_timeline".to_string(),
        description: "Show what surrounds a memory note in time: the notes dated just before \
            and just after it, in time order, as the same index rows memory_search returns \
            (no body); then read the ids you need with memory_read, several at once. Draw it \
            around exactly one of `anchor` (a note id), `query` (its first search hit becomes \
            the anchor) or `date` (an instant). Unlike a search, it lists notes that have left \
            recall — superseded, archived, or whose validity window has ended — each marked by \
            its `status`, so questions like \"what did I decide before the move\" or \"what \
            did we discuss last week\" have one tool; unconfirmed candidate notes are never \
            listed. A note's date is when it was observed, else when it was saved."
            .to_string(),
        parameters: closed_schema(
            serde_json::json!({
                "anchor": { "type": "string", "description": "A note id: the timeline around that note" },
                "query": { "type": "string", "description": "1–512 characters: the timeline around the best match" },
                "date": { "type": "string", "description": "An instant (RFC 3339, or a date YYYY-MM-DD from its start): notes dated before it, then at or after it" },
                "before": { "type": "integer", "description": format!("How many notes before the anchor: 0–{TIMELINE_MAX_SIDE}, default 5") },
                "after": { "type": "integer", "description": format!("How many notes after it: 0–{TIMELINE_MAX_SIDE}, default 5") }
            }),
            &[],
        ),
        strict: Some(true),
        input_examples: Some(vec![
            serde_json::json!({"query": "office move", "before": 5, "after": 2}),
            serde_json::json!({"date": "2026-09-14", "after": 10}),
        ]),
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

/// The read surface: `memory_search`, `memory_read`, `memory_list` and
/// `memory_timeline`, each providing the `memory_read` capability (design 09
/// §13.1: no new grant). All nine shipped templates name it, and
/// `AgentTemplate::to_subagent` grants it to every internal template that
/// does not deny it (design 09 §5); a plugin template gets it only by naming
/// it. Registered globally by the daemon.
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
            Arc::new(MemoryListTool {
                service: service.clone(),
            }),
            "memory_read",
        ),
        registered(
            memory_timeline_tool_definition(),
            Arc::new(MemoryTimelineTool { service }),
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
        let mut json = wire::store_output_json(&out);
        if out.suggestion
            && let [replaces] = out.superseded.as_slice()
        {
            says_suggestion(&mut json, out.id, *replaces);
        }
        Ok(json.to_string())
    }
}

/// Owner decision LQ11: what a tool result says when the chat model's
/// replacement of a note the user saved, edited by hand or imported was
/// written as a suggestion — in words the model acts on: nothing changed
/// yet, how the user confirms it, and that it is not to be stored again.
fn says_suggestion(json: &mut serde_json::Value, id: NoteId, replaces: NoteId) {
    json["suggestion"] = serde_json::Value::Bool(true);
    json["replaces"] = serde_json::Value::String(replaces.to_string());
    json["note"] = serde_json::Value::String(format!(
        "Saved as a suggestion, not applied: note {replaces} is the user's own — they saved it, \
         edited it by hand or imported it — so a change to it waits for them. Until the user \
         confirms suggestion {id}, their note stays in use and yours is not recalled. The user \
         answers it in the Memory view (Suggestions: Replace, or Keep mine to decline) or \
         confirms it with `openalpaca memory confirm {id}`. On an attended app turn you may \
         raise it with memory_confirm on a later turn, when it is listed in \
         <memory_suggestions>. Tell the user it is waiting; do not store it again."
    ));
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
            revision. Replacing a note the user saved, edited by hand or imported — or saving \
            under such a note's `fact_key` — does not change it: yours is saved as a suggestion \
            the user confirms, and the result says so. An exact duplicate returns the existing \
            note."
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
        let mut json = wire::update_output_json(&out);
        if let Some(replaces) = out.suggested_for {
            says_suggestion(&mut json, out.id, replaces);
        }
        Ok(json.to_string())
    }

    /// Design 09 §4.1: `confirm` is refused from a tool call, so it is
    /// refused **before** the card — a person is no longer asked to approve
    /// something the service would then refuse. The refusal names
    /// `memory_confirm` for a suggestion confirmable in chat (§4.2 rule 5:
    /// one that cites a message the user typed — learning's or, since LQ11,
    /// the assistant's own) on a turn that offers it (an attended `gui`
    /// turn, §4.1), and the Memory view otherwise. It says whose act the
    /// confirm is, never whose words the suggestion holds. Every other
    /// action raises the ordinary card.
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
                "Note {id} is a suggestion waiting for the user, and confirming it is the user's act, not a tool call's: call memory_confirm with its id, its revision and the revision of the note it replaces, and the user answers on the card."
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
///
/// Narrower than `ATTENDED_CHAT_SOURCES` (`memory/service/context.rs`: `gui`
/// and `cli`), which admits the chat memory commands (§12.2), on purpose. A
/// memory command only needs its reply read; a `memory_confirm` card needs
/// a client that shows its preview and answers it for this one call, which
/// the app's card and the CLI's inline prompt do — both on turns that derive
/// `gui`. `cli` is a name `HandleRequest::source` reserves and no production
/// path derives, so the two coincide today; a path that starts deriving
/// `cli` gets the commands at once, and the card only when it is added here.
pub(crate) const IN_CHAT_SOURCE: &str = "gui";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmArgs {
    id: String,
    expected_revision: String,
    target_revision: String,
}

/// `memory_confirm`: the assistant raises one suggestion that cites a
/// message the user typed — learning's, or since LQ11 its own (§4.2 rule
/// 5) — and the person answers on the card (design 09 §4). The model's
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

    /// The card's text (design 09 §4.4), composed from the notes read at
    /// the named revisions — never from the call's arguments (the
    /// suggestion's own text may be the chat model's, LQ11, and the card
    /// says so). A note that is not confirmable
    /// in chat (§4.2 rule 5), or whose notes moved, is refused here, before
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
        let preview = blocking(move || {
            let preview = service
                .suggestion_preview(&access, id, &expected_revision, &target_revision)
                .map_err(refuse)?;
            if let Some(lane) = &lane {
                service.mark_raised(lane, id, preview.target);
            }
            Ok(preview)
        })
        .await?;
        Ok(Some(preview.text))
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
            names: the user answers Approve or Deny on a card that shows both texts, composed \
            by OpenAlpaca from the notes, never from your arguments. Call it only for a \
            suggestion listed in \
            <memory_suggestions> on this turn, with that line's id, revision and \
            replaces_revision, when the conversation reaches its topic or comes to a natural \
            pause — at most once per turn. Once its card has been shown, whatever the answer, \
            a suggestion and every other suggestion for the same note leave that list, in \
            every conversation on this chat channel, until OpenAlpaca restarts: never raise \
            one that is not listed now, even one you saw earlier. Your call alone changes \
            nothing: only the user's Approve applies it, and a Deny dismisses it for good. A suggestion inferred from the conversation rather than from the user's \
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
            (memory_read gives both, as do memory_list and memory_search with \
            `detail: \"snippets\"`); a changed revision is a conflict and nothing is written. \
            `edit` applies a `patch` to the named fields and keeps the rest byte for byte; \
            `archive` retires the note from recall. An edit of a note the user saved, edited \
            by hand or imported does not change it: the edited note is saved as a suggestion \
            the user confirms, and the result says so; one that rests on a message the user \
            asked to forget is refused. A note the user saved cannot be archived from here. `confirm` (making a candidate \
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
            with the revision you read (memory_read gives both, as do memory_list and \
            memory_search with `detail: \"snippets\"`), never a search phrase — find and name \
            the exact notes first, and resolve any ambiguity with the user before calling. \
            Removes the notes, their history and their index rows, and says what remains \
            elsewhere (the transcript messages they cited are not erased). Asks the user for \
            confirmation before it runs."
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
