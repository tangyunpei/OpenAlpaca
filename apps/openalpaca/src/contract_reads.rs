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
//! `current_revision`, on a conflict) or under a nested value the sample
//! leaves empty is not listed: CONTRACT-SAMPLES-2 fills those shapes.

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
        &["[].id", "[].name", "[].status", "[].description"],
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
/// check (CLI-PROTOCOL; `tasks/pending-decisions.md` §1): `GET /v1/agents`
/// answers the stored rows, `skills_json` and `llm_config_json`, so
/// `agent list`'s Model and Skills columns always print `-`. Pinned as
/// absent, so the fix, on either side, updates this list.
const READ_BUT_NOT_SENT: &[(&str, &[&str])] = &[("get-agents", &["[].model", "[].skills"])];

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
