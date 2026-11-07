use super::*;
use crate::test_util::{json_answer, serve};
use clap::Parser;

#[derive(Parser)]
struct Harness {
    #[command(subcommand)]
    command: SkillsCommands,
}

fn parse(argv: &[&str]) -> SkillsCommands {
    Harness::try_parse_from(argv)
        .unwrap_or_else(|e| panic!("{argv:?} did not parse: {e}"))
        .command
}

/// `colored` decides by tty at first use; pin it off so output reads literally.
fn plain() {
    colored::control::set_override(false);
}

#[test]
fn list_and_info_are_the_two_verbs() {
    assert!(matches!(
        parse(&["skills", "list"]),
        SkillsCommands::List {
            format: OutputFormat::Table
        }
    ));
    assert!(matches!(
        parse(&["skills", "list", "--format", "json"]),
        SkillsCommands::List {
            format: OutputFormat::Json
        }
    ));
    assert!(matches!(
        parse(&["skills", "info", "/review"]),
        SkillsCommands::Info { ref skill, .. } if skill == "/review"
    ));
    assert!(Harness::try_parse_from(["skills"]).is_err());
    assert!(Harness::try_parse_from(["skills", "info"]).is_err());
}

const SKILLS: &str = r#"[
  {"id": "code-review", "name": "Code Review", "description": "Review code for bugs",
   "source": "file", "origin": null, "requires_capabilities": ["file_read"],
   "triggers": {"slash": "review", "keywords": ["review"]}, "schedule": null,
   "invocations_today": 3, "version": "0.1.0", "author": "file:project"},
  {"id": "second-review", "name": "Second Review", "description": "Also reviews",
   "source": "file", "origin": null, "requires_capabilities": [],
   "triggers": {"slash": "review", "keywords": []}, "schedule": null,
   "invocations_today": 0, "version": null, "author": "file:project"}
]"#;

/// What the daemon serves for a skills directory holding a malformed
/// SKILL.md, two skills claiming `/review` and a FIFO named SKILL.md.
const DIAGNOSTICS: &str = r#"[
  {"code": "invalid_yaml", "severity": "error", "path": "/cfg/skills/broken/SKILL.md",
   "skill_id": "broken", "message": "Invalid YAML: did not find expected node content at line 2 column 7"},
  {"code": "read_failed", "severity": "error", "path": "/cfg/skills/piped/SKILL.md",
   "skill_id": "piped", "message": "Could not read SKILL.md: not a regular file"},
  {"code": "slash_conflict", "severity": "warning", "path": "/cfg/skills/second-review/SKILL.md",
   "skill_id": "second-review",
   "message": "Slash command '/review' is also claimed by skill 'code-review'; this skill now answers it"}
]"#;

fn rows() -> (Vec<SkillRow>, Vec<DiagnosticRow>) {
    (
        serde_json::from_str(SKILLS).unwrap(),
        serde_json::from_str(DIAGNOSTICS).unwrap(),
    )
}

/// The list reads both routes and prints the catalog, then every problem —
/// the malformed file, the FIFO and the slash conflict — with its code, its
/// file and the daemon's message.
#[tokio::test]
async fn list_shows_the_catalog_then_each_problem_the_scan_found() {
    plain();
    let (addr, server) = serve(vec![
        json_answer("200 OK", "skills", SKILLS),
        json_answer("200 OK", "diagnostics", DIAGNOSTICS),
    ]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let (skills, diagnostics) = fetch(&client).await.expect("both reads");
    let text = list_lines(&skills, &diagnostics).join("\n");

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[0].starts_with("GET /v1/skills HTTP/1.1"),
        "{}",
        requests[0]
    );
    assert!(
        requests[1].starts_with("GET /v1/skills/diagnostics HTTP/1.1"),
        "{}",
        requests[1]
    );

    assert!(text.contains("code-review"), "{text}");
    assert!(text.contains("/review"), "{text}");
    assert!(
        text.contains("3 problems in the skill files (2 not loaded, 1 warning):"),
        "{text}"
    );
    for (code, path) in [
        ("invalid_yaml", "/cfg/skills/broken/SKILL.md"),
        ("read_failed", "/cfg/skills/piped/SKILL.md"),
        ("slash_conflict", "/cfg/skills/second-review/SKILL.md"),
    ] {
        assert!(
            text.lines().any(|l| l.contains(code) && l.contains(path)),
            "{code} {path}: {text}"
        );
    }
    assert!(
        text.contains("Could not read SKILL.md: not a regular file"),
        "{text}"
    );
    assert!(text.contains("error  "), "{text}");
    assert!(text.contains("warning"), "{text}");
}

