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
            assert!(
                chunk.text.len() <= CHUNK_MAX_BYTES,
                "chunk {} too large",
                chunk.ordinal
            );
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
        assert_eq!(
            chunks[0].text,
            "# Editor preference\n\nUser prefers vim for editing."
        );
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
        assert_eq!(
            chunks.len(),
            2,
            "{:?}",
            chunks.iter().map(|c| c.text.len()).collect::<Vec<_>>()
        );
        assert_eq!(chunks[0].heading.as_deref(), Some("One"));
        assert_eq!(chunks[1].heading.as_deref(), Some("Two"));
        assert_eq!(chunks[1].line_from, 5);
        assert_spans_match(&body, &chunks);
    }

    #[test]
    fn paragraphs_cut_at_the_target_and_never_exceed_the_maximum() {
        let para = "lorem ipsum dolor sit amet ".repeat(20); // 540 bytes
        let body = (0..12)
            .map(|_| para.clone())
            .collect::<Vec<_>>()
            .join("\n\n");
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

    fn parsed(q: &str) -> ParsedQuery {
        parse_query(q).unwrap()
    }

    fn words(q: &str) -> Vec<String> {
        parsed(q).words
    }

    fn literal(text: &str, parts: &[&str]) -> Literal {
        Literal {
            text: text.to_string(),
            parts: parts.iter().map(|p| p.to_string()).collect(),
        }
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
        assert_eq!(words("what editor do I prefer?"), vec!["editor", "prefer"]);
        // Stop words survive when nothing else remains.
        assert_eq!(words("and"), vec!["and"]);
        assert_eq!(words("AND OR NOT"), vec!["and", "not"]);
        // A short Latin term is dropped beside a longer one, kept alone.
        let p = parsed("or");
        assert!(p.words.is_empty());
        assert_eq!(p.shorts, vec!["or"]);
        let p = parsed("tabs OR tomatoes");
        assert_eq!(p.words, vec!["tabs", "tomatoes"]);
        assert!(p.shorts.is_empty());
        let p = parsed("p50/p95 (ms)");
        assert!(p.words.is_empty());
        assert_eq!(p.literals, vec![literal("p50/p95", &["p50", "p95"])]);
        assert!(p.shorts.is_empty());
    }

    #[test]
    fn short_cjk_terms_are_kept_beside_longer_ones() {
        let p = parsed("简洁 quick");
        assert_eq!(p.words, vec!["quick"]);
        assert_eq!(p.shorts, vec!["简洁"]);
        assert_eq!(parsed("答").shorts, vec!["答"]);
        assert_eq!(words("简洁的回答"), vec!["简洁的回答"]);
        // Two-character CJK is a word of the language, not a fragment.
        let p = parsed("please remind me which answer style the user 喜欢");
        assert_eq!(p.shorts, vec!["喜欢"]);
        assert!(p.words.contains(&"answer".to_string()));
        assert!(!p.words.contains(&"the".to_string()));
    }

    #[test]
    fn identifiers_are_literals_with_fallback_parts_and_punctuation_is_trimmed() {
        let p = parsed("OPENALPACA_HOME_STORE");
        assert!(p.words.is_empty());
        assert_eq!(
            p.literals,
            vec![literal(
                "openalpaca_home_store",
                &["openalpaca", "home", "store"]
            )]
        );
        assert_eq!(
            parsed("openalpaca_storage::memory_files").literals,
            vec![literal(
                "openalpaca_storage::memory_files",
                &["openalpaca", "storage", "memory", "files"]
            )]
        );
        assert_eq!(words("\"tabs for indentation"), vec!["tabs", "indentation"]);
        assert_eq!(words("tabs*"), vec!["tabs"]);
        let p = parsed("title:tabs");
        assert!(p.words.is_empty());
        assert_eq!(p.literals, vec![literal("title:tabs", &["title", "tabs"])]);
        assert_eq!(words("(tabs"), vec!["tabs"]);
        assert_eq!(words("-tabs"), vec!["tabs"]);
        assert_eq!(words("^tabs"), vec!["tabs"]);
        assert_eq!(
            words("cargo test --ignored bench_"),
            vec!["cargo", "test", "ignored", "bench"]
        );
        // A stop word inside an identifier is not a fallback part.
        assert_eq!(
            parsed("for/each").literals,
            vec![literal("for/each", &["each"])]
        );
    }

    #[test]
    fn punctuation_only_queries_have_no_terms_but_are_not_refused() {
        assert!(parsed("???").terms().is_empty());
        assert!(parsed("\"").terms().is_empty());
        assert!(parsed("; -- ").terms().is_empty());
    }

    #[test]
    fn match_strings_are_quoted_literals_joined_by_or() {
        let p = parsed("tabs AND spaces");
        assert_eq!(
            p.word_match(&p.words).as_deref(),
            Some("\"tabs\" OR \"spaces\"")
        );
        assert_eq!(
            p.trigram_match(&p.words).as_deref(),
            Some("\"tabs\" OR \"spaces\"")
        );
        let p = parsed("{tabs} [spaces] ; DROP TABLE notes; --");
        assert_eq!(
            p.word_match(&p.words).as_deref(),
            Some("\"tabs\" OR \"spaces\" OR \"drop\" OR \"table\" OR \"notes\"")
        );
        // A quote inside a literal is doubled, never a delimiter.
        let p = parsed("a\"b\"c");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"a\"\"b\"\"c\""));
        // A literal rides beside the words; the search adds its parts to the
        // word list when the literal matches nothing.
        let p = parsed("title:tabs");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"title:tabs\""));
        assert_eq!(
            p.word_match(&p.literals[0].parts).as_deref(),
            Some("\"title\" OR \"tabs\" OR \"title:tabs\"")
        );
        // Nothing routed: no MATCH at all.
        let p = parsed("or");
        assert_eq!(p.word_match(&p.words), None);
        let p = parsed("???");
        assert_eq!(p.trigram_match(&p.words), None);
    }

    #[test]
    fn duplicate_terms_collapse_and_the_term_count_is_capped() {
        assert_eq!(words("tabs tabs TABS"), vec!["tabs"]);
        let long: Vec<String> = (0..50).map(|i| format!("word{i}")).collect();
        assert_eq!(words(&long.join(" ")).len(), MAX_TERMS);
    }
}

