//! The responses only the CLI reads, checked from this side (CLI-PROTOCOL's
//! amended half; *Rev 107*'s CLI-reads item, (b)): the daemon's tests write
//! each one's sample into the API contract's folder (`cli/`, beside the
//! GUI's `responses/`), where a field renamed or removed fails the
//! daemon's bump check unless `PROTOCOL` moved; this test holds every key
//! the CLI reads to the sample, so the CLI is caught up in the same commit
//! as the bump, as the GUI's vitest catches the app.
//!
//! The paths are the ones the CLI reads (`a`, `a.b`, `a[].b`, `[].a`, as
//! the folder's check spells them), each at the code that reads it. A key
//! the CLI reads only on another outcome than the sample's (a forget's
//! `current_revision`, on a conflict) is not listed.
//!
//! **The CLI's typed reads** (`holds`; *Rev 124*'s 124-2, (b), with
//! CONTRACT-SAMPLES-2): every struct the CLI reads a response into — from
//! the shared `responses/` samples the app reads too, from `cli/`, and the
//! one events-socket frame it reads — is read from its sample at the path
//! the CLI reads it, and each wire name the struct reads (serde's own list,
//! after renames, aliases among them) must be a key there. A deserialize
//! alone proves nothing for an `Option` or `#[serde(default)]` field, which
//! is how `agent list`'s `model` and `skills` went unread: the name check
//! is what fails them. A field the daemon does not send is pinned in its
//! hold's `unsent` list, a finding, so a fix on either side updates it.
//!
//! **The `serde_json::Value` reads** (*Rev 142*'s item 12, (b), with
//! CONTRACT-SAMPLES-3): the memory commands, the key list, the REPL and
//! `chat_stream`'s SSE frames read their answers as `serde_json::Value`, so
//! no struct names their keys; each key path they read is listed in
//! [`READS`] against its `responses/` or `stream/` sample, as the `cli/`
//! reads are. The `POST`s whose route reads no body send
//! [`crate::client::empty_body`], held to their route having no request
//! sample ([`EMPTY_BODY_POSTS`]); their answers are held in [`holds`].

use std::collections::BTreeSet;
use std::path::Path;

/// The contract folder, whose kinds the reads and holds name.
const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract"
);

