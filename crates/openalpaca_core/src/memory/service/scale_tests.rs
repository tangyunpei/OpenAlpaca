//! The scale slice's service tests (L8 benches, `scale-fix/SPEC.md` S1–S7):
//! each pins a cost the benches measured away, or a guarantee the fix must
//! keep, through the service's own entry points. Temp home stores and
//! in-memory (or temp-file) indexes only; no model, no network.

use super::tests::{ALICE, Harness};
use super::*;
use openalpaca_storage::memory_files::{NoteKind, NoteSource, SourceType};
use openalpaca_storage::memory_index::MemoryIndex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn learned_note(
    h: &Harness,
    extraction: &MemoryAccessContext,
    title: &str,
    body: &str,
    row: i64,
) -> StoreOutput {
    h.service
        .store(
            extraction,
            StoreInput {
                title: title.into(),
                body: body.into(),
                kind: Some(NoteKind::Preference),
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: row.to_string(),
                    observed_at: None,
                }],
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap()
}

// ============================================================================
// S2 — verification reads the snapshot by reference (O2)
// ============================================================================

/// Recall's step 7 re-reads every selected file and refuses a tombstoned
/// one (design §5 step 7) — by reading the snapshot in place: a turn that
/// verifies many hits copies no control snapshot, where each verification
/// used to copy the whole snapshot (tombstones and every note record) once.
/// The same holds for learning's lookup, whose `notes_citing` rows are each
/// verified the same way.
#[test]
fn verification_copies_no_control_snapshot() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let extraction = h
        .ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
        .unwrap();
    let row = h.message_row("I like green tea in the garden.");
    for i in 0..6 {
        h.store(
            &user,
            &format!("Garden note {i}"),
            &format!("# Garden note {i}\n\nThe garden has green tea plants, row {i}.\n"),
            NoteKind::Fact,
        );
        learned_note(
            &h,
            &extraction,
            &format!("Tea preference {i}"),
            &format!("# Tea {i}\n\n## Knowledge\nThe user drinks green tea, cup {i}.\n"),
            row,
        );
    }
    // A tombstone, so the check has something to look through.
    let gone = h.store(
        &user,
        "Gone note",
        "# Gone\n\ngreen garden\n",
        NoteKind::Fact,
    );
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(gone.id, gone.revision)],
                reason: None,
            },
        )
        .unwrap();

    let tool = h.tool(ALICE, None);
    let before = files::snapshot_copies_on_this_thread();
    let recalled = h
        .service
        .recall(&tool, "green tea garden", RecallBudget::default())
        .unwrap();
    assert!(
        recalled.notes.len() + recalled.listed.len() >= 4,
        "the query selects several notes: {recalled:?}"
    );
    assert_eq!(
        files::snapshot_copies_on_this_thread() - before,
        0,
        "recall's verification copies no snapshot"
    );

    let before = files::snapshot_copies_on_this_thread();
    let view = h
        .service
        .learning_view(&extraction, &["green tea".to_string()], &[row])
        .unwrap();
    assert!(view.from_this_window.len() >= 6, "{view:?}");
    assert_eq!(
        files::snapshot_copies_on_this_thread() - before,
        0,
        "learning's lookup copies no snapshot"
    );
}

// ============================================================================
// S4 — a write's index stamp (O10)
// ============================================================================

/// After a write the index says its partition was scanned — now, with the
/// files the write's own view listed — as the stat scan before a read would
/// record it. The stamp comes from the view; no row is read back for it.
#[test]
fn a_write_stamps_its_partition_scan_with_the_files_it_saw() {
    use chrono::TimeZone;
    let at = Utc.with_ymd_and_hms(2026, 9, 26, 9, 30, 0).unwrap();
    let h = Harness::with_index(
        MemoryIndex::open_in_memory()
            .unwrap()
            .with_clock(move || at),
    );
    let user = h.user(ALICE, None);
    for i in 0..3 {
        h.store(
            &user,
            &format!("Stamp {i}"),
            &format!("# S{i}\n\nx {i}\n"),
            NoteKind::Fact,
        );
    }
    let key = files::home_partition_key(&user.owner_key);
    assert_eq!(
        h.service.index().last_scan(&key).unwrap(),
        Some(("2026-09-26T09:30:00Z".to_string(), 3))
    );
}

