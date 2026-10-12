//! R24: a project re-point the previous process did not finish is finished
//! at boot — after the database opens, before anything resolves a project
//! (design §6, *Re-point*).

use openalpaca_storage::Database;
use openalpaca_storage::project_life;
use tracing::{info, warn};

/// Runs [`project_life::recover_repoints`] and says what it did: one INFO per
/// re-point finished, one WARN per failure naming the project. A failure
/// never stops the boot — the next one tries again.
pub fn recover_repoints(db: &Database) {
    let report = project_life::recover_repoints(db);
    for root in &report.finished {
        info!("Finished re-pointing the project now at {}", root.display());
    }
    for (root, reason) in &report.failed {
        warn!(
            "Could not finish re-pointing the project now at {}: {reason}; it is tried again at the next boot",
            root.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::project_life::{RepointStep, repoint_with};
    use openalpaca_storage::store::projects;

    /// R24 at boot: a re-point the previous process left after step 1 is
    /// finished before anything resolves a project.
    #[test]
    fn a_re_point_left_unfinished_is_finished_at_boot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&root.join("home"));
        let old = root.join("work").join("app");
        let new = root.join("work").join("app-moved");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let db = Database::open(&root.join("test.db")).unwrap();
        let project = projects::register(&old).unwrap();
        repoint_with(&db, &old, &new, &mut |step| step != RepointStep::Marked).unwrap_err();
        assert_eq!(
            projects::lookup(&new).unwrap(),
            None,
            "unfinished: nothing finds it yet"
        );

        recover_repoints(&db);

        let after = projects::lookup(&new)
            .unwrap()
            .expect("the boot finished the re-point");
        assert_eq!(after.id, project.id);
    }
}
