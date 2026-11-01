//! N-SKILL-2: the catalog keeps what its last scan found (audit Z3-03), and
//! the scan reads only a regular file of bounded size (Z3-10). Every test
//! works in its own temporary directory.

use super::*;
use crate::middleware::skill::SkillParseError;
use std::time::Duration;
use tempfile::TempDir;

fn write_skill(parent: &Path, dir: &str, content: &str) -> PathBuf {
    let skill_dir = parent.join(dir);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let path = skill_dir.join("SKILL.md");
    std::fs::write(&path, content).unwrap();
    path
}

fn skill_md(frontmatter: &str) -> String {
    format!("---\n{frontmatter}---\n\n## Instructions\n\nDo it.\n")
}

/// Scan on a helper thread and fail, rather than hang the test run, if the
/// scan does not finish — the parent blocked forever opening a FIFO.
fn scan_within(catalog: &Arc<SkillCatalog>, dir: &Path, limit: Duration) -> usize {
    let (tx, rx) = std::sync::mpsc::channel();
    let catalog = catalog.clone();
    let scanned = dir.to_path_buf();
    std::thread::spawn(move || {
        let count = catalog.scan_directory(&scanned, SkillScope::Project);
        let _ = tx.send(count);
    });
    rx.recv_timeout(limit).unwrap_or_else(|_| {
        panic!(
            "the scan of {} did not finish within {limit:?}",
            dir.display()
        )
    })
}

fn the_one(catalog: &SkillCatalog, code: SkillDiagnosticCode) -> SkillDiagnostic {
    let found: Vec<SkillDiagnostic> = catalog
        .diagnostics()
        .into_iter()
        .filter(|d| d.code == code)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "one {} diagnostic expected, got {:?}",
        code.as_str(),
        catalog.diagnostics()
    );
    found.into_iter().next().unwrap()
}

#[test]
fn a_malformed_skill_md_yields_invalid_yaml_and_is_skipped() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(
        tmp.path(),
        "Broken",
        "---\nname: [unclosed\ndescription: x\n---\n",
    );
    write_skill(
        tmp.path(),
        "fine",
        &skill_md("name: Fine\ndescription: Loads\nrequires_capabilities: [file_read]\n"),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    let d = the_one(&catalog, SkillDiagnosticCode::InvalidYaml);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Error);
    assert_eq!(d.skill_id.as_deref(), Some("broken"));
    assert_eq!(d.path, path.display().to_string());
    assert!(d.message.starts_with("Invalid YAML: "), "{}", d.message);
    assert!(catalog.get("broken").is_none());
    assert_eq!(
        catalog.diagnostics().len(),
        1,
        "{:?}",
        catalog.diagnostics()
    );
}

#[test]
fn missing_frontmatter_and_a_missing_field_are_named_with_the_parse_errors_words() {
    let tmp = TempDir::new().unwrap();
    write_skill(tmp.path(), "bare", "# Just a heading\n");
    write_skill(tmp.path(), "open", "---\nname: Open\n");
    write_skill(tmp.path(), "nameless", &skill_md("description: No name\n"));

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 0);

    let found = catalog.diagnostics();
    let message_of = |id: &str| {
        found
            .iter()
            .find(|d| d.skill_id.as_deref() == Some(id))
            .map(|d| (d.code, d.message.clone()))
            .unwrap_or_else(|| panic!("no diagnostic for {id}: {found:?}"))
    };
    assert_eq!(
        message_of("bare"),
        (
            SkillDiagnosticCode::MissingFrontmatter,
            SkillParseError::MissingFrontmatter.to_string()
        )
    );
    assert_eq!(
        message_of("open"),
        (
            SkillDiagnosticCode::MissingFrontmatter,
            SkillParseError::UnterminatedFrontmatter.to_string()
        )
    );
    assert_eq!(
        message_of("nameless"),
        (
            SkillDiagnosticCode::MissingField,
            SkillParseError::MissingField("name").to_string()
        )
    );
}