// ============================================================================
// Catalog
// ============================================================================

#[test]
fn upsert_indexes_a_note_and_get_reports_its_catalog_row() {
    let index = ready_index();
    let rec = record(
        "n1",
        "Editor preference",
        "# Editor preference\n\nUser prefers vim for editing.\n",
    );
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
    assert_eq!(
        index.get("home-p", "n1").unwrap().unwrap().relative_path,
        "notes/n1--new-title.md"
    );
    // A file rewritten under a new id at the old path replaces that row.
    let mut other = record("n2", "Other", "Other body.\n");
    other.relative_path = "notes/n1--new-title.md".into();
    index.upsert(&other).unwrap();
    assert_eq!(index.index_state().unwrap().notes, 1);
    assert!(index.get("home-p", "n1").unwrap().is_none());
    assert!(index.get("home-p", "n2").unwrap().is_some());
    let conn = index.lock();
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunk", [], |r| r.get(0))
        .unwrap();
    let fts: i64 = conn
        .query_row("SELECT COUNT(*) FROM fts_word", [], |r| r.get(0))
        .unwrap();
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
    assert!(
        matches!(index.upsert(&bad).unwrap_err(), IndexError::InvalidInput(m) if m.contains("valid_to"))
    );
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
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        (
            title.as_str(),
            aliases.as_str(),
            tags.as_str(),
            body.as_str()
        ),
        ("only a title", "alias one", "dev", "")
    );
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
        .query_row("SELECT line_from, line_to FROM chunk", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
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
    let fts: i64 = conn
        .query_row("SELECT COUNT(*) FROM fts_tri", [], |r| r.get(0))
        .unwrap();
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
    assert_eq!(
        report,
        RebuildReport {
            notes: 2,
            chunks: 2
        }
    );
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

// ============================================================================
// Search
// ============================================================================

fn q(query: &str) -> SearchQuery {
    SearchQuery::new(query, vec!["home-p".to_string()])
}

fn ids(results: &SearchResults) -> Vec<&str> {
    results.hits.iter().map(|h| h.id.as_str()).collect()
}

fn hit<'a>(results: &'a SearchResults, id: &str) -> &'a SearchHit {
    results
        .hits
        .iter()
        .find(|h| h.id == id)
        .unwrap_or_else(|| panic!("no hit {id} in {:?}", ids(results)))
}

