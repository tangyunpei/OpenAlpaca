use arc_swap::ArcSwap;
use openalpaca_core::chat::ChatStreamManager;
use openalpaca_storage::{Database, FileAssetRepository, FileAssetStatus};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::events::EventBroadcaster;

/// Spawn daemon-level heartbeat task.
///
/// Re-reads interval from ArcSwap each tick for hot-reload support.
pub fn spawn_heartbeat(
    eb: EventBroadcaster,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let secs = daemon_config.load().server.heartbeat_interval_secs;
            if cancel
                .run_until_cancelled(tokio::time::sleep(tokio::time::Duration::from_secs(secs)))
                .await
                .is_none()
            {
                tracing::info!("Heartbeat task shutting down");
                break;
            }
            eb.heartbeat();
        }
    });
}

/// Spawn chat stream cleanup task.
///
/// Re-reads interval and stale timeout from ArcSwap each tick for hot-reload support.
pub fn spawn_chat_cleanup(
    csm: Arc<ChatStreamManager>,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let cfg = daemon_config.load();
            let cleanup_secs = cfg.server.chat_streams.cleanup_interval_secs;
            let stale_secs = cfg.server.chat_streams.stale_timeout_secs;
            if cancel
                .run_until_cancelled(tokio::time::sleep(tokio::time::Duration::from_secs(
                    cleanup_secs,
                )))
                .await
                .is_none()
            {
                tracing::info!("Chat cleanup task shutting down");
                break;
            }
            csm.cleanup_stale(std::time::Duration::from_secs(stale_secs));
        }
    });
}

/// Spawn background file processing worker.
///
/// Polls for newly-uploaded file assets and extracts text content (PDF, plain text).
/// Vision-model-native formats (images) are left with empty extracted_text.
/// Failed extractions go to `Error` status and are retried up to `extraction_retry_count` times.
/// Each attempt is tagged `[attempt:N]` in `extract_error` from its
/// `Processing` write on (`processing_tag`), so the count survives an
/// attempt that never reaches its last write.
///
/// At its start, once, before its first poll, every row found `Processing`
/// is made `Error` with `INTERRUPTED_REASON`, its attempt counted
/// ([`FileAssetRepository::fail_interrupted_processing`]): a daemon stopped
/// mid-extraction leaves its row there, which the poll never lists and the
/// chat service reads as pending. Counting the cut attempt is what ends a
/// crash loop: a file whose extraction takes the daemon down is tried at
/// most `extraction_retry_count + 1` times in all, then stays `Error`.
/// None can be in flight then — this process has started no extraction, and
/// the singleton lock keeps any other daemon off the store. A later poll
/// never does this: a row `Processing` then is a task of this process.
///
/// A stop begins no new extraction: a task waiting for an extraction slot
/// checks for the stop once it has the slot and returns without writing, so
/// a file that had not started when the stop came keeps its row as it was.
/// The worker does not cut its batch short: an extraction already running
/// when the stop comes runs to its end, and the worker's task ends only once
/// every task of its batch has ended, so none outlives it. A result is
/// written only while the daemon's runtime still runs; the daemon does not
/// wait for it, so an extraction still running when the shutdown's tail
/// ends is lost, its row left `Processing` for the next start to count as
/// above.
pub fn spawn_file_processing_worker(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    spawn_file_processing_worker_with(
        db,
        daemon_config,
        cancel,
        Arc::new(crate::extraction::extract_text),
    );
}

/// How the worker extracts a file's text: `(storage_path, mime_type,
/// max_chars)` to `(text, error)`, as [`crate::extraction::extract_text`].
type Extractor = Arc<dyn Fn(&str, &str, usize) -> (Option<String>, Option<String>) + Send + Sync>;