#[test]
fn a_slash_conflict_yields_slash_conflict_and_both_skills_load() {
    let tmp = TempDir::new().unwrap();
    for id in ["review-a", "review-b"] {
        write_skill(
            tmp.path(),
            id,
            &skill_md(&format!(
                "name: {id}\ndescription: Reviews\ninvoke:\n  slash: /review\n"
            )),
        );
    }

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 2);

    let d = the_one(&catalog, SkillDiagnosticCode::SlashConflict);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Warning);
    let (winner, other) = match d.skill_id.as_deref() {
        Some("review-a") => ("review-a", "review-b"),
        Some("review-b") => ("review-b", "review-a"),
        other => panic!("unexpected skill id {other:?}"),
    };
    assert_eq!(
        d.message,
        SkillParseError::SlashConflict {
            command: "review".to_string(),
            other: other.to_string()
        }
        .to_string()
    );
    assert_eq!(
        catalog
            .get_by_command("review")
            .map(|e| e.skill_dir.unwrap()),
        Some(tmp.path().join(winner)),
        "the diagnostic names the skill that answers the slash"
    );
}

#[cfg(unix)]
fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path; mkfifo only creates the node.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(
        rc,
        0,
        "mkfifo {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

/// A FIFO named SKILL.md is reported, and the scan goes on: the parent opened
/// it and waited for a writer that never came.
#[cfg(unix)]
#[test]
fn a_fifo_named_skill_md_yields_read_failed_without_blocking_the_scan() {
    let tmp = TempDir::new().unwrap();
    let fifo_dir = tmp.path().join("piped");
    std::fs::create_dir_all(&fifo_dir).unwrap();
    make_fifo(&fifo_dir.join("SKILL.md"));
    write_skill(
        tmp.path(),
        "fine",
        &skill_md("name: Fine\ndescription: Loads\n"),
    );

    let catalog = Arc::new(SkillCatalog::new());
    let loaded = scan_within(&catalog, tmp.path(), Duration::from_secs(10));
    assert_eq!(loaded, 1);

    let d = the_one(&catalog, SkillDiagnosticCode::ReadFailed);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Error);
    assert_eq!(d.skill_id.as_deref(), Some("piped"));
    assert_eq!(
        d.message,
        SkillParseError::ReadFailed("not a regular file".to_string()).to_string()
    );
}

/// The hot-reload path reads the same guarded way.
#[cfg(unix)]
#[test]
fn reloading_a_fifo_named_skill_md_does_not_block() {
    let tmp = TempDir::new().unwrap();
    let fifo_dir = tmp.path().join("piped");
    std::fs::create_dir_all(&fifo_dir).unwrap();
    make_fifo(&fifo_dir.join("SKILL.md"));

    let catalog = Arc::new(SkillCatalog::new());
    let (tx, rx) = std::sync::mpsc::channel();
    let reloading = catalog.clone();
    let dir = fifo_dir.clone();
    std::thread::spawn(move || {
        let _ = tx.send(reloading.reload_skill(&dir));
    });
    let outcome = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the reload did not finish");
    assert!(outcome.is_err());
    assert_eq!(
        the_one(&catalog, SkillDiagnosticCode::ReadFailed)
            .skill_id
            .as_deref(),
        Some("piped")
    );
}

#[test]
fn an_oversized_skill_md_yields_too_large_and_is_skipped() {
    let tmp = TempDir::new().unwrap();
    let padding = "x".repeat(MAX_SKILL_MD_BYTES as usize);
    write_skill(
        tmp.path(),
        "huge",
        &skill_md(&format!("name: Huge\ndescription: Big\n# {padding}\n")),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 0);

    let d = the_one(&catalog, SkillDiagnosticCode::TooLarge);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Error);
    assert!(
        d.message
            .ends_with(&format!("over the {MAX_SKILL_MD_BYTES}-byte limit")),
        "{}",
        d.message
    );
    assert!(catalog.get("huge").is_none());
}

/// Call `load_full` on a helper thread and fail, rather than hang the test
/// run, if it does not return — the parent opened a FIFO and waited for a
/// writer that never came.
fn load_full_within(
    catalog: &Arc<SkillCatalog>,
    name: &str,
    limit: Duration,
) -> Result<crate::middleware::skill::SkillDocument, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let catalog = catalog.clone();
    let wanted = name.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(catalog.load_full(&wanted));
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("load_full({name}) did not return within {limit:?}"))
}

