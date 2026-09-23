//! Tests for the memory index: the chunker, the query builder, the catalog,
//! search semantics, the stat scan, rebuild equivalence and `index_state`.
//! Temp directories only; no store root, no daemon, no network.

use super::*;
use tempfile::tempdir;

// ============================================================================
// Open and state
// ============================================================================

#[test]
fn a_fresh_index_reports_rebuilding_until_rebuilt() {
    let index = MemoryIndex::open_in_memory().unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0);
    assert_eq!(report.chunks, 0);
    assert_eq!(report.partitions, 0);
    assert_eq!(report.path, None);
}

#[test]
fn open_creates_the_file_and_its_parent_directory() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("state").join("cache").join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&path).unwrap();
    assert!(path.exists());
    assert_eq!(index.path(), Some(path.as_path()));
    assert_eq!(index.index_state().unwrap().state, IndexState::Rebuilding);
}

#[test]
fn open_recreates_a_corrupt_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    std::fs::write(&path, b"this is not a database").unwrap();
    let index = MemoryIndex::open(&path).unwrap();
    assert_eq!(index.index_state().unwrap().state, IndexState::Rebuilding);
    assert!(path.exists());
}

#[test]
fn open_recreates_a_file_of_another_schema_version() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '999' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
        write_state(&conn, IndexState::Ready).unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
}

#[test]
fn canonicalize_timestamp_normalizes_offsets_and_subseconds() {
    assert_eq!(
        canonicalize_timestamp("t", "2026-06-01T02:00:00.123+02:00").unwrap(),
        "2026-06-01T00:00:00Z"
    );
    assert_eq!(
        canonicalize_timestamp("t", "2026-06-01T00:00:00Z").unwrap(),
        "2026-06-01T00:00:00Z"
    );
    let err = canonicalize_timestamp("valid_to", "June 2026").unwrap_err();
    assert!(matches!(err, IndexError::InvalidInput(m) if m.contains("valid_to")));
}

// ============================================================================
// Record builder
// ============================================================================

/// A record with every field the index needs, defaulting to an active,
/// user-origin, global note in partition `p`.
pub(super) fn record(id: &str, title: &str, body: &str) -> IndexRecord {
    IndexRecord {
        id: id.to_string(),
        revision: format!("{:0>64}", id.len()),
        partition_key: "home-p".to_string(),
        scope: "global".to_string(),
        relative_path: format!("notes/{id}--note.md"),
        title: title.to_string(),
        aliases: Vec::new(),
        tags: Vec::new(),
        kind: "fact".to_string(),
        status: "active".to_string(),
        effective_status: EffectiveStatus::Active,
        origin: "user".to_string(),
        fact_key: None,
        valid_from: None,
        valid_to: None,
        updated_at: "2026-09-23T10:00:00Z".to_string(),
        pinned: false,
        source_forgotten: false,
        conflict: false,
        same_key_loser: false,
        superseded_by: Vec::new(),
        body: body.to_string(),
        body_line_offset: 0,
        file: FileStat {
            mtime_ns: 1,
            size: body.len() as u64,
            inode: 7,
        },
    }
}

fn ready_index() -> MemoryIndex {
    let index = MemoryIndex::open_in_memory().unwrap();
    index.rebuild(&[]).unwrap();
    index
}

// ============================================================================
// Chunker
// ============================================================================

mod chunker_tests {
    use super::super::chunker::*;

    /// Every chunk's text is exactly the lines its span names.
    fn assert_spans_match(body: &str, chunks: &[Chunk]) {
        let lines: Vec<&str> = body.split('\n').collect();
        for chunk in chunks {
            let from = chunk.line_from as usize - 1;
            let to = chunk.line_to as usize - 1;
            let expected = lines[from..=to].join("\n");
            if chunk.line_from == chunk.line_to && expected.len() > CHUNK_MAX_BYTES {
                // A cut line: the chunk is one piece of it.
                assert!(expected.contains(&chunk.text));
            } else {
                assert_eq!(chunk.text, expected, "chunk {}", chunk.ordinal);
            }
            assert!(chunk.text.len() <= CHUNK_MAX_BYTES, "chunk {} too large", chunk.ordinal);
        }
    }