/// [`spawn_file_processing_worker`] with its extractor given, returning the
/// worker's task: the daemon passes `extract_text` and drops the handle; a
/// test passes an extraction it holds and awaits the worker's end.
fn spawn_file_processing_worker_with(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
    extract: Extractor,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        match FileAssetRepository::new(&db).fail_interrupted_processing(INTERRUPTED_REASON) {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "File processing worker: {n} assets left processing at a stop are counted as interrupted attempts"
            ),
            Err(e) => tracing::warn!(
                "File processing worker: failed to mark the assets left processing as interrupted: {e}"
            ),
        }
        loop {
            let cfg = daemon_config.load();
            let gov = &cfg.upload.governance;
            let poll_secs = gov.processing_poll_interval_secs;
            let batch_size = gov.processing_batch_size;
            let max_text_chars = gov.max_extracted_text_chars;
            let max_concurrent = gov.max_concurrent_extractions;
            let retry_count = gov.extraction_retry_count;

            if cancel
                .run_until_cancelled(tokio::time::sleep(tokio::time::Duration::from_secs(
                    poll_secs,
                )))
                .await
                .is_none()
            {
                tracing::info!("File processing worker shutting down");
                break;
            }

            let repo = FileAssetRepository::new(&db);

            // Collect assets to process: new uploads + retryable errors
            let mut assets = match repo.list_by_status(&FileAssetStatus::Uploaded, batch_size) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("File processing worker: failed to list assets: {e}");
                    continue;
                }
            };

            // Also pick up Error-state assets eligible for retry
            if retry_count > 0 && assets.len() < batch_size {
                let remaining = batch_size - assets.len();
                if let Ok(error_assets) = repo.list_retryable_errors(retry_count, remaining) {
                    assets.extend(error_assets);
                }
            }

            if assets.is_empty() {
                continue;
            }

            let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
            // Drained below before the next poll; nothing between here and
            // the drain's end leaves the loop, so the worker never drops
            // the set (which would abort its tasks) before the drain ends;
            // only the runtime's own teardown does, as it drops every task.
            let mut batch = tokio::task::JoinSet::new();

            for asset in assets {
                let sem = semaphore.clone();
                let db2 = db.clone();
                let max_text = max_text_chars;
                let cancel = cancel.clone();
                let extract = extract.clone();

                batch.spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    // A stop asked for while this task waited for its slot:
                    // begin nothing, so the row keeps its status and its
                    // attempt count.
                    if cancel.is_cancelled() {
                        return Attempt::NotBegun;
                    }
                    let repo = FileAssetRepository::new(&db2);

                    let prev_attempts = match begin_attempt(&repo, &asset) {
                        Ok(prev) => prev,
                        Err(e) => {
                            tracing::warn!("Failed to mark asset {} as processing: {e}", asset.id);
                            return Attempt::NotBegun;
                        }
                    };

                    let storage_path = asset.storage_path.clone();
                    let mime_type = asset.mime_type.clone();
                    let (text, error) = tokio::task::spawn_blocking(move || {
                        extract(&storage_path, &mime_type, max_text)
                    })
                    .await
                    .unwrap_or((None, Some("Extraction task panicked".to_string())));

                    let final_status = if error.is_some() {
                        FileAssetStatus::Error
                    } else {
                        FileAssetStatus::Ready
                    };
                    let annotated_error =
                        error.map(|e| format!("[attempt:{}] {}", prev_attempts + 1, e));
                    if let Err(e) = repo.update_status(
                        &asset.id,
                        &final_status,
                        text.as_deref(),
                        annotated_error.as_deref(),
                    ) {
                        tracing::warn!("Failed to update asset {} status: {e}", asset.id);
                    }
                    Attempt::Written
                });
            }

            let drained = drain_batch(&mut batch).await;
            match (drained.written, drained.not_begun) {
                (0, 0) => {}
                (written, 0) => {
                    tracing::info!("File processing worker: extracted {written} assets");
                }
                (written, not_begun) => tracing::info!(
                    "File processing worker: extracted {written} assets; {not_begun} not begun"
                ),
            }
        }
    })
}

/// How one task of the worker's batch ended.
#[derive(Debug)]
enum Attempt {
    /// The attempt was begun and its result written (a failed last write
    /// is warned where it fails).
    Written,
    /// Nothing was begun: the stop came while the task waited for its
    /// slot, or its `Processing` write failed.
    NotBegun,
}

/// What [`drain_batch`] counted.
#[derive(Debug, Default, PartialEq)]
struct Drained {
    written: usize,
    not_begun: usize,
    panicked: usize,
}

/// Waits for every task of a batch, in the order they end, and counts how
/// each ended. A panicked task is warned about once and the others are
/// still awaited; the set is empty when this returns.
async fn drain_batch(batch: &mut tokio::task::JoinSet<Attempt>) -> Drained {
    let mut drained = Drained::default();
    while let Some(joined) = batch.join_next().await {
        match joined {
            Ok(Attempt::Written) => drained.written += 1,
            Ok(Attempt::NotBegun) => drained.not_begun += 1,
            Err(e) if e.is_panic() => {
                drained.panicked += 1;
                tracing::warn!("File processing worker: an extraction task panicked: {e}");
            }
            Err(e) => {
                tracing::warn!("File processing worker: an extraction task ended early: {e}");
            }
        }
    }
    drained
}

/// What a row left `Processing` at a stop carries after its `[attempt:N]`
/// tag once the worker's next start counts it as a failed attempt.
const INTERRUPTED_REASON: &str =
    "extraction was interrupted: the daemon stopped before it finished";

/// The `Processing` write that starts an attempt on a listed row: status
/// `Processing`, the text cleared, `extract_error` tagged with the attempt
/// it starts ([`processing_tag`]). Returns the attempts made before it.
fn begin_attempt(
    repo: &FileAssetRepository<'_>,
    asset: &openalpaca_storage::FileAsset,
) -> anyhow::Result<u32> {
    let prev_attempts = parse_retry_count(asset.extract_error.as_deref());
    repo.update_status(
        &asset.id,
        &FileAssetStatus::Processing,
        None,
        Some(&processing_tag(prev_attempts)),
    )?;
    Ok(prev_attempts)
}

/// The `extract_error` the `Processing` write stores: the attempt it starts,
/// so a stop or a crash before the last write still leaves the count.
fn processing_tag(prev_attempts: u32) -> String {
    format!("[attempt:{}] extraction in progress", prev_attempts + 1)
}

/// Parse the retry attempt count from an annotated error string like "[attempt:2] PDF extraction failed".
///
/// The poll's retry listing reads the same tag in SQL
/// ([`FileAssetRepository::list_retryable_errors`]). The two agree on every
/// string a writer produces — a canonical tag, `[attempt:` then ASCII
/// digits within `u32` then `]`, and NULL, empty or untagged text — which
/// `the_sql_retry_listing_reads_every_written_tag_as_this_does` holds. They
/// are not equal on other strings: `[ATTEMPT:3] x` is 3 to the SQL (its
/// LIKE ignores ASCII case) and 0 here; `[attempt:2x] x` is 2 to the SQL
/// (CAST takes the leading digits) and 0 here.
fn parse_retry_count(error: Option<&str>) -> u32 {
    error
        .and_then(|e| e.strip_prefix("[attempt:"))
        .and_then(|e| e.split(']').next())
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0)
}