/// Each sample and the key paths the CLI reads in it: its kind (`cli`,
/// `responses` or `stream`), its name, the paths. Every `cli/` sample is
/// listed; a `responses/` or `stream/` one is listed where the CLI reads it
/// as a `serde_json::Value` (CONTRACT-SAMPLES-3, *Rev 142*'s item 12, (b)).
const READS: &[(&str, &str, &[&str])] = &[
    // `sessions.rs` `MeResponse`; `tasks.rs`'s lane.
    ("cli", "get-me", &["default_lane_key"]),
    // `agents.rs` `AgentItem`, `config_tui.rs` `TuiAgentItem`; the REPL's
    // `/agents` (`repl/commands` `cmd_agents`) reads `id`, `name`, `status`.
    (
        "cli",
        "get-agents",
        &[
            "[].id",
            "[].name",
            "[].status",
            "[].description",
            "[].skills_json",
            "[].llm_config_json",
        ],
    ),
    // `agents.rs` `create_interactive`, `create_from_file`.
    ("cli", "post-agents", &["agent_id"]),
    ("cli", "post-agents-from-toml", &["agent_id"]),
    // `agents.rs` `remove_agent`, `config_tui.rs` `remove_agent_tui`.
    ("cli", "delete-agents-id", &["status"]),
    // `agents.rs` `agent_set_with`, `config_tui.rs` `interactive_edit_agent_config`.
    ("cli", "put-agents-id-config", &["config_version"]),
    // `chat.rs` `named_session`, `refuse_foreign_lane` (`SessionItem`).
    (
        "cli",
        "get-sessions-id",
        &["id", "lane_key", "workspace_id"],
    ),
    // `memory.rs` `print_doctor`, `idless_snapshot_line`.
    (
        "cli",
        "get-memories-doctor",
        &[
            "owner_key",
            "partitions",
            "partitions[].health",
            "partitions[].scope",
            "partitions[].partition_key",
            "partitions[].orphan",
            "partitions[].root",
            "partitions[].quarantine_reason",
            "partitions[].notes",
            "partitions[].unverified",
            "partitions[].tombstones",
            "partitions[].pending",
            "partitions[].conflicts",
            "partitions[].malformed",
            "partitions[].malformed[].path",
            "partitions[].malformed[].reason",
            "partitions[].duplicates",
            "partitions[].duplicates[].id",
            "partitions[].duplicates[].files",
            "idless_project_snapshots",
            "refused",
            "unreadable_snapshots",
        ],
    ),
    // `memory.rs` `MemoryCommands::Sync`.
    (
        "cli",
        "post-memories-sync",
        &[
            "scanned",
            "changed",
            "index_state",
            "changes",
            "changes[].note_id",
            "changes[].revision",
        ],
    ),
    // `memory.rs` `print_timeline`.
    (
        "cli",
        "get-memories-timeline",
        &[
            "results",
            "results[].status",
            "results[].date",
            "results[].id",
            "results[].kind",
            "results[].scope",
            "results[].title",
            "results[].reason",
            "index_state",
        ],
    ),
    // `memory.rs` `forget_notes` (`results[].current_revision` only on a
    // conflict, another outcome than the sample's).
    (
        "cli",
        "post-memories-forget",
        &[
            "results",
            "results[].id",
            "results[].status",
            "retained.note",
        ],
    ),
    // ── The `serde_json::Value` reads (*Rev 142*'s item 12) ──
    // `memory.rs` `list_lines`, `list_more_hint`, `Api::resolve_id`.
    (
        "responses",
        "get-memories",
        &[
            "items",
            "items[].id",
            "items[].effective_status",
            "items[].status",
            "items[].kind",
            "items[].scope",
            "items[].origin",
            "items[].updated_at",
            "items[].title",
            "items[].pinned",
            "next_cursor",
        ],
    ),
    // `memory.rs` `note_text`, `note_from_read`, `edit_note`,
    // `confirm_note`, `undo_message`, `forget_line`, `is_pending_suggestion`,
    // `revision_of`, `memory add --supersedes`.
    (
        "responses",
        "get-memories-id",
        &[
            "id",
            "title",
            "revision",
            "status",
            "effective_status",
            "kind",
            "scope",
            "origin",
            "freshness",
            "body",
            "truncated",
            "path",
            "supersedes",
            "sources",
            "sources[].type",
            "sources[].id",
            "sources_status",
            "superseded_by",
            "conflict",
            "frontmatter",
            "frontmatter.title",
            "frontmatter.kind",
            "frontmatter.status",
            "frontmatter.created_at",
            "frontmatter.updated_at",
            "frontmatter.observed_at",
            "frontmatter.valid_from",
            "frontmatter.valid_to",
            "frontmatter.review_after",
            "frontmatter.pinned",
            "frontmatter.fact_key",
            "frontmatter.aliases",
            "frontmatter.tags",
            "frontmatter.supersedes",
            "frontmatter.sources",
            "frontmatter.sources[].type",
            "frontmatter.sources[].id",
            "frontmatter.sources[].observed_at",
        ],
    ),
    // `memory.rs` `MemoryCommands::Add`.
    (
        "responses",
        "post-memories",
        &["id", "dedup", "revision", "scope", "superseded"],
    ),
    // `memory.rs` `search_lines`.
    (
        "responses",
        "get-memories-search",
        &[
            "results",
            "results[].id",
            "results[].title",
            "results[].kind",
            "results[].scope",
            "results[].effective_status",
            "results[].origin",
            "results[].freshness",
            "results[].conflict",
            "results[].same_key_loser",
            "results[].sources_status",
            "results[].match",
            "results[].heading",
            "results[].line_range",
            "results[].excerpt",
            "partial",
            "index_state",
        ],
    ),
    // `memory.rs` `suggestions_text`, `suggestion_line`,
    // `suggestion_changes_line`, `suggestion_hidden`, `change_values`.
    (
        "responses",
        "get-memories-suggestions",
        &[
            "items",
            "items[].id",
            "items[].target",
            "items[].target.title",
            "items[].target.origin",
            "items[].target.excerpt",
            "items[].observed_at",
            "items[].created_at",
            "items[].origin",
            "items[].title",
            "items[].reason",
            "items[].older_count",
            "items[].knowledge",
            "items[].changes",
            "items[].changes[].field",
            "items[].changes[].from",
            "items[].changes[].to",
        ],
    ),
    // `memory.rs` `Confirmed::lines`; `edit_note`'s saved revision.
    (
        "responses",
        "post-memories-id-confirm",
        &["revision", "status", "dismissed"],
    ),
    ("responses", "post-memories-id-edit", &["revision"]),
    // `memory.rs` `undo_message`.
    ("responses", "post-memories-id-undo", &["restored"]),
    // `llm_keys.rs` `key_rows` (`llm keys list`), `current_key_order`.
    (
        "responses",
        "get-settings-llm",
        &[
            "providers",
            "providers.*.keys",
            "providers.*.keys[].id",
            "providers.*.keys[].masked_secret",
            "providers.*.keys[].priority",
            "providers.*.keys[].source",
            "providers.*.keys[].status",
            "providers.*.requires_key",
        ],
    ),
    // `llm_keys.rs` `keys_add`, `keys_validate`.
    (
        "responses",
        "post-settings-llm-validate",
        &[
            "valid",
            "format_error",
            "tier",
            "rate_limits",
            "models_available",
        ],
    ),
    // The REPL (`repl/commands`: `/model`, `/models`, `/keys`; `repl/mod.rs`
    // reads `model` at start).
    (
        "responses",
        "get-orchestrator-config",
        &["model", "fallback_models"],
    ),
    ("responses", "get-models", &["[].id", "[].provider"]),
    (
        "responses",
        "get-settings-llm-status",
        &["*", "*[].is_available", "*[].health"],
    ),
    // `chat_stream`'s frames (`handle_sse_event`, `confirmation_details`,
    // `inline_answer`, `skipped_attachment_lines`, `continued_from_line`).
    ("stream", "reasoning", &["text"]),
    ("stream", "delta", &["content"]),
    (
        "stream",
        "done",
        &[
            "content",
            "delegation",
            "attachments_skipped",
            "attachments_skipped[].id",
            "attachments_skipped[].reason",
            "continued_from",
            "continued_from.topic",
            "model",
            "tokens_in",
            "tokens_out",
            "duration_ms",
        ],
    ),
    ("stream", "error", &["message"]),
    (
        "stream",
        "confirmation_requested",
        &["tool_name", "preview", "tool_arguments", "request_id"],
    ),
    // `chat_stream` `poll_task_completion`, `task_outcome_text`.
    (
        "responses",
        "get-tasks-id",
        &[
            "task",
            "task.result_summary",
            "task.status",
            "task.created_at",
            "task.completed_at",
        ],
    ),
];

