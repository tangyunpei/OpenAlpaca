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
