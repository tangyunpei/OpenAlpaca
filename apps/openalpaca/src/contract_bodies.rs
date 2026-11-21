//! The request bodies the CLI sends, checked from this side (*Rev 123*'s
//! 123-4, (b), with CONTRACT-SAMPLES-2): each body comes from a named
//! function — the one its command calls — and this test calls each with
//! every input it can take and holds what it builds to the request sample
//! the daemon's tests wrote from the route's own type
//! (`apps/openalpaca-gui/src/lib/api/contract/requests/`), as the GUI's
//! `contract-requests.test.ts` holds the app's wrappers. A key the daemon
//! renames fails its bump check there; once `PROTOCOL` moved and the folder
//! was rewritten, the CLI still sending the old name fails here. A key the
//! function stops sending fails the case's own list of what it sends.
//!
//! Where the route's type is one the CLI can name (`openalpaca_llm`'s
//! settings types, core's `AgentConfigFile`), the body is also read through
//! it, so a key the type requires and the body lacks fails too. Every
//! request sample is either sent here or listed in [`NOT_SENT`].
//!
//! AGENT-CREATE-CLI: the interactive creations' body is also pinned whole
//! (`commands/agents/create-bodies.json`), the file the daemon's
//! `routes::agents_cli_create` test posts through `POST /v1/agents`'s own
//! request type and service, so the two tests together prove the route
//! takes what the CLI sends.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};

/// The folder, as the daemon's `contract::DIR` names it.
const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract/requests"
);

/// The interactive creations' bodies, pinned for the daemon's test.
pub(crate) const CREATE_BODIES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/commands/agents/create-bodies.json"
);