#[test]
fn search_validates_the_limit_and_the_query() {
    let index = ready_index();
    let mut bad = q("x");
    bad.limit = 0;
    assert!(
        matches!(index.search(&bad).unwrap_err(), IndexError::InvalidInput(m) if m.contains("limit"))
    );
    bad.limit = MAX_LIMIT + 1;
    assert!(index.search(&bad).is_err());
    assert!(matches!(
        index.search(&q("")).unwrap_err(),
        IndexError::InvalidInput(_)
    ));
    assert!(index.search(&q(&"x".repeat(513))).is_err());
    // Punctuation only: nothing routed, nothing found, not refused.
    let r = index.search(&q("???")).unwrap();
    assert_eq!((r.count, r.partial), (0, false));
    // No partitions: nothing searched.
    let r = index.search(&SearchQuery::new("editor", vec![])).unwrap();
    assert_eq!(r.count, 0);
    assert_eq!(r.index_state, IndexState::Ready);
}

#[test]
fn a_trigram_only_hit_finds_prefer_inside_prefers_and_ranks_below_a_whole_token_hit() {
    let index = ready_index();
    index
        .upsert_many(&[
            record(
                "whole",
                "Note A",
                "The user said prefer, exactly that word.\n",
            ),
            record("sub", "Note B", "User prefers vim for editing.\n"),
            record("none", "Coffee", "User drinks espresso after lunch.\n"),
        ])
        .unwrap();
    let r = index.search(&q("prefer")).unwrap();
    assert_eq!(ids(&r), vec!["whole", "sub"]);
    assert_eq!(hit(&r, "whole").matches, vec![MatchReason::Term]);
    assert_eq!(hit(&r, "sub").matches, vec![MatchReason::Trigram]);
    assert!(hit(&r, "whole").score > hit(&r, "sub").score);
    assert_eq!(r.index_state, IndexState::Ready);
    assert!(!r.partial);
}

#[test]
fn exact_title_alias_and_fact_key_hits_carry_their_reasons_and_outrank_term_hits() {
    let index = ready_index();
    let mut t = record("title", "Tabs and spaces", "Use tabs for indentation.\n");
    t.updated_at = "2026-01-01T00:00:00Z".into();
    let mut a = record(
        "alias",
        "Answer style",
        "The user likes short answers about tabs and spaces.\n",
    );
    a.aliases = vec!["Tabs  AND Spaces".into(), "简洁".into()];
    let mut k = record(
        "key",
        "Indentation",
        "Tabs and spaces are both used here.\n",
    );
    k.fact_key = Some("tabs.and.spaces".into());
    let body_only = record("body", "Other", "tabs and spaces, said the body.\n");
    index.upsert_many(&[t, a, k, body_only]).unwrap();

    let r = index.search(&q("tabs and spaces")).unwrap();
    assert_eq!(ids(&r)[0], "title");
    assert_eq!(ids(&r)[1], "alias");
    assert!(hit(&r, "title").matches.contains(&MatchReason::Title));
    assert!(hit(&r, "alias").matches.contains(&MatchReason::Alias));
    assert!(!hit(&r, "body").matches.contains(&MatchReason::Title));
    assert!(hit(&r, "body").score < hit(&r, "alias").score);

    let r = index.search(&q("简洁")).unwrap();
    assert_eq!(ids(&r), vec!["alias"]);
    assert_eq!(hit(&r, "alias").matches, vec![MatchReason::Alias]);

    let r = index.search(&q("tabs.and.spaces")).unwrap();
    assert_eq!(ids(&r)[0], "key");
    assert!(hit(&r, "key").matches.contains(&MatchReason::FactKey));
}