/// Keys the CLI reads that the daemon does not send — a finding of this
/// check (CLI-PROTOCOL; `tasks/pending-decisions.md` §1): kind, sample,
/// paths. Empty since *Rev 123*'s 123-6 (b): `agent list` reads the stored
/// rows' `skills_json` and `llm_config_json`, the keys `GET /v1/agents`
/// sends.
const READ_BUT_NOT_SENT: &[(&str, &str, &[&str])] = &[];

/// Keys the CLI reads that the daemon sends, but that no producer's fixture
/// fills, so no sample carries them (CONTRACT-SAMPLES-3): `memory doctor`'s
/// conflict, snapshot and refusal rows. Held the other way round: a fixture
/// that fills one moves it into [`READS`].
const READ_BUT_UNFILLED: &[(&str, &str, &[&str])] = &[(
    "cli",
    "get-memories-doctor",
    &[
        "partitions[].conflicts[].kind",
        "partitions[].conflicts[].note",
        "partitions[].conflicts[].target",
        "idless_project_snapshots[].partition_key",
        "idless_project_snapshots[].root",
        "refused[].scope",
        "refused[].owner_key",
        "refused[].component",
        "refused[].reason",
        "unreadable_snapshots[].path",
        "unreadable_snapshots[].reason",
    ],
)];

/// The `POST`s whose route reads no body, which the CLI sends
/// [`crate::client::empty_body`] (*Rev 142*'s item 12, (b)): the route's
/// sample name and the command. Each route's answer is held in [`holds`].
const EMPTY_BODY_POSTS: &[(&str, &str)] = &[
    ("post-extensions-kind-id-verb", "ext::verb"),
    ("post-models-refresh", "llm_status::llm_models --refresh"),
    ("post-sessions-id-activate", "chat::resume"),
];