    #[test]
    fn empty_and_blank_bodies_yield_no_chunk() {
        assert!(chunk_body("").is_empty());
        assert!(chunk_body("\n\n  \n").is_empty());
    }

    #[test]
    fn one_small_note_is_one_chunk_with_its_heading() {
        let body = "# Editor preference\n\nUser prefers vim for editing.\n";
        let chunks = chunk_body(body);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].ordinal, 0);
        assert_eq!(chunks[0].heading.as_deref(), Some("Editor preference"));
        assert_eq!((chunks[0].line_from, chunks[0].line_to), (1, 3));
        assert_eq!(chunks[0].text, "# Editor preference\n\nUser prefers vim for editing.");
        assert_spans_match(body, &chunks);
    }

    #[test]
    fn sections_pack_into_one_chunk_when_small_and_carry_the_nearest_heading() {
        let body = "# Build\n\n## Knowledge\nRun the checks.\n\n## Recheck when\nThe entry points change.\n";
        let chunks = chunk_body(body);
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert_eq!(chunks[0].heading.as_deref(), Some("Build"));
        assert_eq!((chunks[0].line_from, chunks[0].line_to), (1, 7));
        assert_spans_match(body, &chunks);
    }

    #[test]
    fn a_heading_starts_a_new_chunk_once_the_open_one_is_half_full() {
        let para = "word ".repeat(200); // ~1000 bytes
        let body = format!("# One\n\n{para}\n\n## Two\n\n{para}\n");
        let chunks = chunk_body(&body);
        assert_eq!(chunks.len(), 2, "{:?}", chunks.iter().map(|c| c.text.len()).collect::<Vec<_>>());
        assert_eq!(chunks[0].heading.as_deref(), Some("One"));
        assert_eq!(chunks[1].heading.as_deref(), Some("Two"));
        assert_eq!(chunks[1].line_from, 5);
        assert_spans_match(&body, &chunks);
    }

    #[test]
    fn paragraphs_cut_at_the_target_and_never_exceed_the_maximum() {
        let para = "lorem ipsum dolor sit amet ".repeat(20); // 540 bytes
        let body = (0..12).map(|_| para.clone()).collect::<Vec<_>>().join("\n\n");
        let chunks = chunk_body(&body);
        assert!(chunks.len() >= 4, "{}", chunks.len());
        for c in &chunks {
            assert!(c.text.len() <= CHUNK_TARGET_BYTES, "{}", c.text.len());
            assert!(c.heading.is_none());
        }
        assert_spans_match(&body, &chunks);
        // Ordinals are consecutive and spans are increasing.
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.ordinal as usize, i);
            if i > 0 {
                assert!(c.line_from > chunks[i - 1].line_to);
            }
        }
    }

    #[test]
    fn a_block_over_the_maximum_splits_at_line_boundaries() {
        let line = "x".repeat(700);
        let body = format!("{line}\n{line}\n{line}\n{line}\n");
        let chunks = chunk_body(&body);
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].line_from, chunks[0].line_to), (1, 2));
        assert_eq!((chunks[1].line_from, chunks[1].line_to), (3, 4));
        assert_spans_match(&body, &chunks);
    }

    #[test]
    fn a_single_long_cjk_line_is_cut_on_char_boundaries() {
        let line: String = "用户喜欢简洁的回答。".repeat(300); // 9000 bytes, 3-byte chars
        let body = format!("# 长\n\n{line}\n");
        let chunks = chunk_body(&body);
        let pieces: Vec<&Chunk> = chunks.iter().filter(|c| c.line_from == 3).collect();
        assert!(pieces.len() >= 5, "{}", pieces.len());
        let joined: String = pieces.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(joined, line);
        for (i, p) in pieces.iter().enumerate() {
            assert!(p.text.len() <= CHUNK_MAX_BYTES);
            // Cut on a 3-byte character boundary: every piece but the last is
            // within one character of the bound.
            if i + 1 < pieces.len() {
                assert!(p.text.len() >= CHUNK_MAX_BYTES - 2, "{}", p.text.len());
            }
            assert!(!p.text.is_empty());
            assert_eq!(p.heading.as_deref(), Some("长"));
        }
        assert_spans_match(&body, &chunks);
    }

    #[test]
    fn a_fenced_code_block_stays_whole_when_it_fits() {
        let body = "# Cmd\n\n```sh\ncargo test\n\ncargo clippy\n```\n\nThen read the output.\n";
        let chunks = chunk_body(body);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("cargo test\n\ncargo clippy"));
        assert_spans_match(body, &chunks);
    }

    #[test]
    fn heading_text_strips_markers_and_ignores_non_headings() {
        let chunks = chunk_body("###   Spaced heading ##\nbody\n");
        assert_eq!(chunks[0].heading.as_deref(), Some("Spaced heading"));
        let chunks = chunk_body("#hashtag is not a heading\n");
        assert_eq!(chunks[0].heading, None);
        let chunks = chunk_body("####### seven is too many\n");
        assert_eq!(chunks[0].heading, None);
    }
}

