use super::*;
use crate::store::tests::HomeStoreGuard;
use chrono::TimeZone;
use std::fs;
use tempfile::tempdir;

// ============================================================================
// slugify
// ============================================================================

#[test]
fn strips_path_separators_and_traversal() {
    assert_eq!(slugify("../../etc/passwd", 60), "etc-passwd");
    assert_eq!(slugify("a/b\\c", 60), "a-b-c");
    assert_eq!(slugify("..", 60), "artifact");
    assert_eq!(slugify("/", 60), "artifact");
}

#[test]
fn drops_a_nul_byte() {
    assert_eq!(slugify("hello\u{0}world", 60), "hello-world");
}

#[test]
fn nfkd_strips_combining_marks_and_splits_compatibility_ligatures() {
    // é (precomposed) decomposes to e + combining acute; the mark is dropped.
    assert_eq!(slugify("café", 60), "cafe");
    // ﬁ (U+FB01) has a *compatibility* decomposition into "fi" — this is
    // exactly why NFKD, not NFD, is required.
    assert_eq!(slugify("\u{FB01}le", 60), "file");
}

#[test]
fn folds_case() {
    assert_eq!(slugify("HELLO World", 60), "hello-world");
}

#[test]
fn collapses_punctuation_and_unrepresentable_runs_to_one_hyphen() {
    assert_eq!(slugify("a   b!!!c", 60), "a-b-c");
    // CJK has no ASCII transliteration in this implementation; it is folded
    // to a separator exactly like whitespace or punctuation, never merging
    // the words on either side of it.
    assert_eq!(slugify("hello 文档 world", 60), "hello-world");
}

#[test]
fn trims_leading_and_trailing_separators() {
    assert_eq!(slugify("  --hello--  ", 60), "hello");
}

#[test]
fn empty_and_all_punctuation_input_falls_back_to_artifact() {
    assert_eq!(slugify("", 60), "artifact");
    assert_eq!(slugify("!!!===", 60), "artifact");
    assert_eq!(slugify("   ", 60), "artifact");
    assert_eq!(slugify("文档", 60), "artifact");
    assert_eq!(slugify("\u{1F600}", 60), "artifact"); // an emoji alone
}

#[test]
fn truncates_to_max_bytes_on_a_char_boundary() {
    let s = slugify(&"a".repeat(100), 10);
    assert_eq!(s, "a".repeat(10));
    assert_eq!(s.len(), 10);
}

#[test]
fn truncation_does_not_leave_a_trailing_hyphen() {
    // Collapsed form is "abcd-efgh"; byte offset 5 lands exactly on the
    // separator that truncation must then trim away.
    assert_eq!(slugify("abcd efgh", 5), "abcd");
}

#[test]
fn reserved_windows_device_names_get_a_guard_prefix() {
    for name in [
        "con", "PRN", "Aux", "nul", "com1", "COM9", "lpt1", "LPT9", "Com5", "lPt3",
    ] {
        let slug = slugify(name, 60);
        assert_eq!(slug, format!("_{}", name.to_ascii_lowercase()));
    }
}

#[test]
fn com10_and_lpt10_are_not_reserved() {
    // Only the single-digit COM/LPT device names are reserved.
    assert_eq!(slugify("com10", 60), "com10");
    assert_eq!(slugify("lpt10", 60), "lpt10");
    assert_eq!(slugify("console", 60), "console");
}

#[test]
fn slugify_is_pure_total_and_always_a_valid_slug_shape() {
    let is_slug_char =
        |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
    for input in [
        "",
        "A",
        "-",
        "1",
        "Hello, World! 123",
        "文档",
        "\u{1F600}",
        "../../../etc/shadow",
        "CON",
        "  spaced out title  ",
    ] {
        let slug = slugify(input, 60);
        assert!(!slug.is_empty(), "slugify({input:?}) was empty");
        assert!(
            !slug.starts_with('-') && !slug.ends_with('-'),
            "slugify({input:?}) = {slug:?} has a leading/trailing hyphen"
        );
        assert!(
            slug.chars().all(is_slug_char),
            "slugify({input:?}) = {slug:?} has a character outside the slug grammar"
        );
        // Calling it again on its own output is idempotent — a slug is
        // already in normal form.
        assert_eq!(
            slugify(&slug, 60),
            slug,
            "slugify is not idempotent on {slug:?}"
        );
    }
}