// ============================================================================
// S1 — no whole-partition re-index where an incremental one is exact (O1)
// ============================================================================

/// How many index rows this thread has built (each a copy of a body) while
/// `f` ran.
fn records_built(f: impl FnOnce()) -> u64 {
    let before = super::RECORDS_BUILT.with(|n| n.get());
    f();
    super::RECORDS_BUILT.with(|n| n.get()) - before
}

/// A service over a cache file in the harness's temp root — what a daemon
/// restart reopens.
fn file_service(h: &Harness, path: &Path) -> MemoryService {
    MemoryService::new(Some(h.db.clone()), MemoryIndex::open(path).unwrap())
}

/// After a restart the service knows nothing of what it indexed, but the
/// cache does: the first write re-indexes the note it wrote and the notes
/// whose derived facts moved — here the note it replaces — never the whole
/// partition, and the rows it leaves are the rows a whole re-index would
/// have left.
#[test]
fn the_first_write_after_a_restart_reindexes_only_what_changed() {
    let h = Harness::new();
    let path = h.root.path().join("cache").join("memory-index.sqlite");
    let user = h.user(ALICE, None);
    let first = file_service(&h, &path);
    first.rebuild_index().unwrap();
    let mut old = None;
    for i in 0..8 {
        let out = first
            .store(
                &user,
                StoreInput {
                    title: format!("Restart note {i}"),
                    body: format!("# R{i}\n\nThe restart note number {i}.\n"),
                    kind: Some(NoteKind::Fact),
                    fact_key: (i == 0).then(|| "restart.probe".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        if i == 0 {
            old = Some(out);
        }
    }
    let old = old.unwrap();
    drop(first);

    let second = file_service(&h, &path);
    let built = records_built(|| {
        second
            .store(
                &user,
                StoreInput {
                    title: "Restart replacement".into(),
                    body: "# R\n\nThe replacement.\n".into(),
                    kind: Some(NoteKind::Fact),
                    fact_key: Some("restart.probe".into()),
                    supersedes: Some((old.id, old.revision.clone())),
                    ..Default::default()
                },
            )
            .unwrap();
    });
    assert_eq!(built, 2, "the new note and the one it supersedes, not all nine");
    let key = files::home_partition_key(&user.owner_key);
    let row = second.index().get(&key, &old.id.to_string()).unwrap().unwrap();
    assert_eq!(row.effective_status, index::EffectiveStatus::Superseded);
    assert_eq!(second.index().index_state().unwrap().notes, 9);
}

/// The first reconciliation tick after a restart re-derives the partition
/// and finds its rows already right: it rebuilds none of them.
#[test]
fn the_first_tick_after_a_restart_rebuilds_no_unchanged_row() {
    let h = Harness::new();
    let path = h.root.path().join("cache").join("memory-index.sqlite");
    let user = h.user(ALICE, None);
    let first = file_service(&h, &path);
    first.rebuild_index().unwrap();
    for i in 0..6 {
        first
            .store(
                &user,
                StoreInput {
                    title: format!("Tick note {i}"),
                    body: format!("# T{i}\n\nTick body {i}.\n"),
                    kind: Some(NoteKind::Fact),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    drop(first);
    let second = file_service(&h, &path);
    let built = records_built(|| {
        second.reconcile_owner(ALICE).unwrap();
    });
    assert_eq!(built, 0, "nothing changed on disk or in the derivation");
}

/// An edit made behind the service's back is found by the stat scan before
/// the next read, and only that note is re-indexed — its new words are
/// searchable, and the answer says the note moved (`stale`).
#[test]
fn an_external_edit_reindexes_only_the_edited_note() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let mut edited = None;
    for i in 0..10 {
        let out = h.store(
            &user,
            &format!("Edit note {i}"),
            &format!("# E{i}\n\nOriginal words {i}.\n"),
            NoteKind::Fact,
        );
        if i == 3 {
            edited = Some(out);
        }
    }
    let edited = edited.unwrap();
    // Prime the scan, then edit one file by hand.
    let tool = h.tool(ALICE, None);
    h.service
        .recall(&tool, "original words", RecallBudget::default())
        .unwrap();
    let dir = h.home().join("memory/owners").join(&user.owner_key).join("notes");
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&edited.id.to_string()))
        })
        .unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("Original words 3.", "Marzipan harbour.")).unwrap();

    let mut found = None;
    let built = records_built(|| {
        found = Some(
            h.service
                .search(
                    &tool,
                    SearchInput {
                        query: "marzipan harbour".into(),
                        ..Default::default()
                    },
                )
                .unwrap(),
        );
    });
    assert_eq!(built, 1, "the edited note only");
    let found = found.unwrap();
    assert_eq!(found.results.len(), 1, "{found:?}");
    assert_eq!(found.results[0].id, edited.id);
    assert_eq!(found.results[0].freshness, Freshness::Stale);
}