#[test]
fn identifiers_match_as_phrases_and_the_excerpt_keeps_the_original_text() {
    let index = ready_index();
    index
        .upsert_many(&[
            record(
                "env",
                "Scratch store location",
                "Set `OPENALPACA_HOME_STORE` to an absolute scratch path; the library lives in `openalpaca_storage::memory_files`.\n",
            ),
            record("decoy", "Fruit", "Apples are in season in October.\n"),
        ])
        .unwrap();
    let r = index.search(&q("OPENALPACA_HOME_STORE")).unwrap();
    assert_eq!(ids(&r), vec!["env"]);
    assert!(hit(&r, "env").matches.contains(&MatchReason::Phrase));
    assert!(hit(&r, "env").excerpt.contains("OPENALPACA_HOME_STORE"));
    let r = index
        .search(&q("openalpaca_storage::memory_files"))
        .unwrap();
    assert_eq!(ids(&r), vec!["env"]);
    assert!(hit(&r, "env").matches.contains(&MatchReason::Phrase));
    let r = index.search(&q("memory_files")).unwrap();
    assert_eq!(ids(&r), vec!["env"]);
}

#[test]
fn f4_operators_are_literal_text_and_never_a_syntax_error() {
    let index = ready_index();
    index
        .upsert_many(&[
            record("tabs", "Tabs and spaces", "Use tabs for indentation and spaces for alignment; never one or the other alone.\n"),
            record("garden", "Garden", "The tomatoes ripen in August.\n"),
        ])
        .unwrap();
    for query in [
        "and",
        "tabs AND spaces",
        "alignment AND nothingelse",
        "NOT tabs",
        "tabs NEAR spaces",
        "\"tabs for indentation",
        "tabs*",
        "title:tabs",
        "(tabs",
        "tabs) OR (",
        "-tabs",
        "^tabs",
        "or",
        "AND OR NOT",
        "{tabs} [spaces] ; DROP TABLE notes; --",
    ] {
        let r = index
            .search(&q(query))
            .unwrap_or_else(|e| panic!("{query:?}: {e}"));
        assert_eq!(ids(&r), vec!["tabs"], "{query:?}");
    }
    let r = index.search(&q("tabs OR tomatoes")).unwrap();
    assert_eq!(r.count, 2);
    let r = index.search(&q("nothingelse whatsoever")).unwrap();
    assert_eq!(r.count, 0);
    assert_eq!(index.index_state().unwrap().notes, 2);
}

#[test]
fn short_queries_use_the_substring_pass_and_carry_no_reason() {
    let index = ready_index();
    let mut cjk = record("cjk", "回答风格", "# 回答风格\n\n用户喜欢简洁的回答\n");
    cjk.aliases = vec!["简洁".into()];
    index
        .upsert_many(&[
            cjk,
            record("fruit", "水果", "# 水果\n\n用户每天吃一个苹果\n"),
            record(
                "quick",
                "Quick replies",
                "The user likes quick replies in chat.\n",
            ),
        ])
        .unwrap();
    let r = index.search(&q("答")).unwrap();
    assert_eq!(ids(&r), vec!["cjk"]);
    assert!(hit(&r, "cjk").matches.is_empty());
    assert!(!r.partial);
    let r = index.search(&q("用户")).unwrap();
    assert_eq!(r.count, 2);
    let r = index.search(&q("简洁的回答")).unwrap();
    assert_eq!(hit(&r, "cjk").matches, vec![MatchReason::Trigram]);
    let r = index.search(&q("回答风格")).unwrap();
    assert!(hit(&r, "cjk").matches.contains(&MatchReason::Title));
    // Fullwidth folds; scripts mix by the union rule.
    let r = index.search(&q("ｑｕｉｃｋ　replies")).unwrap();
    assert_eq!(ids(&r), vec!["quick"]);
    let r = index.search(&q("简洁 quick")).unwrap();
    assert_eq!(r.count, 2);
    // A short Latin query alone is a substring pass too.
    let r = index.search(&q("ck")).unwrap();
    assert_eq!(ids(&r), vec!["quick"]);
    let r = index.search(&q("香蕉")).unwrap();
    assert_eq!(r.count, 0);
}

