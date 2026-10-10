//! `store::projects` (storage design rev 0.8, Stage 1, slice S1). Every test
//! that reaches a home store holds the crate's `HomeStoreGuard`, through
//! `World` or directly, so none reads or writes the developer's
//! `~/.openalpaca`.

use super::*;
use crate::store::tests::HomeStoreGuard;
use std::fs;
use tempfile::{TempDir, tempdir};

/// A v4 UUID in its canonical spelling (third group starts with 4, fourth
/// with 9).
const ID: &str = "0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b";

#[test]
fn a_project_id_reads_only_its_canonical_spelling() {
    let id = ProjectId::parse(ID).unwrap();
    assert_eq!(id.to_string(), ID);
    assert_eq!(ID.parse::<ProjectId>().unwrap(), id);
    for other in [
        "0B0E0D4C-5A6F-4E3D-9C2B-1A0F9E8D7C6B",
        "{0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b}",
        "0b0e0d4c5a6f4e3d9c2b1a0f9e8d7c6b",
        "urn:uuid:0b0e0d4c-5a6f-4e3d-9c2b-1a0f9e8d7c6b",
        "",
        "not-an-id",
    ] {
        assert!(
            ProjectId::parse(other).is_err(),
            "{other:?} must be refused"
        );
    }
}

#[test]
fn a_new_project_id_is_a_v4_uuid_in_its_canonical_spelling() {
    let id = ProjectId::new();
    assert_eq!(id.as_uuid().get_version_num(), 4);
    assert_eq!(ProjectId::parse(&id.to_string()).unwrap(), id);
    assert_ne!(ProjectId::new(), id);
}

#[test]
fn a_project_id_travels_through_json_as_its_canonical_string() {
    let id = ProjectId::parse(ID).unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{ID}\""));
    assert_eq!(
        serde_json::from_str::<ProjectId>(&format!("\"{ID}\"")).unwrap(),
        id
    );
    let upper = format!("\"{}\"", ID.to_uppercase());
    assert!(serde_json::from_str::<ProjectId>(&upper).is_err());
}

// ── Folder names (S1.2) ─────────────────────────────────────────────────────

#[test]
fn a_folder_name_is_the_root_with_every_other_character_a_dash() {
    // R5's table, as the design writes it.
    for (root, name) in [
        (
            "/Users/tangyunpei/dev/AlpacaLink",
            "-Users-tangyunpei-dev-AlpacaLink",
        ),
        (
            "/Users/tangyunpei/dev/my_app",
            "-Users-tangyunpei-dev-my-app",
        ),
        ("/Users/tangyunpei/文档/笔记", "-Users-tangyunpei-文档-笔记"),
    ] {
        assert_eq!(folder_name(Path::new(root)), name, "{root}");
    }
}

#[test]
fn runs_of_other_characters_are_not_collapsed() {
    assert_eq!(folder_name(Path::new("/srv/a b/.c_d")), "-srv-a-b--c-d");
}

#[test]
fn a_folder_name_is_nfc_and_keeps_combining_marks() {
    // `e` + U+0301 composes to `é` (U+00E9) …
    assert_eq!(folder_name(Path::new("/srv/cafe\u{301}")), "-srv-caf\u{e9}");
    // … and a mark with nothing to compose with stays, as a mark.
    assert_eq!(folder_name(Path::new("/srv/x\u{301}")), "-srv-x\u{301}");
    // Devanagari: the virama (U+094D) is a mark and not a letter, and the
    // word is not whole without it (R5).
    let hindi = "\u{939}\u{93F}\u{928}\u{94D}\u{926}\u{940}";
    assert!(
        !'\u{94D}'.is_alphanumeric(),
        "the case this test exists for"
    );
    assert_eq!(
        folder_name(Path::new(&format!("/home/{hindi}"))),
        format!("-home-{hindi}")
    );
}

#[test]
fn two_roots_with_one_plain_name_have_two_hashed_names() {
    let (underscore, dash) = (Path::new("/a/b_c"), Path::new("/a/b-c"));
    assert_eq!(folder_name(underscore), "-a-b-c");
    assert_eq!(folder_name(dash), "-a-b-c");
    // R6: <plain>-<8 hex of sha256(the canonical path)>.
    assert_eq!(hashed_folder_name(underscore), "-a-b-c-df04c200");
    assert_eq!(hashed_folder_name(dash), "-a-b-c-693a1a8d");
}

#[test]
fn the_hash_is_over_the_path_as_the_volume_spells_it_not_its_nfc_form() {
    let decomposed = Path::new("/srv/cafe\u{301}");
    let composed = Path::new("/srv/caf\u{e9}");
    assert_eq!(folder_name(decomposed), folder_name(composed));
    assert_eq!(hashed_folder_name(decomposed), "-srv-caf\u{e9}-559b2d05");
    assert_eq!(hashed_folder_name(composed), "-srv-caf\u{e9}-9e472a79");
}

#[test]
fn a_plain_name_of_exactly_two_hundred_bytes_is_used_whole() {
    let root = format!("/{}", "a".repeat(199));
    let name = folder_name(Path::new(&root));
    assert_eq!(name.len(), NAME_MAX_BYTES);
    assert_eq!(name, format!("-{}", "a".repeat(199)));
}

#[test]
fn a_longer_plain_name_is_cut_at_two_hundred_bytes_and_always_hashed() {
    let root = format!("/{}", "a".repeat(200));
    let expected = format!("-{}-f7b36aaa", "a".repeat(199));
    assert_eq!(folder_name(Path::new(&root)), expected);
    // R7's name already carries the hash: R6 has nothing to add.
    assert_eq!(hashed_folder_name(Path::new(&root)), expected);
}

