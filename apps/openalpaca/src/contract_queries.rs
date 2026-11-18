//! The query strings the CLI sends, checked from this side (*Rev 142*'s
//! item 10, (b), with CONTRACT-SAMPLES-3): each comes from a named
//! function — the one its command calls — and this test calls each with
//! every input it can take and holds the keys it builds to the query sample
//! the daemon's tests wrote from the route's own `Query<T>` type
//! (`apps/openalpaca-gui/src/lib/api/contract/queries/`), as
//! `contract_bodies` holds the bodies and the GUI's
//! `contract-requests.test.ts` the app's queries. A query key the daemon
//! renames fails its bump check there; once `PROTOCOL` moved and the folder
//! was rewritten, the CLI still sending the old name fails here. A key the
//! function stops sending fails the case's own list of what it sends, and a
//! path that leaves its route fails the route the case names.
//!
//! Every query sample is either sent here or listed in [`NOT_SENT`]. The two
//! routes that read their query into a map, not a type — the events socket
//! and the chat stream, each only its `?token=` — have no sample (the
//! daemon's `request_samples` says so); [`TOKEN_ONLY`] holds that they
//! still have none.

use std::collections::BTreeSet;
use std::path::Path;

/// The folder, as the daemon's `contract::DIR` names it.
const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract/queries"
);

/// The query samples no CLI command sends: the routes only the app asks.
const NOT_SENT: &[&str] = &[
    "delete-chat-history",
    "get-agent-templates",
    "get-artifacts",
    "get-artifacts-id-content",
    "get-artifacts-id-diff",
    "get-artifacts-id-versions-n-content",
    "get-chat-history",
    "get-files-id-content",
    "get-memories-recently-learned",
    "get-memories-recently-used",
    "get-orchestrator-decisions",
    "get-orchestrator-latency",
    "get-orchestrator-latency-aggregate",
    "get-sessions-id-events",
    "get-usage-summary",
];

/// The routes whose only query is `?token=`, read into a map: the CLI's
/// `tail`, `confirmations watch` and the chat stream ask them, and the
/// daemon samples neither.
const TOKEN_ONLY: &[&str] = &["get-events", "get-chat-stream-stream_id"];

/// One path the CLI asks: the route it is held to (`{name}` a segment), the
/// function that built it, the path, and the query keys it sends.
struct Case {
    method: &'static str,
    route: &'static str,
    built_by: &'static str,
    path: String,
    sends: &'static [&'static str],
}

fn case(
    method: &'static str,
    route: &'static str,
    built_by: &'static str,
    path: String,
    sends: &'static [&'static str],
) -> Case {
    Case {
        method,
        route,
        built_by,
        path,
        sends,
    }
}

/// A route's sample name, as the daemon names it: the method, then each
/// path segment after `/v1`, a `{name}` as its name.
fn sample_name(method: &str, route: &str) -> String {
    let segments: Vec<&str> = route
        .trim_start_matches("/v1/")
        .split('/')
        .map(|s| s.trim_start_matches('{').trim_end_matches('}'))
        .collect();
    format!("{method}-{}", segments.join("-"))
}

/// Whether `path` (no query) is `route`, a `{name}` matching one non-empty
/// segment.
fn on_route(path: &str, route: &str) -> bool {
    let path: Vec<&str> = path.split('/').collect();
    let route: Vec<&str> = route.split('/').collect();
    path.len() == route.len()
        && path.iter().zip(&route).all(|(p, r)| {
            if r.starts_with('{') && r.ends_with('}') {
                !p.is_empty()
            } else {
                p == r
            }
        })
}

/// The path before its `?`, and each key after it, in order.
fn split(path: &str) -> (&str, Vec<&str>) {
    match path.split_once('?') {
        None => (path, Vec::new()),
        Some((path, query)) => (
            path,
            query
                .split('&')
                .map(|pair| pair.split_once('=').map_or(pair, |(key, _)| key))
                .collect(),
        ),
    }
}