// ============================================================================
// Query builder
// ============================================================================

mod query_tests {
    use super::super::query::*;

    fn terms(q: &str) -> Vec<Term> {
        parse_query(q).unwrap().terms
    }

    #[test]
    fn normalize_folds_width_case_and_latin_diacritics_and_keeps_cjk() {
        assert_eq!(normalize("ｑｕｉｃｋ　Replies"), "quick replies");
        assert_eq!(normalize("Café CRÈME"), "cafe creme");
        assert_eq!(normalize("用户喜欢简洁的回答"), "用户喜欢简洁的回答");
        assert_eq!(normalize_exact("  Tabs   and\tspaces "), "tabs and spaces");
        // Devanagari vowel signs are not Latin diacritics: untouched.
        assert_eq!(normalize("हिन्दी"), "हिन्दी");
    }

    #[test]
    fn empty_and_over_long_queries_are_refused() {
        assert!(parse_query("").is_err());
        assert!(parse_query("   ").is_err());
        assert!(parse_query(&"a".repeat(512)).is_ok());
        assert!(parse_query(&"a".repeat(513)).is_err());
        assert!(parse_query(&"答".repeat(512)).is_ok());
        assert!(parse_query(&"答".repeat(513)).is_err());
    }

    #[test]
    fn words_of_three_or_more_characters_are_routed_and_stop_words_dropped() {
        assert_eq!(
            terms("what editor do I prefer?"),
            vec![Term::Word("editor".into()), Term::Word("prefer".into())]
        );
        // Stop words survive when nothing else remains.
        assert_eq!(terms("and"), vec![Term::Word("and".into())]);
        assert_eq!(
            terms("AND OR NOT"),
            vec![Term::Word("and".into()), Term::Word("not".into())]
        );
        // A short Latin term is dropped beside a longer one, kept alone.
        assert_eq!(terms("or"), vec![Term::Short("or".into())]);
        assert_eq!(terms("tabs OR tomatoes"), vec![Term::Word("tabs".into()), Term::Word("tomatoes".into())]);
        assert_eq!(terms("p50/p95 (ms)"), vec![
            Term::Literal("p50/p95".into()),
            Term::Word("p50".into()),
            Term::Word("p95".into()),
        ]);
    }

    #[test]
    fn short_cjk_terms_are_kept_beside_longer_ones() {
        assert_eq!(
            terms("简洁 quick"),
            vec![Term::Short("简洁".into()), Term::Word("quick".into())]
        );
        assert_eq!(terms("答"), vec![Term::Short("答".into())]);
        assert_eq!(terms("简洁的回答"), vec![Term::Word("简洁的回答".into())]);
        // Two-character CJK is a word of the language, not a fragment.
        let t = terms("please remind me which answer style the user 喜欢");
        assert!(t.contains(&Term::Short("喜欢".into())));
        assert!(t.contains(&Term::Word("answer".into())));
        assert!(!t.iter().any(|x| x.text() == "the"));
    }