#[test]
fn a_cut_never_splits_a_character() {
    // 1 + 80 × 3 = 241 bytes; the last boundary at or below 200 is 199.
    let root = format!("/{}", "文".repeat(80));
    let name = folder_name(Path::new(&root));
    assert_eq!(name, format!("-{}-479c5867", "文".repeat(66)));
    assert_eq!(name.len(), 199 + 1 + NAME_HASH_HEX);
}

// ── Roots (S1.3) ────────────────────────────────────────────────────────────

/// A temp directory holding the home store (`<tmp>/home`, set through
/// `OPENALPACA_HOME_STORE` for the world's life) and, beside it, the folders a
/// test makes projects of (`<tmp>/work/…`) — beside, never above: a folder
/// that contains the home store is never a project (R13).
struct World {
    _env: HomeStoreGuard,
    tmp: TempDir,
}

impl World {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        let env = HomeStoreGuard::set(&tmp.path().join("home"));
        Self { _env: env, tmp }
    }

    fn home(&self) -> PathBuf {
        self.tmp.path().join("home")
    }

    /// `<tmp>/work/<rel>`, created, as the volume spells it.
    fn folder(&self, rel: &str) -> PathBuf {
        let dir = self.tmp.path().join("work").join(rel);
        fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }
}

#[cfg(unix)]
#[test]
fn a_canonical_root_resolves_links_and_the_volumes_own_spelling() {
    let tmp = tempdir().unwrap();
    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    // On macOS a temp dir is `/var/…`, and the volume spells it `/private/var/…`.
    let canonical = real.canonicalize().unwrap();
    assert_eq!(canonical_root(&real).unwrap(), canonical);
    assert_eq!(canonical_root(&link).unwrap(), canonical);
    assert_eq!(
        canonical_root(&link.join("missing")).unwrap(),
        canonical.join("missing")
    );
}

#[test]
fn a_canonical_root_keeps_a_missing_tail_as_written() {
    let tmp = tempdir().unwrap();
    let gone = tmp.path().join("gone").join("deeper");
    assert_eq!(
        canonical_root(&gone).unwrap(),
        tmp.path()
            .canonicalize()
            .unwrap()
            .join("gone")
            .join("deeper")
    );
}

#[test]
fn a_relative_path_has_no_canonical_root_even_when_it_exists_here() {
    // `src` exists under the test's working directory (the crate root).
    // Resolving it there would make the daemon's CWD a project (R22).
    assert!(Path::new("src").is_dir());
    let err = canonical_root(Path::new("src")).unwrap_err();
    assert!(err.to_string().contains("absolute"), "{err}");
}

#[test]
fn dot_dot_through_a_missing_folder_has_no_canonical_root() {
    let tmp = tempdir().unwrap();
    assert!(canonical_root(&tmp.path().join("gone").join("..").join("other")).is_err());
    // Through a folder that exists, `..` resolves like any link.
    fs::create_dir(tmp.path().join("here")).unwrap();
    assert_eq!(
        canonical_root(&tmp.path().join("here").join("..").join("other")).unwrap(),
        tmp.path().canonicalize().unwrap().join("other")
    );
}

#[test]
fn the_home_store_what_lies_inside_it_and_what_contains_it_overlap_it() {
    let w = World::new();
    assert!(overlaps_home_store(&w.home()));
    assert!(overlaps_home_store(&w.home().join("projects").join("app")));
    assert!(overlaps_home_store(w.tmp.path()));
    assert!(overlaps_home_store(Path::new("/")));
    assert!(!overlaps_home_store(&w.folder("project")));
    assert!(!overlaps_home_store(&w.tmp.path().join("home-sibling")));
    assert!(!overlaps_home_store(Path::new("relative/home")));
}

#[cfg(unix)]
#[test]
fn a_home_store_reached_through_a_link_still_overlaps() {
    let w = World::new();
    fs::create_dir_all(w.home()).unwrap();
    let link = w.tmp.path().join("store-link");
    std::os::unix::fs::symlink(w.home(), &link).unwrap();
    assert!(overlaps_home_store(&link));
    assert!(overlaps_home_store(&link.join("artifacts")));
}

#[test]
fn with_no_usable_home_store_nothing_overlaps_it() {
    // `home_root()` refuses a relative override.
    let _env = HomeStoreGuard::set(Path::new("relative/store"));
    assert!(!overlaps_home_store(Path::new("/")));
}

// ── The file, the entry and the refusals (S1.4) ─────────────────────────────

const ROOT: &str = "/Users/someone/dev/AlpacaLink";

fn sample_file() -> ProjectFile {
    ProjectFile {
        version: 1,
        id: ProjectId::parse(ID).unwrap(),
        root: ROOT.to_string(),
        created_at: "2026-10-07T09:30:00Z".to_string(),
        previous_root: None,
        previous_name: None,
    }
}

#[test]
fn a_project_file_serializes_in_r9s_order_with_no_re_point_keys() {
    let text = String::from_utf8(serde_json::to_vec_pretty(&sample_file()).unwrap()).unwrap();
    assert_eq!(
        text,
        format!(
            "{{\n  \"version\": 1,\n  \"id\": \"{ID}\",\n  \"root\": \"{ROOT}\",\n  \"created_at\": \"2026-10-07T09:30:00Z\"\n}}"
        )
    );
}

#[test]
fn a_re_point_in_flight_adds_its_two_keys_after_created_at() {
    let mut file = sample_file();
    file.previous_root = Some("/Users/someone/work/AlpacaLink".to_string());
    file.previous_name = Some("-Users-someone-work-AlpacaLink".to_string());
    let text = String::from_utf8(serde_json::to_vec_pretty(&file).unwrap()).unwrap();
    assert!(
        text.ends_with(
            "\"created_at\": \"2026-10-07T09:30:00Z\",\n  \"previous_root\": \"/Users/someone/work/AlpacaLink\",\n  \"previous_name\": \"-Users-someone-work-AlpacaLink\"\n}"
        ),
        "{text}"
    );
    assert_eq!(serde_json::from_str::<ProjectFile>(&text).unwrap(), file);
}