/// The invocation's read (level 2) is guarded as the scan's is: a SKILL.md
/// swapped for a FIFO after the scan is refused at once, naming the file,
/// instead of blocking the invocation for good.
#[cfg(unix)]
#[test]
fn a_skill_md_swapped_for_a_fifo_after_the_scan_is_refused_at_invocation_without_blocking() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(
        tmp.path(),
        "swapped",
        &skill_md("name: Swapped\ndescription: Loads\n"),
    );
    let catalog = Arc::new(SkillCatalog::new());
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    std::fs::remove_file(&path).unwrap();
    make_fifo(&path);

    let refused = load_full_within(&catalog, "swapped", Duration::from_secs(10)).unwrap_err();
    assert!(refused.contains(&path.display().to_string()), "{refused}");
    assert!(
        refused
            .contains(&SkillParseError::ReadFailed("not a regular file".to_string()).to_string()),
        "{refused}"
    );
}

/// The invocation's read is capped as the scan's is: a SKILL.md grown past
/// [`MAX_SKILL_MD_BYTES`] after the scan is refused with the `too_large`
/// wording, not read whole and run.
#[test]
fn a_skill_md_grown_past_the_cap_after_the_scan_is_refused_at_invocation() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(
        tmp.path(),
        "grown",
        &skill_md("name: Grown\ndescription: Small at first\n"),
    );
    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    let padding = "x".repeat(MAX_SKILL_MD_BYTES as usize);
    std::fs::write(
        &path,
        skill_md(&format!("name: Grown\ndescription: Big now\n# {padding}\n")),
    )
    .unwrap();
    let size = std::fs::metadata(&path).unwrap().len();

    let refused = catalog.load_full("grown").unwrap_err();
    assert!(refused.contains(&path.display().to_string()), "{refused}");
    assert!(
        refused.contains(
            &SkillParseError::TooLarge {
                bytes: size,
                limit: MAX_SKILL_MD_BYTES
            }
            .to_string()
        ),
        "{refused}"
    );
}

/// A typo leaves a skill with no tools; it still loads (the parse stays
/// lenient), and the key is named.
#[test]
fn an_unknown_key_is_named_and_the_skill_still_loads() {
    let tmp = TempDir::new().unwrap();
    write_skill(
        tmp.path(),
        "typo",
        &skill_md("name: Typo\ndescription: Misspelt\nrequire_capabilities:\n  - web_access\n"),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    let d = the_one(&catalog, SkillDiagnosticCode::UnknownKey);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Warning);
    assert_eq!(
        d.message,
        "Unknown frontmatter key 'require_capabilities' is ignored"
    );
    assert!(catalog.get("typo").is_some());
}

/// A skill that declares no tools at all is named once, as a warning, and
/// still loads; one tool from any of the four places is enough to stay quiet.
#[test]
fn a_skill_with_no_tools_is_named_and_still_loads() {
    let tmp = TempDir::new().unwrap();
    write_skill(
        tmp.path(),
        "bare",
        &skill_md("name: Bare\ndescription: Nothing to use\n"),
    );
    write_skill(
        tmp.path(),
        "capable",
        &skill_md("name: Capable\ndescription: D\nrequires_capabilities: [file_read]\n"),
    );
    write_skill(
        tmp.path(),
        "allowed",
        &skill_md("name: Allowed\ndescription: D\ntools:\n  allow: [file_read]\n"),
    );
    write_skill(
        tmp.path(),
        "scripted",
        &skill_md(
            "name: Scripted\ndescription: D\nscripts:\n  - file: a.sh\n    name: a\n    description: d\n",
        ),
    );
    write_skill(
        tmp.path(),
        "composer",
        &skill_md("name: Composer\ndescription: D\ndepends_on: [capable]\n"),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 5);

    let d = the_one(&catalog, SkillDiagnosticCode::NoTools);
    assert_eq!(d.severity, SkillDiagnosticSeverity::Warning);
    assert_eq!(d.skill_id.as_deref(), Some("bare"));
    assert_eq!(d.message, SkillParseError::NoTools.to_string());
    assert!(catalog.get("bare").is_some());
}