/// Every key path a sample holds, spelled as the folder's check spells it.
fn key_paths(value: &serde_json::Value) -> BTreeSet<String> {
    fn walk(value: &serde_json::Value, path: &str, out: &mut BTreeSet<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, field) in fields {
                    let here = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    out.insert(here.clone());
                    walk(field, &here, out);
                }
            }
            serde_json::Value::Array(items) => {
                let here = format!("{path}[]");
                out.insert(here.clone());
                for item in items {
                    walk(item, &here, out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(value, "", &mut out);
    out
}

fn sample(kind: &str, name: &str) -> BTreeSet<String> {
    key_paths(&read_sample(kind, name))
}

#[test]
fn every_cli_sample_is_read_here_and_every_read_has_its_sample() {
    let dir = Path::new(ROOT).join("cli");
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .expect("the folder")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.starts_with('.'))
        .map(|name| name.trim_end_matches(".json").to_string())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = READS
        .iter()
        .filter(|(kind, _, _)| *kind == "cli")
        .map(|(_, name, _)| name.to_string())
        .collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

#[test]
fn every_key_the_cli_reads_is_in_its_sample() {
    let mut missing = Vec::new();
    for (kind, name, paths) in READS {
        let held = sample(kind, name);
        for path in *paths {
            if !held.contains(*path) {
                missing.push(format!("{kind}/{name}.json lacks {path}"));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn the_keys_read_but_not_sent_or_unfilled_are_still_absent() {
    for (kind, name, paths) in READ_BUT_NOT_SENT.iter().chain(READ_BUT_UNFILLED) {
        let held = sample(kind, name);
        for path in *paths {
            assert!(
                !held.contains(*path),
                "{kind}/{name}.json now carries {path}: drop it from READ_BUT_NOT_SENT or \
                 READ_BUT_UNFILLED and read it in READS"
            );
        }
    }
}

#[test]
fn every_empty_body_post_goes_to_a_route_that_reads_no_body() {
    assert_eq!(crate::client::empty_body(), serde_json::json!({}));
    for (name, by) in EMPTY_BODY_POSTS {
        let request = Path::new(ROOT)
            .join("requests")
            .join(format!("{name}.json"));
        assert!(
            !request.exists(),
            "{by}: {name}'s route now reads a body (requests/{name}.json): send it, with a \
             named body function held in contract_bodies"
        );
        let response = Path::new(ROOT)
            .join("responses")
            .join(format!("{name}.json"));
        assert!(
            response.exists(),
            "{by}: no responses/{name}.json, so the route's answer is unheld"
        );
    }
}

// ── The CLI's typed reads (124-2) ───────────────────────────────────────────

/// serde's own list of the wire names a struct reads (the daemon's
/// `contract::requests::fields`, the same introspection: a deserializer
/// that answers only the struct question).
mod introspect {
    use serde::de::{self, Deserializer, Visitor};

    #[derive(Debug)]
    pub(super) struct Stop(pub(super) Option<&'static [&'static str]>);

    impl std::fmt::Display for Stop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("introspection stop")
        }
    }

    impl std::error::Error for Stop {}

    impl de::Error for Stop {
        fn custom<T: std::fmt::Display>(_: T) -> Self {
            Stop(None)
        }
    }

    pub(super) struct Introspect;

    impl<'de> Deserializer<'de> for Introspect {
        type Error = Stop;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop(None))
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            fields: &'static [&'static str],
            _visitor: V,
        ) -> Result<V::Value, Stop> {
            Err(Stop(Some(fields)))
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map enum identifier ignored_any
        }
    }
}

/// The wire names `T` reads.
fn fields<T: serde::de::DeserializeOwned>() -> &'static [&'static str] {
    match T::deserialize(introspect::Introspect) {
        Err(introspect::Stop(Some(fields))) => fields,
        _ => panic!(
            "{} does not describe itself as a struct to serde",
            std::any::type_name::<T>()
        ),
    }
}

/// One typed read: the struct, the sample, the path the CLI reads it at,
/// and the names it reads that the daemon does not send there.
struct Hold {
    label: &'static str,
    /// `responses`, `cli`, `events` or `stream`.
    kind: &'static str,
    name: &'static str,
    /// `""` the body; `[]` an array's element; `.`-joined keys, each maybe
    /// ending `[]`; `*` a map's value.
    path: &'static str,
    read: fn(&serde_json::Value) -> Result<(), String>,
    fields: fn() -> &'static [&'static str],
    unsent: &'static [&'static str],
}