// ============================================================================
// truncate_at_char_boundary (private helper — defense in depth)
// ============================================================================

#[test]
fn truncate_at_char_boundary_backs_off_from_a_split_multibyte_char() {
    let s = "a\u{1F600}b"; // 'a' (1 byte) + an emoji (4 bytes) + 'b' (1 byte) = 6 bytes
    assert_eq!(truncate_at_char_boundary(s, 3), "a"); // 3 lands inside the emoji
    assert_eq!(truncate_at_char_boundary(s, 4), "a"); // so does 4
    assert_eq!(truncate_at_char_boundary(s, 5), "a\u{1F600}"); // 5 is right after it
    assert_eq!(truncate_at_char_boundary(s, 100), s); // no truncation needed
    assert_eq!(truncate_at_char_boundary(s, 0), "");
}

// ============================================================================
// confine_to_root
// ============================================================================

#[test]
fn confine_to_root_rejects_a_symlinked_parent_escape() {
    let tmp = tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();

    let candidate = root.join("escape").join("file.txt");
    let err = confine_to_root(&root, &candidate).unwrap_err();
    assert!(
        err.to_string().contains("escapes"),
        "unexpected error: {err}"
    );
}

#[test]
fn confine_to_root_accepts_an_in_root_path_with_a_not_yet_existing_leaf() {
    let tmp = tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let candidate = root.join("sub").join("new.txt");
    assert!(!candidate.exists());

    let resolved = confine_to_root(&root, &candidate).unwrap();
    assert_eq!(resolved, candidate);
}

#[test]
fn confine_to_root_accepts_candidate_equal_to_root() {
    let tmp = tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    assert_eq!(confine_to_root(&root, &root).unwrap(), root);
}

#[test]
fn confine_to_root_rejects_relative_paths() {
    let tmp = tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    assert!(confine_to_root(Path::new("relative"), &root.join("x")).is_err());
    assert!(confine_to_root(&root, Path::new("relative")).is_err());
}

#[test]
fn confine_to_root_rejects_dotdot_components() {
    let tmp = tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    let candidate = root.join("sub").join("..").join("evil");
    assert!(confine_to_root(&root, &candidate).is_err());
}

#[test]
fn confine_to_root_rejects_a_missing_root() {
    let tmp = tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("does-not-exist");
    assert!(confine_to_root(&root, &root.join("x")).is_err());
}

// ============================================================================
// run_dir / loose_dir
// ============================================================================

#[test]
fn run_dir_name_matches_the_grammar_and_stays_within_68_bytes() {
    let tmp = tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap().join("home");
    let _guard = HomeStoreGuard::set(&home);

    let created = Utc.with_ymd_and_hms(2026, 9, 1, 8, 0, 0).unwrap();
    let title = "a".repeat(200); // far longer than the 48-byte slug budget
    let task_id = "3f2a1b7c-89ab-4cde-8123-456789abcdef";

    let dir = run_dir(&StoreScope::Home, created, &title, task_id).unwrap();
    let name = dir.file_name().unwrap().to_str().unwrap();

    assert_eq!(name, format!("2026-09-01-{}-3f2a1b7c", "a".repeat(48)));
    assert_eq!(
        name.len(),
        68,
        "run_dir name is {} bytes, expected 68",
        name.len()
    );

    let artifacts_root = content_dir(&StoreScope::Home, ContentKind::Artifacts).unwrap();
    assert_eq!(dir, artifacts_root.join(name));
}