#[test]
fn a_project_file_without_the_re_point_keys_reads_them_as_absent() {
    let text = format!(
        r#"{{"version": 1, "id": "{ID}", "root": "{ROOT}", "created_at": "2026-10-07T09:30:00Z"}}"#
    );
    assert_eq!(
        serde_json::from_str::<ProjectFile>(&text).unwrap(),
        sample_file()
    );
}

#[test]
fn a_project_hands_back_the_file_it_was_read_from() {
    let project = Project {
        id: ProjectId::parse(ID).unwrap(),
        root: PathBuf::from(ROOT),
        name: "-Users-someone-dev-AlpacaLink".to_string(),
        dir: PathBuf::from("/Users/someone/.openalpaca/projects/-Users-someone-dev-AlpacaLink"),
        created_at: "2026-10-07T09:30:00Z".to_string(),
        missing: false,
        previous_root: None,
        previous_name: None,
    };
    assert_eq!(project.root_string(), ROOT);
    assert_eq!(project.file(), sample_file());
}

#[test]
fn each_refusal_has_its_code_and_one_sentence() {
    let p = "/work/app".to_string();
    let cases = [
        (
            ProjectError::NotRegistered { root: p.clone() },
            "WORKSPACE_NOT_FOUND",
            "/work/app is not a project OpenAlpaca knows: no folder under projects/ records it",
        ),
        (
            ProjectError::IsHome { path: p.clone() },
            "WORKSPACE_IS_HOME",
            "/work/app is, contains or lies inside the OpenAlpaca home store, so it cannot be a project. Name the project directory itself",
        ),
        (
            ProjectError::NotADirectory { path: p.clone() },
            "WORKSPACE_NOT_A_DIRECTORY",
            "/work/app is not a directory. Create it first, or name the folder the project is in now",
        ),
        (
            ProjectError::Exists { path: p.clone() },
            "WORKSPACE_EXISTS",
            "/work/app is already a project of its own, or OpenAlpaca's records already name it. Re-pointing onto it would merge two projects: delete that project's data first, or re-point onto another path",
        ),
        (
            ProjectError::RepointUnfinished { path: p.clone() },
            "WORKSPACE_EXISTS",
            "/work/app is either path of another project's re-point that has not finished; the next start of the daemon finishes that re-point. Re-pointing onto it would merge two projects: re-point onto another path",
        ),
        (
            ProjectError::SamePath { path: p.clone() },
            "INVALID_PATH",
            "/work/app is already where the project is",
        ),
    ];
    for (error, code, text) in cases {
        assert_eq!(error.code(), code);
        assert_eq!(error.to_string(), text);
    }
}

#[test]
fn not_registered_is_recognised_through_any_context() {
    let refusal = || {
        anyhow::Error::new(ProjectError::NotRegistered {
            root: "/work/app".to_string(),
        })
    };
    assert!(is_not_registered(&refusal()));
    assert!(is_not_registered(
        &refusal().context("opening the project's artifacts")
    ));
    assert!(!is_not_registered(&anyhow::Error::new(
        ProjectError::IsHome {
            path: "/".to_string()
        }
    )));
    assert!(!is_not_registered(&anyhow::anyhow!(
        "/work/app is not a project OpenAlpaca knows"
    )));
}

// ── Reading and lookup (S1.5) ───────────────────────────────────────────────

/// A `project.json` naming `root`, with a fresh id, as R9 writes one.
fn file_for(root: &Path) -> ProjectFile {
    ProjectFile {
        version: PROJECT_FILE_VERSION,
        id: ProjectId::new(),
        root: root.to_string_lossy().into_owned(),
        created_at: "2026-10-07T09:30:00Z".to_string(),
        previous_root: None,
        previous_name: None,
    }
}

impl World {
    fn projects(&self) -> PathBuf {
        self.home().join(PROJECTS_DIR)
    }

    /// `projects/<name>/project.json` holding `file`, written by hand — as
    /// another build, a copy or a person leaves one.
    fn plant(&self, name: &str, file: &ProjectFile) -> PathBuf {
        let dir = self.projects().join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut bytes = serde_json::to_vec_pretty(file).unwrap();
        bytes.push(b'\n');
        fs::write(dir.join(PROJECT_FILE), bytes).unwrap();
        dir
    }
}

#[test]
fn a_fresh_home_has_no_projects_and_lookup_creates_nothing() {
    let w = World::new();
    assert_eq!(projects_dir().unwrap(), w.projects());
    assert_eq!(lookup(&w.folder("app")).unwrap(), None);
    assert!(!w.projects().exists());
}

#[test]
fn lookup_finds_a_project_at_its_plain_name() {
    let w = World::new();
    let root = w.folder("dev/my_app");
    let file = file_for(&root);
    w.plant(&folder_name(&root), &file);

    let project = lookup(&root).unwrap().expect("registered");
    assert_eq!(project.id, file.id);
    assert_eq!(project.root, root);
    assert_eq!(project.name, folder_name(&root));
    assert_eq!(
        project.dir,
        w.projects().canonicalize().unwrap().join(&project.name)
    );
    assert!(!project.missing);
    assert_eq!(project.file(), file);
}

#[cfg(unix)]
#[test]
fn lookup_through_a_link_finds_the_root_it_reaches() {
    let w = World::new();
    let root = w.folder("real");
    let file = file_for(&root);
    w.plant(&folder_name(&root), &file);
    let link = w.tmp.path().join("work").join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert_eq!(lookup(&link).unwrap().map(|p| p.id), Some(file.id));
}

