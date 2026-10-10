//! Artifact file delivery for task notifications.

use std::path::{Path, PathBuf};

use openalpaca_core::orchestrator::ConnectorSendProvider;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{ArtifactRecord, ArtifactStore, Database, FileAssetRepository};
use tracing::warn;

/// Maximum file size for artifact delivery (50 MB — Telegram Bot API limit).
const MAX_ARTIFACT_FILE_SIZE: i64 = 50 * 1024 * 1024;

/// Maximum number of artifacts to deliver per task.
const MAX_ARTIFACTS_PER_TASK: usize = 5;

/// Resolve a file asset for an artifact pointer with owner validation.
///
/// Resolution strategy:
/// 1. Use `file_asset_id` if present
/// 2. Try `key` as a file_asset ID fallback
/// 3. Return None (workspace-only artifact)
///
/// After resolving, validates that the asset belongs to `expected_owner`.
/// Returns None (with a warning) if the owner doesn't match — prevents
/// delivering files belonging to another user.
pub(crate) fn resolve_artifact_file(
    repo: &FileAssetRepository<'_>,
    file_asset_id: Option<&str>,
    key: &str,
    expected_owner: &str,
) -> Option<openalpaca_storage::FileAsset> {
    // 1. Explicit file_asset_id
    let asset = if let Some(id) = file_asset_id {
        repo.get_by_id(id).ok().flatten()
    } else {
        None
    };
    // 2. Try key as file_asset ID fallback
    let asset = asset.or_else(|| repo.get_by_id(key).ok().flatten());

    // 3. Validate owner
    match asset {
        Some(a) if a.owner_id == expected_owner => Some(a),
        Some(a) => {
            warn!(
                file_id = %a.id,
                actual_owner = %a.owner_id,
                expected_owner,
                "Artifact file owner mismatch — skipping delivery"
            );
            None
        }
        None => None,
    }
}

/// The project a run worked in, as `send` reads a turn's: the request root
/// its row records (`task.workspace_id`, the root its tools' context carried)
/// through `store::project_root_of` — an absolute root a project, anything
/// else none (`artifact_write::scope_from_context`'s rule). `Ok(None)`: a
/// run with no project. An error is a row or a root that cannot be read.
fn run_project(db: &Database, task_id: &str) -> anyhow::Result<Option<String>> {
    let root = openalpaca_storage::repository::TaskRepository::new(db)
        .get(task_id)?
        .and_then(|task| task.workspace_id)
        .map(PathBuf::from)
        .filter(|root| root.is_absolute());
    let scope = match root {
        Some(root) => StoreScope::Project(root),
        None => StoreScope::Home,
    };
    openalpaca_storage::store::project_root_of(&scope)
}

/// The path a run's end hands the connector for `record`, or why it sends
/// nothing — what `send` by `artifact_id` refuses, refused here too
/// (d3-approvals-2): `workspace_write` stores any `file_asset_id` with no
/// card (ST-15), and this delivery asks nobody. A result file in the project
/// is the user's file, never a run's deliverable (ST-12; the outcome's
/// automatic half reads by-products only) — which also covers a missing
/// project's result file (R-T18, R-T19); a file of a project the run is not
/// in is refused (R-T3: one of no project is the owner's from any run); and
/// a by-product's or an upload's file is confined under its own folder in
/// the store (`ArtifactRecord::content_root`, R-T20), so a link put at it is
/// never followed out — the path returned is the file the link reaches,
/// inside that folder. Read only.
fn deliverable_path(record: &ArtifactRecord, run_project: Option<&str>) -> Result<String, String> {
    if record.in_project {
        return Err("a result file in its project is never delivered".to_string());
    }
    if let Some(root) = record.project_root.as_deref()
        && run_project != Some(root)
    {
        return Err(format!("it belongs to another project ({root})"));
    }
    let folder = record
        .content_root()
        .map_err(|e| format!("its folder in the store cannot be found: {e:#}"))?;
    if !folder.exists() {
        return Err(format!("its folder {} is gone", folder.display()));
    }
    let confined =
        openalpaca_storage::store::confine_to_root(&folder, Path::new(&record.storage_path))
            .map_err(|e| format!("its file is not inside its folder in the store: {e:#}"))?;
    if !std::fs::metadata(&confined).is_ok_and(|m| m.is_file()) {
        return Err(format!("its file is not on disk ({})", confined.display()));
    }
    confined
        .into_os_string()
        .into_string()
        .map_err(|_| "its path is not UTF-8 text, which no channel can send".to_string())
}