#[test]
fn a_clean_scan_prints_the_catalog_alone() {
    plain();
    let (skills, _) = rows();
    let text = list_lines(&skills, &[]).join("\n");
    assert!(text.contains("code-review"), "{text}");
    assert!(!text.contains("problem"), "{text}");
}

#[test]
fn info_finds_a_skill_by_id_or_slash_and_shows_its_own_problems() {
    plain();
    let (skills, diagnostics) = rows();

    let by_id = info_lines(
        &lookup(&skills, &diagnostics, "second-review"),
        "second-review",
    )
    .unwrap()
    .join("\n");
    assert!(by_id.contains("Second Review (second-review)"), "{by_id}");
    assert!(by_id.contains("slash_conflict"), "{by_id}");
    assert!(
        !by_id.contains("invalid_yaml"),
        "only its own problems: {by_id}"
    );

    let by_slash = info_lines(&lookup(&skills, &diagnostics, "/review"), "/review")
        .unwrap()
        .join("\n");
    assert!(by_slash.contains("Code Review (code-review)"), "{by_slash}");
    assert!(by_slash.contains("  slash:       /review"), "{by_slash}");
    assert!(!by_slash.contains("problem"), "{by_slash}");
}

/// A hot edit broke a loaded skill's file: the catalog still lists the skill,
/// but an invocation reads the file again and fails, and `info` says so.
#[test]
fn info_on_a_loaded_skill_whose_edit_failed_says_it_does_not_run_until_fixed() {
    plain();
    let (skills, _) = rows();
    let diagnostics: Vec<DiagnosticRow> = serde_json::from_str(
        r#"[{"code": "invalid_yaml", "severity": "error",
             "path": "/cfg/skills/code-review/SKILL.md", "skill_id": "code-review",
             "message": "Invalid YAML: did not find expected node content"}]"#,
    )
    .unwrap();
    let text = info_lines(&lookup(&skills, &diagnostics, "code-review"), "code-review")
        .unwrap()
        .join("\n");
    assert!(text.contains("Code Review (code-review)"), "{text}");
    assert!(
        text.contains(
            "Its SKILL.md as edited could not be loaded: the skill stays listed, \
             but invoking it fails with the error below until the file is fixed."
        ),
        "{text}"
    );
    assert!(
        text.contains("1 problem in the skill files (1 not loaded, 0 warnings):"),
        "{text}"
    );
    assert!(text.contains("invalid_yaml"), "{text}");
}

/// A skill the scan skipped is in no catalog row; `info` still says why.
#[test]
fn info_on_a_skipped_skill_shows_why_it_is_not_loaded() {
    plain();
    let (skills, diagnostics) = rows();
    let text = info_lines(&lookup(&skills, &diagnostics, "piped"), "piped")
        .unwrap()
        .join("\n");
    assert!(
        text.contains("Skill 'piped' is not loaded — the daemon skipped its SKILL.md."),
        "{text}"
    );
    assert!(text.contains("read_failed"), "{text}");
}

#[test]
fn info_on_a_name_nothing_knows_is_an_error() {
    let (skills, diagnostics) = rows();
    let err = info_lines(&lookup(&skills, &diagnostics, "nope"), "nope").unwrap_err();
    assert!(err.to_string().contains("No skill 'nope'"), "{err}");
}