macro_rules! hold {
    ($t:ty, $kind:literal, $name:literal, $path:literal) => {
        hold!($t, $kind, $name, $path, [])
    };
    ($t:ty, $kind:literal, $name:literal, $path:literal, [$($unsent:literal),*]) => {
        Hold {
            label: stringify!($t),
            kind: $kind,
            name: $name,
            path: $path,
            read: |value| {
                serde_json::from_value::<$t>(value.clone())
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            fields: fields::<$t>,
            unsent: &[$($unsent),*],
        }
    };
}

/// Every typed read of the CLI's, at the code that reads it.
fn holds() -> Vec<Hold> {
    use crate::chat_stream::ChatSendResponse;
    use crate::commands::agents::{AgentConfigResponse, AgentDetail, AgentMetricsInfo, AgentRow};
    use crate::commands::chat::{ChosenProject, TranscriptMessage, TranscriptPage};
    use crate::commands::config_tui::TuiAgentConfigResponse;
    use crate::commands::confirmations::{ConfirmationFrame, PendingPrompt, PendingPrompts};
    use crate::commands::connector::ConnectorStatus;
    use crate::commands::ext::{
        DeclaredRow, ExtensionRow, InstallResponse, ManifestSummary, UninstallResponse,
        ValidateResponse,
    };
    use crate::commands::llm_status::{
        BackendEntry, CredentialEntry, DaemonStatusSnapshot, DailyUsageEntry, LlmSettingsSnapshot,
        LlmStatusInfo, ModelEntry, ProviderUsageEntry, SettingsProvider, UsageEntry,
    };
    use crate::commands::persona::{PersonaUserResponse, PersonaUserWritten};
    use crate::commands::sessions::{MeResponse, SessionItem, SessionsResponse};
    use crate::commands::skills::{DiagnosticRow, NotUsedRow, SkillRow, Triggers};
    use crate::commands::store::{
        DeleteResult, Kept, PlanEntry, ProjectView, PurgeCounts, Removed, RepointResult, RowCounts,
        WorkspaceView,
    };
    use crate::commands::tasks::{
        EventHistoryPage, EventLogEntry, TaskDetail, TaskInner, TaskItem, TaskTimeline,
        TimelineLane,
    };
    use crate::commands::tidy;
    use openalpaca_core::gateway::DelegationInfo;
    vec![
        // `chat_stream::send_chat_with_attachments`; `handle_sse_event`'s
        // `done.delegation`.
        hold!(ChatSendResponse, "responses", "post-chat", ""),
        hold!(DelegationInfo, "stream", "done", "delegation"),
        // `agents.rs` `list_agents`, `config_tui.rs` `fetch_agents_or_why`
        // (both through the row type).
        hold!(AgentRow, "cli", "get-agents", "[]"),
        // `agents.rs` `agent_status`.
        hold!(AgentDetail, "responses", "get-agents-id", ""),
        hold!(AgentRow, "responses", "get-agents-id", "agent"),
        // `agent status`'s metrics: no `total_cost_usd`, which the row never
        // sent, since *Rev 142*'s item 9 (b) dropped its "Total cost" line.
        hold!(AgentMetricsInfo, "responses", "get-agents-id", "metrics"),
        // `agents.rs` `agent_config`, `agent_set_with`; the TUI's editor.
        hold!(AgentConfigResponse, "responses", "get-agents-id-config", ""),
        hold!(
            TuiAgentConfigResponse,
            "responses",
            "get-agents-id-config",
            ""
        ),
        // `persona.rs` `edit_user`.
        hold!(PersonaUserResponse, "responses", "get-persona-user", ""),
        hold!(PersonaUserWritten, "responses", "put-persona-user", ""),
        // `skills.rs` `list`, `tidy.rs` `daemon_rows`.
        hold!(SkillRow, "responses", "get-skills", "[]"),
        hold!(Triggers, "responses", "get-skills", "[].triggers"),
        hold!(NotUsedRow, "responses", "get-skills", "[].not_used[]"),
        hold!(DiagnosticRow, "responses", "get-skills-diagnostics", "[]"),
        hold!(tidy::Row, "responses", "get-skills", "[]"),
        hold!(tidy::NotUsed, "responses", "get-skills", "[].not_used[]"),
        hold!(tidy::Row, "responses", "get-agent-templates", "[]"),
        // `tasks.rs` `list_tasks`, `task_status`, `task_events`.
        hold!(TaskItem, "responses", "get-tasks", "[]"),
        hold!(TaskDetail, "responses", "get-tasks-id", ""),
        hold!(TaskInner, "responses", "get-tasks-id", "task"),
        hold!(TaskTimeline, "responses", "get-tasks-id-timeline", ""),
        hold!(
            TimelineLane,
            "responses",
            "get-tasks-id-timeline",
            "lanes[]"
        ),
        hold!(EventHistoryPage, "responses", "get-events-history", ""),
        hold!(EventLogEntry, "responses", "get-events-history", "events[]"),
        // `llm_status.rs`.
        // The three aliases are the spellings of a daemon before
        // `ModelEntry`'s; this one sends the names they stand beside.
        hold!(
            ModelEntry,
            "responses",
            "get-models",
            "[]",
            ["model_id", "input_price_per_1m", "output_price_per_1m"]
        ),
        // `llm models --refresh`'s answer, to an empty-body post.
        hold!(
            ModelEntry,
            "responses",
            "post-models-refresh",
            "[]",
            ["model_id", "input_price_per_1m", "output_price_per_1m"]
        ),
        hold!(LlmSettingsSnapshot, "responses", "get-settings-llm", ""),
        hold!(
            SettingsProvider,
            "responses",
            "get-settings-llm",
            "providers.*"
        ),
        hold!(DaemonStatusSnapshot, "responses", "get-status", ""),
        hold!(LlmStatusInfo, "responses", "get-status", "llm"),
        hold!(UsageEntry, "responses", "get-llm-usage", "[]"),
        hold!(DailyUsageEntry, "responses", "get-llm-usage-daily", "[]"),
        hold!(
            CredentialEntry,
            "responses",
            "get-settings-llm-credentials",
            "[]"
        ),
        hold!(
            BackendEntry,
            "responses",
            "get-settings-llm-cli-backends",
            "[]"
        ),
        hold!(
            ProviderUsageEntry,
            "responses",
            "get-settings-llm-providers-usage",
            "[]"
        ),
        // `confirmations.rs` `list` and `watch`.
        hold!(PendingPrompts, "responses", "get-chat-confirmations", ""),
        hold!(
            PendingPrompt,
            "responses",
            "get-chat-confirmations",
            "confirmations[]"
        ),
        hold!(
            ConfirmationFrame,
            "events",
            "tool_confirmation_requested",
            ""
        ),
        // `store.rs`.
        hold!(ProjectView, "responses", "get-workspaces", "[]"),
        hold!(WorkspaceView, "responses", "get-workspaces-path", ""),
        hold!(RowCounts, "responses", "get-workspaces-path", "rows"),
        hold!(RepointResult, "responses", "patch-workspaces", ""),
        hold!(DeleteResult, "responses", "post-workspaces-purge", ""),
        hold!(PlanEntry, "responses", "post-workspaces-purge", "entries[]"),
        hold!(PurgeCounts, "responses", "post-workspaces-purge", "counts"),
        hold!(Kept, "responses", "post-workspaces-purge", "kept"),
        hold!(Removed, "responses", "post-workspaces-purge", "removed"),
        // `sessions.rs`, `chat.rs` (`named_session` reads `GET
        // /v1/sessions/{id}`, `resume` its `activate`'s answer).
        hold!(SessionsResponse, "responses", "get-sessions", ""),
        hold!(SessionItem, "responses", "get-sessions", "sessions[]"),
        hold!(SessionItem, "cli", "get-sessions-id", ""),
        hold!(SessionItem, "responses", "post-sessions-id-activate", ""),
        hold!(MeResponse, "cli", "get-me", ""),
        hold!(ChosenProject, "responses", "post-workspaces", ""),
        hold!(TranscriptPage, "responses", "get-sessions-id-messages", ""),
        hold!(
            TranscriptMessage,
            "responses",
            "get-sessions-id-messages",
            "messages[]"
        ),
        // `ext.rs`.
        // `warnings` is per-call: only a verb that produced one sends it.
        hold!(
            ExtensionRow,
            "responses",
            "get-extensions",
            "[]",
            ["warnings"]
        ),
        // `ext::verb`'s answer, to an empty-body post.
        hold!(
            ExtensionRow,
            "responses",
            "post-extensions-kind-id-verb",
            "",
            ["warnings"]
        ),
        hold!(
            DeclaredRow,
            "responses",
            "post-extensions-plugin",
            "extension.declared"
        ),
        hold!(InstallResponse, "responses", "post-extensions-plugin", ""),
        // An MCP server's install answers `{extension, manifest}` alone.
        hold!(
            InstallResponse,
            "responses",
            "post-extensions-mcp",
            "",
            ["added_capabilities", "consent_reset"]
        ),
        hold!(
            ManifestSummary,
            "responses",
            "post-extensions-plugin",
            "manifest"
        ),
        hold!(
            ValidateResponse,
            "responses",
            "post-extensions-plugin-validate",
            ""
        ),
        hold!(
            UninstallResponse,
            "responses",
            "delete-extensions-kind-id",
            ""
        ),
        // `connector.rs` `list`.
        hold!(ConnectorStatus, "responses", "get-connectors", "[]"),
    ]
}

/// The value at `path` in `value`, or why there is none.
fn at<'a>(value: &'a serde_json::Value, path: &str) -> Result<&'a serde_json::Value, String> {
    let mut here = value;
    if path.is_empty() {
        return Ok(here);
    }
    for segment in path.split('.') {
        let (key, mut arrays) = match segment.find("[]") {
            Some(i) => (&segment[..i], segment[i..].matches("[]").count()),
            None => (segment, 0),
        };
        if key == "*" {
            here = here
                .as_object()
                .and_then(|o| o.values().next())
                .ok_or_else(|| format!("{path}: no map value at *"))?;
        } else if !key.is_empty() {
            here = here
                .get(key)
                .ok_or_else(|| format!("{path}: no key {key}"))?;
        }
        while arrays > 0 {
            here = here
                .as_array()
                .and_then(|a| a.first())
                .ok_or_else(|| format!("{path}: no element under {key}"))?;
            arrays -= 1;
        }
    }
    Ok(here)
}

