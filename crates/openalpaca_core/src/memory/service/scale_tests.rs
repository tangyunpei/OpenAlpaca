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

/// A row the seeding cannot read as a fingerprint — only a damaged or
/// foreign cache holds one — is never trusted: a row whose id is no note id
/// is removed, and a row whose origin, status or `superseded_by` does not
/// parse is written again from its note, also where the value the seeding
/// falls back to for it (`active`, no successor) is the note's own. Nothing
/// else is rebuilt, and the index is then what it was before the damage.
#[test]
fn a_row_the_seeding_cannot_read_is_removed_or_written_again() {
    let h = Harness::new();
    let path = h.root.path().join("cache").join("memory-index.sqlite");
    let user = h.user(ALICE, None);
    let first = file_service(&h, &path);
    first.rebuild_index().unwrap();
    let notes: Vec<StoreOutput> = (0..6)
        .map(|i| {
            first
                .store(
                    &user,
                    StoreInput {
                        title: format!("Seed note {i}"),
                        body: format!("# S{i}\n\nThe seeding note number {i}.\n"),
                        kind: Some(NoteKind::Fact),
                        ..Default::default()
                    },
                )
                .unwrap()
        })
        .collect();
    drop(first);
    let before = index_contents(&path);
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let columns = "partition_key, id, scope, revision, relative_path, title, norm_title, \
             aliases, tags, norm_aliases, norm_tags, kind, status, effective_status, origin, \
             authority, fact_key, \
             valid_from, valid_to, updated_at, created_at, observed_at, date, pinned, \
             source_forgotten, conflict, same_key_loser, superseded_by, supersedes, \
             body_line_offset, mtime_ns, size, inode";
        let copied = columns
            .replace(" id,", " 'not-a-uuid',")
            .replace(" relative_path,", " 'notes/not-a-uuid.md',");
        let stray = conn
            .execute(
                &format!("INSERT INTO note ({columns}) SELECT {copied} FROM note WHERE id = ?1"),
                [notes[0].id.to_string()],
            )
            .unwrap();
        assert_eq!(stray, 1, "a stray row, a copy of a note's under an id no note has");
        for (note, damage) in [
            (&notes[1], "origin = 'bogus'"),
            (&notes[2], "effective_status = 'bogus'"),
            (&notes[3], "superseded_by = '[\"not-a-uuid\"]'"),
        ] {
            let changed = conn
                .execute(
                    &format!("UPDATE note SET {damage} WHERE id = ?1"),
                    [note.id.to_string()],
                )
                .unwrap();
            assert_eq!(changed, 1, "{damage}");
        }
    }
    assert_ne!(index_contents(&path), before);

    let second = file_service(&h, &path);
    let built = records_built(|| {
        second.reconcile_owner(ALICE).unwrap();
    });
    assert_eq!(built, 3, "the three unreadable rows are written again, nothing else");
    assert_eq!(
        index_contents(&path),
        before,
        "the stray row is gone and each rewritten row is its note's"
    );
}

/// Everything the index holds about the notes of the cache file at `path`
/// — every `note` column but the rowid, every chunk, alias and source keyed
/// by its note — one line per row, sorted; FTS5's own structural check of
/// each full-text index; and what each full-text table indexes, term by
/// term (`fts5vocab`'s `instance` rows keyed by note id and chunk ordinal:
/// the tables are contentless, so a row count would pass with text written
/// into the wrong column — design 09 §6.5.7).
fn index_contents(path: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).unwrap();
    for table in FTS_TABLES {
        conn.execute(
            &format!("INSERT INTO {table}({table}, rank) VALUES ('integrity-check', 0)"),
            [],
        )
        .unwrap_or_else(|e| panic!("{table} is intact: {e}"));
    }
    let mut out = Vec::new();
    for sql in [
        "SELECT partition_key, id, scope, revision, relative_path, title, norm_title, aliases,
                tags, norm_aliases, norm_tags, kind, status, effective_status, origin,
                authority, fact_key, valid_from, valid_to, updated_at, created_at,
                observed_at, date, pinned, source_forgotten, conflict, same_key_loser,
                superseded_by, supersedes, body_line_offset, mtime_ns, size, inode
         FROM note",
        "SELECT n.partition_key, n.id, c.partition_key, c.ordinal, c.heading, c.line_from,
                c.line_to, c.body
         FROM chunk c JOIN note n ON n.rowid = c.note_rowid",
        "SELECT n.partition_key, n.id, a.norm_alias
         FROM note_alias a JOIN note n ON n.rowid = a.note_rowid",
        "SELECT n.partition_key, n.id, s.source_key
         FROM note_source s JOIN note n ON n.rowid = s.note_rowid",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([], |r| {
                (0..columns)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i))
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        let mut lines: Vec<String> = rows.map(|r| format!("{:?}", r.unwrap())).collect();
        lines.sort();
        out.push(format!("-- {} rows", lines.len()));
        out.extend(lines);
    }
    for table in FTS_TABLES {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE temp.vocab_{table} USING fts5vocab(main, {table}, instance);"
        ))
        .unwrap();
        let orphans: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM temp.vocab_{table} v
                     WHERE NOT EXISTS (SELECT 1 FROM chunk c WHERE c.id = v.doc)"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "{table} holds no row of a deleted chunk");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT n.partition_key, n.id, c.ordinal, v.term, v.col, v.offset
                 FROM temp.vocab_{table} v
                 JOIN chunk c ON c.id = v.doc
                 JOIN note n ON n.rowid = c.note_rowid"
            ))
            .unwrap();
        let mut lines: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{table} {} {} {} {:?} {} {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        lines.sort();
        out.push(format!("-- {table}: {} instances", lines.len()));
        out.extend(lines);
    }
    out
}

/// The index's three full-text tables (design 09 §6.5.1).
const FTS_TABLES: [&str; 3] = ["fts_word", "fts_tri_head", "fts_tri"];

