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
//! The commands that read a body as `serde_json::Value` (`memory`'s, the
//! key list's, the REPL's) are not typed reads and are not held here.

use std::collections::BTreeSet;
use std::path::Path;

/// The folder, as the daemon's `contract::DIR` names it.
const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract/cli"
);

/// Each sample and the key paths the CLI reads in it.
const READS: &[(&str, &[&str])] = &[
    // `sessions.rs` `MeResponse`; `tasks.rs`'s lane.
    ("get-me", &["default_lane_key"]),
    // `agents.rs` `AgentItem`, `config_tui.rs` `TuiAgentItem`.
    (
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
    ("post-agents", &["agent_id"]),
    ("post-agents-from-toml", &["agent_id"]),
    // `agents.rs` `remove_agent`, `config_tui.rs` `remove_agent_tui`.
    ("delete-agents-id", &["status"]),
    // `agents.rs` `agent_set_with`, `config_tui.rs` `interactive_edit_agent_config`.
    ("put-agents-id-config", &["config_version"]),
    // `memory.rs` `print_doctor`.
    (
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
            "partitions[].duplicates",
        ],
    ),
    // `memory.rs` `MemoryCommands::Sync`.
    (
        "post-memories-sync",
        &["scanned", "changed", "index_state", "changes"],
    ),
    // `memory.rs` `print_timeline`.
    (
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
    // `memory.rs` `forget_notes`.
    (
        "post-memories-forget",
        &["results", "results[].id", "results[].status"],
    ),
];

/// Keys the CLI reads that the daemon does not send — a finding of this
/// check (CLI-PROTOCOL; `tasks/pending-decisions.md` §1). Empty since
/// *Rev 123*'s 123-6 (b): `agent list` reads the stored rows'
/// `skills_json` and `llm_config_json`, the keys `GET /v1/agents` sends.
const READ_BUT_NOT_SENT: &[(&str, &[&str])] = &[];

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

fn sample(name: &str) -> BTreeSet<String> {
    let path = Path::new(DIR).join(format!("{name}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    key_paths(&value)
}

#[test]
fn every_cli_sample_is_read_here_and_every_read_has_its_sample() {
    let mut on_disk: Vec<String> = std::fs::read_dir(DIR)
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
    let mut listed: Vec<String> = READS.iter().map(|(name, _)| name.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

#[test]
fn every_key_the_cli_reads_is_in_its_sample() {
    let mut missing = Vec::new();
    for (name, paths) in READS {
        let held = sample(name);
        for path in *paths {
            if !held.contains(*path) {
                missing.push(format!("cli/{name}.json lacks {path}"));
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn the_keys_read_but_not_sent_are_still_not_sent() {
    for (name, paths) in READ_BUT_NOT_SENT {
        let held = sample(name);
        for path in *paths {
            assert!(
                !held.contains(*path),
                "cli/{name}.json now carries {path}: the finding is fixed; drop it from \
                 READ_BUT_NOT_SENT and read it in READS"
            );
        }
    }
}

// ── The CLI's typed reads (124-2) ───────────────────────────────────────────

/// The contract folder, whose kinds the holds name.
const ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract"
);

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
    /// `responses`, `cli` or `events`.
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
    vec![
        // `chat_stream::send_chat_with_attachments`.
        hold!(ChatSendResponse, "responses", "post-chat", ""),
        // `agents.rs` `list_agents`, `config_tui.rs` `fetch_agents_or_why`
        // (both through the row type).
        hold!(AgentRow, "cli", "get-agents", "[]"),
        // `agents.rs` `agent_status`.
        hold!(AgentDetail, "responses", "get-agents-id", ""),
        hold!(AgentRow, "responses", "get-agents-id", "agent"),
        // A finding: the metrics row has no `total_cost_usd`, so `agent
        // status` never prints its "Total cost" line (an owner item).
        hold!(
            AgentMetricsInfo,
            "responses",
            "get-agents-id",
            "metrics",
            ["total_cost_usd"]
        ),
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
        // `sessions.rs` (`GET /v1/sessions/{id}` has no sample of its own;
        // its row is the list's), `chat.rs`.
        hold!(SessionsResponse, "responses", "get-sessions", ""),
        hold!(SessionItem, "responses", "get-sessions", "sessions[]"),
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