/// Deliver artifact files to a channel. Called from a spawned task with timeout.
///
/// Each file goes through [`deliverable_path`], the rules `send` by
/// `artifact_id` applies, against the run's own project ([`run_project`]); a
/// run whose project cannot be read delivers nothing.
pub(super) async fn deliver_artifacts(
    db: &Database,
    send: &dyn ConnectorSendProvider,
    task_id: &str,
    channel: &str,
    recipient: &str,
    outcome_json: Option<&str>,
    expected_owner: &str,
) {
    use openalpaca_core::orchestrator::task_state::TaskOutcome;

    let outcome_json = match outcome_json {
        Some(oj) => oj,
        None => return,
    };
    let outcome: TaskOutcome = match serde_json::from_str(outcome_json) {
        Ok(o) => o,
        Err(e) => {
            warn!(task_id, "Failed to parse outcome_json for artifact delivery: {e}");
            return;
        }
    };
    if outcome.artifacts.is_empty() {
        return;
    }
    if !send.file_capable_channels().contains(&channel.to_string()) {
        return;
    }

    let run_project = match run_project(db, task_id) {
        Ok(project) => project,
        Err(e) => {
            warn!(
                task_id,
                "Cannot read the run's project, delivering no artifact: {e:#}"
            );
            return;
        }
    };

    let file_repo = FileAssetRepository::new(db);
    let store = ArtifactStore::new(db);
    for artifact in outcome.artifacts.iter().take(MAX_ARTIFACTS_PER_TASK) {
        let asset = match resolve_artifact_file(
            &file_repo,
            artifact.file_asset_id.as_deref(),
            &artifact.key,
            expected_owner,
        ) {
            Some(a) => a,
            None => continue,
        };
        let record = match store.get(&asset.id, expected_owner) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(e) => {
                warn!(task_id, file_id = %asset.id, "Failed to read the artifact's row: {e:#}");
                continue;
            }
        };
        let path = match deliverable_path(&record, run_project.as_deref()) {
            Ok(path) => path,
            Err(why) => {
                warn!(task_id, file_id = %asset.id, "Artifact not delivered: {why}");
                continue;
            }
        };

        // Check file size
        if asset.size_bytes > MAX_ARTIFACT_FILE_SIZE {
            warn!(
                task_id,
                file_id = %asset.id,
                size_bytes = asset.size_bytes,
                "Artifact file exceeds 50MB limit, skipping"
            );
            continue;
        }

        let caption = Some(format!("{} ({})", artifact.label, artifact.key));
        if let Err(e) = send
            .send_file(
                channel,
                recipient,
                &path,
                &asset.filename,
                &asset.mime_type,
                caption.as_deref(),
            )
            .await
        {
            warn!(
                task_id,
                file_id = %asset.id,
                "Failed to deliver artifact file: {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_core::orchestrator::task_state::{ArtifactPointer, TaskOutcome};
    use openalpaca_storage::{ArtifactKind, ArtifactStore, NewArtifact, OutcomeKind};
    use openalpaca_storage::store::StoreScope;
    use std::sync::Mutex;

    const OWNER: &str = "owner-1";
    const TASK_ID: &str = "t-5555eeee-0000-0000-0000-000000000000";

    /// Records what the connector was asked to send.
    #[derive(Default)]
    struct RecordingSender {
        files: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl ConnectorSendProvider for RecordingSender {
        async fn send_message(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
            Ok("sent".to_string())
        }
        fn sendable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
        async fn send_file(
            &self,
            _channel: &str,
            _recipient: &str,
            file_path: &str,
            filename: &str,
            mime_type: &str,
            _caption: Option<&str>,
        ) -> Result<String, String> {
            self.files.lock().unwrap().push((
                file_path.to_string(),
                filename.to_string(),
                mime_type.to_string(),
            ));
            Ok("sent".to_string())
        }
        fn file_capable_channels(&self) -> Vec<String> {
            vec!["telegram".to_string()]
        }
    }

    /// The run's row. `workspace` is the request root its tools' context
    /// carried (`task.workspace_id`): the run's project, when it has one.
    fn task_row(db: &Database, workspace: Option<&std::path::Path>) {
        let now = chrono::Utc::now();
        openalpaca_storage::repository::TaskRepository::new(db)
            .create(&openalpaca_storage::Task {
                id: TASK_ID.to_string(),
                title: "Delivery run".to_string(),
                description: None,
                status: openalpaca_storage::TaskStatus::Completed,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: OWNER.to_string(),
                source_lane: "telegram".to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
                state_json: None,
                state_version: 0,
                outcome_json: None,
                outcome_kind: None,
                artifact_count: 0,
                workspace_id: workspace.map(|root| root.to_string_lossy().into_owned()),
                source_task_id: None,
                session_id: None,
                unattended: false,
            })
            .unwrap();
    }

    /// A produced artifact, exactly as `artifact_write` / the `workspace_write`
    /// spill writes one.
    fn produce(db: &Database, project: &std::path::Path, title: &str, body: &str) -> String {
        let scope = StoreScope::Project(project.to_path_buf());
        let mut new = NewArtifact::new(OWNER, &scope, ArtifactKind::Markdown, title, body.as_bytes());
        new.task_id = Some(TASK_ID);
        new.task_title = Some("Delivery run");
        ArtifactStore::new(db).put(new).unwrap().0.id
    }

    fn outcome_json(pointer: ArtifactPointer) -> String {
        serde_json::to_string(&TaskOutcome {
            summary: "done".to_string(),
            outcome_kind: OutcomeKind::ArtifactOnly,
            artifacts: vec![pointer],
            no_artifact_reason: None,
        })
        .unwrap()
    }

    /// The payoff of plan §4.6: with a real `file_asset_id` on the pointer,
    /// `deliver_artifacts` resolves the asset and sends the file instead of
    /// silently `continue`-ing past it.
    #[tokio::test]
    async fn delivers_a_produced_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let project = tempfile::tempdir().unwrap();
        let project = project.path().canonicalize().unwrap();
        openalpaca_storage::store::projects::register(&project).unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&project));
        let id = produce(&db, &project, "final-report", "# Report\n");

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "final_report".to_string(),
                label: "final_report".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: Some(id),
            })),
            OWNER,
        )
        .await;

        let files = send.files.lock().unwrap();
        assert_eq!(files.len(), 1, "the artifact should have been delivered");
        let (path, filename, mime) = &files[0];
        assert!(
            std::path::Path::new(path).starts_with(
                openalpaca_storage::store::store_root(&StoreScope::Project(project.clone()))
                    .unwrap()
                    .join("artifacts")
            ),
            "delivered {path}"
        );
        assert_eq!(filename, "01-final-report.md");
        assert_eq!(mime, "text/markdown");
    }

    /// The pre-§4.6 shape — a pointer with no id — still delivers nothing.
    #[tokio::test]
    async fn an_unbacked_pointer_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db, None);

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "notes".to_string(),
                label: "notes".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: None,
            })),
            OWNER,
        )
        .await;

        assert!(send.files.lock().unwrap().is_empty());
    }

    /// Owner validation still bites: another user's artifact is never sent.
    #[tokio::test]
    async fn refuses_to_deliver_another_owners_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let project = tempfile::tempdir().unwrap();
        let project = project.path().canonicalize().unwrap();
        openalpaca_storage::store::projects::register(&project).unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&project));
        let id = produce(&db, &project, "secret", "classified\n");

        let send = RecordingSender::default();
        deliver_artifacts(
            &db,
            &send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(ArtifactPointer {
                key: "secret".to_string(),
                label: "secret".to_string(),
                agent_id: "writing_agent".to_string(),
                step_order: -1,
                file_asset_id: Some(id),
            })),
            "someone-else",
        )
        .await;

        assert!(send.files.lock().unwrap().is_empty());
    }

    // ── d3-approvals-2: what `send` by `artifact_id` refuses, the run's end
    //    refuses too ────────────────────────────────────────────────────

    /// The pointer a run's `workspace_write` (no card since ST-15) can name
    /// with any `file_asset_id`.
    fn pointer(id: &str) -> ArtifactPointer {
        ArtifactPointer {
            key: "summary".to_string(),
            label: "summary".to_string(),
            agent_id: "writing_agent".to_string(),
            step_order: -1,
            file_asset_id: Some(id.to_string()),
        }
    }

    async fn deliver(db: &Database, send: &RecordingSender, id: &str) {
        deliver_artifacts(
            db,
            send,
            TASK_ID,
            "telegram",
            "12345",
            Some(&outcome_json(pointer(id))),
            OWNER,
        )
        .await;
    }

    /// A result file is the user's file in the project (ST-12), which a
    /// run's end never delivers: the outcome's automatic half reads
    /// by-products only, and an explicit id naming one is refused the same.
    #[tokio::test]
    async fn a_result_file_in_the_project_is_never_delivered() {
        let gp = crate::test_util::GuardedProject::new();
        std::fs::create_dir_all(gp.root.join("docs")).unwrap();
        std::fs::write(gp.root.join("docs/plan.md"), "# Plan\n").unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&gp.root));
        let (record, _) = ArtifactStore::new(&db)
            .record_project_file(openalpaca_storage::NewProjectFile {
                owner_id: OWNER,
                project_root: gp.root.to_str().unwrap(),
                rel_path: "docs/plan.md",
                content: b"# Plan\n",
                task_id: Some(TASK_ID),
                agent_id: Some("writing_agent"),
            })
            .unwrap();

        let send = RecordingSender::default();
        deliver(&db, &send, &record.id).await;

        assert!(
            send.files.lock().unwrap().is_empty(),
            "a result file was delivered: {:?}",
            send.files.lock().unwrap()
        );
    }

    /// R-T3 at the run's end: a by-product of another project — an id the
    /// model read from `task_status`, say — is never sent from this run.
    #[tokio::test]
    async fn another_projects_by_product_is_never_delivered() {
        let gp = crate::test_util::GuardedProject::new();
        let other = tempfile::tempdir().unwrap();
        let other =
            openalpaca_storage::store::projects::register(&other.path().canonicalize().unwrap())
                .unwrap()
                .root;
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&gp.root));
        let id = produce(&db, &other, "other-report", "# Another project's\n");

        let send = RecordingSender::default();
        deliver(&db, &send, &id).await;

        assert!(
            send.files.lock().unwrap().is_empty(),
            "another project's by-product was delivered: {:?}",
            send.files.lock().unwrap()
        );
    }

    /// R-T20 at the run's end: a by-product's file is confined under its own
    /// folder in the store, so a link put at it is never followed out — the
    /// outside file is not sent, and its path never reaches the connector.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_by_product_whose_file_is_a_link_out_of_its_folder_is_never_delivered() {
        let gp = crate::test_util::GuardedProject::new();
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir
            .path()
            .canonicalize()
            .unwrap()
            .join("secret.txt");
        std::fs::write(&outside, "outside the store\n").unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&gp.root));
        let id = produce(&db, &gp.root, "final-report", "# Report\n");
        let head = ArtifactStore::new(&db)
            .get(&id, OWNER)
            .unwrap()
            .unwrap()
            .storage_path;
        std::fs::remove_file(&head).unwrap();
        std::os::unix::fs::symlink(&outside, &head).unwrap();

        let send = RecordingSender::default();
        deliver(&db, &send, &id).await;

        let files = send.files.lock().unwrap();
        assert!(
            files.is_empty(),
            "a link out of the store was followed: {files:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "outside the store\n"
        );
    }

    /// The rule `send` names a by-product by: one of no project is the
    /// owner's from any run, so a home-store by-product still goes out at the
    /// end of a run in a project.
    #[tokio::test]
    async fn a_home_store_by_product_is_delivered_from_a_run_in_a_project() {
        let gp = crate::test_util::GuardedProject::new();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&gp.root));
        let mut new = NewArtifact::new(
            OWNER,
            &StoreScope::Home,
            ArtifactKind::Markdown,
            "loose-notes",
            b"# Notes\n",
        );
        new.task_id = Some(TASK_ID);
        let id = ArtifactStore::new(&db).put(new).unwrap().0.id;

        let send = RecordingSender::default();
        deliver(&db, &send, &id).await;

        let files = send.files.lock().unwrap();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(
            std::path::Path::new(&files[0].0).starts_with(gp.home_root().join("artifacts")),
            "{files:?}"
        );
    }

    /// A file over the connector's 50 MB limit is skipped — its row's size,
    /// as before; the old test of this read no store and asserted nothing.
    #[tokio::test]
    async fn an_artifact_over_the_size_limit_is_never_delivered() {
        let gp = crate::test_util::GuardedProject::new();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        task_row(&db, Some(&gp.root));
        let id = produce(&db, &gp.root, "big", "# Big\n");
        db.with_connection(|conn| {
            // The id is a UUID this test minted: nothing to quote.
            conn.execute(
                &format!(
                    "UPDATE file_assets SET size_bytes = {} WHERE id = '{id}'",
                    MAX_ARTIFACT_FILE_SIZE + 1
                ),
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let send = RecordingSender::default();
        deliver(&db, &send, &id).await;

        assert!(send.files.lock().unwrap().is_empty());
    }
}