fn read_sample(kind: &str, name: &str) -> serde_json::Value {
    let path = Path::new(ROOT).join(kind).join(format!("{name}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

#[test]
fn every_typed_read_reads_its_sample_and_every_name_it_reads_is_sent() {
    let mut problems = Vec::new();
    for hold in holds() {
        let label = format!(
            "{} at {}/{}:{:?}",
            hold.label, hold.kind, hold.name, hold.path
        );
        let sample = read_sample(hold.kind, hold.name);
        let value = match at(&sample, hold.path) {
            Ok(value) => value,
            Err(e) => {
                problems.push(format!("{label}: {e}"));
                continue;
            }
        };
        if let Err(e) = (hold.read)(value) {
            problems.push(format!("{label}: the sample does not read ({e})"));
        }
        let Some(keys) = value.as_object() else {
            problems.push(format!("{label}: not an object in the sample"));
            continue;
        };
        for name in (hold.fields)() {
            let sent = keys.contains_key(*name);
            let pinned = hold.unsent.contains(name);
            if !sent && !pinned {
                problems.push(format!("{label}: reads {name}, which the sample lacks"));
            }
            if sent && pinned {
                problems.push(format!(
                    "{label}: {name} is now sent: drop it from the hold's unsent list"
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_struct_field_the_sample_lacks_fails_its_hold() {
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Renamed {
        default_lane: String,
    }
    let hold = hold!(Renamed, "cli", "get-me", "");
    let sample = read_sample(hold.kind, hold.name);
    assert!(
        (hold.read)(&sample).is_err(),
        "a renamed required field fails the read"
    );
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Optional {
        #[serde(default)]
        default_lane: Option<String>,
    }
    let hold = hold!(Optional, "cli", "get-me", "");
    let sample = read_sample(hold.kind, hold.name);
    assert!(
        (hold.read)(&sample).is_ok(),
        "an optional field reads anything"
    );
    assert!(
        !(hold.fields)()
            .iter()
            .all(|name| sample.as_object().unwrap().contains_key(*name)),
        "the name check is what fails it"
    );
}
