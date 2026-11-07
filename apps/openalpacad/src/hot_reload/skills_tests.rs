//! The watcher's half of the skills directory against `openalpaca skills
//! tidy`'s archive (subagent design §2.12 D-3): a folder moved into
//! `skills/.archived/` is never read as a skill — not by a scan, and not by
//! the event its move raises. The store is a temporary directory, with the
//! crate's home-store guard on a home inside it.

use super::*;
use crate::test_util::HomeStoreGuard;
use openalpaca_core::bundled::BUNDLED_SKILLS;
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;

#[test]
fn a_tidied_skill_folder_under_archived_is_not_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home"));
    let skills = root.join("config/skills");
    // The current create-skill, as `skills tidy --apply` leaves it.
    let archived = skills.join(".archived/create-skill.20261107120000");
    for file in BUNDLED_SKILLS {
        if let Some(relative) = file.path.strip_prefix("skills/create-skill/") {
            let target = archived.join(relative);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, file.contents).unwrap();
        }
    }
    let catalog = SkillCatalog::new();
    assert_eq!(
        catalog.load_bundled_layer(&root.join("state/cache/bundled-skills")),
        4
    );

    // A scan loads a child folder only by its own SKILL.md.
    assert_eq!(catalog.scan_config_layer(&skills), 0);
    assert_eq!(catalog.count(), 4, "the four bundled skills, nothing more");
    for (key, entry) in catalog.entries_snapshot() {
        assert!(
            !entry
                .skill_dir
                .as_deref()
                .is_some_and(|dir| dir.starts_with(skills.join(".archived"))),
            "{key} loaded from the archive"
        );
    }
    assert!(catalog.provenance("create-skill").not_used.is_empty());
    assert!(
        catalog.diagnostics().is_empty(),
        "{:?}",
        catalog.diagnostics()
    );

    // An event under the archive — the move's own among them — names no
    // skill folder, so the watcher reloads nothing.
    for changed in [
        skills.join(".archived"),
        archived.clone(),
        archived.join("SKILL.md"),
        archived.join("scripts/scaffold.sh"),
    ] {
        assert_eq!(
            changed_skill_folder(&skills, &changed),
            None,
            "{}",
            changed.display()
        );
    }
    // Any other change names its folder, as before.
    assert_eq!(
        changed_skill_folder(&skills, &skills.join("create-skill/SKILL.md")),
        Some(skills.join("create-skill"))
    );
    assert_eq!(
        changed_skill_folder(&skills, &skills.join("my-skill")),
        Some(skills.join("my-skill"))
    );
    assert_eq!(changed_skill_folder(&skills, &skills), None);
}
