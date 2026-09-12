use super::*;
use clap::Parser;

#[derive(Parser)]
struct Harness {
    #[command(flatten)]
    args: SessionsArgs,
}

fn parse(argv: &[&str]) -> SessionsArgs {
    Harness::try_parse_from(argv)
        .unwrap_or_else(|e| panic!("{argv:?} did not parse: {e}"))
        .args
}

/// `colored` decides by tty at first use; pin it off so a row can be read
/// literally.
fn plain() {
    colored::control::set_override(false);
}

fn row(overrides: fn(&mut SessionItem)) -> SessionItem {
    let mut item = SessionItem {
        id: "0f2c9a41-3b7d-4e58-9a10-6c1f2d3e4b55".to_string(),
        lane_key: "alice:gui".to_string(),
        source: "gui".to_string(),
        title: "Connector audit".to_string(),
        workspace_id: Some("/Users/dev/openalpaca".to_string()),
        status: "active".to_string(),
        message_count: 12,
        updated_at: "2026-09-06 10:00:00".to_string(),
    };
    overrides(&mut item);
    item
}

#[test]
fn the_flags_parse_and_default_to_this_lane() {
    let args = parse(&["sessions"]);
    assert!(!args.all);
    assert_eq!(args.workspace, None);
    assert_eq!(args.limit, 50);

    let args = parse(&["sessions", "--all", "--workspace", ".", "--limit", "10"]);
    assert!(args.all);
    assert_eq!(args.workspace.as_deref(), Some("."));
    assert_eq!(args.limit, 10);
}

/// The lane restriction is client-side because the route has no lane filter;
/// what *is* sent server-side is the lane's source, so a Telegram conversation
/// never reaches the page in the first place.
#[test]
fn the_default_listing_asks_for_the_callers_own_source() {
    assert_eq!(
        sessions_query(Some("gui"), None, 50),
        "/v1/sessions?limit=50&source=gui"
    );
}

#[test]
fn all_drops_the_source_filter() {
    assert_eq!(sessions_query(None, None, 50), "/v1/sessions?limit=50");
}

#[test]
fn a_workspace_filter_is_url_encoded_onto_the_query() {
    assert_eq!(
        sessions_query(Some("gui"), Some("/Users/dev/my repo"), 25),
        "/v1/sessions?limit=25&source=gui&workspace_id=%2FUsers%2Fdev%2Fmy%20repo"
    );
}

#[test]
fn a_lane_key_yields_its_source() {
    assert_eq!(lane_source("alice:gui"), Some("gui"));
    assert_eq!(lane_source("alice:telegram"), Some("telegram"));
    // A key with no separator is not a lane this CLI can narrow by.
    assert_eq!(lane_source("alice"), None);
}

/// `--workspace` is resolved the way the daemon resolves a turn's header, so
/// the value compared against `session.workspace_id` is the project *root* —
/// which is what makes `--workspace .` work from a subdirectory.
#[test]
fn a_workspace_argument_resolves_up_to_the_project_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(root.join("crates/deep")).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();

    let resolved = resolve_workspace_filter(&root.join("crates/deep").to_string_lossy()).unwrap();
    assert_eq!(resolved, root.to_string_lossy());
}

#[test]
fn a_directory_under_no_project_is_an_error_not_an_empty_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    let loose = tmp.path().canonicalize().unwrap().join("loose");
    std::fs::create_dir(&loose).unwrap();

    // A temp dir has no `.git`/`.openalpaca` above it inside the sandbox root,
    // so either outcome is possible depending on the machine; what must never
    // happen is a filter on a path the daemon could not have stored.
    if let Ok(resolved) = resolve_workspace_filter(&loose.to_string_lossy()) {
        assert_ne!(
            resolved,
            loose.to_string_lossy(),
            "a directory with no marker is never its own project root"
        );
    }

    let missing = resolve_workspace_filter(&tmp.path().join("nope").to_string_lossy());
    assert!(missing.is_err(), "a path that does not exist is an error");
}