/// A plugin's skill carries the plugin's own text: a hidden character in it
/// is printed as `<U+XXXX>`, never raw.
#[test]
fn a_hidden_character_is_printed_escaped() {
    plain();
    let (mut skills, _) = rows();
    skills[0].description = "Review\u{202E} code".to_string();
    let diagnostics = vec![DiagnosticRow {
        code: "unknown_key".to_string(),
        severity: "warning".to_string(),
        path: "/cfg/skills/x\u{200B}/SKILL.md".to_string(),
        skill_id: Some("x".to_string()),
        message: "Unknown frontmatter key 'a\u{1b}[2J' is ignored".to_string(),
    }];
    let text = list_lines(&skills, &diagnostics).join("\n");
    assert!(text.contains("Review<U+202E> code"), "{text}");
    assert!(text.contains("x<U+200B>/SKILL.md"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text:?}");
}

/// What the daemon serves for a store whose `config/skills/code-review/` is
/// the person's own folder over the built-in skill, beside a built-in skill
/// nothing overrides and one whose config folder is an unedited copy of an
/// earlier shipped version, set aside (subagent design §2.7 R-2).
const PROVENANCE: &str = r#"[
  {"id": "code-review", "name": "Code Review", "description": "Review code",
   "source": "file", "origin": null, "requires_capabilities": [],
   "triggers": {"slash": "review", "keywords": []}, "schedule": null,
   "invocations_today": 0, "version": null, "author": "file:project",
   "overrides_bundled": true,
   "not_used": [{"source": "bundled", "path": "/store/state/cache/bundled-skills/code-review",
                 "plugin": null, "reason": "overridden", "version": null, "edit": null}]},
  {"id": "create-skill", "name": "Create Skill", "description": "Create a skill",
   "source": "file", "origin": null, "requires_capabilities": [],
   "triggers": {"slash": "create-skill", "keywords": []}, "schedule": null,
   "invocations_today": 2, "version": null, "author": "file:bundled",
   "overrides_bundled": false,
   "not_used": [{"source": "file", "path": "/store/config/skills/create-skill",
                 "plugin": null, "reason": "unedited_copy", "version": "earlier", "edit": null}]},
  {"id": "daily-digest", "name": "Daily Digest", "description": "A digest",
   "source": "file", "origin": null, "requires_capabilities": [],
   "triggers": {"slash": null, "keywords": []}, "schedule": null,
   "invocations_today": 0, "version": null, "author": "file:project",
   "overrides_bundled": false, "not_used": []}
]"#;

/// R-3: the SOURCE column says `file:bundled` for a built-in skill and
/// `file:override` for a config folder over one (the route's
/// `file:project` with `overrides_bundled: true`), each whole in the
/// 16-character column; a folder of the person's own that overrides
/// nothing stays `file:project`.
#[test]
fn skills_list_shows_file_bundled_and_overrides() {
    plain();
    let skills: Vec<SkillRow> = serde_json::from_str(PROVENANCE).unwrap();
    let lines = list_lines(&skills, &[]);
    for row in [
        "code-review              /review          file:override    0     Review code",
        "create-skill             /create-skill    file:bundled     2     Create a skill",
        "daily-digest             -                file:project     0     A digest",
    ] {
        assert!(lines.iter().any(|l| l == row), "{row:?} in {lines:#?}");
    }
}