#[test]
fn the_substring_pass_reports_partial_when_its_cap_is_hit() {
    let index = ready_index();
    let records: Vec<IndexRecord> = (0..SUBSTRING_SCAN_CAP + 10)
        .map(|i| {
            record(
                &format!("n{i}"),
                &format!("Note {i}"),
                &format!("Body {i} 苹果\n"),
            )
        })
        .collect();
    index.upsert_many(&records).unwrap();
    let mut query = q("答");
    query.limit = 20;
    let r = index.search(&query).unwrap();
    assert!(r.partial);
    assert_eq!(r.count, 0);
    let mut query = q("苹果");
    query.limit = 20;
    let r = index.search(&query).unwrap();
    assert!(r.partial);
    assert_eq!(r.count, 20);
    // A two-partition scope only scans what it is allowed.
    let r = index
        .search(&SearchQuery::new("苹果", vec!["home-other".into()]))
        .unwrap();
    assert_eq!((r.count, r.partial), (0, false));
}

#[test]
fn filters_are_applied_before_the_limit() {
    let index = ready_index();
    let mut records: Vec<IndexRecord> = (0..30)
        .map(|i| {
            let mut r = record(
                &format!("old{i}"),
                "Zebra",
                "The zebra crossing was moved.\n",
            );
            r.effective_status = EffectiveStatus::Superseded;
            r.superseded_by = vec!["new".into()];
            r.updated_at = format!("2026-09-{:02}T00:00:00Z", i % 28 + 1);
            r
        })
        .collect();
    records.push(record(
        "new",
        "Zebra",
        "The zebra crossing is on Elm Street.\n",
    ));
    index.upsert_many(&records).unwrap();
    let r = index.search(&q("zebra")).unwrap();
    assert_eq!(ids(&r), vec!["new"]);
    let mut historical = q("zebra");
    historical.historical = true;
    historical.limit = 20;
    let r = index.search(&historical).unwrap();
    assert_eq!(r.count, 20);
    assert!(
        r.hits
            .iter()
            .all(|h| h.superseded_by == vec!["new".to_string()] || h.id == "new")
    );
    assert!(
        r.hits
            .iter()
            .any(|h| h.effective_status == EffectiveStatus::Superseded)
    );
}

#[test]
fn candidates_are_never_returned_and_archived_only_historically() {
    let index = ready_index();
    let mut cand = record("cand", "Lunch", "Lunch moved to 12:30, said a tool.\n");
    cand.status = "candidate".into();
    cand.effective_status = EffectiveStatus::Candidate;
    let mut arch = record("arch", "Lunch", "Lunch is at noon, archived.\n");
    arch.status = "archived".into();
    arch.effective_status = EffectiveStatus::Archived;
    index
        .upsert_many(&[cand, arch, record("live", "Lunch", "Lunch is at noon.\n")])
        .unwrap();
    let r = index.search(&q("lunch")).unwrap();
    assert_eq!(ids(&r), vec!["live"]);
    let mut historical = q("lunch");
    historical.historical = true;
    let r = index.search(&historical).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["arch", "live"]);
    assert_eq!(hit(&r, "arch").status, "archived");
}

#[test]
fn the_automatic_path_excludes_same_key_losers_and_source_forgotten_notes() {
    let index = ready_index();
    let mut loser = record("loser", "Meeting day", "The team meets on Wednesdays.\n");
    loser.origin = "extracted_user".into();
    loser.fact_key = Some("team.meeting_day".into());
    loser.conflict = true;
    loser.same_key_loser = true;
    let mut winner = record("winner", "Meeting day", "The team meets on Thursdays.\n");
    winner.fact_key = Some("team.meeting_day".into());
    let mut forgotten = record(
        "cites",
        "Birthday",
        "The team lead's birthday is in October.\n",
    );
    forgotten.source_forgotten = true;
    let mut linked = record(
        "linked",
        "Dropped-in",
        "The team meets in room 4, says a dropped-in file.\n",
    );
    linked.origin = "external".into();
    linked.conflict = true; // carries an un-honored link; not a loser
    index
        .upsert_many(&[loser, winner, forgotten, linked])
        .unwrap();

    let mut explicit = q("team");
    explicit.limit = 20;
    let r = index.search(&explicit).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["cites", "linked", "loser", "winner"]);
    assert!(hit(&r, "loser").conflict);
    assert!(hit(&r, "loser").same_key_loser);
    assert!(!hit(&r, "winner").conflict);
    assert!(hit(&r, "cites").source_forgotten);

    let mut auto = explicit.clone();
    auto.mode = SearchMode::Automatic;
    let r = index.search(&auto).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["linked", "winner"]);
}