fn sample(name: &str) -> Value {
    let path = Path::new(DIR).join(format!("{name}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// Every key path `body` holds that `sample` does not; a `null` in the
/// sample is a field whose inside it does not describe, not looked inside
/// (the GUI's `unknownKeys`).
fn unknown_keys(body: &Value, sample: &Value, path: &str, out: &mut Vec<String>) {
    match (body, sample) {
        (_, Value::Null) => {}
        (Value::Array(items), Value::Array(element)) if let Some(element) = element.first() => {
            for item in items {
                unknown_keys(item, element, &format!("{path}[]"), out);
            }
        }
        (Value::Object(fields), Value::Object(known)) => {
            for (key, value) in fields {
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match known.get(key) {
                    None => out.push(here),
                    Some(inner) => unknown_keys(value, inner, &here, out),
                }
            }
        }
        _ => {}
    }
}

/// One body the CLI sends: the sample it is held to, the body as its
/// function builds it, and the top-level keys that function sends.
struct Case {
    sample: &'static str,
    /// Which function, for the failure's sentence.
    built_by: &'static str,
    body: Value,
    sends: &'static [&'static str],
}

fn case(
    sample: &'static str,
    built_by: &'static str,
    body: Value,
    sends: &'static [&'static str],
) -> Case {
    Case {
        sample,
        built_by,
        body,
        sends,
    }
}

/// Keys a body sends that its route's type does not read — findings of
/// this check (CONTRACT-SAMPLES-2; `tasks/pending-decisions.md` §1, owner
/// items): `llm strategy` sends a `strategy` that
/// `UpdateOrchestratorRequest` does not have. Pinned as unread, so the
/// fix, on either side, updates this list.
const SENT_UNREAD: &[(&str, &str)] = &[("llm_status::strategy_body", "strategy")];

/// Bodies the route's type refuses — a finding of this check. Empty since
/// *Rev 142*'s item 7, (b): `llm keys set-primary` and `llm keys reorder`
/// send `{provider, key_order, primary_key_id}`, as `ReorderKeysRequest`
/// reads it.
const REFUSED: &[&str] = &[];

/// The request samples no CLI command sends: the routes only the app (or
/// nothing yet) writes.
const NOT_SENT: &[&str] = &[
    "patch-sessions-id",
    "post-agent-templates",
    "post-command",
    "post-connectors-id-config",
    "post-lanes-key-followups",
    "post-sessions",
    "post-tasks-id-rerun",
    "post-tasks-id-steer",
    "put-agent-templates-id",
    "put-artifacts-id-pin",
    "put-chat-messages-id-feedback",
    "put-connectors-id-settings",
    "put-daemon-config-providers-web-search",
    "put-settings-llm-keys-priority",
    "put-settings-llm-providers-provider-enabled",
];

fn cases() -> Vec<Case> {
    use crate::chat_stream::ChatTarget;
    use crate::commands::{
        agents, chat, confirmations, connector, ext, llm_keys, llm_status, memory, persona, plugin,
        store, tasks,
    };
    let skills = vec!["research".to_string()];
    let note =
        openalpaca_storage::memory_files::NoteId::parse("00000021-0000-4000-8000-000000000001")
            .expect("a note id");
    let mcp = ext::ExtMcpCommands::Add {
        name: "github".to_string(),
        transport: "stdio".to_string(),
        command: Some("npx".to_string()),
        args: vec!["-y".to_string()],
        envs: vec!["A=b".to_string()],
        envs_from: vec!["TOKEN=GITHUB_TOKEN".to_string()],
        cwd: Some("/tmp/x".to_string()),
        url: Some("http://127.0.0.1:1".to_string()),
        bearer_env: Some("B".to_string()),
        api_key_header: Some("X-Key".to_string()),
        api_key_env: Some("K".to_string()),
        headers_from: vec!["X-Key=KEY".to_string()],
        connect_timeout_secs: Some(1),
        request_timeout_secs: Some(1),
        disabled: true,
    };
    vec![
        case(
            "post-chat",
            "ChatTarget::body",
            ChatTarget::for_project("/r".to_string())
                .resuming("s".to_string())
                .unattended()
                .body("c", &[json!({"file_id": "f", "caption": "c"})]),
            &["attachments", "content", "session_id", "unattended"],
        ),
        case(
            "post-chat-confirmations-id",
            "confirmations::answer_body",
            confirmations::answer_body(true, true),
            &["approval_scope", "approved"],
        ),
        case(
            "post-agents",
            "agents::create_agent_body",
            agents::create_agent_body("reader", "Reader", "Reads", "m", "You read.", &skills, 2.5),
            &["config"],
        ),
        case(
            "post-agents-from-toml",
            "agents::from_toml_body",
            agents::from_toml_body("[agent]\n"),
            &["toml_content"],
        ),
        case(
            "post-agents-id-action",
            "agents::action_body",
            agents::action_body("pause"),
            &["action"],
        ),
        case(
            "put-agents-id-config",
            "agents::config_body",
            agents::config_body(
                agents::create_agent_body(
                    "reader",
                    "Reader",
                    "Reads",
                    "m",
                    "You read.",
                    &skills,
                    2.5,
                )["config"]
                    .clone(),
                3,
            ),
            &["config", "config_version"],
        ),
        case(
            "post-tasks",
            "tasks::create_body",
            tasks::create_body("Ship it", 1, true, "user-1:cli"),
            &[
                "created_by",
                "priority",
                "source_lane",
                "title",
                "unattended",
            ],
        ),
        case(
            "post-tasks-id-action",
            "tasks::action_body",
            tasks::action_body("cancel"),
            &["action"],
        ),
        case(
            "post-connectors-id-action",
            "connector::action_body",
            connector::action_body("enable"),
            &["action"],
        ),
        case(
            "put-persona-user",
            "persona::PersonaUserBody",
            serde_json::to_value(persona::PersonaUserBody { content: "c" }).unwrap(),
            &["content"],
        ),
        case(
            "post-extensions-plugin",
            "ext::source_body",
            ext::source_body("/tmp/p"),
            &["path", "source"],
        ),
        case(
            "post-extensions-plugin-validate",
            "ext::source_body",
            ext::source_body("/tmp/p"),
            &["path", "source"],
        ),
        case(
            "put-extensions-plugin-id",
            "ext::source_body",
            ext::source_body("/tmp/p"),
            &["path", "source"],
        ),
        case(
            "post-extensions-mcp",
            "ext::mcp_add_body",
            ext::mcp_add_body(mcp).expect("an add body"),
            &[
                "api_key_env",
                "api_key_header",
                "args",
                "bearer_env",
                "command",
                "connect_timeout_secs",
                "cwd",
                "enabled",
                "env",
                "env_from",
                "extra_headers_from",
                "name",
                "request_timeout_secs",
                "transport",
                "url",
            ],
        ),
        case(
            "post-extensions-kind-id-config",
            "plugin::config_body",
            plugin::config_body("k", json!("v")),
            &["key", "value"],
        ),
        case(
            "post-workspaces",
            "chat::project_body",
            chat::project_body("/r"),
            &["path"],
        ),
        case(
            "patch-workspaces",
            "store::RebaseRequest",
            serde_json::to_value(store::RebaseRequest {
                old_path: "/a",
                new_path: "/b",
            })
            .unwrap(),
            &["new_path", "old_path"],
        ),
        case(
            "post-workspaces-purge",
            "store::PurgeRequest",
            serde_json::to_value(store::PurgeRequest {
                path: "/r",
                dry_run: true,
            })
            .unwrap(),
            &["dry_run", "path"],
        ),
        case(
            "post-settings-llm-validate",
            "llm_keys::validate_body",
            llm_keys::validate_body("openai", "not-a-secret"),
            &["provider", "secret"],
        ),
        case(
            "put-settings-llm",
            "llm_keys::add_key_body",
            llm_keys::add_key_body(
                "openai",
                "openai_1",
                "not-a-secret",
                "manual",
                "n",
                "primary",
            ),
            &["key", "provider"],
        ),
        case(
            "put-settings-llm-keys-reorder",
            "llm_keys::set_primary_body",
            llm_keys::set_primary_body(
                "openai",
                "k",
                &json!({ "providers": { "openai": { "keys": [{ "id": "k" }] } } }),
            ),
            &["key_order", "primary_key_id", "provider"],
        ),
        case(
            "put-settings-llm-keys-reorder",
            "llm_keys::reorder_body",
            llm_keys::reorder_body("openai", &["k".to_string()]),
            &["key_order", "primary_key_id", "provider"],
        ),
        case(
            "put-orchestrator-config",
            "llm_status::strategy_body",
            llm_status::strategy_body(
                &json!({"model": "m", "fallback_models": ["f"]}),
                "openai",
                "round_robin",
            ),
            &["fallback_models", "model", "strategy"],
        ),
        case(
            "post-memories",
            "memory::create_body",
            memory::create_body(memory::NewNote {
                title: "t",
                body: "b",
                kind: "fact",
                aliases: &["a".to_string()],
                tags: &["x".to_string()],
                scope: Some("global".to_string()),
                fact_key: Some("k".to_string()),
                supersedes: Some((note.to_string(), json!("r"))),
            }),
            &[
                "aliases",
                "body",
                "fact_key",
                "kind",
                "scope",
                "supersedes",
                "tags",
                "title",
            ],
        ),
        case(
            "post-memories-id",
            "memory::confirm_body",
            memory::confirm_body(Some("r".to_string())),
            &["action", "target_revision"],
        ),
        case(
            "post-memories-id",
            "memory::edit_body",
            memory::edit_body(json!({
                "title": "t", "body": "b", "aliases": ["a"], "tags": ["x"], "fact_key": "k",
                "valid_from": null, "valid_to": null, "review_after": null
            })),
            &["action", "patch"],
        ),
        case(
            "post-memories-forget",
            "memory::forget_body",
            memory::forget_body(&[(note, "r".to_string())]),
            &["targets"],
        ),
    ]
}

/// Whether the route's own type, where the CLI can name it, reads `body`.
fn read_by_route_type(case: &Case) -> Option<Result<(), String>> {
    use openalpaca_llm::config::settings_types::{
        AddKeyRequest, ReorderKeysRequest, UpdateOrchestratorRequest, ValidateKeyRequest,
    };
    fn read<T: serde::de::DeserializeOwned>(body: &Value) -> Result<(), String> {
        serde_json::from_value::<T>(body.clone())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    Some(match case.sample {
        "put-settings-llm" => read::<AddKeyRequest>(&case.body),
        "put-settings-llm-keys-reorder" => read::<ReorderKeysRequest>(&case.body),
        "put-orchestrator-config" => read::<UpdateOrchestratorRequest>(&case.body),
        "post-settings-llm-validate" => read::<ValidateKeyRequest>(&case.body),
        "post-agents" | "put-agents-id-config" => {
            read::<openalpaca_core::agent::AgentConfigFile>(&case.body["config"])
        }
        _ => return None,
    })
}

#[test]
fn every_body_the_cli_sends_holds_only_keys_its_route_reads() {
    let mut problems = Vec::new();
    for case in cases() {
        let held = sample(case.sample);
        let mut unknown = Vec::new();
        unknown_keys(&case.body, &held, "", &mut unknown);
        let pinned: BTreeSet<&str> = SENT_UNREAD
            .iter()
            .filter(|(by, _)| *by == case.built_by)
            .map(|(_, key)| *key)
            .collect();
        for key in &unknown {
            if !pinned.contains(key.as_str()) {
                problems.push(format!(
                    "{}: sends {key}, which requests/{}.json does not hold",
                    case.built_by, case.sample
                ));
            }
        }
        for key in pinned {
            if !unknown.iter().any(|k| k == key) {
                problems.push(format!(
                    "{}: {key} is now read (or no longer sent): drop it from SENT_UNREAD",
                    case.built_by
                ));
            }
        }
        let sent: Vec<&str> = case
            .body
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let mut expected: Vec<&str> = case.sends.to_vec();
        expected.sort_unstable();
        if sent != expected {
            problems.push(format!(
                "{}: sends {sent:?}, its case lists {expected:?} — a key dropped or renamed",
                case.built_by
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_body_the_route_type_can_read_is_read_by_it() {
    let mut problems = Vec::new();
    for case in cases() {
        let Some(read) = read_by_route_type(&case) else {
            continue;
        };
        let refused = REFUSED.contains(&case.built_by);
        match (read, refused) {
            (Ok(()), false) | (Err(_), true) => {}
            (Err(e), false) => problems.push(format!(
                "{}: the route's type refuses it ({e})",
                case.built_by
            )),
            (Ok(()), true) => problems.push(format!(
                "{}: the route's type now reads it: drop it from REFUSED",
                case.built_by
            )),
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_request_sample_is_sent_here_or_listed_as_not_sent() {
    let on_disk: BTreeSet<String> = std::fs::read_dir(DIR)
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
    let sent: BTreeSet<String> = cases().iter().map(|c| c.sample.to_string()).collect();
    let not_sent: BTreeSet<String> = NOT_SENT.iter().map(|s| s.to_string()).collect();
    assert!(sent.is_disjoint(&not_sent), "a sample both sent and not");
    let listed: BTreeSet<String> = sent.union(&not_sent).cloned().collect();
    assert_eq!(on_disk, listed);
}

/// AGENT-CREATE-CLI: both interactive creations — `agent create` and the
/// `openalpaca config` screen — send [`agents::create_agent_body`]'s body,
/// pinned whole in the file the daemon's test posts through the route.
#[test]
fn the_interactive_creations_body_is_the_one_the_daemon_test_posts() {
    use crate::commands::agents;
    let skills = vec!["research".to_string(), "code_review".to_string()];
    let built = json!([
        agents::create_agent_body(
            &agents::agent_id_for("Code Reviewer"),
            "Code Reviewer",
            "Reviews code",
            "model-a",
            "You review.",
            &skills,
            2.5
        ),
        agents::create_agent_body(
            &agents::agent_id_for("Reader"),
            "Reader",
            "",
            "model-a",
            "",
            &[],
            0.0
        ),
    ]);
    let pinned: Value =
        serde_json::from_slice(&std::fs::read(CREATE_BODIES).expect("the pinned bodies"))
            .expect("JSON");
    assert_eq!(
        built, pinned,
        "the interactive creation's body moved: rewrite {CREATE_BODIES} with it, and run the \
         daemon's routes::agents_cli_create test"
    );
    assert_eq!(built[0]["config"]["agent"]["id"], "code_reviewer");
    assert_eq!(
        built[0]["config"]["capabilities"]["assigned"],
        json!(skills)
    );
    assert_eq!(built[0]["config"]["preset"]["persona"], "You review.");
    assert_eq!(built[0]["config"]["llm"]["model"], "model-a");
    assert_eq!(built[0]["config"]["constraints"]["max_cost_per_task"], 2.5);
    assert!(built[1]["config"]["constraints"].is_null());
}

#[test]
fn an_agent_id_is_the_name_in_lower_case_with_runs_of_other_characters_one_underscore() {
    use crate::commands::agents::agent_id_for;
    assert_eq!(agent_id_for("Code Reviewer"), "code_reviewer");
    assert_eq!(agent_id_for("  --A.b  c!! "), "a_b_c");
    assert_eq!(agent_id_for("日本"), "agent");
}