/// R-3: `info` prints the override in full and one line per definition of
/// the key that is not used — the built-in skill under the person's folder,
/// a folder set aside as an unedited copy — while `source` keeps the
/// route's word.
#[test]
fn skills_info_prints_the_override_and_each_not_used_line() {
    plain();
    let skills: Vec<SkillRow> = serde_json::from_str(PROVENANCE).unwrap();

    let review = info_lines(&lookup(&skills, &[], "code-review"), "code-review")
        .unwrap()
        .join("\n");
    assert!(review.contains("  source:      file:project"), "{review}");
    assert!(
        review.contains("  overrides:   the bundled skill code-review"),
        "{review}"
    );
    assert!(
        review.contains(
            "  not used:    the bundled skill, /store/state/cache/bundled-skills/code-review \
             (this skill's folder overrides it)"
        ),
        "{review}"
    );

    let create = info_lines(&lookup(&skills, &[], "create-skill"), "create-skill")
        .unwrap()
        .join("\n");
    assert!(create.contains("  source:      file:bundled"), "{create}");
    assert!(!create.contains("overrides:"), "{create}");
    assert!(
        create.contains(
            "  not used:    /store/config/skills/create-skill \
             (an unedited copy of a version an earlier build shipped)"
        ),
        "{create}"
    );

    let digest = info_lines(&lookup(&skills, &[], "daily-digest"), "daily-digest")
        .unwrap()
        .join("\n");
    assert!(!digest.contains("overrides:"), "{digest}");
    assert!(!digest.contains("not used:"), "{digest}");
}

/// A not-used entry's path is the daemon's string: a hidden character in
/// it is printed as `<U+XXXX>`, never raw.
#[test]
fn a_hidden_character_in_a_not_used_path_is_printed_escaped() {
    plain();
    let skills: Vec<SkillRow> = serde_json::from_str(&PROVENANCE.replace(
        "/store/config/skills/create-skill",
        "/store/config/skills/create\u{202E}-skill\u{200B}",
    ))
    .unwrap();
    let text = info_lines(&lookup(&skills, &[], "create-skill"), "create-skill")
        .unwrap()
        .join("\n");
    assert!(
        text.contains("/store/config/skills/create<U+202E>-skill<U+200B>"),
        "{text}"
    );
    assert!(!text.contains('\u{202E}'), "{text:?}");
    assert!(!text.contains('\u{200B}'), "{text:?}");
}

// ── tidy (subagent design §2.12 D-1, D-2) ─────────────────────────────

/// The instant every tidy test runs at, so the archive's name is pinned.
fn now() -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc.with_ymd_and_hms(2026, 11, 7, 12, 0, 0).unwrap()
}

const TIDY_WARNING: &str = "An earlier build started after this finds skills/ without these \
     folders and does not seed them again; to go back, move skills/ aside and let that build \
     seed it (Installation Manual, Keep the builds apart).";

/// The five files `82022776`'s seeding wrote as `create-skill/`, as core's
/// tests hold them.
const CREATE_SKILL_82022776: [&str; 5] = [
    "SKILL.md",
    "scripts/check_exists.sh",
    "scripts/list_skills.sh",
    "scripts/scaffold.sh",
    "scripts/write_skill_md.sh",
];

fn create_skill_fixture(name: &str) -> Vec<u8> {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/openalpaca_core/src/bundled/fixtures/82022776/skills/create-skill")
        .join(name);
    std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

/// A scratch config directory under the sandbox whose `skills/create-skill/`
/// is what `82022776`'s seeding wrote — an earlier build's unedited copy.
struct TidyStore {
    // Declared first, so it drops first.
    _env: crate::test_util::EnvSandbox,
    _dir: tempfile::TempDir,
    config: std::path::PathBuf,
    skills: std::path::PathBuf,
}

impl TidyStore {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let env = crate::test_util::EnvSandbox::enter(dir.path());
        let config = dir.path().join("config");
        let skills = config.join("skills");
        for name in CREATE_SKILL_82022776 {
            let target = skills.join("create-skill").join(name);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, create_skill_fixture(name)).unwrap();
        }
        Self {
            _env: env,
            _dir: dir,
            config,
            skills,
        }
    }
}

#[test]
fn tidy_is_a_verb_and_apply_its_one_flag() {
    assert!(matches!(
        parse(&["skills", "tidy"]),
        SkillsCommands::Tidy { apply: false }
    ));
    assert!(matches!(
        parse(&["skills", "tidy", "--apply"]),
        SkillsCommands::Tidy { apply: true }
    ));
}

