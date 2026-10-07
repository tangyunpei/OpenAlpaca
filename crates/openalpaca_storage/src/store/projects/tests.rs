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