#[test]
fn lookup_reads_the_hashed_folder_when_the_plain_one_records_another_root() {
    let w = World::new();
    let (underscore, dash) = (w.folder("b_c"), w.folder("b-c"));
    assert_eq!(folder_name(&underscore), folder_name(&dash));
    let first = file_for(&underscore);
    let second = file_for(&dash);
    w.plant(&folder_name(&underscore), &first);
    w.plant(&hashed_folder_name(&dash), &second);
    assert_eq!(lookup(&underscore).unwrap().map(|p| p.id), Some(first.id));
    assert_eq!(
        lookup(&dash).unwrap().map(|p| (p.id, p.name)),
        Some((second.id, hashed_folder_name(&dash)))
    );
}

#[test]
#[tracing_test::traced_test]
fn a_folder_with_no_project_json_is_skipped_and_warned_about_once() {
    let w = World::new();
    let root = w.folder("skipped");
    let plain = w.projects().join(folder_name(&root));
    fs::create_dir_all(&plain).unwrap();
    let file = file_for(&root);
    w.plant(&hashed_folder_name(&root), &file);

    assert_eq!(lookup(&root).unwrap().map(|p| p.id), Some(file.id));
    assert_eq!(lookup(&root).unwrap().map(|p| p.id), Some(file.id));
    assert!(plain.is_dir(), "never deleted (R11)");
    let shown = plain.display().to_string();
    logs_assert(|lines: &[&str]| {
        let n = lines
            .iter()
            .filter(|l| {
                l.contains("WARN") && l.contains(&shown) && l.contains("has no project.json")
            })
            .count();
        if n == 1 {
            Ok(())
        } else {
            Err(format!("one WARN for {shown}, got {n}"))
        }
    });
}

#[test]
#[tracing_test::traced_test]
fn a_project_json_this_build_cannot_read_is_never_opened() {
    let w = World::new();
    let root = w.folder("app");
    let good = file_for(&root);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not-json", b"{ this is not json".to_vec()),
        (
            "version-two",
            serde_json::to_vec(&serde_json::json!({
                "version": 2, "id": good.id, "root": good.root, "created_at": good.created_at
            }))
            .unwrap(),
        ),
        (
            "relative-root",
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "id": good.id, "root": "dev/app", "created_at": good.created_at
            }))
            .unwrap(),
        ),
        (
            "loose-id",
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "id": good.id.to_string().to_uppercase(),
                "root": good.root, "created_at": good.created_at
            }))
            .unwrap(),
        ),
    ];
    for (name, bytes) in &cases {
        let dir = w.projects().join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(PROJECT_FILE), bytes).unwrap();
        assert_eq!(read_project_dir(&dir).unwrap(), None, "{name}");
        assert_eq!(read_project_dir(&dir).unwrap(), None, "{name}");
        assert!(dir.join(PROJECT_FILE).exists(), "{name}: never deleted");
    }
    let names: Vec<String> = cases.iter().map(|(name, _)| name.to_string()).collect();
    logs_assert(move |lines: &[&str]| {
        for name in &names {
            let marker = format!("/{name} is not opened as a project");
            let n = lines
                .iter()
                .filter(|l| l.contains("WARN") && l.contains(&marker))
                .count();
            if n != 1 {
                return Err(format!("one WARN for {name}, got {n}"));
            }
        }
        Ok(())
    });
}

#[cfg(unix)]
#[test]
fn a_linked_folder_or_a_linked_project_json_is_never_opened() {
    let w = World::new();
    let root = w.folder("app");
    let real = w.plant("real", &file_for(&root));
    let linked_dir = w.projects().join("linked-dir");
    std::os::unix::fs::symlink(&real, &linked_dir).unwrap();
    assert_eq!(read_project_dir(&linked_dir).unwrap(), None);
    let linked_file = w.projects().join("linked-file");
    fs::create_dir_all(&linked_file).unwrap();
    std::os::unix::fs::symlink(real.join(PROJECT_FILE), linked_file.join(PROJECT_FILE)).unwrap();
    assert_eq!(read_project_dir(&linked_file).unwrap(), None);
    assert!(read_project_dir(&real).unwrap().is_some());
}

#[test]
fn a_project_json_with_a_key_this_build_does_not_know_still_reads() {
    let w = World::new();
    let root = w.folder("app");
    let file = file_for(&root);
    let dir = w.projects().join(folder_name(&root));
    fs::create_dir_all(&dir).unwrap();
    let mut value = serde_json::to_value(&file).unwrap();
    value["written_by"] = serde_json::json!("a newer build");
    fs::write(
        dir.join(PROJECT_FILE),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    assert_eq!(lookup(&root).unwrap().map(|p| p.id), Some(file.id));
}

#[test]
fn a_missing_project_is_found_at_the_path_it_was_registered_at() {
    let w = World::new();
    let root = w.folder("moved-away");
    let file = file_for(&root);
    w.plant(&folder_name(&root), &file);
    fs::remove_dir(&root).unwrap();
    let project = lookup(&root).unwrap().expect("still registered (ST-7)");
    assert_eq!(project.id, file.id);
    assert!(project.missing);
}

#[test]
fn lookup_refuses_a_relative_root() {
    let _w = World::new();
    assert!(lookup(Path::new("dev/app")).is_err());
}

// ── list and by_id (S1.6) ───────────────────────────────────────────────────

#[test]
fn list_is_empty_and_creates_nothing_without_a_projects_folder() {
    let w = World::new();
    assert_eq!(list().unwrap(), Vec::new());
    assert!(!w.projects().exists());
}

#[test]
fn list_reads_every_project_by_folder_name_and_marks_the_missing_one() {
    let w = World::new();
    let (b, a, gone) = (w.folder("b"), w.folder("a"), w.folder("gone"));
    for root in [&b, &a, &gone] {
        w.plant(&folder_name(root), &file_for(root));
    }
    fs::remove_dir(&gone).unwrap();

    let listed = list().unwrap();
    let names: Vec<String> = listed.iter().map(|p| p.name.clone()).collect();
    let mut expected = vec![folder_name(&a), folder_name(&b), folder_name(&gone)];
    expected.sort();
    assert_eq!(names, expected);
    let missing: Vec<PathBuf> = listed
        .iter()
        .filter(|p| p.missing)
        .map(|p| p.root.clone())
        .collect();
    assert_eq!(missing, vec![gone]);
}

#[cfg(unix)]
#[test]
fn list_passes_over_hidden_folders_links_and_files() {
    let w = World::new();
    let root = w.folder("kept");
    let kept = w.plant(&folder_name(&root), &file_for(&root));
    // A folder `register` is still building, a link, and a stray file.
    w.plant(".-half-built.1-0.tmp", &file_for(&w.folder("half")));
    std::os::unix::fs::symlink(&kept, w.projects().join("a-link")).unwrap();
    fs::write(w.projects().join("notes.txt"), "mine").unwrap();

    let names: Vec<String> = list().unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, vec![folder_name(&root)]);
}