/// The capture refuses no frontmatter the parse accepted without it: a
/// numeric key, a value that is a map with a numeric key and a list holding
/// one are each named, and the skill loads. (A map of JSON values would
/// refuse the last two.)
#[test]
fn unknown_keys_of_any_shape_are_named_and_the_skill_still_loads() {
    let tmp = TempDir::new().unwrap();
    write_skill(
        tmp.path(),
        "shapes",
        &skill_md("name: Shapes\ndescription: D\n3: x\nfoo: {1: a}\nbar: [1, {2: b}]\n"),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    let named: Vec<String> = catalog
        .diagnostics()
        .into_iter()
        .filter(|d| d.code == SkillDiagnosticCode::UnknownKey)
        .map(|d| d.message)
        .collect();
    assert_eq!(
        named,
        [
            "Unknown frontmatter key '3' is ignored",
            "Unknown frontmatter key 'bar' is ignored",
            "Unknown frontmatter key 'foo' is ignored",
        ]
    );
    assert!(catalog.get("shapes").is_some());
}

/// An unknown key is reported, not kept: the frontmatter serializes with
/// only the keys this build reads, as it did before the capture existed.
#[test]
fn an_unknown_key_is_named_in_order_and_not_written_back() {
    let fm = crate::middleware::skill::parse_skill_frontmatter(&skill_md(
        "name: N\ndescription: D\nzeta: 1\nrequire_capabilities: [web_access]\n",
    ))
    .unwrap();
    assert_eq!(fm.unknown_keys.names(), ["zeta", "require_capabilities"]);

    let written = serde_json::to_value(&fm).unwrap();
    assert!(written.get("zeta").is_none(), "{written}");
    assert!(written.get("require_capabilities").is_none(), "{written}");
    assert_eq!(written["name"], "N");
}

#[test]
fn an_id_mismatch_and_a_deprecated_field_are_warnings_on_a_loaded_skill() {
    let tmp = TempDir::new().unwrap();
    write_skill(
        tmp.path(),
        "summarizer",
        &skill_md(
            "id: summariser\nname: Summarizer\ndescription: Sums up\n\
             context:\n  summarize:\n    enabled: true\n",
        ),
    );

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);

    let mismatch = the_one(&catalog, SkillDiagnosticCode::IdMismatch);
    assert_eq!(mismatch.severity, SkillDiagnosticSeverity::Warning);
    assert_eq!(
        mismatch.message,
        "Frontmatter id 'summariser' does not match directory name 'summarizer'"
    );
    let deprecated = the_one(&catalog, SkillDiagnosticCode::DeprecatedField);
    assert!(
        deprecated
            .message
            .starts_with("'context.summarize.enabled' is deprecated and has no effect."),
        "{}",
        deprecated.message
    );
}

/// The catalog keeps the **last** scan's findings: a fixed file's goes.
#[test]
fn a_rescan_replaces_what_the_last_scan_found() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(tmp.path(), "later", "---\nname: [oops\n---\n");

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert_eq!(catalog.diagnostics().len(), 1);

    std::fs::write(
        &path,
        skill_md("name: Later\ndescription: Fixed\nrequires_capabilities: [file_read]\n"),
    )
    .unwrap();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);
    assert!(
        catalog.diagnostics().is_empty(),
        "{:?}",
        catalog.diagnostics()
    );
}

/// A hot edit that breaks a loaded skill's SKILL.md is not loaded: the skill
/// stays listed (its level-1 entry, and the warnings found in it, beside the
/// edited file's error), but it does not run — an invocation reads the file
/// again (`load_full`) and answers with the error the diagnostic names. Nothing
/// of the previous version's instructions is kept. The next good edit replaces
/// both.
#[test]
fn a_failed_reload_keeps_the_skill_listed_and_its_warnings_but_it_does_not_run() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(
        tmp.path(),
        "kept",
        &skill_md("name: Kept\ndescription: First\nzeta: 1\nrequires_capabilities: [file_read]\n"),
    );
    let skill_dir = path.parent().unwrap().to_path_buf();

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);
    assert_eq!(
        the_one(&catalog, SkillDiagnosticCode::UnknownKey).severity,
        SkillDiagnosticSeverity::Warning
    );

    std::fs::write(&path, "---\nname: [oops\n---\n").unwrap();
    assert!(catalog.reload_skill(&skill_dir).is_err());
    assert_eq!(
        catalog.get("kept").unwrap().frontmatter.description,
        "First"
    );
    // Listed, but not runnable: the invocation's read answers with the
    // edited file's error, the one the diagnostic names.
    let refused = catalog.load_full("kept").unwrap_err();
    let error = the_one(&catalog, SkillDiagnosticCode::InvalidYaml);
    assert_eq!(error.path, path.display().to_string());
    assert!(
        refused.contains(&error.path) && refused.contains(&error.message),
        "{refused} / {error:?}"
    );
    assert!(!refused.contains("Do it."), "{refused}");
    let found: Vec<(SkillDiagnosticCode, SkillDiagnosticSeverity)> = catalog
        .diagnostics()
        .into_iter()
        .map(|d| (d.code, d.severity))
        .collect();
    assert_eq!(
        found,
        [
            (
                SkillDiagnosticCode::InvalidYaml,
                SkillDiagnosticSeverity::Error
            ),
            (
                SkillDiagnosticCode::UnknownKey,
                SkillDiagnosticSeverity::Warning
            ),
        ]
    );

    // A second broken edit keeps one error, not two, and the same warning.
    std::fs::write(&path, "---\nname: [still\n---\n").unwrap();
    assert!(catalog.reload_skill(&skill_dir).is_err());
    assert_eq!(
        catalog.diagnostics().len(),
        2,
        "{:?}",
        catalog.diagnostics()
    );

    std::fs::write(
        &path,
        skill_md("name: Kept\ndescription: Second\nrequires_capabilities: [file_read]\n"),
    )
    .unwrap();
    assert!(catalog.reload_skill(&skill_dir).is_ok());
    assert_eq!(
        catalog.get("kept").unwrap().frontmatter.description,
        "Second"
    );
    assert!(
        catalog.diagnostics().is_empty(),
        "{:?}",
        catalog.diagnostics()
    );
}