/// The notes on disk now, with their current revisions: every status, the
/// candidates and the superseded included.
/// Every note of the partition, in an order that does not depend on the
/// clock or on the random ids the notes were minted with: by title, then by
/// id. The service lists newest first, and notes written in the same second
/// tie on the timestamp and fall back to their ids, so a seeded run that
/// picks "the n-th note" from the service's own order picks a different note
/// on every run — and what it reaches (a forget of a note that shares a
/// source, say) with it. Every title a run writes carries its step, so this
/// order is the run's own.
fn every_note(service: &MemoryService, user: &MemoryAccessContext) -> Vec<ListItem> {
    let mut items = service
        .list(
            user,
            ListInput {
                limit: Some(LIST_MAX_LIMIT),
                include_history: true,
                include_candidates: true,
                ..Default::default()
            },
        )
        .unwrap()
        .items;
    items.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    items
}

/// The rows the incremental passes leave (scale slice S1) are the rows a
/// whole rebuild writes from the same files. A seeded random run mixes the
/// service's writes — saves, replacements, learned notes citing shared
/// message rows, suggestions and their confirmation, forgets (one of a
/// learned note flips its siblings' `source_forgotten` with no file of
/// theirs changing), archives — with hand edits and hand deletions, and
/// restarts in between, each of which reads the fingerprints back from the
/// rows. At each checkpoint a reconciliation tick and a search run, and
/// everything the index holds equals what a separate cache rebuilt from
/// the same files holds — its notes, chunks, aliases and citations — and
/// both full-text indexes pass FTS5's integrity check. No rebuild ran in
/// the incremental service meanwhile.
#[test]
fn incremental_passes_after_restarts_leave_the_rows_a_rebuild_writes() {
    let h = Harness::new();
    let path = h.root.path().join("cache").join("incremental.sqlite");
    let rebuilt = h.root.path().join("cache").join("rebuilt.sqlite");
    let user = h.user(ALICE, None);
    let extraction = h
        .ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
        .unwrap();
    let tool = h.tool(ALICE, None);
    let rows: Vec<i64> = (0..3)
        .map(|i| h.message_row(&format!("I said thing {i} about the orchard.")))
        .collect();
    file_service(&h, &path).rebuild_index().unwrap();
    let mut service = file_service(&h, &path);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut pick = |n: usize| -> usize {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let words = ["orchard", "harbour", "lantern", "meadow", "quarry", "cellar"];
    let keys = ["user.orchard", "user.harbour", "user.lantern"];
    let (mut checkpoints, mut restarts) = (0, 0);
    // What the run reached, over every checkpoint: the derived facts an
    // incremental pass has to follow.
    let mut reached: HashMap<&str, i64> = HashMap::new();
    let facts = [
        ("superseded", "effective_status = 'superseded'"),
        ("archived", "effective_status = 'archived'"),
        ("candidate", "effective_status = 'candidate'"),
        ("source_forgotten", "source_forgotten = 1"),
        ("same_key_loser", "same_key_loser = 1"),
        ("external", "origin = 'external'"),
    ];
    for step in 0..160 {
        let notes = every_note(&service, &user);
        let any = |p: usize| (!notes.is_empty()).then(|| notes[p % notes.len().max(1)].clone());
        let word = words[pick(words.len())];
        let op = pick(12);
        match op {
            0..=2 => {
                let _ = service.store(
                    &user,
                    StoreInput {
                        title: format!("Note {step} {word}"),
                        body: format!("# {word}\n\nStep {step} says the {word} is near.\n"),
                        kind: Some(NoteKind::Fact),
                        fact_key: (pick(3) == 0).then(|| keys[pick(keys.len())].to_string()),
                        aliases: if pick(2) == 0 { vec![format!("{word}-{step}")] } else { vec![] },
                        ..Default::default()
                    },
                );
            }
            3 => {
                let _ = service.store(
                    &extraction,
                    StoreInput {
                        title: format!("Learned {step} {word}"),
                        body: format!("# L\n\n## Knowledge\nThe user likes the {word}, {step}.\n"),
                        kind: Some(NoteKind::Preference),
                        sources: vec![NoteSource {
                            kind: SourceType::Message,
                            id: rows[pick(rows.len())].to_string(),
                            observed_at: None,
                        }],
                        provenance: Some(ExtractionProvenance::UserWords),
                        ..Default::default()
                    },
                );
            }
            4 | 5 => {
                // A replacement: the user's own, or learning's suggestion.
                if let Some(target) = any(pick(97)) {
                    let learned = op == 5;
                    let _ = service.store(
                        if learned { &extraction } else { &user },
                        StoreInput {
                            title: format!("Replacement {step}"),
                            body: format!("# R\n\n## Knowledge\nNow the {word}, {step}.\n"),
                            kind: Some(NoteKind::Preference),
                            sources: if learned {
                                vec![NoteSource {
                                    kind: SourceType::Message,
                                    id: rows[pick(rows.len())].to_string(),
                                    observed_at: None,
                                }]
                            } else {
                                vec![]
                            },
                            supersedes: Some((target.id, target.revision.clone())),
                            replace_policy: if learned {
                                ReplacePolicy::SuggestUnlessLearned
                            } else {
                                ReplacePolicy::Direct
                            },
                            provenance: learned.then_some(ExtractionProvenance::UserWords),
                            ..Default::default()
                        },
                    );
                }
            }
            6 => {
                if let Some(note) = any(pick(97)) {
                    let _ = service.forget(
                        &user,
                        ForgetInput {
                            targets: vec![(note.id, note.revision.clone())],
                            reason: None,
                        },
                    );
                }
            }
            7 | 8 => {
                // An archive, or the confirmation of a suggestion.
                let candidate = notes
                    .iter()
                    .filter(|n| n.status == NoteStatus::Candidate)
                    .nth(pick(4));
                let (note, action) = match (op, candidate) {
                    (8, Some(c)) => {
                        let target = c.supersedes.first().and_then(|t| {
                            notes.iter().find(|n| n.id == *t).map(|n| n.revision.clone())
                        });
                        (
                            Some(c.clone()),
                            UpdateAction::Confirm {
                                target_revision: target,
                            },
                        )
                    }
                    _ => (any(pick(97)), UpdateAction::Archive),
                };
                if let Some(note) = note {
                    let _ = service.update(
                        &user,
                        UpdateInput {
                            id: note.id,
                            expected_revision: note.revision.clone(),
                            action,
                            reason: None,
                        },
                    );
                }
            }
            9 | 10 => {
                // By hand: an edit, or a deletion.
                if let Some(note) = any(pick(97)) {
                    let file = file_of(&h, &user.owner_key, note.id);
                    if op == 9 {
                        let text = std::fs::read_to_string(&file).unwrap();
                        std::fs::write(&file, format!("{text}\nEdited by hand at {step}: {word}.\n"))
                            .unwrap();
                    } else {
                        std::fs::remove_file(&file).unwrap();
                    }
                }
            }
            _ => {
                drop(service);
                service = file_service(&h, &path);
                restarts += 1;
            }
        }
        if step % 40 == 39 {
            service.reconcile_owner(ALICE).unwrap();
            service
                .search(
                    &tool,
                    SearchInput {
                        query: word.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(
                service.rebuild_state().started,
                0,
                "no rebuild ran in the incremental service"
            );
            let reference = file_service(&h, &rebuilt);
            reference.rebuild_index().unwrap();
            drop(reference);
            let incremental = index_contents(&path);
            assert!(incremental.len() > 20, "the run wrote notes: {incremental:?}");
            assert_eq!(
                incremental,
                index_contents(&rebuilt),
                "step {step}: the incremental index is the rebuilt one"
            );
            let conn = rusqlite::Connection::open(&path).unwrap();
            for (fact, condition) in facts {
                let n: i64 = conn
                    .query_row(&format!("SELECT count(*) FROM note WHERE {condition}"), [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                *reached.entry(fact).or_default() += n;
            }
            checkpoints += 1;
        }
    }
    assert_eq!(checkpoints, 4);
    assert!(restarts >= 5, "{restarts} restarts");
    for (fact, _) in facts {
        assert!(reached.get(fact).copied().unwrap_or(0) > 0, "the run reached {fact}: {reached:?}");
    }
}

/// The L8 product slice's P5 (the scale slice's round-3 test review): what
/// only the fingerprint's path and file stat see is caught by the
/// incremental path exactly as a whole rebuild catches it — a note renamed
/// by hand to another slug with its bytes kept (only `relative_path`
/// differs), a same-size edit in place, and a rewrite of the same bytes that
/// moves the file's stat (only the stat differs) — before a restart and
/// after one. At each step a search's stat scan and a tick run, and the
/// index holds what a separate cache rebuilt from the same files holds; the
/// renamed note is found, and the rewritten one is `stale` once, then
/// `verified`.
#[test]
fn a_hand_rename_and_a_same_size_rewrite_are_caught_as_a_rebuild_catches_them() {
    let h = Harness::new();
    let path = h.root.path().join("cache").join("incremental.sqlite");
    let rebuilt = h.root.path().join("cache").join("rebuilt.sqlite");
    let user = h.user(ALICE, None);
    let tool = h.tool(ALICE, None);
    file_service(&h, &path).rebuild_index().unwrap();
    let mut service = file_service(&h, &path);
    let notes: Vec<StoreOutput> = (0..4)
        .map(|i| {
            service
                .store(
                    &user,
                    StoreInput {
                        title: format!("Quince {i}"),
                        body: format!("# Quince {i}\n\nThe quince tree number {i} fruits.\n"),
                        kind: Some(NoteKind::Fact),
                        ..Default::default()
                    },
                )
                .unwrap()
        })
        .collect();
    let search = |service: &MemoryService, query: &str| {
        service
            .search(
                &tool,
                SearchInput {
                    query: query.into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .results
    };
    let as_rebuilt = |service: &MemoryService, what: &str| {
        search(service, "quince");
        service.reconcile_owner(ALICE).unwrap();
        let reference = file_service(&h, &rebuilt);
        reference.rebuild_index().unwrap();
        drop(reference);
        assert_eq!(
            index_contents(&path),
            index_contents(&rebuilt),
            "{what}: the incremental index is the rebuilt one"
        );
    };
    let rename = |id: NoteId, slug: &str| {
        let file = file_of(&h, &user.owner_key, id);
        std::fs::rename(&file, file.with_file_name(format!("{id}--{slug}.md"))).unwrap();
    };
    as_rebuilt(&service, "at the start");

    rename(notes[0].id, "renamed-by-hand");
    as_rebuilt(&service, "after a hand rename");
    let found = search(&service, "quince number 0");
    let hit = found.iter().find(|r| r.id == notes[0].id).expect("found");
    assert!(hit.path.ends_with("--renamed-by-hand.md"), "{}", hit.path);

    let file = file_of(&h, &user.owner_key, notes[1].id);
    let text = std::fs::read_to_string(&file).unwrap();
    let edited = text.replace("fruits", "blooms");
    assert_eq!(edited.len(), text.len());
    std::fs::write(&file, edited).unwrap();
    as_rebuilt(&service, "after a same-size edit in place");

    let file = file_of(&h, &user.owner_key, notes[2].id);
    let bytes = std::fs::read(&file).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000))
        .unwrap();
    let freshness = |service: &MemoryService| {
        search(service, "quince number 2")
            .into_iter()
            .find(|r| r.id == notes[2].id)
            .map(|r| r.freshness)
    };
    assert_eq!(freshness(&service), Some(Freshness::Stale));
    assert_eq!(freshness(&service), Some(Freshness::Verified));
    as_rebuilt(&service, "after a rewrite of the same bytes");

    drop(service);
    service = file_service(&h, &path);
    rename(notes[3].id, "renamed-after-a-restart");
    as_rebuilt(&service, "after a restart and a hand rename");
    assert_eq!(freshness(&service), Some(Freshness::Verified));
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

/// A forgotten note's `.history` put back on its own — no note file, so
/// no scan meets it — is deleted by the next reconciliation tick: leg (2)
/// of why sweeping once per process is exactly as safe (design 03 rev
/// 7.2). A partition opened read-only deletes nothing.
#[test]
fn a_tick_deletes_a_forgotten_notes_history_put_back_alone() {
    let h = Harness::new();
    let user = h.user(ALICE, None);
    let gone = h.store(&user, "Gone", "# Gone\n\nx\n", NoteKind::Fact);
    h.store(&user, "Kept", "# Kept\n\ny\n", NoteKind::Fact);
    h.service
        .forget(
            &user,
            ForgetInput {
                targets: vec![(gone.id, gone.revision.clone())],
                reason: None,
            },
        )
        .unwrap();
    // The service has swept the partition since the forget.
    h.service.list(&user, ListInput::default()).unwrap();
    h.service.reconcile_owner(ALICE).unwrap();
    let history = h
        .home()
        .join("memory/owners")
        .join(&user.owner_key)
        .join(".history")
        .join(gone.id.to_string());
    assert!(!history.exists());
    std::fs::create_dir_all(&history).unwrap();
    std::fs::write(
        history.join(format!("{}.md", gone.revision)),
        "# Gone\n\nx\n",
    )
    .unwrap();

    let read_only = Partition::open_readonly(&StoreScope::Home, &user.owner_key).unwrap();
    assert_eq!(read_only.sweep_forgotten_history().unwrap(), 0);
    assert!(history.exists(), "a read-only partition deletes nothing");

    h.service.reconcile_owner(ALICE).unwrap();
    assert!(!history.exists(), "the tick deleted it");
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
/// waited for. Nothing is indexed inline meanwhile: the answers before the
/// rebuild leave the index empty, and the ones during it come on their own
/// thread, so a read that waited for the rebuild fails the test rather than
/// hang it. The rebuild then leaves a ready index that answers the same.
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
    assert_eq!(
        service.index().index_state().unwrap().notes,
        0,
        "the files answered: nothing was indexed inline"
    );
    assert!(service.rebuild_pending(), "the rebuild is still owed");

    let hold = service.gates.rebuild.write().unwrap();
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild starts", || service.rebuild_running());
    let (sent, answered) = std::sync::mpsc::channel();
    {
        let (service, tool) = (service.clone(), tool.clone());
        std::thread::spawn(move || {
            let during = search_cedar(&service, &tool);
            let suggestions = service
                .pending_suggestions(&tool, 3, &HashSet::new())
                .map(|s| s.len());
            let _ = sent.send((during, suggestions));
        });
    }
    let (during, suggestions) = answered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("search and the suggestions answer while the rebuild runs, not after it");
    assert_eq!(during.results.len(), 6, "answered while the rebuild runs");
    assert_eq!(during.index_state, IndexState::Rebuilding);
    assert_eq!(suggestions.unwrap(), 0, "a turn's suggestions are left out");
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
/// next reconciliation tick rebuilds the cache from a new file. Two cases:
/// the full-text index's pages, and `chunk`'s — since index schema 4 the one
/// copy of the text, read by the ranker on every search (design 09 §6.5.1).
#[test]
fn a_damaged_cache_met_on_use_is_rebuilt_by_the_next_tick() {
    for table in ["fts_word_data", "chunk"] {
        damaged_cache_is_rebuilt_by_the_next_tick(table);
    }
}

/// The pages `dbstat` names as `table`'s, as `(page number, page size)`.
fn pages_of(path: &Path, table: &str) -> (Vec<i64>, i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    let mut stmt = conn
        .prepare("SELECT pageno FROM dbstat WHERE name = ?1 ORDER BY pageno")
        .unwrap();
    let pages: Vec<i64> = stmt
        .query_map([table], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(!pages.is_empty(), "dbstat names pages of {table}");
    (pages, page_size)
}

fn damaged_cache_is_rebuilt_by_the_next_tick(table: &str) {
    let h = Harness::new();
    cedar_notes(&h, 60);
    let path = MemoryIndex::default_path().unwrap();
    drop(MemoryService::open_default(Some(h.db.clone())));
    // Scribble over every page of `table`; the catalog and the schema
    // version, which open reads, are elsewhere and still read.
    {
        use std::io::{Seek, SeekFrom, Write};
        let (pages, page_size) = pages_of(&path, table);
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        for page in pages {
            file.seek(SeekFrom::Start(((page - 1) * page_size) as u64))
                .unwrap();
            file.write_all(&vec![0xAB; page_size as usize]).unwrap();
        }
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
    assert!(err.is_index_corruption(), "{table}: {err}");
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
    // The timeline, which only the index answers, says it is rebuilt.
    let err = timeline_rows(&service, &tool).unwrap_err();
    assert_eq!(err.class(), "index_rebuilding");
    // A read that waits for a rebuild — learning's — waits only while one
    // runs.
    let extraction = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::Extraction,
            Capabilities::EXTRACTION,
            Attribution::default(),
        )
        .unwrap();
    let (sent, read) = std::sync::mpsc::channel();
    {
        let service = service.clone();
        std::thread::spawn(move || {
            let notes = service
                .learning_view(&extraction, &["cedar grove".to_string()], &[])
                .map(|v| v.notes.len());
            let _ = sent.send(notes);
        });
    }
    let notes = read
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the lookup returns rather than wait for a dead rebuild");
    assert_eq!(notes.unwrap(), 4);
    assert!(service.run_pending_rebuild().unwrap(), "the next attempt rebuilds");
    assert!(!service.rebuild_pending());
    let found = search_cedar(&service, &tool);
    assert_eq!(found.index_state, IndexState::Ready);
    assert_eq!(found.results.len(), 4);
}

/// A deferred service over a cache already rebuilt with `n` cedar notes,
/// and Alice's `ModelTool` and `Extraction` contexts on it.
fn ready_cedar_service(
    h: &Harness,
    n: usize,
) -> (Arc<MemoryService>, MemoryAccessContext, MemoryAccessContext) {
    cedar_notes(h, n);
    let service = Arc::new(MemoryService::open_default_deferred(Some(h.db.clone())));
    assert!(service.run_pending_rebuild().unwrap());
    let ctx = |surface, caps| {
        service
            .context(ALICE, WorkspaceHint::None, surface, caps, Attribution::default())
            .unwrap()
    };
    let tool = ctx(Surface::ModelTool, Capabilities::ALL);
    let extraction = ctx(Surface::Extraction, Capabilities::EXTRACTION);
    (service, tool, extraction)
}

/// Every note on the timeline around now.
fn timeline_rows(service: &MemoryService, tool: &MemoryAccessContext) -> Result<usize, MemoryError> {
    service
        .timeline(
            tool,
            TimelineInput {
                anchor: TimelineAnchor::Date(Utc::now()),
                before: Some(20),
                after: Some(20),
            },
        )
        .map(|t| t.rows.len())
}

/// How many notes learning's known-notes lookup lists for the cedar notes:
/// a read that waits for a rebuild (background learning may).
fn cedar_lookup(service: &MemoryService, extraction: &MemoryAccessContext) -> usize {
    service
        .learning_view(extraction, &["cedar grove".to_string()], &[])
        .unwrap()
        .notes
        .len()
}

/// Owes a whole rebuild and starts it on its own thread, held still once
/// it has emptied the index; the rebuild ends when the returned guard is
/// dropped.
fn start_held_rebuild(
    service: &Arc<MemoryService>,
) -> (
    std::sync::RwLockWriteGuard<'_, ()>,
    std::thread::JoinHandle<Result<bool, MemoryError>>,
) {
    let hold = service.gates.rebuild.write().unwrap();
    service.rebuild_state().pending = true;
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild starts", || service.rebuild_running());
    (hold, rebuild)
}

/// A read of the index that has no file-scan answer — learning's
/// known-notes lookup here — waits while a whole rebuild runs, and answers
/// from the rebuilt index with every note, never from the emptied one.
#[test]
fn a_learning_lookup_during_a_rebuild_waits_for_it() {
    let h = Harness::new();
    let (service, _, extraction) = ready_cedar_service(&h, 4);
    let (hold, rebuild) = start_held_rebuild(&service);
    let (sent, read) = std::sync::mpsc::channel();
    {
        let service = service.clone();
        std::thread::spawn(move || {
            let view = service.learning_view(&extraction, &["cedar grove".to_string()], &[]);
            let _ = sent.send((view.map(|v| v.notes.len()), service.rebuild_pending()));
        });
    }
    assert!(
        read.recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "no answer while the rebuild runs"
    );
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
    let (notes, pending) = read
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(!pending, "it answered after the rebuild");
    assert_eq!(notes.unwrap(), 4, "with every note");
}

/// The L8 product slice's P7: recall and search never wait out a rebuild —
/// not even one that starts after they found the index serving and before
/// their stat scans, which used to make those scans wait for the whole
/// rebuild (up to half a minute at 100K notes). Both answer at once, from
/// the files, and say `rebuilding`.
#[test]
fn a_search_or_recall_that_meets_a_rebuild_starting_under_it_reads_the_files() {
    use std::sync::atomic::Ordering::SeqCst;
    let h = Harness::new();
    let (service, tool, _) = ready_cedar_service(&h, 4);
    let serving = service.gates.serving.write().unwrap();
    let seen = service.gates.serving_seen.load(SeqCst);
    let (sent, answered) = std::sync::mpsc::channel();
    {
        let (service, tool, sent) = (service.clone(), tool.clone(), sent.clone());
        std::thread::spawn(move || {
            let found = search_cedar(&service, &tool);
            let _ = sent.send(("search", found.results.len(), found.index_state));
        });
    }
    {
        let (service, tool) = (service.clone(), tool.clone());
        std::thread::spawn(move || {
            let recalled = service
                .recall(&tool, "cedar grove river", RecallBudget::default())
                .unwrap();
            let n = recalled.notes.len() + recalled.listed.len();
            let _ = sent.send(("recall", n, recalled.index_state));
        });
    }
    eventually("both found the index serving", || {
        service.gates.serving_seen.load(SeqCst) >= seen + 2
    });
    let (hold, rebuild) = start_held_rebuild(&service);
    drop(serving);
    for _ in 0..2 {
        let (read, n, state) = answered
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("answered while the rebuild runs, not after it");
        assert_eq!(n, 4, "{read}");
        assert_eq!(state, IndexState::Rebuilding, "{read}");
    }
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
}

/// The L8 product slice's P8: while a whole rebuild runs, the reads a model
/// or a person waits on answer at once — `memory_timeline`, which only the
/// index can answer, says the index is being rebuilt (a refusal the model
/// relays); the suggestions list (the Memory view's, `openalpaca memory
/// suggestions`, `/memories`) comes from the notes' files, as the index
/// would list it; and a deleted conversation's summaries are forgotten from
/// the files. Background learning's lookup still waits for the rebuild.
#[test]
fn the_reads_a_model_or_a_person_waits_on_answer_at_once_while_the_index_is_rebuilt() {
    let h = Harness::new();
    let (service, tool, extraction) = ready_cedar_service(&h, 2);
    let user = service
        .context(
            ALICE,
            WorkspaceHint::None,
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let saved = service
        .store(
            &user,
            StoreInput {
                title: "Commute".into(),
                body: "# Commute\n\nThe user takes the bus.\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("user.commute".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let message = |id: i64| NoteSource {
        kind: SourceType::Message,
        id: id.to_string(),
        observed_at: None,
    };
    let said = h.message_row("I cycle to work now.");
    let suggestion = service
        .store(
            &extraction,
            StoreInput {
                title: "Commute".into(),
                body: "# Commute\n\n## Knowledge\nThe user cycles to work.\n".into(),
                kind: Some(NoteKind::Fact),
                fact_key: Some("user.commute".into()),
                // What else it changes is listed too (the L8 close-out's A4).
                aliases: vec!["bike".into()],
                tags: vec!["travel".into()],
                sources: vec![message(said)],
                observed_at: Some(Utc::now()),
                supersedes: Some((saved.id, saved.revision.clone())),
                replace_policy: ReplacePolicy::SuggestUnlessLearned,
                provenance: Some(ExtractionProvenance::UserWords),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(suggestion.suggestion);
    let talked = h.message_row("Let us plan the Lisbon trip.");
    service
        .store(
            &extraction,
            StoreInput {
                title: "Conversation on 2026-09-20: Lisbon".into(),
                body: "# Lisbon\n\n## Knowledge\nDiscussed flights.\n".into(),
                kind: Some(NoteKind::Episode),
                sources: vec![message(talked)],
                provenance: Some(ExtractionProvenance::Summary),
                ..Default::default()
            },
        )
        .unwrap();
    let from_the_index = format!("{:?}", service.suggestions(&user, None).unwrap());
    assert!(
        from_the_index.contains(r#"FieldChange { field: "aliases", from: [], to: ["bike"] }"#),
        "{from_the_index}"
    );

    let (hold, rebuild) = start_held_rebuild(&service);
    let (sent, answered) = std::sync::mpsc::channel();
    {
        let (service, tool, user) = (service.clone(), tool.clone(), user.clone());
        std::thread::spawn(move || {
            let timeline = timeline_rows(&service, &tool).map_err(|e| e.class());
            let listed = service.suggestions(&user, None).map(|l| format!("{l:?}"));
            let forgotten = service.forget_conversation_summaries(ALICE, None, &[talked]);
            let _ = sent.send((timeline, listed, forgotten));
        });
    }
    let (timeline, listed, forgotten) = answered
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("answered while the rebuild runs, not after it");
    assert_eq!(timeline.unwrap_err(), "index_rebuilding");
    assert_eq!(listed.unwrap(), from_the_index, "the files list what the index lists");
    assert_eq!(forgotten.unwrap(), 1);

    let (sent, looked) = std::sync::mpsc::channel();
    {
        let (service, extraction) = (service.clone(), extraction.clone());
        std::thread::spawn(move || {
            let _ = sent.send(cedar_lookup(&service, &extraction));
        });
    }
    assert!(
        looked
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "learning's lookup waits for the rebuild"
    );
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
    assert_eq!(
        looked
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        2
    );
    assert_eq!(timeline_rows(&service, &tool).unwrap(), 3, "two notes and the saved one");
}

/// The L8 product slice's P6 (the scale slice's round-3 safety review,
/// F1): a service write to a partition while a whole rebuild has opened it
/// but not yet read it is indexed with the derived facts of the partition
/// as it stands after the write — not those of the control snapshot the
/// rebuild read when it opened the partition. Here the user replaces a note
/// (same `fact_key`) and a reconciliation tick runs meanwhile, each on its
/// own thread since the rebuild may hold the partition; afterwards the
/// replacement's row is the user's, the old note's is superseded, and
/// recall serves the replacement and not the note it replaced.
#[test]
fn a_write_while_a_rebuild_reads_its_partition_is_indexed_as_it_stands() {
    use std::sync::atomic::Ordering::SeqCst;
    let h = Harness::new();
    let (service, tool, _) = ready_cedar_service(&h, 2);
    let user = h.user(ALICE, None);
    let old = service
        .store(
            &user,
            StoreInput {
                title: "Editor choice".into(),
                body: "# Editor\n\nThe user edits with marmalade vim.\n".into(),
                kind: Some(NoteKind::Preference),
                fact_key: Some("user.editor".into()),
                ..Default::default()
            },
        )
        .unwrap();

    // A rebuild is owed — boot, damage or a bloated cache — and it has
    // opened the partition.
    service.gates.opened.store(false, SeqCst);
    let hold = service.gates.after_open.write().unwrap();
    service.rebuild_state().pending = true;
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild opened the partition", || {
        service.gates.opened.load(SeqCst)
    });
    let writer = {
        let (service, user, old) = (service.clone(), user.clone(), old.clone());
        std::thread::spawn(move || {
            service.store(
                &user,
                StoreInput {
                    title: "Editor choice now".into(),
                    body: "# Editor\n\nThe user edits with marmalade helix now.\n".into(),
                    kind: Some(NoteKind::Preference),
                    fact_key: Some("user.editor".into()),
                    supersedes: Some((old.id, old.revision.clone())),
                    ..Default::default()
                },
            )
        })
    };
    // The write lands now unless the partition is held; then a tick, which
    // lands now on the same terms. Either way the rebuild then goes on.
    fn settle<T>(t: &std::thread::JoinHandle<T>) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while !t.is_finished() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    settle(&writer);
    let tick = {
        let service = service.clone();
        std::thread::spawn(move || service.reconcile_owner(ALICE).map(|_| ()))
    };
    settle(&tick);
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
    let new = writer.join().unwrap().unwrap();
    tick.join().unwrap().unwrap();

    // As soon as the three have joined — no later tick heals the index
    // first (repair round 1 of the L8 product slice) — and again after one.
    let key = files::home_partition_key(&user.owner_key);
    let check = |when: &str| {
        let recalled = service
            .recall(&tool, "which editor marmalade", RecallBudget::default())
            .unwrap();
        let ids: Vec<NoteId> = recalled
            .notes
            .iter()
            .map(|n| n.id)
            .chain(recalled.listed.iter().map(|l| l.id))
            .collect();
        assert!(
            ids.contains(&new.id) && !ids.contains(&old.id),
            "{when}: recall serves the replacement, not what it replaced: {ids:?}"
        );
        let row = |id: NoteId| {
            service
                .index()
                .row_facts(&key)
                .unwrap()
                .into_iter()
                .find(|r| r.id == id.to_string())
                .unwrap()
        };
        assert_eq!(row(new.id).origin, "user", "{when}");
        assert!(
            !row(new.id).same_key_loser && !row(new.id).conflict,
            "{when}"
        );
        assert_eq!(
            row(old.id).effective_status,
            Some(index::EffectiveStatus::Superseded),
            "{when}"
        );
    };
    check("before any further tick");
    service.reconcile_owner(ALICE).unwrap();
    check("after a tick");
}

/// P6, the forget contract's half: a forget while a rebuild has opened the
/// partition leaves the note that cites the same message out of automatic
/// recall (design 03 §6, *Forget contract*) — its row says
/// `source_forgotten` — as it would with no rebuild running.
#[test]
fn a_forget_while_a_rebuild_reads_its_partition_leaves_its_sources_forgotten() {
    use std::sync::atomic::Ordering::SeqCst;
    let h = Harness::new();
    let (service, tool, extraction) = ready_cedar_service(&h, 2);
    let user = h.user(ALICE, None);
    let row = h.message_row("I drink oolong-marmalade tea every morning.");
    let learned = |title: &str, body: &str| {
        service
            .store(
                &extraction,
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
    };
    let forgotten = learned("Morning tea", "# Tea\n\nThe user drinks oolong-marmalade tea.\n");
    let sibling = learned("Tea habit", "# Habit\n\nEvery morning: oolong-marmalade tea.\n");

    service.gates.opened.store(false, SeqCst);
    let hold = service.gates.after_open.write().unwrap();
    service.rebuild_state().pending = true;
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild opened the partition", || {
        service.gates.opened.load(SeqCst)
    });
    let forget = {
        let (service, user) = (service.clone(), user.clone());
        let target = (forgotten.id, forgotten.revision.clone());
        std::thread::spawn(move || {
            service.forget(
                &user,
                ForgetInput {
                    targets: vec![target],
                    reason: None,
                },
            )
        })
    };
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while !forget.is_finished() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let tick = {
        let service = service.clone();
        std::thread::spawn(move || service.reconcile_owner(ALICE).map(|_| ()))
    };
    let until = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while !tick.is_finished() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
    forget.join().unwrap().unwrap();
    tick.join().unwrap().unwrap();

    // As soon as the three have joined, and again after a tick.
    let key = files::home_partition_key(&user.owner_key);
    let check = |when: &str| {
        let recalled = service
            .recall(&tool, "oolong marmalade tea", RecallBudget::default())
            .unwrap();
        let ids: Vec<NoteId> = recalled
            .notes
            .iter()
            .map(|n| n.id)
            .chain(recalled.listed.iter().map(|l| l.id))
            .collect();
        assert!(
            !ids.contains(&sibling.id),
            "{when}: a note citing a forgotten note's source is recalled: {ids:?}"
        );
        let facts = service.index().row_facts(&key).unwrap();
        let sib = facts
            .iter()
            .find(|r| r.id == sibling.id.to_string())
            .unwrap();
        assert!(sib.source_forgotten, "{when}");
    };
    check("before any further tick");
    service.reconcile_owner(ALICE).unwrap();
    check("after a tick");
}

/// P6's tick bookkeeping (repair round 1 of the L8 product slice): a tick
/// whose index pass a running rebuild skipped does not remember its scan,
/// and a completed rebuild forgets every remembered scan — so the next tick
/// itself, with no read between, brings the index in line with a hand edit
/// made after the rebuild read the partition. Every read's stat scan would
/// also find that edit; this pins the tick's own catch-up (its doc: a scan
/// is remembered once its work has succeeded). Either half alone suffices
/// here; the test fails only with both taken out.
#[test]
fn a_tick_whose_pass_a_rebuild_skipped_is_done_by_the_next_tick() {
    use std::sync::atomic::Ordering::SeqCst;
    let h = Harness::new();
    let (service, _, _) = ready_cedar_service(&h, 2);
    let user = h.user(ALICE, None);
    let note = service
        .store(
            &user,
            StoreInput {
                title: "Herb garden".into(),
                body: "# Herb garden\n\nThe user grows basil.\n".into(),
                kind: Some(NoteKind::Fact),
                ..Default::default()
            },
        )
        .unwrap();
    service.reconcile_owner(ALICE).unwrap();

    // A rebuild that has read every partition and not yet finished.
    service.gates.all_read.store(false, SeqCst);
    let hold = service.gates.after_read.write().unwrap();
    service.rebuild_state().pending = true;
    let rebuild = {
        let service = service.clone();
        std::thread::spawn(move || service.run_pending_rebuild())
    };
    eventually("the rebuild read every partition", || {
        service.gates.all_read.load(SeqCst)
    });
    let file = file_of(&h, &user.owner_key, note.id);
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("grows basil", "grows basil and thyme")).unwrap();
    let during = service.reconcile_owner(ALICE).unwrap();
    let [edit] = &during.changes[..] else {
        panic!("the tick finds the hand edit: {:?}", during.changes);
    };
    let edited = edit.revision.clone().expect("a revision");
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());

    service.reconcile_owner(ALICE).unwrap();
    let key = files::home_partition_key(&user.owner_key);
    let row = service
        .index()
        .row_facts(&key)
        .unwrap()
        .into_iter()
        .find(|r| r.id == note.id.to_string())
        .unwrap();
    assert_eq!(row.revision, edited, "the tick indexed the edited file");
    assert_eq!(row.origin, "external");
}

/// The wait above is checked before the index is read, so a rebuild can
/// start between the check and the read: here once the lookup's `touch`
/// has done its work. The read then meets an emptied index; it waits for
/// the rebuild and reads again rather than answer from it.
#[test]
fn a_rebuild_that_starts_under_a_read_makes_it_read_again() {
    let h = Harness::new();
    let (service, _, extraction) = ready_cedar_service(&h, 4);
    assert_eq!(cedar_lookup(&service, &extraction), 4);
    let touched = service.gates.touched.write().unwrap();
    let before = service
        .gates
        .touches
        .load(std::sync::atomic::Ordering::SeqCst);
    let (sent, read) = std::sync::mpsc::channel();
    {
        let (service, extraction) = (service.clone(), extraction.clone());
        std::thread::spawn(move || {
            let _ = sent.send(cedar_lookup(&service, &extraction));
        });
    }
    eventually("the lookup has touched its partition", || {
        service
            .gates
            .touches
            .load(std::sync::atomic::Ordering::SeqCst)
            > before
    });
    let (hold, rebuild) = start_held_rebuild(&service);
    drop(touched);
    assert!(
        read.recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "no answer from the emptied index"
    );
    drop(hold);
    assert!(rebuild.join().unwrap().unwrap());
    let rows = read
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert_eq!(rows, 4, "read again from the rebuilt index");
}

/// Overwrites the root page of the B-tree `name` in the closed cache file
/// at `path` — damage below the catalog that only the reads using that
/// B-tree meet.
fn damage_btree(path: &Path, name: &str) {
    let (root, page_size): (i64, i64) = {
        let conn = rusqlite::Connection::open(path).unwrap();
        let root = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap();
        let page_size = conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap();
        (root, page_size)
    };
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(((root - 1) * page_size) as u64))
        .unwrap();
    file.write_all(&vec![0xAB; page_size as usize]).unwrap();
}

/// A deferred service over a ready cache of 20 cedar notes — the first
/// pinned, the second citing a message row — whose B-tree `btree` is
/// damaged: what a restart meets, since open runs no integrity check.
/// Answers the service and the message row.
fn service_with_damaged(h: &Harness, btree: &str) -> (MemoryService, i64) {
    cedar_notes(h, 20);
    let user = h.user(ALICE, None);
    let first = h
        .service
        .list(&user, ListInput::default())
        .unwrap()
        .items
        .into_iter()
        .map(|i| i.id)
        .min()
        .unwrap();
    let file = file_of(h, &user.owner_key, first);
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text.replace("pinned: false", "pinned: true")).unwrap();
    let row = h.message_row("The cedar grove by the river is where I walk.");
    let extraction = h
        .ctx(ALICE, None, Surface::Extraction, Capabilities::EXTRACTION)
        .unwrap();
    learned_note(
        h,
        &extraction,
        "Walks",
        "# Walks\n\n## Knowledge\nThe user walks in the cedar grove.\n",
        row,
    );
    let path = MemoryIndex::default_path().unwrap();
    drop(MemoryService::open_default(Some(h.db.clone())));
    damage_btree(&path, btree);
    let service = MemoryService::open_default_deferred(Some(h.db.clone()));
    assert!(!service.rebuild_pending(), "open does not look");
    (service, row)
}

fn context(service: &MemoryService, surface: Surface, caps: Capabilities) -> MemoryAccessContext {
    service
        .context(ALICE, WorkspaceHint::None, surface, caps, Attribution::default())
        .unwrap()
}

/// Damage in a B-tree only one read reaches — here the chunk-by-note index,
/// read by recall's pinned notes (step 2) and by no stat scan, search or
/// tick — owes a rebuild when that read meets it, as damage met anywhere
/// else does: recall reads the files from then on, and the next tick
/// rebuilds the cache. Before, nothing marked it, so every turn's recall
/// failed at its pinned notes, and not even a restart repaired the file.
#[test]
fn damage_met_by_recalls_pinned_notes_owes_a_rebuild() {
    let h = Harness::new();
    let (service, _) = service_with_damaged(&h, "chunk_note");
    let tool = context(&service, Surface::ModelTool, Capabilities::ALL);
    let recall = || service.recall(&tool, "cedar grove river", RecallBudget::default());
    let err = recall().expect_err("the pinned note's chunks are counted");
    assert!(err.is_index_corruption(), "{err}");
    assert!(service.rebuild_pending(), "the damage owes a rebuild");
    let recalled = recall().unwrap();
    assert_eq!(recalled.index_state, IndexState::Rebuilding);
    assert!(!recalled.notes.is_empty(), "the files answer");

    service.reconcile_owner(ALICE).unwrap();
    assert!(!service.rebuild_pending(), "the tick rebuilt it");
    let recalled = recall().unwrap();
    assert_eq!(recalled.index_state, IndexState::Ready);
    assert!(!recalled.notes.is_empty());
}

/// The same for a read with no file-scan answer: learning's lookup meets
/// damage in the citations' index, owes the rebuild, and after the next
/// tick finds the note citing its row again — where it used to fail on
/// every flush, learning backing off and retrying for good.
#[test]
fn damage_met_by_learnings_lookup_owes_a_rebuild() {
    let h = Harness::new();
    let (service, row) = service_with_damaged(&h, "note_source_key");
    let extraction = context(&service, Surface::Extraction, Capabilities::EXTRACTION);
    let lookup = || service.learning_view(&extraction, &["cedar grove".to_string()], &[row]);
    let err = lookup().expect_err("the citations' index is read");
    assert!(err.is_index_corruption(), "{err}");
    assert!(service.rebuild_pending(), "the damage owes a rebuild");

    service.reconcile_owner(ALICE).unwrap();
    assert!(!service.rebuild_pending(), "the tick rebuilt it");
    let view = lookup().unwrap();
    assert_eq!(view.from_this_window.len(), 1, "{view:?}");
}

// ============================================================================
// S5 — the index is maintained (M1)
// ============================================================================

/// How many segments each full-text index of the cache file at `path`
/// holds, `(fts_word, fts_tri_head, fts_tri)`, read on a connection of the
/// test's own.
fn fts_segments(path: &Path) -> (i64, i64, i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(
            &format!("SELECT count(DISTINCT segid) FROM {table}_idx"),
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    (count("fts_word"), count("fts_tri_head"), count("fts_tri"))
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
                    // A Chinese line, so the body trigram table takes rows
                    // too (design 09 §6.5.1).
                    body: format!(
                        "# {prefix} {i}\n\n{} {i}\n\n核桃园的账本 {i}\n",
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
    let (word, head, tri) = fts_segments(&path);
    assert!(
        word > 1 && head > 1 && tri > 1,
        "the writes left segments to merge: {word}, {head}, {tri}"
    );
    service.reconcile_owner(ALICE).unwrap();
    assert!(wal_len(&path) > 0, "a tick right after writes leaves it");
    assert_eq!(
        fts_segments(&path),
        (word, head, tri),
        "and merges nothing"
    );
    service.reconcile_owner(ALICE).unwrap();
    assert_eq!(wal_len(&path), 0, "the idle tick truncated it");
    assert_eq!(
        fts_segments(&path),
        (1, 1, 1),
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