// ============================================================================
// S3 — a forgotten note stays forgotten when its file comes back (O3)
// ============================================================================

fn file_of(h: &Harness, owner_key: &str, id: NoteId) -> PathBuf {
    let dir = h.home().join("memory/owners").join(owner_key).join("notes");
    std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(&id.to_string()))
        })
        .unwrap()
}

/// Once a tombstone exists, recovery's sweep of forgotten files ran at
/// every open — a listing of `notes/` before every read (+236 ms at 100K
/// notes). Through the service's cache it runs once: a read after the
/// first lists `notes/` once, for the read itself.
#[test]
fn a_read_with_a_tombstone_lists_notes_once() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let mut first = None;
    for i in 0..5 {
        let out = h.store(
            &user,
            &format!("Sweep {i}"),
            &format!("# S{i}\n\nx\n"),
            NoteKind::Fact,
        );
        first.get_or_insert(out);
    }
    let first = first.unwrap();
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(first.id, first.revision)],
                reason: None,
            },
        )
        .unwrap();
    h.service.list(&user, ListInput::default()).unwrap();
    let before = files::scans_on_this_thread();
    let listed = h.service.list(&user, ListInput::default()).unwrap();
    assert_eq!(listed.items.len(), 4);
    assert_eq!(
        files::scans_on_this_thread() - before,
        1,
        "the read's own listing, and no sweep"
    );
}