#[test]
#[tracing_test::traced_test]
fn two_folders_with_one_id_are_both_left_out_and_reported_once() {
    let w = World::new();
    let root = w.folder("copied");
    let file = file_for(&root);
    let original = w.plant(&folder_name(&root), &file);
    let copy = w.plant("copy-of-it", &file);
    let other_root = w.folder("other");
    let other = file_for(&other_root);
    w.plant(&folder_name(&other_root), &other);

    for _ in 0..2 {
        let ids: Vec<ProjectId> = list().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![other.id]);
    }
    assert_eq!(by_id(&file.id).unwrap(), None);
    assert!(original.join(PROJECT_FILE).exists() && copy.join(PROJECT_FILE).exists());
    let id = file.id.to_string();
    logs_assert(move |lines: &[&str]| {
        let n = lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains(&id))
            .count();
        if n == 1 {
            Ok(())
        } else {
            Err(format!("one WARN for the shared id, got {n}"))
        }
    });
}

#[test]
fn by_id_finds_a_project_and_nothing_for_an_id_no_folder_carries() {
    let w = World::new();
    let root = w.folder("app");
    let file = file_for(&root);
    w.plant(&folder_name(&root), &file);
    assert_eq!(by_id(&file.id).unwrap().map(|p| p.root), Some(root));
    assert_eq!(by_id(&ProjectId::new()).unwrap(), None);
}

#[test]
fn an_entry_mid_re_point_is_listed_with_where_it_was() {
    let w = World::new();
    let (old, new) = (w.folder("old"), w.folder("new"));
    let mut file = file_for(&new);
    file.previous_root = Some(old.to_string_lossy().into_owned());
    file.previous_name = Some(folder_name(&old));
    w.plant(&folder_name(&old), &file);

    let listed = list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].root, new);
    assert_eq!(listed[0].previous_root.as_deref(), Some(old.as_path()));
    assert_eq!(
        listed[0].previous_name.as_deref(),
        Some(folder_name(&old).as_str())
    );
    assert_eq!(listed[0].file(), file);
}

// ── The test-build guard (S1.7) ─────────────────────────────────────────────

/// The message a call panicked with, or `None` when it returned.
fn panic_of(f: impl FnOnce()) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .err()
        .map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default()
        })
}

#[test]
fn the_guard_names_an_unset_or_real_home_store_and_passes_any_other() {
    use std::ffi::OsStr;
    let real = Path::new("/Users/someone/.openalpaca");
    assert_eq!(unguarded_home(None, Some(real)), Some("unset"));
    assert_eq!(unguarded_home(None, None), Some("unset"));
    assert_eq!(
        unguarded_home(Some(OsStr::new("/Users/someone/.openalpaca")), Some(real)),
        Some("pointing into the real home store")
    );
    assert_eq!(
        unguarded_home(
            Some(OsStr::new("/Users/someone/.openalpaca/scratch")),
            Some(real)
        ),
        Some("pointing into the real home store")
    );
    assert_eq!(
        unguarded_home(
            Some(OsStr::new("/Users/someone/.openalpaca-test")),
            Some(real)
        ),
        None
    );
    assert_eq!(
        unguarded_home(Some(OsStr::new("/tmp/oa-test/home")), Some(real)),
        None
    );
    assert_eq!(
        unguarded_home(Some(OsStr::new("/tmp/oa-test/home")), None),
        None
    );
}

#[test]
fn the_guard_is_armed_in_this_crates_test_build() {
    assert!(test_home_guard_armed());
}

#[test]
fn an_unguarded_home_store_stops_a_test_build_and_nothing_is_written() {
    let user_home = tempdir().unwrap();
    let real_store = user_home.path().join(".openalpaca");
    for (store, why) in [
        (None, "unset"),
        (
            Some(real_store.clone()),
            "pointing into the real home store",
        ),
    ] {
        let _env = HomeStoreGuard::for_user_home(user_home.path(), store.as_deref());
        // With HOME moved, the store `home_root` falls back to is a temp dir.
        assert_eq!(crate::store::home_root().unwrap(), real_store);
        let message = panic_of(|| {
            let _ = register_home();
        })
        .expect("the guard must refuse");
        assert!(
            message.contains(&format!("{} {why}", crate::store::HOME_STORE_ENV)),
            "{message}"
        );
        assert!(
            message.contains("take the crate's home-store guard"),
            "{message}"
        );
        // The lead's R-S5: a daemon binary `cargo test` built carries the
        // check, so the message names the rebuild.
        assert!(
            message.contains("rebuild it with `cargo build -p openalpacad`"),
            "{message}"
        );
    }
    let _env = HomeStoreGuard::set(&user_home.path().join("scratch-store"));
    let mut resolved = None;
    assert_eq!(panic_of(|| resolved = Some(register_home().unwrap())), None);
    assert_eq!(resolved, Some(user_home.path().join("scratch-store")));
    assert!(!real_store.exists(), "the guard writes nothing");
}

