//! The scale slice's service tests (L8 benches, `scale-fix/SPEC.md` S1–S7):
//! each pins a cost the benches measured away, or a guarantee the fix must
//! keep, through the service's own entry points. Temp home stores and
//! in-memory (or temp-file) indexes only; no model, no network.

use super::tests::{ALICE, Harness};
use super::*;
use openalpaca_storage::memory_files::{NoteKind, NoteSource, SourceType};
use openalpaca_storage::memory_index::MemoryIndex;

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