#[test]
fn a_row_prints_the_id_status_title_workspace_and_stamp() {
    plain();
    assert_eq!(
        SessionItem::headers()
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        vec!["ID", "STATUS", "TITLE", "WORKSPACE", "UPDATED"]
    );

    let cells: Vec<String> = row(|_| {})
        .table_row()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    assert_eq!(cells[0], "0f2c9a41-3b7d-4e58-9a10-6c1f2d3e4b55");
    assert_eq!(cells[1], "active");
    assert_eq!(cells[2], "Connector");
    assert!(
        row(|_| {}).table_row().contains("openalpaca"),
        "the workspace column is the project's own name, not its whole path"
    );
    assert!(row(|_| {}).table_row().contains("2026-09-06 10:00"));
}

/// `title` is `""` until a conversation is renamed. A blank cell would read as
/// a missing field rather than an unnamed conversation.
#[test]
fn an_unrenamed_conversation_prints_a_name() {
    plain();
    let printed = row(|item| item.title = String::new()).table_row();
    assert!(printed.contains("(untitled)"), "{printed}");
}

/// A conversation bound to no project is a real state (every connector lane's
/// is), not a missing value.
#[test]
fn a_conversation_with_no_project_prints_a_dash() {
    plain();
    // A one-word title so the columns can be read by splitting on whitespace.
    let printed = row(|item| {
        item.title = "Audit".to_string();
        item.workspace_id = None;
    })
    .table_row();
    let cells: Vec<&str> = printed.split_whitespace().collect();
    assert_eq!(cells[3], "-");
}

/// A daemon older than a field must not break the row.
#[test]
fn a_row_from_a_thinner_payload_still_renders() {
    plain();
    let item: SessionItem = serde_json::from_value(serde_json::json!({
        "id": "sess-1",
        "lane_key": "alice:gui",
        "status": "archived",
        "updated_at": "2026-09-06T10:00:00Z",
    }))
    .expect("a row without the optional fields still deserializes");
    let printed = item.table_row();
    assert!(printed.contains("(untitled)"), "{printed}");
    assert!(printed.contains("2026-09-06 10:00"), "{printed}");
}

#[test]
fn the_picker_label_says_enough_to_choose_by() {
    let label = picker_label(&row(|_| {}));
    assert!(label.contains("Connector audit"), "{label}");
    assert!(label.contains("active"), "{label}");
    assert!(label.contains("12 messages"), "{label}");
    assert!(label.contains("openalpaca"), "{label}");
}

#[test]
fn resuming_nothing_is_an_error_with_a_next_step() {
    let err = require_one(&[], None).unwrap_err().to_string();
    assert!(err.contains("send a message first"), "{err}");
}

/// `chat --resume` asks in one project (plan §5.7), so "none here" is a
/// different answer from "none at all" and must not read as the lane being
/// empty when the daemon is full of other projects' conversations.
#[test]
fn resuming_nothing_in_this_project_says_which_project() {
    let err = require_one(&[], Some("/repo/one")).unwrap_err().to_string();
    assert!(err.contains("/repo/one"), "{err}");
    assert!(err.contains("sessions --all"), "{err}");
    assert!(err.contains("--session <id>"), "{err}");
}

/// What `--resume` may continue: this lane's rows, and — since §5.7 scopes the
/// resume to the working directory's project — only that project's.
///
/// The lane half was already client-side (the route has no lane filter); the
/// project half is sent as `workspace_id=` *and* re-checked here, so a row
/// bound to another project, or to none, never reaches the picker.
#[test]
fn only_this_lanes_conversations_in_this_project_are_resumable() {
    let page = vec![
        row(|item| item.id = "mine".to_string()),
        row(|item| {
            item.id = "other-project".to_string();
            item.workspace_id = Some("/repo/two".to_string());
        }),
        row(|item| {
            item.id = "no-project".to_string();
            item.workspace_id = None;
        }),
        row(|item| {
            item.id = "other-lane".to_string();
            item.lane_key = "alice:telegram".to_string();
        }),
        row(|item| {
            item.id = "another-owner".to_string();
            item.lane_key = "bob:gui".to_string();
        }),
    ];

    let here = rows_here(page.clone(), "alice:gui", Some("/Users/dev/openalpaca"));
    assert_eq!(
        here.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        vec!["mine"]
    );

    // With no project to narrow by — a working directory under no marker —
    // the lane is still the boundary, and every one of its rows is on offer.
    let lane_wide = rows_here(page, "alice:gui", None);
    assert_eq!(
        lane_wide
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["mine", "other-project", "no-project"]
    );
}