/// A broken edit of a skill that is not loaded leaves nothing of an earlier
/// version behind: the file's error alone.
#[test]
fn a_failed_reload_of_a_skill_not_loaded_keeps_no_warning() {
    let tmp = TempDir::new().unwrap();
    let path = write_skill(
        tmp.path(),
        "off",
        &skill_md("name: Off\ndescription: D\nzeta: 1\ninvoke:\n  mode: disabled\n"),
    );
    let skill_dir = path.parent().unwrap().to_path_buf();

    let catalog = SkillCatalog::new();
    catalog.scan_directory(tmp.path(), SkillScope::Project);
    assert!(catalog.get("off").is_none());

    std::fs::write(&path, "---\nname: [oops\n---\n").unwrap();
    assert!(catalog.reload_skill(&skill_dir).is_err());
    assert_eq!(
        the_one(&catalog, SkillDiagnosticCode::InvalidYaml).severity,
        SkillDiagnosticSeverity::Error
    );
    assert_eq!(
        catalog.diagnostics().len(),
        1,
        "{:?}",
        catalog.diagnostics()
    );
}

/// An owner's own link to a SKILL.md elsewhere is still followed.
#[cfg(unix)]
#[test]
fn a_linked_skill_md_is_followed() {
    let tmp = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    let target = elsewhere.path().join("SKILL.md");
    std::fs::write(
        &target,
        skill_md("name: Linked\ndescription: Via a link\nrequires_capabilities: [file_read]\n"),
    )
    .unwrap();
    let skill_dir = tmp.path().join("linked");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();

    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(tmp.path(), SkillScope::Project), 1);
    assert!(catalog.get("linked").is_some());
    assert!(
        catalog.diagnostics().is_empty(),
        "{:?}",
        catalog.diagnostics()
    );
}

/// The skills a stock install seeds raise nothing — no key of theirs is
/// unknown, no slash collides.
#[test]
fn the_shipped_skills_scan_clean() {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/skills");
    let catalog = SkillCatalog::new();
    assert_eq!(catalog.scan_directory(&shipped, SkillScope::Project), 4);
    assert!(
        catalog.diagnostics().is_empty(),
        "{:?}",
        catalog.diagnostics()
    );
}

/// A diagnostic serializes as the wire shape `GET /v1/skills/diagnostics`
/// serves.
#[test]
fn a_diagnostic_serializes_with_its_five_keys() {
    let d = SkillDiagnostic::new(
        &SkillParseError::UnknownKey("tool".to_string()),
        Path::new("/skills/x/SKILL.md"),
        Some("x"),
    );
    assert_eq!(
        serde_json::to_value(&d).unwrap(),
        serde_json::json!({
            "code": "unknown_key",
            "severity": "warning",
            "path": "/skills/x/SKILL.md",
            "skill_id": "x",
            "message": "Unknown frontmatter key 'tool' is ignored",
        })
    );
}