#[test]
fn validity_is_a_half_open_interval_evaluated_at_as_of_or_now() {
    let index = ready_index();
    let mut x = record("x", "Bundler", "The project bundles with Webpack.\n");
    x.aliases = vec!["webpack".into()];
    x.fact_key = Some("build.bundler".into());
    x.valid_to = Some("2026-06-01T00:00:00Z".into());
    x.effective_status = EffectiveStatus::Superseded;
    x.superseded_by = vec!["y".into()];
    x.updated_at = "2026-01-10T09:00:00Z".into();
    let mut y = record(
        "y",
        "Bundler",
        "The project bundles with Vite since June 2026; Webpack was used before.\n",
    );
    y.aliases = vec!["vite".into()];
    y.fact_key = Some("build.bundler".into());
    y.valid_from = Some("2026-06-01T00:00:00Z".into());
    y.updated_at = "2026-06-02T09:00:00Z".into();
    index.upsert_many(&[x, y]).unwrap();

    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let r = index.search(&q("which bundler")).unwrap();
    assert_eq!(ids(&r), vec!["y"]);
    assert_eq!(
        hit(&r, "y").valid_from.as_deref(),
        Some("2026-06-01T00:00:00Z")
    );
    let mut march = q("which bundler");
    march.as_of = Some(at("2026-03-01T00:00:00Z"));
    assert_eq!(index.search(&march).unwrap().count, 0);
    march.historical = true;
    assert_eq!(ids(&index.search(&march).unwrap()), vec!["x"]);
    let mut all = q("which bundler");
    all.historical = true;
    assert_eq!(ids(&index.search(&all).unwrap()), vec!["y", "x"]);
    all.as_of = Some(at("2026-06-01T00:00:00Z"));
    assert_eq!(ids(&index.search(&all).unwrap()), vec!["y"]);
    all.as_of = Some(at("2026-05-31T23:59:59Z"));
    assert_eq!(ids(&index.search(&all).unwrap()), vec!["x"]);
    let r = index.search(&q("webpack")).unwrap();
    assert_eq!(ids(&r), vec!["y"]);
    assert!(hit(&r, "y").matches.contains(&MatchReason::Term));
}

#[test]
fn authority_orders_notes_with_equal_lexical_evidence() {
    let index = ready_index();
    let mut low = record(
        "low",
        "Editor preference from a dropped file",
        "The user prefers nano for editing.\n",
    );
    low.origin = "external".into();
    low.conflict = true;
    low.updated_at = "2026-09-23T10:06:00Z".into();
    let mut high = record(
        "high",
        "Editor preference",
        "## Knowledge\nThe user prefers vim for editing, not emacs.\n",
    );
    high.updated_at = "2026-09-23T10:05:00Z".into();
    index.upsert_many(&[low, high]).unwrap();
    let r = index.search(&q("which editor for editing")).unwrap();
    assert_eq!(ids(&r), vec!["high", "low"]);
    assert!(hit(&r, "low").conflict);
}

#[test]
fn kind_and_partition_scope_the_search() {
    let index = ready_index();
    let mut pref = record(
        "pref",
        "Indentation preference",
        "Prefers tabs over spaces for indentation.\n",
    );
    pref.kind = "preference".into();
    let mut dec = record(
        "dec",
        "Indentation in project A",
        "Prefers tabs over spaces for indentation.\n",
    );
    dec.kind = "decision".into();
    dec.partition_key = "proj-p-a".into();
    dec.scope = "project".into();
    let mut bob = record(
        "bob",
        "Indentation preference",
        "Prefers tabs over spaces for indentation.\n",
    );
    bob.partition_key = "home-bob".into();
    index.upsert_many(&[pref, dec, bob]).unwrap();
    let both = SearchQuery::new(
        "tabs or spaces for indentation",
        vec!["home-p".into(), "proj-p-a".into()],
    );
    let r = index.search(&both).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["dec", "pref"]);
    assert_eq!(hit(&r, "pref").scope, "global");
    assert_eq!(hit(&r, "dec").scope, "project");
    let mut only_decisions = both.clone();
    only_decisions.kind = Some("decision".into());
    assert_eq!(ids(&index.search(&only_decisions).unwrap()), vec!["dec"]);
    let r = index.search(&q("tabs or spaces for indentation")).unwrap();
    assert_eq!(ids(&r), vec!["pref"]);
}