/// With the sweep once per process, a forgotten note's file put back
/// between two reads — a backup restore, a sync client — is still never
/// listed, found, re-recorded by the reconciliation tick, indexed or
/// written into `MEMORY.md` (design §6's hard guarantee): the scans every
/// read and tick make leave it out and delete it.
#[test]
fn a_forgotten_notes_file_put_back_between_reads_is_never_served() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let mut events = h.bus.subscribe();
    let gone = h.store(
        &user,
        "Forgotten lighthouse",
        "# F\n\nThe lighthouse keeper words.\n",
        NoteKind::Fact,
    );
    h.store(
        &user,
        "Kept lighthouse",
        "# K\n\nThe lighthouse stays lit.\n",
        NoteKind::Fact,
    );
    let file = file_of(&h, &user.owner_key, gone.id);
    let bytes = std::fs::read(&file).unwrap();
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(gone.id, gone.revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    // The service has opened (and swept) the partition since the forget.
    h.service.list(&user, ListInput::default()).unwrap();
    h.service.reconcile_owner(ALICE).unwrap();
    while events.try_recv().is_ok() {}

    let tool = h.tool(ALICE, None);
    std::fs::write(&file, &bytes).unwrap();
    let found = h
        .service
        .search(
            &tool,
            SearchInput {
                query: "lighthouse keeper".into(),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(found.results.iter().all(|r| r.id != gone.id), "{found:?}");
    assert!(!file.exists(), "the read's scan deleted it");

    std::fs::write(&file, &bytes).unwrap();
    let listed = h.service.list(&user, ListInput::default()).unwrap();
    assert!(listed.items.iter().all(|i| i.id != gone.id));

    std::fs::write(&file, &bytes).unwrap();
    let tick = h.service.reconcile_owner(ALICE).unwrap();
    assert!(tick.changes.iter().all(|c| c.note_id != gone.id), "{tick:?}");
    while let Ok(event) = events.try_recv() {
        assert!(
            !format!("{event:?}").contains(&gone.id.to_string()),
            "no memory_changed names it: {event:?}"
        );
    }

    std::fs::write(&file, &bytes).unwrap();
    h.store(&user, "Another", "# A\n\nA write.\n", NoteKind::Fact);
    let key = files::home_partition_key(&user.owner_key);
    assert!(
        h.service
            .index()
            .get(&key, &gone.id.to_string())
            .unwrap()
            .is_none(),
        "never indexed"
    );
    let navigation = std::fs::read_to_string(
        h.home()
            .join("memory/owners")
            .join(&user.owner_key)
            .join(files::NAVIGATION_FILE),
    )
    .unwrap();
    assert!(!navigation.contains("Forgotten lighthouse"), "{navigation}");
    assert!(navigation.contains("Kept lighthouse"));
    assert!(!file.exists());
    assert!(matches!(
        h.service.read(
            &tool,
            ReadInput {
                id: gone.id,
                ..Default::default()
            }
        ),
        Err(MemoryError::NotFound(_))
    ));
}

// ============================================================================
// S6 — the index never blocks boot (M2, O11)
// ============================================================================

/// Polls `f` for up to ten seconds.
fn eventually(what: &str, f: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !f() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn cedar_notes(h: &Harness, n: usize) {
    let user = h.user(ALICE, None);
    for i in 0..n {
        h.store(
            &user,
            &format!("Cedar {i}"),
            &format!("# Cedar {i}\n\nThe cedar grove number {i} by the river.\n"),
            NoteKind::Fact,
        );
    }
}

fn search_cedar(service: &MemoryService, ctx: &MemoryAccessContext) -> SearchOutput {
    service
        .search(
            ctx,
            SearchInput {
                query: "cedar grove".into(),
                limit: Some(SEARCH_MAX_LIMIT),
                ..Default::default()
            },
        )
        .unwrap()
}

/// The daemon's open over a missing cache returns at once, owing the
/// rebuild; until it is done — and while it runs, held still here once it
/// has emptied the index — search and recall answer from the note files and
/// say `rebuilding`, and a turn's suggestions are left out rather than
/// waited for. The rebuild then leaves a ready index that answers the same.
#[test]
fn the_daemon_open_returns_at_once_and_search_reads_the_files_while_it_rebuilds() {
    let h = Harness::new();
    cedar_notes(&h, 6);
    let service = Arc::new(MemoryService::open_default_deferred(Some(h.db.clone())));
    assert!(service.rebuild_pending(), "a missing cache is owed a rebuild");
    assert_eq!(
        service.index().index_state().unwrap().notes,
        0,
        "and the open did not do it"
    );
    let tool = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::ModelTool,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let found = search_cedar(&service, &tool);
    assert_eq!(found.results.len(), 6, "{found:?}");
    assert_eq!(found.index_state, IndexState::Rebuilding);
    let recalled = service
        .recall(&tool, "cedar grove river", RecallBudget::default())
        .unwrap();
    assert!(!recalled.notes.is_empty());
    assert_eq!(recalled.index_state, IndexState::Rebuilding);

    let hold = service.gates.rebuild.write().unwrap();
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild starts", || service.rebuild_running());
    let during = search_cedar(&service, &tool);
    assert_eq!(during.results.len(), 6, "answered while the rebuild runs");
    assert_eq!(during.index_state, IndexState::Rebuilding);
    assert!(
        service
            .pending_suggestions(&tool, 3, &HashSet::new())
            .unwrap()
            .is_empty()
    );
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap(), "it rebuilt");
    assert!(!service.rebuild_pending());
    let after = search_cedar(&service, &tool);
    assert_eq!(after.index_state, IndexState::Ready);
    assert_eq!(after.results.len(), 6);
    assert_eq!(service.index().index_state().unwrap().notes, 6);
}

/// Damage to the cache met on use — there is no integrity check at open —
/// owes a rebuild: recall and search read the files from then on, and the
/// next reconciliation tick rebuilds the cache from a new file.
#[test]
fn a_damaged_cache_met_on_use_is_rebuilt_by_the_next_tick() {
    let h = Harness::new();
    cedar_notes(&h, 60);
    let path = MemoryIndex::default_path().unwrap();
    drop(MemoryService::open_default(Some(h.db.clone())));
    // Scribble over the second half of the file, where the full-text index
    // lies; the catalog and the schema version, at its start, still read.
    {
        use std::io::{Seek, SeekFrom, Write};
        let len = std::fs::metadata(&path).unwrap().len();
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(len / 2)).unwrap();
        file.write_all(&vec![0xAB; (len - len / 2) as usize]).unwrap();
    }
    let service = MemoryService::open_default_deferred(Some(h.db.clone()));
    assert!(!service.rebuild_pending(), "open does not look");
    let tool = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::ModelTool,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let err = service
        .search(
            &tool,
            SearchInput {
                query: "cedar grove".into(),
                ..Default::default()
            },
        )
        .expect_err("the damaged pages are read");
    assert!(err.is_index_corruption(), "{err}");
    assert!(service.rebuild_pending(), "damage owes a rebuild");
    let found = search_cedar(&service, &tool);
    assert_eq!(found.index_state, IndexState::Rebuilding);
    assert_eq!(found.results.len(), 50, "the files answer, to the limit");

    service.reconcile_owner(ALICE).unwrap();
    assert!(!service.rebuild_pending());
    let found = search_cedar(&service, &tool);
    assert_eq!(found.index_state, IndexState::Ready);
    assert_eq!(found.results.len(), 50);
}

/// A rebuild that panics part-way — a note the chunker chokes on, say —
/// leaves no rebuild running for the life of the process: it stays owed,
/// so recall and search read the files, a read that waits for a running
/// rebuild returns at once, and the next attempt rebuilds.
#[test]
fn a_rebuild_that_panics_stays_owed_and_holds_no_reader() {
    let h = Harness::new();
    cedar_notes(&h, 4);
    let service = Arc::new(MemoryService::open_default_deferred(Some(h.db.clone())));
    assert!(service.rebuild_pending());
    service
        .gates
        .panic_rebuild
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        service.run_pending_rebuild()
    }));
    assert!(unwound.is_err(), "the injected panic unwound");
    assert!(!service.rebuild_running(), "no rebuild is left running");
    assert!(service.rebuild_pending(), "and the rebuild is still owed");
    let tool = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::ModelTool,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    assert_eq!(search_cedar(&service, &tool).index_state, IndexState::Rebuilding);
    // A read with no file-scan answer waits only while a rebuild runs.
    let (sent, read) = std::sync::mpsc::channel();
    {
        let (service, tool) = (service.clone(), tool.clone());
        std::thread::spawn(move || {
            let rows = service
                .timeline(
                    &tool,
                    TimelineInput {
                        anchor: TimelineAnchor::Date(Utc::now()),
                        before: Some(20),
                        after: Some(20),
                    },
                )
                .map(|t| t.rows.len());
            let _ = sent.send(rows);
        });
    }
    let rows = read
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the timeline returns rather than wait for a dead rebuild");
    assert_eq!(rows.unwrap(), 4);
    assert!(service.run_pending_rebuild().unwrap(), "the next attempt rebuilds");
    assert!(!service.rebuild_pending());
    let found = search_cedar(&service, &tool);
    assert_eq!(found.index_state, IndexState::Ready);
    assert_eq!(found.results.len(), 4);
}