    #[test]
    fn identifiers_are_literals_plus_their_parts_and_punctuation_is_trimmed() {
        assert_eq!(terms("OPENALPACA_HOME_STORE"), vec![
            Term::Literal("openalpaca_home_store".into()),
            Term::Word("openalpaca".into()),
            Term::Word("home".into()),
            Term::Word("store".into()),
        ]);
        assert_eq!(terms("openalpaca_storage::memory_files")[0], Term::Literal("openalpaca_storage::memory_files".into()));
        assert_eq!(terms("\"tabs for indentation"), vec![Term::Word("tabs".into()), Term::Word("indentation".into())]);
        assert_eq!(terms("tabs*"), vec![Term::Word("tabs".into())]);
        assert_eq!(terms("title:tabs"), vec![Term::Literal("title:tabs".into()), Term::Word("title".into()), Term::Word("tabs".into())]);
        assert_eq!(terms("(tabs"), vec![Term::Word("tabs".into())]);
        assert_eq!(terms("-tabs"), vec![Term::Word("tabs".into())]);
        assert_eq!(terms("^tabs"), vec![Term::Word("tabs".into())]);
        assert_eq!(terms("cargo test --ignored bench_"), vec![
            Term::Word("cargo".into()),
            Term::Word("test".into()),
            Term::Word("ignored".into()),
            Term::Word("bench".into()),
        ]);
    }

    #[test]
    fn punctuation_only_queries_have_no_terms_but_are_not_refused() {
        assert!(terms("???").is_empty());
        assert!(terms("\"").is_empty());
        assert!(terms("; -- ").is_empty());
    }

    #[test]
    fn match_strings_are_quoted_literals_joined_by_or() {
        let q = parse_query("tabs AND spaces").unwrap();
        assert_eq!(q.word_match().as_deref(), Some("\"tabs\" OR \"spaces\""));
        assert_eq!(q.trigram_match().as_deref(), Some("\"tabs\" OR \"spaces\""));
        let q = parse_query("{tabs} [spaces] ; DROP TABLE notes; --").unwrap();
        assert_eq!(
            q.word_match().as_deref(),
            Some("\"tabs\" OR \"spaces\" OR \"drop\" OR \"table\" OR \"notes\"")
        );
        // A quote inside a literal is doubled, never a delimiter.
        let q = parse_query("a\"b\"c").unwrap();
        assert_eq!(q.word_match().as_deref(), Some("\"a\"\"b\"\"c\""));
        // Nothing routed: no MATCH at all.
        assert_eq!(parse_query("or").unwrap().word_match(), None);
        assert_eq!(parse_query("???").unwrap().trigram_match(), None);
    }

    #[test]
    fn duplicate_terms_collapse_and_the_term_count_is_capped() {
        assert_eq!(terms("tabs tabs TABS"), vec![Term::Word("tabs".into())]);
        let long: Vec<String> = (0..50).map(|i| format!("word{i}")).collect();
        assert_eq!(terms(&long.join(" ")).len(), MAX_TERMS);
    }
}

// ============================================================================
// Catalog
// ============================================================================

#[test]
fn upsert_indexes_a_note_and_get_reports_its_catalog_row() {
    let index = ready_index();
    let rec = record("n1", "Editor preference", "# Editor preference\n\nUser prefers vim for editing.\n");
    assert_eq!(index.upsert(&rec).unwrap(), 1);
    let got = index.get("home-p", "n1").unwrap().unwrap();
    assert_eq!(got.id, "n1");
    assert_eq!(got.revision, rec.revision);
    assert_eq!(got.relative_path, "notes/n1--note.md");
    assert_eq!(got.effective_status, EffectiveStatus::Active);
    assert_eq!(got.file, rec.file);
    assert_eq!(got.chunks, 1);
    assert!(index.get("home-p", "nope").unwrap().is_none());
    assert!(index.get("home-other", "n1").unwrap().is_none());
    let report = index.index_state().unwrap();
    assert_eq!((report.notes, report.chunks, report.partitions), (1, 1, 1));
    assert_eq!(report.state, IndexState::Ready);
}