/// Spawn background asset cleanup task.
///
/// Periodically deletes orphaned file assets (not linked to any message)
/// that are older than the configured grace period.
pub fn spawn_asset_cleanup(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let cfg = daemon_config.load();
            let interval_secs = cfg.upload.governance.cleanup_interval_hours * 3600;
            let grace_hours = cfg.upload.governance.orphan_grace_period_hours as i64;

            if cancel
                .run_until_cancelled(tokio::time::sleep(tokio::time::Duration::from_secs(
                    interval_secs,
                )))
                .await
                .is_none()
            {
                tracing::info!("Asset cleanup task shutting down");
                break;
            }

            let cleaned = sweep_orphaned_uploads(&db, grace_hours);
            if cleaned > 0 {
                tracing::info!("Asset cleanup: removed {cleaned} orphaned assets");
            }
        }
    });
}

/// One sweep pass: for every orphaned upload, the bytes first and then the row.
/// Returns how many rows were removed.
///
/// **The row never goes without the bytes.** A row is the only handle anything
/// has on an upload's file — nothing walks the store's directories — so deleting
/// it while `remove_file` failed (a permission error, an unmounted project
/// volume) strands that file forever, and frees its sequence number for a name
/// that is still taken on disk. When the removal fails the row stays, the
/// failure is warned about with the path, and the next pass tries again. A file
/// that is already gone is simply removed: there is nothing left but the row.
fn sweep_orphaned_uploads(db: &Database, grace_hours: i64) -> usize {
    let repo = FileAssetRepository::new(db);
    let orphans = match repo.list_orphaned(grace_hours) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("Asset cleanup: failed to list orphans: {e}");
            return 0;
        }
    };

    let mut cleaned = 0usize;
    for orphan in &orphans {
        match std::fs::remove_file(&orphan.storage_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(
                    "Asset cleanup: keeping asset {} — failed to remove {}: {e}",
                    orphan.id,
                    orphan.storage_path
                );
                continue;
            }
        }
        if let Err(e) = repo.delete_by_id(&orphan.id) {
            tracing::warn!("Failed to delete orphan asset {}: {e}", orphan.id);
        } else {
            cleaned += 1;
        }
    }
    cleaned
}

/// The memory index's boot rebuild, off the request path (scale slice S6):
/// a cache that was missing, damaged or of another schema version is
/// rebuilt on a blocking worker started with the daemon's background tasks
/// — the socket is already bound — while recall and search read the note
/// files (design §5). A valid cache has nothing owed and this returns at
/// once. One that fails stays owed, and the reconciliation timer tries
/// again.
pub fn spawn_memory_index_rebuild(
    service: Arc<openalpaca_core::memory::service::MemoryService>,
) {
    if !service.rebuild_pending() {
        return;
    }
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        match tokio::task::spawn_blocking(move || service.run_pending_rebuild()).await {
            Ok(Ok(true)) => tracing::info!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "memory index rebuilt"
            ),
            Ok(Ok(false)) => {}
            Ok(Err(e)) => tracing::warn!(
                "memory index rebuild failed; recall and search read the note files, and the \
                 reconciliation timer tries again: {e}"
            ),
            // A panic: the rebuild stays owed all the same (its run guard).
            Err(e) => tracing::warn!(
                "memory index rebuild task failed; recall and search read the note files, and \
                 the reconciliation timer tries again: {e}"
            ),
        }
    });
}

/// The memory reconciliation timer (memory redesign, design §5 *Freshness
/// without a watcher*): every [`MEMORY_RECONCILE_INTERVAL`], a bounded stat
/// scan of every partition the owner's control snapshots know — a file
/// edited, added or removed behind the service's back is re-recorded
/// `external`, the index follows, and each finding is one `memory_changed`
/// with no surface (§7b P13). It first runs any whole rebuild the index is
/// still owed (scale slice S6). The first scan runs one interval after
/// boot: the cache was opened (and, if owed, is being rebuilt by
/// [`spawn_memory_index_rebuild`]), and a partition is also stat-scanned
/// before every read, so nothing waits on this.
pub fn spawn_memory_reconciliation(
    service: Arc<openalpaca_core::memory::service::MemoryService>,
    owner_id: String,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            if cancel
                .run_until_cancelled(tokio::time::sleep(MEMORY_RECONCILE_INTERVAL))
                .await
                .is_none()
            {
                tracing::info!("Memory reconciliation task shutting down");
                break;
            }
            let service = service.clone();
            let owner = owner_id.clone();
            let outcome =
                tokio::task::spawn_blocking(move || service.reconcile_owner(&owner)).await;
            match outcome {
                Ok(Ok(report)) if !report.changes.is_empty() => {
                    tracing::info!(
                        scanned = report.scanned,
                        changed = report.changes.len(),
                        "memory reconciliation: external changes recorded"
                    );
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => tracing::warn!("memory reconciliation failed: {e}"),
                Err(e) => tracing::warn!("memory reconciliation task failed: {e}"),
            }
        }
    });
}

/// Learning v2's sweeper (design 09 §1.5): every 15 s, a session with
/// unlearned turns that has been idle `[orchestrator.learning] idle_secs`, or
/// that is no longer its lane's active conversation, becomes due, and a due
/// session starts when no chat turn has been in flight for 20 s. No bus
/// subscriber: the switch is read from `session.status`.
pub fn spawn_learning_sweeper(
    queue: Arc<openalpaca_core::memory::learning::LearningQueue>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            if cancel
                .run_until_cancelled(tokio::time::sleep(
                    openalpaca_core::memory::learning::SWEEP_INTERVAL,
                ))
                .await
                .is_none()
            {
                break;
            }
            queue.sweep().await;
        }
    });
}