// ============================================================================
// S5 — the index is maintained (M1)
// ============================================================================

/// How many segments each full-text index of the cache file at `path`
/// holds, `(fts_word, fts_tri)`, read on a connection of the test's own.
fn fts_segments(path: &Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(
            &format!("SELECT count(DISTINCT segid) FROM {table}_idx"),
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    (count("fts_word"), count("fts_tri"))
}

fn wal_len(path: &Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::metadata(PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn store_many(service: &MemoryService, prefix: &str, n: usize) {
    let user = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    for i in 0..n {
        service
            .store(
                &user,
                StoreInput {
                    title: format!("{prefix} {i}"),
                    body: format!(
                        "# {prefix} {i}\n\n{} {i}\n",
                        "the walnut orchard keeps its ledger ".repeat(40)
                    ),
                    kind: Some(NoteKind::Fact),
                    ..Default::default()
                },
            )
            .unwrap();
    }
}

/// Once a tick passes with no index write since the one before — idle —
/// the tick merges the FTS segments the writes left, into one per index,
/// and truncates the WAL.
/// A cache that has not outgrown its rebuilt size is not rebuilt.
#[test]
fn an_idle_tick_merges_the_index_and_truncates_its_wal() {
    let h = Harness::new();
    let service = MemoryService::open_default(Some(h.db.clone()));
    let path = MemoryIndex::default_path().unwrap();
    store_many(&service, "Walnut", 30);
    service.rebuild_index().unwrap();
    let rebuilt = service
        .index()
        .maintain(std::time::Duration::ZERO)
        .unwrap()
        .rebuilt_bytes;
    store_many(&service, "Pecan", 4);
    assert!(wal_len(&path) > 0, "the writes are in the WAL");
    let (word, tri) = fts_segments(&path);
    assert!(
        word > 1 && tri > 1,
        "the writes left segments to merge: {word}, {tri}"
    );
    service.reconcile_owner(ALICE).unwrap();
    assert!(wal_len(&path) > 0, "a tick right after writes leaves it");
    assert_eq!(fts_segments(&path), (word, tri), "and merges nothing");
    service.reconcile_owner(ALICE).unwrap();
    assert_eq!(wal_len(&path), 0, "the idle tick truncated it");
    assert_eq!(
        fts_segments(&path),
        (1, 1),
        "and merged each full-text index into one segment"
    );
    let pass = service
        .index()
        .maintain(std::time::Duration::ZERO)
        .unwrap();
    assert_eq!(pass.rebuilt_bytes, rebuilt, "and did not rebuild");
}

/// A cache grown past twice its size after its last rebuild — here by a
/// partition that outgrew it tenfold — is rebuilt by the tick, off the
/// request path, and records its new size.
#[test]
fn a_cache_grown_past_twice_its_rebuilt_size_is_rebuilt_by_a_tick() {
    let h = Harness::new();
    let service = MemoryService::open_default(Some(h.db.clone()));
    store_many(&service, "Seed", 3);
    service.rebuild_index().unwrap();
    let small = service.index().maintain(std::time::Duration::ZERO).unwrap();
    store_many(&service, "Growth", 60);
    service.reconcile_owner(ALICE).unwrap();
    service.reconcile_owner(ALICE).unwrap();
    let after = service.index().maintain(std::time::Duration::ZERO).unwrap();
    assert!(
        after.rebuilt_bytes > small.rebuilt_bytes * 2,
        "rebuilt at its new size: {small:?} then {after:?}"
    );
    assert!(!service.rebuild_pending());
    assert_eq!(service.index().index_state().unwrap().notes, 63);
}
