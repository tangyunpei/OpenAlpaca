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