// ── register (S1.8) ─────────────────────────────────────────────────────────

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn register_builds_a_folder_holding_project_json_and_nothing_else() {
    let w = World::new();
    let root = w.folder("dev/AlpacaLink");

    let project = register(&root).unwrap();

    let name = folder_name(&root);
    assert_eq!(project.name, name);
    assert_eq!(project.root, root);
    assert_eq!(
        project.dir,
        w.projects().canonicalize().unwrap().join(&name)
    );
    assert!(!project.missing);
    assert_eq!(project.previous_root, None);
    assert_eq!(project.previous_name, None);
    assert_eq!(project.id.as_uuid().get_version_num(), 4);
    assert!(chrono::DateTime::parse_from_rfc3339(&project.created_at).is_ok());
    assert!(
        project.created_at.len() == 20 && project.created_at.ends_with('Z'),
        "whole seconds, UTC: {}",
        project.created_at
    );
    // R3: project.json and nothing else — and no build folder left beside it.
    assert_eq!(names_in(&project.dir), vec![PROJECT_FILE.to_string()]);
    assert_eq!(names_in(&w.projects()), vec![name.clone()]);
    #[cfg(unix)]
    {
        use crate::memory_files::fsx::mode_of;
        assert_eq!(mode_of(&w.projects()), Some(0o700));
        assert_eq!(mode_of(&project.dir), Some(0o700));
        assert_eq!(mode_of(&project.dir.join(PROJECT_FILE)), Some(0o600));
    }
    // The bytes R9 writes: pretty JSON in field order, one final newline.
    assert_eq!(
        fs::read_to_string(project.dir.join(PROJECT_FILE)).unwrap(),
        format!(
            "{{\n  \"version\": 1,\n  \"id\": \"{}\",\n  \"root\": \"{}\",\n  \"created_at\": \"{}\"\n}}\n",
            project.id,
            root.display(),
            project.created_at
        )
    );
    assert_eq!(lookup(&root).unwrap(), Some(project));
}

#[test]
fn register_answers_with_the_project_already_recorded() {
    let w = World::new();
    let root = w.folder("app");
    let first = register(&root).unwrap();
    assert_eq!(register(&root).unwrap(), first);
    #[cfg(unix)]
    {
        let link = w.tmp.path().join("app-link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert_eq!(
            register(&link).unwrap(),
            first,
            "a link is the root it reaches"
        );
    }
    assert_eq!(names_in(&w.projects()).len(), 1);
}

#[test]
fn a_missing_project_is_answered_as_it_is_and_never_refused() {
    let w = World::new();
    let root = w.folder("gone");
    let first = register(&root).unwrap();
    fs::remove_dir(&root).unwrap();
    let again = register(&root).unwrap();
    assert_eq!(again.id, first.id);
    assert!(again.missing);
}

#[test]
fn the_home_store_what_lies_inside_it_and_what_contains_it_are_refused() {
    let w = World::new();
    fs::create_dir_all(w.home().join("inside")).unwrap();
    for path in [
        w.home(),
        w.home().join("inside"),
        w.tmp.path().to_path_buf(),
    ] {
        let err = register(&path).unwrap_err();
        let refusal = err.downcast_ref::<ProjectError>().expect("a ProjectError");
        assert!(
            matches!(refusal, ProjectError::IsHome { .. }),
            "{path:?}: {refusal}"
        );
        assert_eq!(refusal.code(), "WORKSPACE_IS_HOME");
    }
    assert!(!w.projects().exists(), "a refusal creates nothing");
}

#[test]
fn a_path_that_is_not_a_directory_is_refused_and_so_is_a_relative_one() {
    let w = World::new();
    let file = w.folder("parent").join("notes.txt");
    fs::write(&file, "x").unwrap();
    for path in [w.tmp.path().join("work").join("never-made"), file] {
        let err = register(&path).unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<ProjectError>(),
                Some(ProjectError::NotADirectory { .. })
            ),
            "{path:?}: {err}"
        );
    }
    let relative = register(Path::new("relative/project")).unwrap_err();
    assert!(relative.downcast_ref::<ProjectError>().is_none());
    assert!(relative.to_string().contains("absolute"), "{relative}");
    assert!(!w.projects().exists());
}

#[test]
fn a_root_whose_plain_name_another_root_holds_takes_the_hashed_name() {
    let w = World::new();
    let (underscore, dash) = (w.folder("b_c"), w.folder("b-c"));
    let first = register(&underscore).unwrap();
    let second = register(&dash).unwrap();
    assert_eq!(first.name, folder_name(&underscore));
    assert_eq!(second.name, hashed_folder_name(&dash));
    assert_ne!(first.id, second.id);
    assert_eq!(lookup(&underscore).unwrap(), Some(first));
    assert_eq!(lookup(&dash).unwrap(), Some(second));
}

#[test]
#[tracing_test::traced_test]
fn a_hand_made_folder_at_the_plain_name_is_left_alone() {
    let w = World::new();
    let root = w.folder("handmade");
    let squatter = w.projects().join(folder_name(&root));
    fs::create_dir_all(&squatter).unwrap();
    fs::write(squatter.join("notes.txt"), "mine").unwrap();

    let project = register(&root).unwrap();
    assert_eq!(project.name, hashed_folder_name(&root));
    assert_eq!(register(&root).unwrap(), project);
    // R11: never opened, never deleted — and reported once.
    assert_eq!(names_in(&squatter), vec!["notes.txt".to_string()]);
    assert_eq!(
        fs::read_to_string(squatter.join("notes.txt")).unwrap(),
        "mine"
    );
    let shown = squatter.display().to_string();
    logs_assert(move |lines: &[&str]| {
        let n = lines
            .iter()
            .filter(|l| {
                l.contains("WARN") && l.contains(&shown) && l.contains("has no project.json")
            })
            .count();
        if n == 1 {
            Ok(())
        } else {
            Err(format!("one WARN for the hand-made folder, got {n}"))
        }
    });
}