#[test]
fn upsert_replaces_the_same_id_and_the_same_path() {
    let index = ready_index();
    let mut rec = record("n1", "Old title", "Old body.\n");
    index.upsert(&rec).unwrap();
    // A retitle: same id, new path.
    rec.title = "New title".into();
    rec.relative_path = "notes/n1--new-title.md".into();
    rec.body = "New body.\n".into();
    index.upsert(&rec).unwrap();
    assert_eq!(index.index_state().unwrap().notes, 1);
    assert_eq!(index.get("home-p", "n1").unwrap().unwrap().relative_path, "notes/n1--new-title.md");
    // A file rewritten under a new id at the old path replaces that row.
    let mut other = record("n2", "Other", "Other body.\n");
    other.relative_path = "notes/n1--new-title.md".into();
    index.upsert(&other).unwrap();
    assert_eq!(index.index_state().unwrap().notes, 1);
    assert!(index.get("home-p", "n1").unwrap().is_none());
    assert!(index.get("home-p", "n2").unwrap().is_some());
    let conn = index.lock();
    let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunk", [], |r| r.get(0)).unwrap();
    let fts: i64 = conn.query_row("SELECT COUNT(*) FROM fts_word", [], |r| r.get(0)).unwrap();
    assert_eq!((chunks, fts), (1, 1));
}

#[test]
fn upsert_refuses_an_over_size_body_and_bad_timestamps() {
    let index = ready_index();
    let big = record("n1", "Big", &"x".repeat(MAX_NOTE_BYTES + 1));
    let err = index.upsert(&big).unwrap_err();
    assert!(matches!(err, IndexError::InvalidInput(m) if m.contains("32768")));
    let ok = record("n1", "Big", &"x".repeat(MAX_NOTE_BYTES));
    index.upsert(&ok).unwrap();
    let mut bad = record("n2", "Bad", "body");
    bad.valid_to = Some("June".into());
    assert!(matches!(index.upsert(&bad).unwrap_err(), IndexError::InvalidInput(m) if m.contains("valid_to")));
    let mut bad = record("n3", "Bad", "body");
    bad.updated_at = "yesterday".into();
    assert!(index.upsert(&bad).is_err());
    let mut empty = record("", "Bad", "body");
    empty.id = String::new();
    assert!(index.upsert(&empty).is_err());
    assert_eq!(index.index_state().unwrap().notes, 1);
}