fn sample_keys(name: &str) -> BTreeSet<String> {
    let path = Path::new(DIR).join(format!("{name}.json"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
    value
        .as_object()
        .unwrap_or_else(|| panic!("{} is not an object", path.display()))
        .keys()
        .cloned()
        .collect()
}

fn cases() -> Vec<Case> {
    use crate::commands::memory::{self, Around, ListFilters, SearchFilters};
    use crate::commands::{agents, chat, ext, llm_status, sessions, store, tasks};
    let note =
        openalpaca_storage::memory_files::NoteId::parse("00000021-0000-4000-8000-000000000001")
            .expect("a note id");
    vec![
        case(
            "get",
            "/v1/agents",
            "agents::list_path",
            agents::list_path(Some("active")),
            &["limit", "status"],
        ),
        case(
            "get",
            "/v1/agents",
            "agents::list_path",
            agents::list_path(None),
            &["limit"],
        ),
        case(
            "get",
            "/v1/tasks",
            "tasks::list_path",
            tasks::list_path(Some("running"), 20),
            &["limit", "status"],
        ),
        case(
            "get",
            "/v1/events/history",
            "tasks::log_path",
            tasks::log_path("task-1", 50),
            &["task_id", "limit"],
        ),
        case(
            "get",
            "/v1/llm/usage/daily",
            "llm_status::usage_daily_path",
            llm_status::usage_daily_path(Some("a"), Some("2026-11-18")),
            &["limit", "agent_id", "date"],
        ),
        case(
            "get",
            "/v1/llm/usage",
            "llm_status::usage_path",
            llm_status::usage_path(Some("a"), Some("k")),
            &["limit", "agent_id", "key_id"],
        ),
        case(
            "get",
            "/v1/extensions",
            "ext::list_path",
            ext::list_path(true).to_string(),
            &["include_orphaned"],
        ),
        case(
            "get",
            "/v1/extensions",
            "ext::list_path",
            ext::list_path(false).to_string(),
            &[],
        ),
        case(
            "delete",
            "/v1/extensions/{kind}/{id}",
            "ext::uninstall_path",
            ext::uninstall_path("plugin", "notion", true),
            &["uninstall", "keep_data"],
        ),
        case(
            "get",
            "/v1/sessions",
            "sessions::sessions_query",
            sessions::sessions_query(Some("gui"), Some("/r"), 50),
            &["limit", "source", "workspace_id"],
        ),
        case(
            "get",
            "/v1/sessions/{id}/messages",
            "chat::transcript_page_path",
            chat::transcript_page_path("s", 8, 0),
            &["limit", "offset"],
        ),
        case(
            "get",
            "/v1/workspaces",
            "store::workspace_query",
            store::workspace_query("/r"),
            &["path"],
        ),
        case(
            "get",
            "/v1/memories",
            "memory::list_path",
            memory::list_path(&ListFilters {
                limit: 20,
                kind: Some("fact"),
                status: Some("active"),
                scope: Some("global"),
                history: true,
                candidates: true,
            }),
            &[
                "limit",
                "kind",
                "status",
                "scope",
                "include_history",
                "include_candidates",
            ],
        ),
        case(
            "get",
            "/v1/memories",
            "memory::prefix_page_path",
            memory::prefix_page_path("active", Some("c")),
            &[
                "limit",
                "status",
                "include_history",
                "include_candidates",
                "cursor",
            ],
        ),
        case(
            "get",
            "/v1/memories/{id}",
            "memory::note_path",
            memory::note_path(note, Some("r")),
            &["revision"],
        ),
        case(
            "get",
            "/v1/memories/{id}",
            "memory::note_path",
            memory::note_path(note, None),
            &[],
        ),
        case(
            "get",
            "/v1/memories/suggestions",
            "memory::suggestions_path",
            memory::suggestions_path(10),
            &["limit"],
        ),
        case(
            "get",
            "/v1/memories/search",
            "memory::search_path",
            memory::search_path(&SearchFilters {
                query: "q",
                kind: Some("fact"),
                scope: Some("global"),
                limit: 10,
                historical: true,
                as_of: Some("2026-11-18"),
                since: Some("2026-11-01"),
                until: Some("2026-11-18"),
                order: Some("oldest"),
            }),
            &[
                "q",
                "limit",
                "kind",
                "scope",
                "as_of",
                "date_from",
                "date_to",
                "order_by",
                "historical",
            ],
        ),
        case(
            "get",
            "/v1/memories/timeline",
            "memory::timeline_path",
            memory::timeline_path(&Around::Note(note), 5, 5),
            &["anchor", "before", "after"],
        ),
        case(
            "get",
            "/v1/memories/timeline",
            "memory::timeline_path",
            memory::timeline_path(&Around::Query("q"), 2, 0),
            &["q", "before", "after"],
        ),
        case(
            "get",
            "/v1/memories/timeline",
            "memory::timeline_path",
            memory::timeline_path(&Around::Date("2026-06-10"), 5, 20),
            &["date", "before", "after"],
        ),
    ]
}

#[test]
fn every_query_the_cli_sends_holds_only_keys_its_route_reads() {
    let mut problems = Vec::new();
    for case in cases() {
        let (path, keys) = split(&case.path);
        if !on_route(path, case.route) {
            problems.push(format!(
                "{}: asks {path}, not its route {}",
                case.built_by, case.route
            ));
        }
        if keys != case.sends {
            problems.push(format!(
                "{}: sends {keys:?}, its case lists {:?} — a key dropped, renamed or moved",
                case.built_by, case.sends
            ));
        }
        let name = sample_name(case.method, case.route);
        let held = sample_keys(&name);
        for key in &keys {
            if !held.contains(*key) {
                problems.push(format!(
                    "{}: sends {key}, which queries/{name}.json does not hold",
                    case.built_by
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn every_query_sample_is_sent_here_or_listed_as_not_sent() {
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
    let sent: BTreeSet<String> = cases()
        .iter()
        .filter(|c| c.path.contains('?'))
        .map(|c| sample_name(c.method, c.route))
        .collect();
    let not_sent: BTreeSet<String> = NOT_SENT.iter().map(|s| s.to_string()).collect();
    assert!(sent.is_disjoint(&not_sent), "a sample both sent and not");
    let listed: BTreeSet<String> = sent.union(&not_sent).cloned().collect();
    assert_eq!(on_disk, listed);
    for name in TOKEN_ONLY {
        assert!(
            !on_disk.contains(*name),
            "queries/{name}.json now exists: the route reads a typed query; hold the CLI's \
             builder for it in cases() and drop it from TOKEN_ONLY"
        );
    }
}

#[test]
fn the_query_helpers_split_name_and_match_routes() {
    let (_, keys) = split("/v1/tasks?limit=1&state=x");
    assert!(
        !sample_keys("get-tasks").contains(keys[1]),
        "state is no key"
    );
    assert_eq!(sample_name("get", "/v1/memories/{id}"), "get-memories-id");
    assert!(on_route(
        "/v1/sessions/s/messages",
        "/v1/sessions/{id}/messages"
    ));
    assert!(!on_route(
        "/v1/sessions//messages",
        "/v1/sessions/{id}/messages"
    ));
}

/// The builders this entry named send the bytes their inline `format!`s
/// sent before (CONTRACT-SAMPLES-3 moved them, nothing else), but for the
/// four values the old code did not encode, which are now encoded as the
/// memory builders' are (*Rev 147*'s item 6, (b)): `a b` is `a%20b`.
#[test]
fn the_named_builders_send_what_the_inline_ones_sent() {
    use crate::commands::memory::{self, Around, ListFilters};
    use crate::commands::{agents, ext, llm_status, tasks};
    let note =
        openalpaca_storage::memory_files::NoteId::parse("00000021-0000-4000-8000-000000000001")
            .expect("a note id");
    assert_eq!(agents::list_path(None), "/v1/agents?limit=50");
    assert_eq!(
        agents::list_path(Some("a b")),
        "/v1/agents?limit=50&status=a%20b"
    );
    assert_eq!(tasks::list_path(None, 20), "/v1/tasks?limit=20");
    assert_eq!(
        tasks::list_path(Some("running"), 20),
        "/v1/tasks?limit=20&status=running"
    );
    assert_eq!(
        tasks::log_path("a/b", 50),
        "/v1/events/history?task_id=a%2Fb&limit=50"
    );
    assert_eq!(
        llm_status::usage_daily_path(Some("a"), Some("d")),
        "/v1/llm/usage/daily?limit=30&agent_id=a&date=d"
    );
    assert_eq!(
        llm_status::usage_daily_path(None, None),
        "/v1/llm/usage/daily?limit=30"
    );
    assert_eq!(
        llm_status::usage_path(Some("a"), Some("k")),
        "/v1/llm/usage?limit=50&agent_id=a&key_id=k"
    );
    assert_eq!(llm_status::usage_path(None, None), "/v1/llm/usage?limit=50");
    assert_eq!(ext::list_path(true), "/v1/extensions?include_orphaned=true");
    assert_eq!(ext::list_path(false), "/v1/extensions");
    assert_eq!(
        memory::list_path(&ListFilters {
            limit: 20,
            kind: Some("a b"),
            status: Some("active"),
            scope: None,
            history: true,
            candidates: false,
        }),
        "/v1/memories?limit=20&kind=a%20b&status=active&include_history=true"
    );
    assert_eq!(
        memory::prefix_page_path("active", Some("c/d")),
        "/v1/memories?limit=100&status=active&include_history=true\
         &include_candidates=true&cursor=c%2Fd"
    );
    assert_eq!(
        memory::note_path(note, Some("r 1")),
        format!("/v1/memories/{note}?revision=r%201")
    );
    assert_eq!(
        memory::note_path(note, None),
        format!("/v1/memories/{note}")
    );
    assert_eq!(
        memory::suggestions_path(7),
        "/v1/memories/suggestions?limit=7"
    );
    assert_eq!(
        memory::timeline_path(&Around::Note(note), 5, 5),
        format!("/v1/memories/timeline?anchor={note}&before=5&after=5")
    );
    assert_eq!(
        memory::timeline_path(&Around::Query("office move"), 2, 0),
        "/v1/memories/timeline?q=office%20move&before=2&after=0"
    );
    assert_eq!(
        memory::timeline_path(&Around::Date("2026-06-10"), 5, 20),
        "/v1/memories/timeline?date=2026-06-10&before=5&after=20"
    );
}

/// *Rev 147*'s item 6, (b): a value holding `&`, `#`, `%` or a space no
/// longer splits or cuts the query — each of the four builders sends it
/// encoded, and it decodes back whole.
#[test]
fn the_four_typed_query_values_are_sent_encoded() {
    use crate::commands::{agents, llm_status, tasks};
    let typed = "a&b=c#d%e f";
    let sent = "a%26b%3Dc%23d%25e%20f";
    assert_eq!(
        agents::list_path(Some(typed)),
        format!("/v1/agents?limit=50&status={sent}")
    );
    assert_eq!(
        tasks::list_path(Some(typed), 20),
        format!("/v1/tasks?limit=20&status={sent}")
    );
    assert_eq!(
        llm_status::usage_daily_path(Some(typed), Some(typed)),
        format!("/v1/llm/usage/daily?limit=30&agent_id={sent}&date={sent}")
    );
    assert_eq!(
        llm_status::usage_path(Some(typed), Some(typed)),
        format!("/v1/llm/usage?limit=50&agent_id={sent}&key_id={sent}")
    );
    for path in [
        agents::list_path(Some(typed)),
        tasks::list_path(Some(typed), 20),
        llm_status::usage_daily_path(Some(typed), Some(typed)),
        llm_status::usage_path(Some(typed), Some(typed)),
    ] {
        let (_, query) = path.split_once('?').expect("a query");
        let values: Vec<String> = query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .filter(|(key, _)| *key != "limit")
            .map(|(_, v)| urlencoding::decode(v).expect("UTF-8").into_owned())
            .collect();
        assert!(!values.is_empty(), "{path}");
        assert!(values.iter().all(|v| v == typed), "{path}: {values:?}");
    }
    // A run instance's id keeps its meaning: `:` is encoded too.
    assert_eq!(
        llm_status::usage_path(Some("reader::1a2b"), None),
        "/v1/llm/usage?limit=50&agent_id=reader%3A%3A1a2b"
    );
}