#[test]
fn run_dir_uses_the_first_eight_characters_of_the_task_id() {
    let tmp = tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap().join("home");
    let _guard = HomeStoreGuard::set(&home);
    let created = Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap();

    let dir = run_dir(&StoreScope::Home, created, "Weekly report", "short").unwrap();
    // A task id shorter than 8 characters is used whole, not padded.
    assert!(
        dir.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with("-short")
    );
}

#[test]
fn loose_dir_is_artifacts_loose_date() {
    let tmp = tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap().join("home");
    let _guard = HomeStoreGuard::set(&home);
    let created = Utc.with_ymd_and_hms(2026, 9, 1, 23, 59, 0).unwrap();

    let dir = loose_dir(&StoreScope::Home, created).unwrap();
    let artifacts_root = content_dir(&StoreScope::Home, ContentKind::Artifacts).unwrap();
    assert_eq!(dir, artifacts_root.join("loose").join("2026-09-01"));
}

// ============================================================================
// artifact_file_name / version_file_path
// ============================================================================

#[test]
fn artifact_file_name_widens_the_sequence_prefix_past_99() {
    assert_eq!(artifact_file_name(1, "Findings", "md"), "01-findings.md");
    assert_eq!(artifact_file_name(9, "Findings", "md"), "09-findings.md");
    assert_eq!(artifact_file_name(99, "Findings", "md"), "99-findings.md");
    assert_eq!(artifact_file_name(100, "Findings", "md"), "100-findings.md");
    assert_eq!(
        artifact_file_name(1000, "Findings", "md"),
        "1000-findings.md"
    );
}

#[test]
fn artifact_file_name_sanitizes_the_extension() {
    assert_eq!(artifact_file_name(1, "T", ".MD"), "01-t.md");
    assert_eq!(artifact_file_name(1, "T", ""), "01-t.bin");
    assert_eq!(artifact_file_name(1, "T", "way-too-long"), "01-t.bin");
}

#[test]
fn artifact_file_name_stays_within_72_bytes_for_the_common_two_digit_case() {
    let name = artifact_file_name(42, &"word ".repeat(50), "markdown");
    assert!(
        name.len() <= 72,
        "{name} is {} bytes, expected <= 72",
        name.len()
    );
}

#[test]
fn artifact_file_name_may_exceed_72_bytes_once_the_sequence_widens_past_99() {
    // The grammar's two-digit `NN-` prefix widens rather than truncates past
    // 99 (§4.2) — the 72-byte bound is documented for the common case only.
    let name = artifact_file_name(100, &"word ".repeat(50), "markdown");
    assert!(name.starts_with("100-"));
}

#[test]
fn version_file_path_shape() {
    let head = Path::new("/home/artifacts/run/01-findings.md");
    let v = version_file_path(head, 1).unwrap();
    assert_eq!(
        v,
        Path::new("/home/artifacts/run/.versions/01-findings/v1.md")
    );
}

#[test]
fn version_file_path_without_an_extension() {
    let head = Path::new("/home/artifacts/run/01-findings");
    let v = version_file_path(head, 2).unwrap();
    assert_eq!(v, Path::new("/home/artifacts/run/.versions/01-findings/v2"));
}

#[test]
fn version_file_path_rejects_a_path_with_no_parent() {
    assert!(version_file_path(Path::new("/"), 1).is_err());
}

// ============================================================================
// artifact_extension precedence
// ============================================================================

#[test]
fn artifact_extension_prefers_an_allow_listed_name_hint() {
    assert_eq!(
        artifact_extension(ArtifactKind::Binary, Some("image/png"), Some("report.CSV")),
        "csv"
    );
}

#[test]
fn artifact_extension_ignores_a_name_hint_extension_that_is_not_allow_listed() {
    // "exe" is not on the allow-list, so this falls through to the kind map
    // rather than trusting an arbitrary model-supplied extension.
    assert_eq!(
        artifact_extension(ArtifactKind::Markdown, None, Some("payload.exe")),
        "md"
    );
}

