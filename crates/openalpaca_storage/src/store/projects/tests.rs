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