#[test]
fn when_both_names_are_taken_register_says_so_and_creates_nothing() {
    let w = World::new();
    let root = w.folder("crowded");
    let elsewhere = w.folder("elsewhere");
    w.plant(&folder_name(&root), &file_for(&elsewhere));
    w.plant(&hashed_folder_name(&root), &file_for(&elsewhere));

    let err = register(&root).unwrap_err().to_string();
    assert!(err.contains(&folder_name(&root)), "{err}");
    assert!(err.contains(&hashed_folder_name(&root)), "{err}");
    assert_eq!(names_in(&w.projects()).len(), 2);
}

#[test]
fn a_re_point_keeps_its_own_folder_when_the_new_plain_name_is_the_same() {
    let w = World::new();
    let (old, new) = (w.folder("x_y"), w.folder("x-y"));
    let project = register(&old).unwrap();
    let held = lock();
    assert_eq!(
        name_for(&new, Some(&project.id), &held).unwrap(),
        project.name
    );
    assert_eq!(
        name_for(&new, None, &held).unwrap(),
        hashed_folder_name(&new)
    );
}

#[test]
fn concurrent_registrations_of_one_root_make_one_folder() {
    let w = World::new();
    let root = w.folder("busy");
    let ids: Vec<ProjectId> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..8)
            .map(|_| s.spawn(|| register(&root).unwrap().id))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    assert_eq!(names_in(&w.projects()).len(), 1);
}

#[test]
fn the_projects_lock_is_taken_again_after_a_holder_panicked() {
    let _ = std::thread::spawn(|| {
        let _held = lock();
        panic!("a holder that panics");
    })
    .join();
    let _again = lock();
}

#[test]
fn register_refuses_an_unguarded_home_store_and_writes_nothing() {
    let user_home = tempdir().unwrap();
    let project = tempdir().unwrap();
    let real_store = user_home.path().join(".openalpaca");
    for store in [None, Some(real_store.clone())] {
        let _env = HomeStoreGuard::for_user_home(user_home.path(), store.as_deref());
        let message = panic_of(|| {
            let _ = register(project.path());
        })
        .expect("register must refuse");
        assert!(
            message.contains("take the crate's home-store guard"),
            "{message}"
        );
    }
    assert!(
        !real_store.exists(),
        "nothing was written under the user's home"
    );
}

// ── The guard and the writes name one home store (S1 review, I1) ────────────