/// Learning v2's boot catch-up, the durable half of the stop drain (design
/// 09 §1.5): 30 s after start, every session with unlearned user turns is
/// due, and they are learned one at a time when the daemon is quiet.
pub fn spawn_learning_catch_up(
    queue: Arc<openalpaca_core::memory::learning::LearningQueue>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        // The delay is raced against the token; the catch-up itself is not,
        // as it was not under `select!` (an arm's body runs after the race).
        if cancel
            .run_until_cancelled(tokio::time::sleep(
                openalpaca_core::memory::learning::BOOT_CATCHUP_DELAY,
            ))
            .await
            .is_some()
        {
            queue.catch_up().await;
        }
    });
}

/// Design §5: the reconciliation scan runs "at service start, every 30 s,
/// before management reads, and on `sync`".
pub const MEMORY_RECONCILE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Spawn telemetry cleanup task.
///
/// Runs once per day, removing old skill_execution_log rows (>90 days)
/// and old tool_execution_log rows (>7 days).
pub fn spawn_telemetry_cleanup(db: Database, cancel: CancellationToken) {
    tokio::spawn(async move {
        loop {
            if cancel
                .run_until_cancelled(tokio::time::sleep(tokio::time::Duration::from_hours(24)))
                .await
                .is_none()
            {
                tracing::info!("Telemetry cleanup task shutting down");
                break;
            }
            let repo = openalpaca_storage::SkillExecutionRepository::new(&db);
            match repo.cleanup_old(90, 7) {
                Ok((skill_rows, tool_rows)) => {
                    if skill_rows > 0 || tool_rows > 0 {
                        tracing::info!(
                            "Telemetry cleanup: {skill_rows} skill + {tool_rows} tool rows removed"
                        );
                    }
                }
                Err(e) => tracing::warn!("Telemetry cleanup failed: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An orphaned upload row: past the grace period and attached to no message,
    /// so `list_orphaned` returns it.
    fn orphan_row(db: &Database, id: &str, storage_path: &str) {
        FileAssetRepository::new(db)
            .insert(&openalpaca_storage::FileAsset {
                id: id.to_string(),
                owner_id: "owner-1".to_string(),
                sha256: id.to_string(),
                filename: "notes.txt".to_string(),
                mime_type: "text/plain".to_string(),
                size_bytes: 5,
                storage_path: storage_path.to_string(),
                status: FileAssetStatus::Ready,
                extracted_text: None,
                extract_error: None,
                metadata_json: None,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .expect("insert orphan row");
        // Age it past the grace period the sweep is asked for.
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE file_assets SET created_at = datetime('now', '-48 hours')",
                [],
            )?;
            Ok(())
        })
        .expect("age the row");
    }

    fn test_db(dir: &tempfile::TempDir) -> Database {
        Database::open(&dir.path().join("test.db")).expect("open test db")
    }

    #[test]
    fn a_sweep_removes_the_orphans_bytes_and_its_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        let file = dir.path().join("01-notes.txt");
        std::fs::write(&file, b"hello").unwrap();
        orphan_row(&db, "orphan-1", file.to_str().unwrap());

        assert_eq!(sweep_orphaned_uploads(&db, 24), 1);
        assert!(!file.exists());
        assert!(
            FileAssetRepository::new(&db)
                .get_by_id("orphan-1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_sweep_whose_file_is_already_gone_still_removes_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        orphan_row(
            &db,
            "orphan-1",
            dir.path().join("gone.txt").to_str().unwrap(),
        );

        assert_eq!(sweep_orphaned_uploads(&db, 24), 1);
        assert!(
            FileAssetRepository::new(&db)
                .get_by_id("orphan-1")
                .unwrap()
                .is_none(),
            "nothing on disk to lose, so the row is all there is to remove"
        );
    }

    /// The row is the only handle anything has on an upload's bytes — nothing
    /// walks the store's directories — so deleting it while the file survives
    /// strands that file forever *and* frees its sequence number for a name that
    /// is still taken. When the removal fails, the row stays and the next sweep
    /// tries again.
    #[test]
    fn a_sweep_whose_file_cannot_be_removed_keeps_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        // A directory at the storage path: `remove_file` fails with something
        // that is not `NotFound`, exactly like a permission error or an
        // unmounted project volume.
        let blocked = dir.path().join("01-notes.txt");
        std::fs::create_dir(&blocked).unwrap();
        orphan_row(&db, "orphan-1", blocked.to_str().unwrap());

        assert_eq!(sweep_orphaned_uploads(&db, 24), 0);
        assert!(blocked.exists());
        assert!(
            FileAssetRepository::new(&db)
                .get_by_id("orphan-1")
                .unwrap()
                .is_some(),
            "the row must outlive a failed removal, or the bytes are unreachable"
        );
    }

    /// R39: the orphan sweep deletes an upload's bytes with its row; a result
    /// file's pointer row is no upload — its file is the user's — so a row
    /// past the grace period and linked to no message is left, file and all
    /// (fails when the pointer insert omits `origin`, whose default is
    /// `upload`).
    #[test]
    fn the_orphan_sweep_leaves_a_pointer_rows_file() {
        let gp = crate::test_util::GuardedProject::new();
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        let file = gp.root.join("docs").join("plan.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"# Plan\n").unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
        let root = gp.root.to_string_lossy().into_owned();
        let (row, _) = openalpaca_storage::ArtifactStore::new(&db)
            .record_project_file(openalpaca_storage::NewProjectFile {
                owner_id: "owner-1",
                project_root: &root,
                rel_path: "docs/plan.md",
                content: b"# Plan\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        // Past any grace period, and linked to no message.
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE file_assets SET created_at = datetime('now', '-48 hours')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        assert_eq!(sweep_orphaned_uploads(&db, 24), 0);

        assert_eq!(std::fs::read(&file).unwrap(), b"# Plan\n");
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), mtime);
        assert!(
            !file.starts_with(gp.home_root()),
            "the user's folder, not the store"
        );
        assert!(
            FileAssetRepository::new(&db)
                .get_by_id(&row.id)
                .unwrap()
                .is_some(),
            "the row stays"
        );
    }

    /// One upload row: a small text file the worker can extract.
    fn upload_row(db: &Database, dir: &std::path::Path, id: &str, status: FileAssetStatus) {
        let file = dir.join(format!("{id}.txt"));
        std::fs::write(&file, format!("text of {id}")).unwrap();
        FileAssetRepository::new(db)
            .insert(&openalpaca_storage::FileAsset {
                id: id.to_string(),
                owner_id: "owner-1".to_string(),
                sha256: id.to_string(),
                filename: format!("{id}.txt"),
                mime_type: "text/plain".to_string(),
                size_bytes: 12,
                storage_path: file.to_string_lossy().into_owned(),
                status,
                extracted_text: None,
                extract_error: None,
                metadata_json: None,
                created_at: String::new(),
                updated_at: String::new(),
            })
            .expect("insert upload row");
    }

    fn status_of(db: &Database, id: &str) -> FileAssetStatus {
        FileAssetRepository::new(db)
            .get_by_id(id)
            .unwrap()
            .expect("the row exists")
            .status
    }

    /// Sets a row's status and its error column as the worker's own writes
    /// leave them.
    fn tag(db: &Database, id: &str, status: FileAssetStatus, error: Option<&str>) {
        FileAssetRepository::new(db)
            .update_status(id, &status, None, error)
            .expect("tag the row");
    }

    /// Dates a row's upload a day back, so the poll's oldest-first listing
    /// reaches it before any row the test adds later.
    fn age(db: &Database, id: &str) {
        db.with_connection(|conn| {
            conn.execute(
                "UPDATE file_assets SET created_at = datetime('now', '-1 day') WHERE id = ?1",
                [id],
            )?;
            Ok(())
        })
        .expect("age the row");
    }

    /// Waits (real time: the worker extracts on a blocking thread) until the
    /// row is Ready, or the deadline passes; returns the status it has then.
    /// An Error on the way is not the end: the worker retries it.
    async fn ready(db: &Database, id: &str) -> FileAssetStatus {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let status = status_of(db, id);
            if status == FileAssetStatus::Ready || std::time::Instant::now() >= deadline {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// The worker on a 1 s poll (below what validation allows a person to
    /// set, so the test is quick), with its token to stop it.
    fn start_worker(db: &Database) -> CancellationToken {
        let mut cfg = openalpaca_core::daemon_config::DaemonConfig::default();
        cfg.upload.governance.processing_poll_interval_secs = 1;
        let cancel = CancellationToken::new();
        spawn_file_processing_worker(
            db.clone(),
            Arc::new(ArcSwap::from_pointee(cfg)),
            cancel.clone(),
        );
        cancel
    }

    /// PROCESSING-PICKUP: a daemon stopped mid-extraction leaves its row
    /// `Processing`, which the chat service reads as pending and the poll
    /// never lists — so the attachment stayed pending forever. The worker,
    /// at its start, counts the cut attempt as a failed one, and the retry
    /// that follows takes the row to Ready.
    #[tokio::test]
    async fn a_row_left_processing_at_a_stop_is_picked_up_when_the_worker_starts() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        // As a stop between the worker's `Processing` write and its last
        // write leaves it.
        upload_row(&db, dir.path(), "stuck", FileAssetStatus::Processing);
        assert_eq!(status_of(&db, "stuck"), FileAssetStatus::Processing);

        let cancel = start_worker(&db);
        let status = ready(&db, "stuck").await;
        cancel.cancel();

        assert_eq!(status, FileAssetStatus::Ready, "picked up at the start");
        let row = FileAssetRepository::new(&db)
            .get_by_id("stuck")
            .unwrap()
            .unwrap();
        assert_eq!(row.extracted_text.as_deref(), Some("text of stuck"));
        assert_eq!(row.extract_error, None);
    }

    /// Regression guard: only the worker's start takes `Processing` rows up
    /// again. A row that turns `Processing` while the worker runs is in
    /// flight in this process, so a later poll must leave it alone, or one
    /// file would be extracted twice.
    #[tokio::test]
    async fn a_row_that_turns_processing_after_the_start_is_left_to_its_task() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);

        let cancel = start_worker(&db);
        // A first row through: the worker's start is behind it.
        upload_row(&db, dir.path(), "first", FileAssetStatus::Uploaded);
        assert_eq!(ready(&db, "first").await, FileAssetStatus::Ready);

        // As a task of this process leaves it while it extracts.
        upload_row(&db, dir.path(), "in-flight", FileAssetStatus::Processing);
        // Two more polls, each proven by a row it took through.
        for id in ["second", "third"] {
            upload_row(&db, dir.path(), id, FileAssetStatus::Uploaded);
            assert_eq!(ready(&db, id).await, FileAssetStatus::Ready);
        }
        cancel.cancel();

        assert_eq!(
            status_of(&db, "in-flight"),
            FileAssetStatus::Processing,
            "a later poll never takes up a row in flight"
        );
    }

    /// The `Processing` write tags the attempt it starts, counting on from the
    /// row's last tag, so a stop or a crash before the attempt's last write
    /// still leaves the count for the next start to read.
    #[test]
    fn the_processing_write_tags_the_attempt_it_starts() {
        assert_eq!(processing_tag(0), "[attempt:1] extraction in progress");
        let prev = parse_retry_count(Some("[attempt:1] Failed to read text file"));
        assert_eq!(processing_tag(prev), "[attempt:2] extraction in progress");
        assert_eq!(parse_retry_count(Some(&processing_tag(prev))), 2);
    }

    /// The crash loop at the worker's seams: an attempt begun on a listed row
    /// and cut off (the process dies; the next start fails the row) twice
    /// spends the default one retry, and the row is never listed again. The
    /// `Processing` write's tag is what carries the count across the cut.
    #[test]
    fn an_attempt_cut_off_on_every_try_stops_being_listed() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        let repo = FileAssetRepository::new(&db);
        let retries = openalpaca_core::daemon_config::DaemonConfig::default()
            .upload
            .governance
            .extraction_retry_count;
        assert_eq!(retries, 1, "the default allows one retry");
        upload_row(&db, dir.path(), "crashing", FileAssetStatus::Uploaded);

        let mut listed = repo.get_by_id("crashing").unwrap().unwrap();
        for attempt in 1..=2 {
            assert_eq!(begin_attempt(&repo, &listed).unwrap(), attempt - 1);
            // The process dies here; the next start:
            assert_eq!(
                repo.fail_interrupted_processing(INTERRUPTED_REASON)
                    .unwrap(),
                1
            );
            let retryable = repo.list_retryable_errors(retries, 10).unwrap();
            if attempt == 1 {
                assert_eq!(retryable.len(), 1, "one retry left");
                listed = retryable.into_iter().next().unwrap();
            } else {
                assert!(retryable.is_empty(), "no try left after {attempt}");
            }
        }
        let row = repo.get_by_id("crashing").unwrap().unwrap();
        assert_eq!(row.status, FileAssetStatus::Error);
        assert_eq!(
            row.extract_error.as_deref(),
            Some("[attempt:2] extraction was interrupted: the daemon stopped before it finished")
        );
    }

    /// PROCESSING-PICKUP follow-up: a file whose extraction takes the daemon
    /// down with it would, if every start queued it as new, be extracted
    /// again at each start and take the daemon down again — a crash loop.
    /// The start counts the cut attempt against `extraction_retry_count`
    /// instead, so a row left `Processing` on its last allowed attempt stays
    /// Error with a reason and is never extracted again.
    #[tokio::test]
    async fn a_row_left_processing_on_its_last_attempt_is_not_extracted_again() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        // The default allows one retry: attempt 2 is the last. A stop (or a
        // crash) during it leaves the row so.
        upload_row(&db, dir.path(), "crashing", FileAssetStatus::Uploaded);
        tag(
            &db,
            "crashing",
            FileAssetStatus::Processing,
            Some("[attempt:2] extraction in progress"),
        );

        let cancel = start_worker(&db);
        // Two polls, each proven by a row it took through.
        for id in ["first", "second"] {
            upload_row(&db, dir.path(), id, FileAssetStatus::Uploaded);
            assert_eq!(ready(&db, id).await, FileAssetStatus::Ready);
        }
        cancel.cancel();

        let row = FileAssetRepository::new(&db)
            .get_by_id("crashing")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, FileAssetStatus::Error, "not extracted again");
        assert_eq!(row.extracted_text, None);
        assert_eq!(
            row.extract_error.as_deref(),
            Some("[attempt:2] extraction was interrupted: the daemon stopped before it finished")
        );
    }

    /// Regression guard: rows that have spent their retries never crowd out
    /// one that has not. The poll lists at most `processing_batch_size` Error
    /// rows, oldest first; were the retry check applied only after that
    /// listing, a batch of older spent rows would hide a row the start just
    /// counted as interrupted, and it would never be retried.
    #[tokio::test]
    async fn a_row_left_processing_is_retried_past_rows_out_of_retries() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        let batch = openalpaca_core::daemon_config::DaemonConfig::default()
            .upload
            .governance
            .processing_batch_size;
        let spent: Vec<String> = (0..batch).map(|i| format!("spent-{i}")).collect();
        for id in &spent {
            upload_row(&db, dir.path(), id, FileAssetStatus::Uploaded);
            tag(
                &db,
                id,
                FileAssetStatus::Error,
                Some("[attempt:2] Failed to read text file"),
            );
            age(&db, id);
        }
        upload_row(&db, dir.path(), "stuck", FileAssetStatus::Processing);

        let cancel = start_worker(&db);
        let status = ready(&db, "stuck").await;
        cancel.cancel();

        assert_eq!(
            status,
            FileAssetStatus::Ready,
            "retried past the spent rows"
        );
        for id in &spent {
            assert_eq!(
                status_of(&db, id),
                FileAssetStatus::Error,
                "{id} stays spent"
            );
        }
    }

    /// An extraction the test holds: each call reports its storage path on
    /// the channel, then waits for the latch the test opens, then extracts
    /// the file for real. The wait is bounded (30 s) and the latch opens on
    /// drop, so a failed assert never leaves a blocking thread the test
    /// runtime's drop would wait for.
    struct HeldExtraction {
        latch: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl HeldExtraction {
        fn new() -> (
            Self,
            Extractor,
            tokio::sync::mpsc::UnboundedReceiver<String>,
        ) {
            let latch = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let (started, rx) = tokio::sync::mpsc::unbounded_channel();
            let (held_latch, held_calls) = (latch.clone(), calls.clone());
            let extract: Extractor = Arc::new(move |path: &str, mime: &str, max: usize| {
                held_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = started.send(path.to_string());
                let (lock, opened) = &*held_latch;
                let guard = lock.lock().unwrap_or_else(|p| p.into_inner());
                drop(
                    opened
                        .wait_timeout_while(guard, std::time::Duration::from_secs(30), |open| {
                            !*open
                        })
                        .unwrap_or_else(|p| p.into_inner()),
                );
                crate::extraction::extract_text(path, mime, max)
            });
            (Self { latch, calls }, extract, rx)
        }

        fn release(&self) {
            let (lock, opened) = &*self.latch;
            *lock.lock().unwrap_or_else(|p| p.into_inner()) = true;
            opened.notify_all();
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Drop for HeldExtraction {
        fn drop(&mut self) {
            self.release();
        }
    }

    /// The worker on a 1 s poll with `max_concurrent` extraction slots and
    /// the given extraction, with its token and its task.
    fn start_held_worker(
        db: &Database,
        max_concurrent: usize,
        extract: Extractor,
    ) -> (CancellationToken, tokio::task::JoinHandle<()>) {
        let mut cfg = openalpaca_core::daemon_config::DaemonConfig::default();
        cfg.upload.governance.processing_poll_interval_secs = 1;
        cfg.upload.governance.max_concurrent_extractions = max_concurrent;
        let cancel = CancellationToken::new();
        let worker = spawn_file_processing_worker_with(
            db.clone(),
            Arc::new(ArcSwap::from_pointee(cfg)),
            cancel.clone(),
            extract,
        );
        (cancel, worker)
    }

    /// The row id `upload_row` gave the file at `path`.
    fn id_of(path: &str) -> String {
        std::path::Path::new(path)
            .file_stem()
            .expect("a file name")
            .to_string_lossy()
            .into_owned()
    }

    async fn next_started(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
            .await
            .expect("an extraction starts within 30 s")
            .expect("the extraction channel is open")
    }

    /// ATTACH-MECH: a stop begins no new extraction. One slot, four listed
    /// rows: one extracts, held by the test; three wait for the slot. The
    /// stop comes, then the held extraction ends. A waiting task takes the
    /// slot after the stop and must write nothing — no `Processing` write,
    /// so the next start counts no attempt for a file that had not started
    /// when the stop came — and extract nothing. Deterministic: the test
    /// holds the extraction (no sleep race) and awaits the worker's end.
    #[tokio::test]
    async fn a_stop_begins_no_attempt_on_a_row_waiting_for_a_slot() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        for id in ["a", "b", "c", "d"] {
            upload_row(&db, dir.path(), id, FileAssetStatus::Uploaded);
        }
        // A row on its retry, so a tagged error is held byte for byte too.
        tag(
            &db,
            "d",
            FileAssetStatus::Error,
            Some("[attempt:1] Failed to read text file"),
        );
        let repo = FileAssetRepository::new(&db);
        let before: Vec<_> = ["a", "b", "c", "d"]
            .iter()
            .map(|id| repo.get_by_id(id).unwrap().unwrap())
            .collect();

        let (held, extract, mut started) = HeldExtraction::new();
        let (cancel, worker) = start_held_worker(&db, 1, extract);
        let running = id_of(&next_started(&mut started).await);
        cancel.cancel();
        held.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), worker)
            .await
            .expect("the worker ends after the stop")
            .expect("the worker does not panic");

        assert_eq!(held.calls(), 1, "nothing is extracted after the stop");
        for row in &before {
            let now = repo.get_by_id(&row.id).unwrap().unwrap();
            if row.id == running {
                assert_eq!(now.status, FileAssetStatus::Ready, "left to finish");
                continue;
            }
            assert_eq!(now.status, row.status, "{} keeps its status", row.id);
            assert_eq!(
                now.extract_error, row.extract_error,
                "{} keeps its error",
                row.id
            );
            assert_eq!(now.updated_at, row.updated_at, "{} is not written", row.id);
            assert_eq!(now.extracted_text, row.extracted_text);
        }
    }

    /// Regression guard (ATTACH-MECH): the worker's own `Processing` write
    /// carries the attempt it begins, read while the extraction is held. A
    /// worker that wrote `Processing` with no tag (`begin_attempt` left
    /// unused) would let a crash loop's row be counted as attempt 1 at every
    /// start; the seam test alone does not see the worker's call.
    #[tokio::test]
    async fn the_worker_tags_the_attempt_it_begins_while_it_extracts() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        upload_row(&db, dir.path(), "new", FileAssetStatus::Uploaded);
        upload_row(&db, dir.path(), "retried", FileAssetStatus::Uploaded);
        tag(
            &db,
            "retried",
            FileAssetStatus::Error,
            Some("[attempt:1] Failed to read text file"),
        );

        let (held, extract, mut started) = HeldExtraction::new();
        let (cancel, worker) = start_held_worker(&db, 2, extract);
        let mut seen = vec![
            id_of(&next_started(&mut started).await),
            id_of(&next_started(&mut started).await),
        ];
        seen.sort();
        assert_eq!(seen, ["new", "retried"]);
        let repo = FileAssetRepository::new(&db);
        for (id, tag) in [
            ("new", "[attempt:1] extraction in progress"),
            ("retried", "[attempt:2] extraction in progress"),
        ] {
            let row = repo.get_by_id(id).unwrap().unwrap();
            assert_eq!(row.status, FileAssetStatus::Processing, "{id}");
            assert_eq!(row.extract_error.as_deref(), Some(tag), "{id}");
        }
        cancel.cancel();
        held.release();
        tokio::time::timeout(std::time::Duration::from_secs(30), worker)
            .await
            .expect("the worker ends after the stop")
            .expect("the worker does not panic");
        for id in ["new", "retried"] {
            assert_eq!(status_of(&db, id), FileAssetStatus::Ready, "{id}");
        }
    }

    /// Regression guard (ATTACH-MECH): the poll's retry listing reads the
    /// attempt tag in SQL, the worker in Rust (`parse_retry_count`); for
    /// every string a writer can leave in `extract_error` they must pick the
    /// same rows, at every retry count. Strings no writer produces (another
    /// letter case, a non-digit tag) are outside this: the two are not equal
    /// on them, as both doc comments say.
    #[test]
    fn the_sql_retry_listing_reads_every_written_tag_as_this_does() {
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        let repo = FileAssetRepository::new(&db);
        let texts = [
            "",
            "PDF extraction failed",
            "a ] b",
            "100% done",
            "_x_",
            "line\nbreak",
            "\u{fc}n\u{ef}c\u{f8}d\u{e9}",
            "[attempt:9] second tag",
            "extraction in progress",
            INTERRUPTED_REASON,
        ];
        let mut written: Vec<Option<String>> = vec![
            None,
            Some(String::new()),
            Some("Failed to read text file".to_string()),
        ];
        for n in [1u32, 2, 3, 4, 5, 6, 10, 100] {
            for e in texts {
                written.push(Some(format!("[attempt:{n}] {e}")));
            }
            written.push(Some(processing_tag(n - 1)));
        }
        let ids: Vec<String> = (0..written.len()).map(|i| format!("w{i}")).collect();
        for (id, error) in ids.iter().zip(&written) {
            upload_row(&db, dir.path(), id, FileAssetStatus::Uploaded);
            tag(&db, id, FileAssetStatus::Error, error.as_deref());
        }

        let mut differ = Vec::new();
        for retries in 0u32..=3 {
            let listed: std::collections::HashSet<String> = repo
                .list_retryable_errors(retries, 100_000)
                .unwrap()
                .into_iter()
                .map(|row| row.id)
                .collect();
            for (id, error) in ids.iter().zip(&written) {
                let rust = parse_retry_count(error.as_deref()) <= retries;
                if rust != listed.contains(id) {
                    differ.push(format!("retries {retries}: {error:?} rust {rust}"));
                }
            }
        }
        assert!(differ.is_empty(), "the readers differ: {differ:#?}");
    }

    /// What a test's subscriber writes, read back as text.
    #[derive(Clone, Default)]
    struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

    impl LogBuf {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
        }
    }

    impl std::io::Write for LogBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
        type Writer = LogBuf;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// M40: the batch is drained to its end, a panicked task included. Each
    /// way a task can end is counted, a panic is warned about once per
    /// task, and the tasks after a panic are still awaited (a drain that
    /// re-raised or stopped at the first panic would lose them).
    #[tokio::test]
    async fn drain_batch_counts_each_end_and_warns_once_per_panicked_task() {
        let logs = LogBuf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        let _default = tracing::subscriber::set_default(subscriber);

        let mut batch = tokio::task::JoinSet::new();
        batch.spawn(async { panic!("the first task panics") });
        batch.spawn(async { Attempt::Written });
        batch.spawn(async { Attempt::NotBegun });
        batch.spawn(async { panic!("a later task panics") });
        batch.spawn(async { Attempt::Written });

        let drained = drain_batch(&mut batch).await;

        assert_eq!(
            drained,
            Drained {
                written: 2,
                not_begun: 1,
                panicked: 2,
            }
        );
        assert!(batch.is_empty(), "every task is awaited");
        let text = logs.text();
        let warned = text
            .lines()
            .filter(|line| line.contains("an extraction task panicked"))
            .count();
        assert_eq!(warned, 2, "one warn per panicked task: {text}");
    }

    /// Regression guard (M40): an extractor that panics is caught on its
    /// blocking thread and written as a failed attempt, tagged like any
    /// other. No retry here (`extraction_retry_count` 0), so the first
    /// attempt's write is the row's last.
    #[tokio::test]
    async fn an_extractor_panic_is_written_as_a_failed_attempt() {
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_util::HomeStoreGuard::set(&home.path().join("home"));
        let dir = tempfile::tempdir().unwrap();
        let db = test_db(&dir);
        upload_row(&db, dir.path(), "boom", FileAssetStatus::Uploaded);

        let mut cfg = openalpaca_core::daemon_config::DaemonConfig::default();
        cfg.upload.governance.processing_poll_interval_secs = 1;
        cfg.upload.governance.extraction_retry_count = 0;
        let cancel = CancellationToken::new();
        let extract: Extractor = Arc::new(
            |_: &str, _: &str, _: usize| -> (Option<String>, Option<String>) {
                panic!("the extractor panics")
            },
        );
        let worker = spawn_file_processing_worker_with(
            db.clone(),
            Arc::new(ArcSwap::from_pointee(cfg)),
            cancel.clone(),
            extract,
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while status_of(&db, "boom") != FileAssetStatus::Error {
            assert!(
                std::time::Instant::now() < deadline,
                "the row fails within 30 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(30), worker)
            .await
            .expect("the worker ends after the stop")
            .expect("the worker does not panic");

        let row = FileAssetRepository::new(&db)
            .get_by_id("boom")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, FileAssetStatus::Error);
        assert_eq!(row.extracted_text, None);
        assert_eq!(
            row.extract_error.as_deref(),
            Some("[attempt:1] Extraction task panicked")
        );
    }
}