#[test]
fn the_hit_names_the_chunk_its_excerpt_came_from() {
    let index = ready_index();
    let filler = "filler words about nothing in particular ".repeat(40);
    let body = format!(
        "# Guide\n\n{filler}\n\n## Deployment\n\nDeploy with the blue-green script on Fridays.\n\n{filler}\n"
    );
    let mut rec = record("guide", "Guide", &body);
    rec.body_line_offset = 20;
    index.upsert(&rec).unwrap();
    let r = index.search(&q("blue-green deploy script")).unwrap();
    let h = hit(&r, "guide");
    assert_eq!(h.heading.as_deref(), Some("Deployment"));
    assert!(h.excerpt.contains("blue-green script"));
    assert!(h.chunk_ordinal >= 1);
    assert_eq!(h.line_range, (25, 27));
    assert!(h.matches.contains(&MatchReason::Phrase));
    assert!(h.matches.contains(&MatchReason::Term));
}

#[test]
fn a_stop_word_query_is_searched_when_nothing_else_remains() {
    let index = ready_index();
    index
        .upsert_many(&[
            record("with", "Tabs", "Use tabs and spaces together.\n"),
            record("without", "Garden", "Tomatoes ripen.\n"),
        ])
        .unwrap();
    assert_eq!(ids(&index.search(&q("and")).unwrap()), vec!["with"]);
    assert_eq!(ids(&index.search(&q("the tabs")).unwrap()), vec!["with"]);
}

// ============================================================================
// Reconciliation
// ============================================================================

fn entry(path: &str, mtime_ns: i64, size: u64, inode: u64) -> ScanEntry {
    ScanEntry {
        relative_path: path.to_string(),
        stat: FileStat {
            mtime_ns,
            size,
            inode,
        },
    }
}

#[test]
fn reconcile_reports_an_external_edit_a_deletion_and_a_new_file() {
    let index = ready_index();
    let mut coffee = record("coffee", "Coffee order", "The user drinks a flat white.\n");
    coffee.relative_path = "notes/coffee--coffee-order.md".into();
    coffee.file = FileStat {
        mtime_ns: 100,
        size: 30,
        inode: 11,
    };
    let mut parking = record("parking", "Parking spot", "The user parks in bay 12.\n");
    parking.relative_path = "notes/parking--parking-spot.md".into();
    parking.file = FileStat {
        mtime_ns: 100,
        size: 26,
        inode: 12,
    };
    let mut other = record("other", "Elsewhere", "Another partition.\n");
    other.partition_key = "home-q".into();
    index.upsert_many(&[coffee, parking, other]).unwrap();

    // Nothing changed: a clean report, and a rewrite with identical bytes
    // that keeps the stat triple is invisible by design.
    let report = index
        .reconcile("home-p", || {
            Ok(vec![
                entry("notes/coffee--coffee-order.md", 100, 30, 11),
                entry("notes/parking--parking-spot.md", 100, 26, 12),
            ])
        })
        .unwrap();
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.scanned, 2);
    assert_eq!(index.last_scan("home-p").unwrap().map(|(_, n)| n), Some(2));

    // An edit behind the service's back (mtime and size moved), a deletion,
    // and a hand-written new file.
    let report = index
        .reconcile("home-p", || {
            Ok(vec![
                entry("notes/coffee--coffee-order.md", 200, 75, 11),
                entry("notes/gym--gym-days.md", 300, 40, 13),
            ])
        })
        .unwrap();
    assert_eq!(
        report,
        ReconcileReport {
            partition_key: "home-p".into(),
            scanned: 2,
            added: vec!["notes/gym--gym-days.md".into()],
            changed: vec![("coffee".into(), "notes/coffee--coffee-order.md".into())],
            removed: vec![("parking".into(), "notes/parking--parking-spot.md".into())],
        }
    );
    // The report changed nothing: the caller re-indexes and removes.
    assert!(index.get("home-p", "parking").unwrap().is_some());
    assert!(
        index
            .remove_path("home-p", "notes/parking--parking-spot.md")
            .unwrap()
    );
    // An inode change alone (a replaced file with the same bytes and mtime)
    // is a change; the other partition is untouched.
    let report = index
        .reconcile("home-p", || {
            Ok(vec![entry("notes/coffee--coffee-order.md", 100, 30, 99)])
        })
        .unwrap();
    assert_eq!(report.changed.len(), 1);
    assert!(report.removed.is_empty());
    assert!(index.get("home-q", "other").unwrap().is_some());
    assert_eq!(index.last_scan("home-q").unwrap(), None);
}