#[test]
fn register_writes_only_under_the_home_store_it_checked_when_the_variable_changes_after() {
    // Under parallel tests another test's guard can drop — the variable
    // unset — or another be taken — the variable moved — between the check
    // and the first read. Neither may move a read or a write.
    for unset in [true, false] {
        let user_home = tempdir().unwrap();
        let store = tempdir().unwrap();
        let other = tempdir().unwrap();
        let root = tempdir().unwrap();
        let _env = HomeStoreGuard::for_user_home(user_home.path(), Some(store.path()));
        let other_store = other.path().to_path_buf();
        AFTER_HOME_RESOLVED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                // SAFETY: this test's guard holds ENV_LOCK on this thread for
                // the whole `register` call, and every test that reads or
                // writes the variable takes the same lock.
                unsafe {
                    if unset {
                        std::env::remove_var(crate::store::HOME_STORE_ENV);
                    } else {
                        std::env::set_var(crate::store::HOME_STORE_ENV, &other_store);
                    }
                }
            }));
        });

        let project = register(root.path()).unwrap();

        assert!(
            AFTER_HOME_RESOLVED.with(|hook| hook.borrow().is_none()),
            "register ran the step"
        );
        let checked = store.path().canonicalize().unwrap().join(PROJECTS_DIR);
        assert!(project.dir.starts_with(&checked), "{project:?}");
        assert!(
            !user_home.path().join(".openalpaca").exists(),
            "nothing under the user's home store (unset: {unset})"
        );
        assert!(
            !other.path().join(PROJECTS_DIR).exists(),
            "nothing under the store set after the check (unset: {unset})"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_store_that_reaches_the_real_one_by_a_link_a_dot_dot_or_another_case_is_refused() {
    let user_home = tempdir().unwrap();
    let real_store = user_home.path().join(".openalpaca");
    fs::create_dir(&real_store).unwrap();
    fs::create_dir(user_home.path().join("sub")).unwrap();
    let link = user_home.path().join("store-link");
    std::os::unix::fs::symlink(&real_store, &link).unwrap();
    let mut stores = vec![
        link.clone(),
        link.join("scratch"),
        user_home.path().join("sub").join("..").join(".openalpaca"),
    ];
    // Another case reaches the same folder only on a case-insensitive volume
    // (macOS's default); there the volume's own spelling decides.
    let other_case = user_home.path().join(".OpenAlpaca");
    if other_case.exists() {
        stores.push(other_case);
    }
    let project = tempdir().unwrap();
    for store in &stores {
        let _env = HomeStoreGuard::for_user_home(user_home.path(), Some(store));
        let message = panic_of(|| {
            let _ = register(project.path());
        })
        .unwrap_or_else(|| panic!("{store:?} must be refused"));
        assert!(
            message.contains("pointing into the real home store"),
            "{store:?}: {message}"
        );
    }
    assert_eq!(
        names_in(&real_store),
        Vec::<String>::new(),
        "nothing was written into the real store"
    );
}

// ── A folder that goes while it is read (Stage 1 review R2, R-F5) ───────────

/// `step` set to run once on this thread at `at` in `read_project_dir`.
fn during_read_on_this_thread(at: ReadStep, step: impl FnOnce() + 'static) {
    DURING_READ.with(|hook| *hook.borrow_mut() = Some((at, Box::new(step))));
}

/// A re-point renames a project's folder, and a delete removes it, under
/// `lock()`; a reader that does not hold it can find the folder gone between
/// reading its `project.json` and resolving its path. That folder is no
/// project now — never an error that fails the whole listing, the lookup or
/// `by_id` — so ST-21's check sees the project before or after the rename,
/// and the Projects list does not fail for one moved folder.
#[test]
fn a_folder_renamed_away_while_it_is_read_is_no_project_and_no_error() {
    let w = World::new();
    let away = w.tmp.path().join("renamed-away");
    fs::create_dir_all(&away).unwrap();
    type Reader = fn(&Project) -> Result<Option<Project>>;
    let readers: [(&str, Reader); 4] = [
        ("read_project_dir", |p| read_project_dir(&p.dir)),
        ("lookup", |p| lookup(&p.root)),
        ("list", |p| Ok(list()?.into_iter().find(|q| q.id == p.id))),
        ("by_id", |p| by_id(&p.id)),
    ];
    for (i, (name, read)) in readers.into_iter().enumerate() {
        let project = register(&w.folder(&format!("app-{i}"))).unwrap();
        let (dir, to) = (project.dir.clone(), away.join(i.to_string()));
        during_read_on_this_thread(ReadStep::FileRead, move || fs::rename(dir, to).unwrap());

        let found = read(&project);

        assert!(
            DURING_READ.with(|hook| hook.borrow().is_none()),
            "{name}: the folder was renamed mid-read"
        );
        assert_eq!(found.unwrap(), None, "{name}");
    }
}

/// The same race one step earlier: the folder goes between its own stat and
/// the read of its `project.json`. Nothing is reported, and R11's
/// once-per-process report is still owed to a folder of that name that
/// stays without its file.
#[test]
#[tracing_test::traced_test]
fn a_folder_renamed_away_before_its_file_is_read_spends_no_unreadable_report() {
    let w = World::new();
    let project = register(&w.folder("app")).unwrap();
    let (dir, to) = (project.dir.clone(), w.tmp.path().join("renamed-away"));
    during_read_on_this_thread(ReadStep::FolderStatted, move || {
        fs::rename(dir, to).unwrap()
    });

    assert_eq!(read_project_dir(&project.dir).unwrap(), None);

    assert!(DURING_READ.with(|hook| hook.borrow().is_none()));
    let shown = project.dir.display().to_string();
    logs_assert(move |lines: &[&str]| {
        match lines
            .iter()
            .filter(|l| l.contains("WARN") && l.contains(&shown))
            .count()
        {
            0 => Ok(()),
            n => Err(format!("{n} WARN line(s) for {shown}")),
        }
    });
    assert!(
        first_warning(format!("unreadable {}", project.dir.display())),
        "the R11 report for this folder was spent on a folder that went"
    );
}

// ── ST-23: a root that no longer leads to itself is missing (Stage 2 S0) ───

#[test]
fn a_root_is_missing_when_it_is_not_a_directory_or_no_longer_leads_to_itself() {
    let w = World::new();
    let here = w.folder("here");
    assert!(
        !root_is_missing(&here),
        "a folder that is its own canonical form"
    );
    let file = w.folder("files").join("notes.txt");
    fs::write(&file, "x").unwrap();
    assert!(root_is_missing(&file), "a file is no folder");
    assert!(
        root_is_missing(&w.folder("parent").join("never-made")),
        "nothing there"
    );
    #[cfg(unix)]
    {
        let link = w.folder("links").join("here");
        std::os::unix::fs::symlink(&here, &link).unwrap();
        assert!(link.is_dir(), "the link opens the folder");
        assert!(root_is_missing(&link), "but it leads to another path");
    }
}

/// ST-23: the folder moved and a link left at the old path — the root still
/// opens the folder, but no longer leads to itself — reads missing wherever
/// the entry is read.
#[cfg(unix)]
#[test]
fn a_project_moved_with_a_link_left_at_its_root_reads_missing() {
    let w = World::new();
    let root = w.folder("app");
    let project = register(&root).unwrap();
    assert!(!project.missing);
    let moved = root.with_file_name("app-moved");
    fs::rename(&root, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &root).unwrap();
    assert!(root.is_dir(), "the old path still opens the folder");

    let listed = list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, project.id);
    assert!(
        listed[0].missing,
        "ST-23: its root no longer leads to itself"
    );
    assert!(by_id(&project.id).unwrap().is_some_and(|p| p.missing));
    assert!(by_recorded_root(&root).unwrap().is_some_and(|p| p.missing));
}

/// ST-23 on a case-insensitive volume: the folder renamed by case only — the
/// old spelling still opens it, and the volume answers the new one — reads
/// missing.
#[cfg(unix)]
#[test]
fn a_project_renamed_by_case_only_reads_missing() {
    let w = World::new();
    let work = w.folder("cases");
    if !crate::store::tests::volume_ignores_case(&work) {
        eprintln!(
            "skipped: {} is case-sensitive, so no folder can be renamed by case only",
            work.display()
        );
        return;
    }
    let root = w.folder("cases/App");
    let project = register(&root).unwrap();
    let renamed = work.join("app");
    fs::rename(&root, &renamed).unwrap();
    assert!(root.is_dir(), "the old spelling still opens the folder");
    assert_eq!(
        root.canonicalize().unwrap(),
        renamed,
        "ST-23's premise: the volume answers the spelling on disk"
    );

    let listed = list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, project.id);
    assert!(
        listed[0].missing,
        "ST-23: its root no longer canonicalizes to itself"
    );
}