#[test]
fn an_empty_body_still_indexes_title_aliases_and_tags() {
    let index = ready_index();
    let mut rec = record("n1", "Only a title", "");
    rec.aliases = vec!["alias one".into()];
    rec.tags = vec!["dev".into()];
    assert_eq!(index.upsert(&rec).unwrap(), 1);
    let conn = index.lock();
    let (title, aliases, tags, body, lf, lt): (String, String, String, String, i64, i64) = conn
        .query_row(
            "SELECT title, aliases, tags, body, line_from, line_to FROM chunk",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!((title.as_str(), aliases.as_str(), tags.as_str(), body.as_str()), ("only a title", "alias one", "dev", ""));
    assert_eq!((lf, lt), (0, 0));
}

#[test]
fn body_line_offset_shifts_chunk_lines_to_file_lines() {
    let index = ready_index();
    let mut rec = record("n1", "T", "# T\n\nBody line.\n");
    rec.body_line_offset = 12;
    index.upsert(&rec).unwrap();
    let conn = index.lock();
    let (lf, lt): (i64, i64) = conn
        .query_row("SELECT line_from, line_to FROM chunk", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((lf, lt), (13, 15));
}

#[test]
fn remove_by_id_by_path_and_by_partition() {
    let index = ready_index();
    index.upsert(&record("n1", "One", "one")).unwrap();
    index.upsert(&record("n2", "Two", "two")).unwrap();
    let mut other = record("n3", "Three", "three");
    other.partition_key = "home-q".into();
    index.upsert(&other).unwrap();

    assert!(index.remove("home-p", "n1").unwrap());
    assert!(!index.remove("home-p", "n1").unwrap());
    assert!(index.remove_path("home-p", "notes/n2--note.md").unwrap());
    assert!(!index.remove_path("home-p", "notes/n2--note.md").unwrap());
    assert_eq!(index.index_state().unwrap().notes, 1);
    assert_eq!(index.remove_partition("home-q").unwrap(), 1);
    assert_eq!(index.remove_partition("home-q").unwrap(), 0);
    let report = index.index_state().unwrap();
    assert_eq!((report.notes, report.chunks), (0, 0));
    let conn = index.lock();
    let fts: i64 = conn.query_row("SELECT COUNT(*) FROM fts_tri", [], |r| r.get(0)).unwrap();
    assert_eq!(fts, 0);
}

#[test]
fn rebuild_replaces_everything_and_marks_ready() {
    let index = MemoryIndex::open_in_memory().unwrap();
    assert_eq!(index.index_state().unwrap().state, IndexState::Rebuilding);
    index.upsert(&record("stale", "Stale", "stale")).unwrap();
    let report = index
        .rebuild(&[record("n1", "One", "one"), record("n2", "Two", "two")])
        .unwrap();
    assert_eq!(report, RebuildReport { notes: 2, chunks: 2 });
    let state = index.index_state().unwrap();
    assert_eq!(state.state, IndexState::Ready);
    assert_eq!(state.notes, 2);
    assert!(index.get("home-p", "stale").unwrap().is_none());
}

#[test]
fn replace_partition_touches_only_that_partition() {
    let index = ready_index();
    index.upsert(&record("n1", "One", "one")).unwrap();
    let mut q = record("q1", "Q one", "q");
    q.partition_key = "home-q".into();
    index.upsert(&q).unwrap();
    let mut q2 = record("q2", "Q two", "q two");
    q2.partition_key = "home-q".into();
    index.replace_partition("home-q", &[q2]).unwrap();
    assert!(index.get("home-q", "q1").unwrap().is_none());
    assert!(index.get("home-q", "q2").unwrap().is_some());
    assert!(index.get("home-p", "n1").unwrap().is_some());
    // A record naming another partition is refused before anything changes.
    let stray = record("s", "S", "s");
    let err = index.replace_partition("home-q", &[stray]).unwrap_err();
    assert!(matches!(err, IndexError::InvalidInput(_)));
    assert!(index.get("home-q", "q2").unwrap().is_some());
}

#[test]
fn list_pinned_returns_active_pinned_notes_of_the_named_partitions() {
    let index = ready_index();
    let mut a = record("a", "A", "a");
    a.pinned = true;
    a.updated_at = "2026-09-01T00:00:00Z".into();
    let mut b = record("b", "B", "b");
    b.pinned = true;
    b.updated_at = "2026-09-02T00:00:00Z".into();
    let mut c = record("c", "C", "c");
    c.pinned = true;
    c.effective_status = EffectiveStatus::Superseded;
    let mut d = record("d", "D", "d");
    d.pinned = true;
    d.partition_key = "home-q".into();
    index.upsert_many(&[a, b, c, d]).unwrap();
    let ids: Vec<String> = index
        .list_pinned(&["home-p".to_string()])
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids, vec!["b", "a"]);
    assert!(index.list_pinned(&[]).unwrap().is_empty());
}

#[test]
fn the_index_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index.rebuild(&[record("n1", "One", "one")]).unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Ready);
    assert_eq!(report.notes, 1);
    assert!(index.get("home-p", "n1").unwrap().is_some());
}