#[test]
fn reconcile_propagates_the_scanner_error_and_records_no_scan() {
    let index = ready_index();
    let err = index
        .reconcile("home-p", || Err(std::io::Error::other("boom")))
        .unwrap_err();
    assert!(matches!(err, IndexError::Io(_)));
    assert_eq!(index.last_scan("home-p").unwrap(), None);
}

// ============================================================================
// Rebuild equivalence
// ============================================================================

/// The comparable part of a hit: everything but the float score, which is
/// compared separately to a tolerance.
fn shape(
    h: &SearchHit,
) -> (
    String,
    Vec<MatchReason>,
    String,
    (u32, u32),
    Vec<String>,
    bool,
) {
    (
        h.id.clone(),
        h.matches.clone(),
        h.excerpt.clone(),
        h.line_range,
        h.superseded_by.clone(),
        h.conflict,
    )
}

#[test]
fn an_index_built_incrementally_equals_one_rebuilt_from_the_final_records() {
    let incremental = ready_index();
    let mut a = record("a", "Old office", "The office is on Elm Street.\n");
    a.origin = "extracted_user".into();
    incremental.upsert(&a).unwrap();
    let b = record("b", "Lunch break", "Lunch break is at noon.\n");
    incremental.upsert(&b).unwrap();
    let c = record("c", "Meeting day", "The team meets on Wednesdays.\n");
    incremental.upsert(&c).unwrap();
    // A retitle, a supersession recorded later, and a removal.
    let mut c2 = c.clone();
    c2.title = "Weekly meeting".into();
    c2.relative_path = "notes/c--weekly-meeting.md".into();
    c2.body = "# Weekly meeting\n\nThe team meets on Wednesdays at ten.\n".into();
    incremental.upsert(&c2).unwrap();
    let mut a2 = a.clone();
    a2.effective_status = EffectiveStatus::Superseded;
    a2.superseded_by = vec!["gone".into()];
    incremental.upsert(&a2).unwrap();
    incremental.remove("home-p", "b").unwrap();
    let mut d = record("d", "回答风格", "用户喜欢简洁的回答\n");
    d.aliases = vec!["简洁".into()];
    incremental.upsert(&d).unwrap();

    let rebuilt = MemoryIndex::open_in_memory().unwrap();
    rebuilt.rebuild(&[a2, c2, d]).unwrap();

    for query in [
        "office",
        "team meets",
        "meeting",
        "简洁",
        "答",
        "noon",
        "elm street",
    ] {
        for historical in [false, true] {
            let mut sq = q(query);
            sq.historical = historical;
            let x = incremental.search(&sq).unwrap();
            let y = rebuilt.search(&sq).unwrap();
            let sx: Vec<_> = x.hits.iter().map(shape).collect();
            let sy: Vec<_> = y.hits.iter().map(shape).collect();
            assert_eq!(sx, sy, "{query:?} historical={historical}");
            for (hx, hy) in x.hits.iter().zip(&y.hits) {
                assert!(
                    (hx.score - hy.score).abs() < 1e-9,
                    "{query:?}: {} vs {}",
                    hx.score,
                    hy.score
                );
            }
            assert_eq!((x.count, x.partial), (y.count, y.partial));
        }
    }
    let sx = incremental.index_state().unwrap();
    let sy = rebuilt.index_state().unwrap();
    assert_eq!(
        (sx.notes, sx.chunks, sx.partitions),
        (sy.notes, sy.chunks, sy.partitions)
    );
    // No orphaned FTS rows on either side.
    for index in [&incremental, &rebuilt] {
        let conn = index.lock();
        let (w, t, c): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM fts_word), (SELECT COUNT(*) FROM fts_tri), (SELECT COUNT(*) FROM chunk)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((w, t), (c, c));
    }
}