#[test]
fn artifact_extension_kind_map_wins_over_mime_when_no_usable_name_hint() {
    assert_eq!(
        artifact_extension(ArtifactKind::Html, Some("text/plain"), None),
        "html"
    );
    assert_eq!(
        artifact_extension(ArtifactKind::Plan, Some("text/plain"), Some("plan.exe")),
        "md"
    );
}

#[test]
fn artifact_extension_falls_to_mime_when_kind_has_no_fixed_extension() {
    assert_eq!(
        artifact_extension(ArtifactKind::Code, Some("text/x-python"), None),
        "py"
    );
    assert_eq!(
        artifact_extension(ArtifactKind::Image, Some("image/webp"), None),
        "webp"
    );
}

#[test]
fn artifact_extension_mime_heuristic_fallback_for_an_unlisted_mime() {
    assert_eq!(
        artifact_extension(ArtifactKind::Binary, Some("application/x-toml"), None),
        "toml"
    );
}

#[test]
fn artifact_extension_defaults_to_bin() {
    assert_eq!(artifact_extension(ArtifactKind::Binary, None, None), "bin");
    assert_eq!(artifact_extension(ArtifactKind::Code, None, None), "bin");
    assert_eq!(
        artifact_extension(ArtifactKind::Binary, Some("application/octet-stream"), None),
        "bin"
    );
}

// ============================================================================
// D2 upload placement
// ============================================================================

#[test]
fn upload_dir_is_uploads_date() {
    let tmp = tempdir().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap().join("home");
    let _guard = HomeStoreGuard::set(&home);
    let created = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();

    let dir = upload_dir(&StoreScope::Home, created).unwrap();
    let uploads_root = content_dir(&StoreScope::Home, ContentKind::Uploads).unwrap();
    assert_eq!(dir, uploads_root.join("2026-09-01"));
}

#[test]
fn upload_dir_confines_to_the_uploads_root_for_a_project_scope() {
    let tmp = tempdir().unwrap();
    let project = fs::canonicalize(tmp.path()).unwrap();
    let created = Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap();

    let dir = upload_dir(&StoreScope::Project(project.clone()), created).unwrap();
    assert_eq!(
        dir,
        project
            .join(".openalpaca")
            .join("uploads")
            .join("2026-09-02")
    );
}

#[test]
fn upload_file_name_grammar() {
    assert_eq!(upload_file_name(1, "My Report.PDF"), "01-my-report.pdf");
    assert_eq!(upload_file_name(2, "IMG_0001.jpeg"), "02-img-0001.jpeg");
    assert_eq!(
        upload_file_name(1, "no_extension_file"),
        "01-no-extension-file.bin"
    );
    assert_eq!(upload_file_name(1, ".gitignore"), "01-gitignore.bin");
    assert_eq!(upload_file_name(3, "archive.tar.gz"), "03-archive-tar.gz");
}

// ============================================================================
// End-to-end grammar shape (plan §4.1's worked example)
// ============================================================================

#[test]
fn full_grammar_matches_the_worked_example_in_the_plan() {
    let tmp = tempdir().unwrap();
    let project = fs::canonicalize(tmp.path()).unwrap();
    let scope = StoreScope::Project(project);
    let created = Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap();
    let task_id = "3f2a1b7c-1111-4222-8333-444455556666";

    let dir = run_dir(&scope, created, "Connector audit", task_id).unwrap();
    assert_eq!(
        dir.file_name().unwrap().to_str().unwrap(),
        "2026-09-01-connector-audit-3f2a1b7c"
    );

    let ext = artifact_extension(ArtifactKind::Markdown, None, None);
    let file_name = artifact_file_name(1, "Connector audit findings", &ext);
    assert_eq!(file_name, "01-connector-audit-findings.md");

    let head = dir.join(&file_name);
    let v1 = version_file_path(&head, 1).unwrap();
    assert_eq!(
        v1,
        dir.join(".versions")
            .join("01-connector-audit-findings")
            .join("v1.md")
    );
}