/// `--apply` moves the whole unedited folder to
/// `skills/.archived/<key>.<UTC timestamp>/`, every file in it as it was,
/// says so, and ends with the warning for an earlier build.
#[tokio::test]
async fn skills_tidy_apply_moves_an_unedited_create_skill_folder_to_archived() {
    plain();
    let store = TidyStore::new();

    let listed = tidy_lines(&store.config, None, false, now())
        .await
        .unwrap()
        .join("\n");
    assert!(
        listed.starts_with(&format!(
            "Unedited copies of shipped skills in {}, not read — the built-in ones, or a \
             plugin's skill of the same name, run in their place:",
            store.skills.display()
        )),
        "{listed}"
    );
    let from = store.skills.join("create-skill");
    let to = store.skills.join(".archived/create-skill.20261107120000");
    assert!(
        listed.contains(&format!("{} (version: earlier)", from.display())),
        "{listed}"
    );
    assert!(
        listed.contains(&format!("would move to {}", to.display())),
        "{listed}"
    );
    assert!(from.join("SKILL.md").is_file(), "the listing moves nothing");

    let lines = tidy_lines(&store.config, None, true, now()).await.unwrap();
    let text = lines.join("\n");
    assert!(!from.exists());
    for name in CREATE_SKILL_82022776 {
        assert_eq!(
            std::fs::read(to.join(name)).unwrap(),
            create_skill_fixture(name)
        );
    }
    assert!(
        text.contains(&format!("Moved {} to {}", from.display(), to.display())),
        "{text}"
    );
    assert_eq!(
        lines.last().map(String::as_str),
        Some(TIDY_WARNING),
        "{text}"
    );
}

/// A folder with one file edited is the person's override: it is neither
/// listed nor moved.
#[tokio::test]
async fn skills_tidy_leaves_an_override_folder() {
    plain();
    let store = TidyStore::new();
    let skill_md = store.skills.join("create-skill/SKILL.md");
    let edited = format!(
        "{}\nAsk before writing.\n",
        String::from_utf8(create_skill_fixture("SKILL.md")).unwrap()
    );
    std::fs::write(&skill_md, &edited).unwrap();

    for apply in [false, true] {
        let text = tidy_lines(&store.config, None, apply, now())
            .await
            .unwrap()
            .join("\n");
        assert_eq!(
            text,
            format!(
                "No unedited copies of shipped skills in {}: nothing to tidy.",
                store.skills.display()
            )
        );
    }
    assert_eq!(std::fs::read_to_string(&skill_md).unwrap(), edited);
    for name in &CREATE_SKILL_82022776[1..] {
        assert_eq!(
            std::fs::read(store.skills.join("create-skill").join(name)).unwrap(),
            create_skill_fixture(name)
        );
    }
    assert!(!store.skills.join(".archived").exists());
}

/// With a daemon answering, `--apply` reads `GET /v1/skills` and moves only
/// a folder its listing names as an unedited copy not used.
#[tokio::test]
async fn skills_tidy_apply_moves_nothing_the_running_daemon_reads() {
    plain();
    let store = TidyStore::new();
    let rows = serde_json::json!([
        {"id": "create-skill", "name": "Create Skill", "author": "file:project",
         "overrides_bundled": true, "not_used": [{
            "source": "bundled", "path": "/store/state/cache/bundled-skills/create-skill",
            "plugin": null, "reason": "overridden", "version": null, "edit": null}]}
    ])
    .to_string();
    let (addr, server) = serve(vec![json_answer("200 OK", "skills", &rows)]);
    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

    let text = tidy_lines(&store.config, Some(&client), true, now())
        .await
        .unwrap()
        .join("\n");

    let requests = server.join().expect("the server finishes");
    assert!(
        requests[0].starts_with("GET /v1/skills HTTP/1.1"),
        "{}",
        requests[0]
    );
    let folder = store.skills.join("create-skill");
    assert!(folder.join("SKILL.md").is_file());
    assert!(
        text.contains(&format!(
            "Left {}: the running daemon does not list it as an unused copy",
            folder.display()
        )),
        "{text}"
    );
    assert!(!text.contains(TIDY_WARNING), "nothing moved: {text}");
    assert!(!store.skills.join(".archived").exists());
}
