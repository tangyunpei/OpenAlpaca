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

/// Design 09 §2.3 (ruling LR18): schema 2 adds `note.supersedes` and the
/// `note_source` table. A cache written at schema 1 — no `note_source`, the
/// old version in `meta` — is invalid on open and recreated, and the fresh
/// index reports `rebuilding` until a rebuild has run: the index's
/// documented path for any schema change (design 03 §5).
#[test]
fn an_index_at_schema_1_is_rebuilt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index.rebuild(&[record("n1", "One", "one")]).unwrap();
        let conn = index.lock();
        conn.execute_batch(
            "DROP TABLE IF EXISTS note_source;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';",
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(
        report.state,
        IndexState::Rebuilding,
        "a v1 cache is recreated"
    );
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0, "nothing of the v1 cache is served");
    let conn = index.lock();
    let has_source_table: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'note_source')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_source_table);
    let has_supersedes: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('note') WHERE name = 'supersedes')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(has_supersedes);
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
        created_at: "2026-09-23T10:00:00Z".to_string(),
        observed_at: None,
        pinned: false,
        source_forgotten: false,
        conflict: false,
        same_key_loser: false,
        superseded_by: Vec::new(),
        supersedes: Vec::new(),
        sources: Vec::new(),
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
    use std::borrow::Cow;

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

    /// Design 09 §6.5.2: on the word table every term is a quoted phrase —
    /// a prefix (`"w"*`) when its last token has three or more characters,
    /// exact for a stop word or a term carrying an unsegmented character —
    /// split at script boundaries; the header trigram table takes every term
    /// of three or more characters as it is. All ORed, never syntax.
    #[test]
    fn match_strings_are_quoted_literals_joined_by_or() {
        let p = parsed("tabs AND spaces");
        assert_eq!(
            p.word_match(&p.words).as_deref(),
            Some("\"tabs\"* OR \"spaces\"*")
        );
        assert_eq!(
            p.head_trigram_match(&p.words).as_deref(),
            Some("\"tabs\" OR \"spaces\"")
        );
        let p = parsed("{tabs} [spaces] ; DROP TABLE notes; --");
        assert_eq!(
            p.word_match(&p.words).as_deref(),
            Some("\"tabs\"* OR \"spaces\"* OR \"drop\"* OR \"table\"* OR \"notes\"*")
        );
        // A quote inside a literal is doubled, never a delimiter; a
        // one-letter last token is no prefix (a number would be one).
        let p = parsed("a\"b\"c");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"a\"\"b\"\"c\""));
        // A literal rides beside the words; the search adds its parts to the
        // word list when the literal is found in no eligible note (design 09
        // §6.5.2).
        let p = parsed("title:tabs");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"title:tabs\"*"));
        assert_eq!(
            p.word_match(&p.literals[0].parts).as_deref(),
            Some("\"title\"* OR \"tabs\"* OR \"title:tabs\"*")
        );
        assert_eq!(
            parsed("partner's").word_match(&[]).as_deref(),
            Some("\"partner's\"")
        );
        // A number ends a prefix whatever its length (repair round 2).
        assert_eq!(
            parsed("10:30").word_match(&[]).as_deref(),
            Some("\"10:30\"*")
        );
        // The word table's probes that decide whether a literal is found:
        // its exact phrase, and the prefix phrase only when its last token
        // is one (the search then asks for the literal verbatim too).
        assert_eq!(
            parsed("memory_file").literals[0].word_probes(),
            (
                "\"memory_file\"".to_string(),
                Some("\"memory_file\"*".to_string())
            )
        );
        assert_eq!(
            parsed("partner's").literals[0].word_probes(),
            ("\"partner's\"".to_string(), None)
        );
        // A stop word routed alone, and a CJK term, match exactly; a word
        // glued to CJK is split at the boundary.
        let p = parsed("and");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"and\""));
        let p = parsed("简洁的回答");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"简洁的回答\""));
        let p = parsed("看一遍commit");
        assert_eq!(p.word_match(&p.words).as_deref(), Some("\"看一遍 commit\""));
        assert_eq!(p.head_trigram_match(&p.words).as_deref(), Some("\"看一遍commit\""));
        // Nothing routed: no MATCH at all.
        let p = parsed("or");
        assert_eq!(p.word_match(&p.words), None);
        let p = parsed("???");
        assert_eq!(p.head_trigram_match(&p.words), None);
    }

    /// Design 09 §6.5.1: a space goes where an unsegmented character meets
    /// a letter or digit of another script; a run of unsegmented
    /// characters stays one token, and punctuation is no boundary.
    #[test]
    fn scripts_split_where_an_unsegmented_character_meets_another_script() {
        assert_eq!(split_scripts("看一遍commit 记录"), "看一遍 commit 记录");
        assert_eq!(split_scripts("2026年7月"), "2026 年 7 月");
        assert_eq!(split_scripts("在gui里"), "在 gui 里");
        assert_eq!(split_scripts("用户喜欢简洁的回答"), "用户喜欢简洁的回答");
        assert_eq!(split_scripts("ภาษาไทยabc"), "ภาษาไทย abc");
        assert!(matches!(split_scripts("plain english"), Cow::Borrowed(_)));
        assert_eq!(split_scripts("简\"洁"), "简\"洁");
        assert_eq!(split_scripts(""), "");
        let text = "发版前先看一遍commit记录, 2026年7月";
        assert_eq!(
            tokens_split(text).collect::<Vec<_>>(),
            vec!["发版前先看一遍", "commit", "记录", "2026", "年", "7", "月"]
        );
        // The tokens of the split text are the split tokens.
        assert_eq!(
            tokens(&split_scripts(text)).collect::<Vec<_>>(),
            tokens_split(text).collect::<Vec<_>>()
        );
        // Written without spaces: CJK, Thai, Lao, Myanmar, Khmer, Tibetan,
        // Yi, Javanese, Balinese, Tai Tham, New Tai Lue, Tai Le. Words built
        // with vowel signs unicode61 breaks at: Devanagari, Bengali, Tamil,
        // Thaana (repair rounds 1 and 2 of LQ17).
        for c in [
            '中', 'あ', '한', 'ก', 'ກ', 'က', 'ក', 'ཀ', 'ꆈ', 'ꦏ', 'ᬓ', 'ᨠ', 'ᦀ', 'ᥐ', 'क', 'অ', 'த', 'ދ',
        ] {
            assert!(is_unsegmented(c), "{c}");
        }
        for c in ['a', 'é', 'я', 'α', 'ע', 'ب', '1', 'ß'] {
            assert!(!is_unsegmented(c), "{c}");
        }
    }

    /// Design 09 §6.5.2: a term is a prefix when its last token has three
    /// or more characters and is not a stop word — or, since repair round 2
    /// of LQ17, is a number of any length after another token — and it
    /// carries no unsegmented character.
    #[test]
    fn a_term_is_a_prefix_only_by_its_last_token() {
        for term in [
            "prefer",
            // A last token of exactly three characters: the threshold
            // (§6.5.2, *Why from 3 characters*).
            "tea",
            "x_abc",
            "memory_file",
            "title:tabs",
            "rustfmt::skip",
            "p50/p950",
            "10:30",
            "v1.2",
            "3.12",
            "2026-09-15",
        ] {
            assert!(is_prefix_term(term), "{term}");
        }
        for term in [
            "partner's",
            "x_ab",
            "x_a1",
            "and",
            "for/the",
            "用户喜欢",
            "看一遍commit",
            "a\"b\"c",
            "2026年7月",
        ] {
            assert!(!is_prefix_term(term), "{term}");
        }
    }

    /// The whole-word queries at the cap name only the terms that went out
    /// as prefixes — all of them over the header columns, and each alone
    /// over the body; the body trigram table only terms carrying an
    /// unsegmented character.
    #[test]
    fn the_cap_query_and_the_body_trigram_query_take_their_own_terms() {
        let p = parsed("work at 公司 partner's memory_file 用户喜欢");
        assert_eq!(p.words, vec!["work", "用户喜欢"]);
        assert_eq!(
            p.word_match(&p.words).as_deref(),
            Some("\"work\"* OR \"用户喜欢\" OR \"partner's\" OR \"memory_file\"*")
        );
        assert_eq!(
            p.word_match_exact_at_cap(&p.words),
            Some(WholeWordQueries {
                header: "{title aliases tags} : (\"work\" OR \"memory_file\")".to_string(),
                bodies: vec![
                    "{body} : \"work\"".to_string(),
                    "{body} : \"memory_file\"".to_string(),
                ],
            })
        );
        assert_eq!(
            p.head_trigram_match(&p.words).as_deref(),
            Some("\"work\" OR \"用户喜欢\" OR \"partner's\" OR \"memory_file\"")
        );
        assert_eq!(
            p.body_trigram_match(&p.words).as_deref(),
            Some("\"用户喜欢\"")
        );
        let p = parsed("用户喜欢");
        assert_eq!(p.word_match_exact_at_cap(&p.words), None);
        let p = parsed("presso");
        assert_eq!(p.body_trigram_match(&p.words), None);
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

/// The header — title, aliases, tags — is held once per note, normalized,
/// and the chunk holds only its own text (design 09 §6.5.1); a note with no
/// body still has one empty chunk, so its header is searched.
#[test]
fn an_empty_body_still_indexes_title_aliases_and_tags() {
    let index = ready_index();
    let mut rec = record("n1", "Only a Title", "");
    rec.aliases = vec!["Alias  One".into(), "second alias".into()];
    rec.tags = vec!["dev".into()];
    assert_eq!(index.upsert(&rec).unwrap(), 1);
    {
        let conn = index.lock();
        let (title, aliases, tags): (String, String, String) = conn
            .query_row("SELECT norm_title, norm_aliases, norm_tags FROM note", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(
            (title.as_str(), aliases.as_str(), tags.as_str()),
            ("only a title", "alias one\nsecond alias", "dev")
        );
        let (body, lf, lt): (String, i64, i64) = conn
            .query_row("SELECT body, line_from, line_to FROM chunk", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!((body.as_str(), lf, lt), ("", 0, 0));
    }
    for (query, reason) in [
        ("title", MatchReason::Term),
        ("second", MatchReason::Term),
        ("dev", MatchReason::Term),
        ("itle", MatchReason::Trigram),
    ] {
        let r = index.search(&q(query)).unwrap();
        assert_eq!(ids(&r), vec!["n1"], "{query}");
        assert!(hit(&r, "n1").matches.contains(&reason), "{query}: {:?}", hit(&r, "n1").matches);
    }
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

/// Design §4 *Times* holds for pinned notes too: current recall leaves out
/// a note whose validity window has ended (the bound is exclusive), at the
/// index clock's "now", and keeps one whose window has not begun.
#[test]
fn list_pinned_leaves_out_a_note_whose_window_has_ended() {
    let now = chrono::DateTime::parse_from_rfc3339("2026-09-24T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let index = MemoryIndex::open_in_memory()
        .unwrap()
        .with_clock(move || now);
    index.rebuild(&[]).unwrap();
    let pin = |id: &str, from: Option<&str>, to: Option<&str>| {
        let mut r = record(id, id, id);
        r.pinned = true;
        r.valid_from = from.map(str::to_string);
        r.valid_to = to.map(str::to_string);
        r
    };
    index
        .upsert_many(&[
            pin("open", None, None),
            pin(
                "planned",
                Some("2026-10-05T00:00:00Z"),
                Some("2026-10-20T00:00:00Z"),
            ),
            pin(
                "ended",
                Some("2026-09-01T00:00:00Z"),
                Some("2026-09-06T00:00:00Z"),
            ),
            pin("ends_now", None, Some("2026-09-24T12:00:00Z")),
        ])
        .unwrap();
    let ids: Vec<String> = index
        .list_pinned(&["home-p".to_string()])
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids, vec!["open", "planned"]);
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
// Learning's catalog reads (design 09 §2.3, ruling LR18)
// ============================================================================

/// `note_source` finds the notes that cite a message, whatever their
/// status and in either named partition, and forgets a note's rows when the
/// note is re-indexed without the source or removed.
#[test]
fn notes_citing_finds_notes_through_note_source() {
    let index = ready_index();
    let mut a = record("a", "Editor", "vim");
    a.sources = vec!["message:7".into(), "message:8".into()];
    a.supersedes = vec!["old".into()];
    let mut b = record("b", "Laptop", "a thinkpad");
    b.status = "candidate".into();
    b.effective_status = EffectiveStatus::Candidate;
    b.sources = vec!["message:8".into(), "message:8".into()];
    let mut c = record("c", "Commute", "by train");
    c.partition_key = "proj-q".into();
    c.sources = vec!["message:7".into()];
    let mut d = record("d", "Unrelated", "tea");
    d.sources = vec!["task:t1".into()];
    index.upsert_many(&[a, b, c, d]).unwrap();

    let keys = |notes: &[CatalogNote]| notes.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
    let found = index
        .notes_citing(&["home-p".into()], &["message:8".into()])
        .unwrap();
    assert_eq!(
        keys(&found),
        vec!["a", "b"],
        "one row per note, deduplicated"
    );
    assert_eq!(found[0].supersedes, vec!["old".to_string()]);
    assert_eq!(found[1].effective_status, EffectiveStatus::Candidate);
    let both = index
        .notes_citing(&["home-p".into(), "proj-q".into()], &["message:7".into()])
        .unwrap();
    assert_eq!(keys(&both), vec!["a", "c"]);
    assert!(
        index
            .notes_citing(&["home-p".into()], &[])
            .unwrap()
            .is_empty()
    );

    // Re-indexed without the source, then removed: its rows go with it.
    let mut a2 = record("a", "Editor", "vim");
    a2.sources = vec!["message:9".into()];
    index.upsert(&a2).unwrap();
    assert_eq!(
        keys(
            &index
                .notes_citing(&["home-p".into()], &["message:7".into()])
                .unwrap()
        ),
        Vec::<String>::new()
    );
    index.remove("home-p", "b").unwrap();
    assert!(
        index
            .notes_citing(&["home-p".into()], &["message:8".into()])
            .unwrap()
            .is_empty()
    );
    let conn = index.lock();
    let orphans: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM note_source WHERE note_rowid NOT IN (SELECT rowid FROM note)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);
}

/// `known_keys` (design 09 §2.3 step 4): the distinct keys of effectively
/// active notes, the most recently updated first, capped.
#[test]
fn fact_keys_lists_active_keys_newest_first() {
    let index = ready_index();
    let keyed = |id: &str, key: &str, updated: &str| {
        let mut r = record(id, id, "body");
        r.fact_key = Some(key.into());
        r.updated_at = updated.into();
        r
    };
    let mut superseded = keyed("s", "user.old", "2026-09-24T12:00:00Z");
    superseded.effective_status = EffectiveStatus::Superseded;
    let mut candidate = keyed("c", "user.maybe", "2026-09-24T12:00:00Z");
    candidate.effective_status = EffectiveStatus::Candidate;
    index
        .upsert_many(&[
            keyed("a", "user.editor", "2026-09-20T10:00:00Z"),
            keyed("b", "user.editor", "2026-09-22T10:00:00Z"),
            keyed("l", "user.laptop", "2026-09-21T10:00:00Z"),
            superseded,
            candidate,
            record("n", "No key", "body"),
        ])
        .unwrap();
    assert_eq!(
        index.fact_keys(&["home-p".into()], 150).unwrap(),
        vec!["user.editor".to_string(), "user.laptop".to_string()]
    );
    assert_eq!(
        index.fact_keys(&["home-p".into()], 1).unwrap(),
        vec!["user.editor".to_string()]
    );
    assert!(index.fact_keys(&["proj-q".into()], 150).unwrap().is_empty());
}

/// The key's R9 winner in one partition: never a same-key loser, never a
/// note that is not effectively active.
#[test]
fn fact_key_winner_skips_losers_and_inactive_notes() {
    let index = ready_index();
    let keyed = |id: &str| {
        let mut r = record(id, id, "body");
        r.fact_key = Some("user.editor".into());
        r
    };
    let mut loser = keyed("loser");
    loser.same_key_loser = true;
    let mut gone = keyed("gone");
    gone.effective_status = EffectiveStatus::Superseded;
    let mut maybe = keyed("maybe");
    maybe.effective_status = EffectiveStatus::Candidate;
    index
        .upsert_many(&[loser, gone, maybe, keyed("winner")])
        .unwrap();
    let winner = index
        .fact_key_winner("home-p", "user.editor", &["home-p".to_string()])
        .unwrap()
        .expect("a winner");
    assert_eq!(winner.id, "winner");
    assert_eq!(winner.origin, "user");
    assert!(
        index
            .fact_key_winner("home-p", "user.none", &["home-p".to_string()])
            .unwrap()
            .is_none()
    );
    assert!(
        index
            .fact_key_winner("proj-q", "user.editor", &["proj-q".to_string()])
            .unwrap()
            .is_none()
    );
}

/// Owner decision LQ14 (the L8 product slice's P2): a key held only by
/// notes the chat model's `memory_store` wrote — origin `extracted_user`
/// with no `observed_at`, which only the extractor sets — is not among
/// `known_keys` and has no winner for learning. A key a learned note or a
/// note the user saved also holds is offered as before, and its winner is
/// the R9 winner, whoever wrote it.
#[test]
fn a_key_only_the_chat_model_coined_is_not_offered_to_learning() {
    let index = ready_index();
    // (id, key, origin, has observed_at, updated on 2026-09-…)
    let notes = [
        // The chat model's own keys, alone.
        ("tool-a", "editor.preference", "extracted_user", false, 25),
        ("tool-b", "routine.weekly", "extracted_user", false, 24),
        // One the chat model coined that learning also uses…
        ("tool-c", "user.commute", "extracted_user", false, 23),
        ("learned", "user.commute", "extracted_user", true, 21),
        // …and one the user saved as well.
        ("tool-d", "user.laptop", "extracted_user", false, 22),
        ("saved", "user.laptop", "user", false, 20),
    ];
    let records: Vec<IndexRecord> = notes
        .iter()
        .map(|&(id, key, origin, observed, day)| {
            let mut r = record(id, id, "body");
            r.fact_key = Some(key.into());
            r.origin = origin.into();
            r.observed_at = observed.then(|| "2026-09-20T09:00:00Z".to_string());
            r.updated_at = format!("2026-09-{day}T10:00:00Z");
            r
        })
        .collect();
    index.upsert_many(&records).unwrap();

    assert_eq!(
        index.fact_keys(&["home-p".into()], 150).unwrap(),
        vec!["user.commute".to_string(), "user.laptop".to_string()]
    );
    let winner = |key: &str| {
        index
            .fact_key_winner("home-p", key, &["home-p".to_string()])
            .unwrap()
    };
    assert!(winner("editor.preference").is_none());
    assert!(winner("routine.weekly").is_none());
    assert!(winner("user.commute").is_some());
    assert!(winner("user.laptop").is_some());
}

/// LQ14's holder, over every row and across the partitions learning reads
/// together (repair round 1 of the L8 product slice): a key a learned note
/// held before the chat model's note replaced it stays learning's, its
/// winner the replacement; a key the user saved in one partition is
/// learning's in the other, where the chat model's note is its winner; and
/// a key only the chat model ever held, in any status, is not.
#[test]
fn a_key_is_learnings_when_a_learned_or_saved_note_ever_held_it_anywhere() {
    let index = ready_index();
    // (id, partition, key, origin, has observed_at, status)
    let notes = [
        (
            "learned",
            "home-p",
            "user.laptop",
            "extracted_user",
            true,
            "superseded",
        ),
        (
            "replacement",
            "home-p",
            "user.laptop",
            "extracted_user",
            false,
            "active",
        ),
        ("saved", "home-p", "lantern.build", "user", false, "active"),
        (
            "tool-proj",
            "proj-q",
            "lantern.build",
            "extracted_user",
            false,
            "active",
        ),
        (
            "tool-old",
            "home-p",
            "user.shell",
            "extracted_user",
            false,
            "superseded",
        ),
        (
            "tool-new",
            "home-p",
            "user.shell",
            "extracted_user",
            false,
            "active",
        ),
    ];
    let records: Vec<IndexRecord> = notes
        .iter()
        .enumerate()
        .map(|(i, &(id, partition, key, origin, observed, status))| {
            let mut r = record(id, id, "body");
            r.partition_key = partition.into();
            r.fact_key = Some(key.into());
            r.origin = origin.into();
            r.observed_at = observed.then(|| "2026-09-20T09:00:00Z".to_string());
            r.effective_status = if status == "active" {
                EffectiveStatus::Active
            } else {
                EffectiveStatus::Superseded
            };
            r.updated_at = format!("2026-09-2{i}T10:00:00Z");
            r
        })
        .collect();
    index.upsert_many(&records).unwrap();
    let both = ["proj-q".to_string(), "home-p".to_string()];

    let mut keys = index.fact_keys(&both, 150).unwrap();
    keys.sort();
    assert_eq!(keys, vec!["lantern.build", "user.laptop"]);
    let winner = |partition: &str, key: &str| {
        index
            .fact_key_winner(partition, key, &both)
            .unwrap()
            .map(|n| n.id)
    };
    assert_eq!(
        winner("home-p", "user.laptop").as_deref(),
        Some("replacement")
    );
    assert_eq!(
        winner("proj-q", "lantern.build").as_deref(),
        Some("tool-proj")
    );
    assert_eq!(winner("home-p", "user.shell"), None);
    // Decided in the project alone, the key is the chat model's there.
    assert_eq!(
        index
            .fact_key_winner("proj-q", "lantern.build", &["proj-q".to_string()])
            .unwrap(),
        None
    );
    let among = index
        .learning_keys_among(
            &both,
            &[
                "user.laptop".into(),
                "user.shell".into(),
                "lantern.build".into(),
            ],
        )
        .unwrap();
    assert_eq!(
        among,
        std::collections::HashSet::from(["user.laptop".to_string(), "lantern.build".to_string()])
    );
}

/// A conversation summary holds no key for learning (design 09 §13.3,
/// review of L10c, R1): a key a model's edit put on one is not among
/// `known_keys` and makes it no key's winner. An episode of another origin
/// with a key is an ordinary note.
#[test]
fn fact_keys_and_the_key_winner_skip_a_conversation_summary() {
    let index = ready_index();
    let mut summary = record("summary", "Conversation on 2026-09-20: Lisbon", "body");
    summary.kind = "episode".into();
    summary.origin = "extracted_other".into();
    summary.fact_key = Some("user.editor".into());
    summary.updated_at = "2026-09-24T12:00:00Z".into();
    let mut saved = record("saved", "The Lisbon trip", "body");
    saved.kind = "episode".into();
    saved.fact_key = Some("user.trip".into());
    saved.updated_at = "2026-09-20T10:00:00Z".into();
    index.upsert_many(&[summary, saved]).unwrap();

    assert_eq!(
        index.fact_keys(&["home-p".into()], 150).unwrap(),
        vec!["user.trip".to_string()]
    );
    assert_eq!(
        index
            .fact_key_winner("home-p", "user.editor", &["home-p".to_string()])
            .unwrap()
            .map(|n| n.id),
        None
    );
    assert_eq!(
        index
            .fact_key_winner("home-p", "user.trip", &["home-p".to_string()])
            .unwrap()
            .map(|n| n.id),
        Some("saved".to_string())
    );
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

/// Design 09 §6.5.2 (LQ17): `prefer` finds `prefers` as the beginning of a
/// word — the word table's prefix query — reported `prefix` and ranked below
/// a whole-token hit. A term inside a word of an English body finds nothing
/// (`presso` in `espresso`, `refer` in `prefers`); in a title it is still a
/// substring, `trigram`.
#[test]
fn a_prefix_hit_finds_prefer_inside_prefers_and_ranks_below_a_whole_token_hit() {
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
            record("head", "Espresso machine", "Descale it every month.\n"),
        ])
        .unwrap();
    let r = index.search(&q("prefer")).unwrap();
    assert_eq!(ids(&r), vec!["whole", "sub"]);
    assert_eq!(hit(&r, "whole").matches, vec![MatchReason::Term]);
    assert_eq!(hit(&r, "sub").matches, vec![MatchReason::Prefix]);
    assert!(hit(&r, "whole").score > hit(&r, "sub").score);
    assert_eq!(r.index_state, IndexState::Ready);
    assert!(!r.partial);
    // Inside a body word: nothing. Inside a title: a substring.
    assert_eq!(ids(&index.search(&q("refer")).unwrap()), Vec::<&str>::new());
    let r = index.search(&q("presso")).unwrap();
    assert_eq!(ids(&r), vec!["head"]);
    assert_eq!(hit(&r, "head").matches, vec![MatchReason::Trigram]);
}

/// Design 09 §6.5.2: a Latin term is matched in a Chinese body only as a
/// word or a word's beginning — the text is split where Chinese meets Latin,
/// so a glued word is a whole word — and never inside a word: the note
/// template's `## Knowledge` heading does not match `now`.
#[test]
fn a_latin_term_meets_a_chinese_body_only_as_a_word() {
    let index = ready_index();
    index
        .upsert_many(&[
            record("heading", "通勤", "# 通勤\n\n## Knowledge\n每天坐地铁上班。\n"),
            record("glued", "发版", "# 发版\n\n发版前先看一遍commit 记录。\n"),
            record("latin", "Deploy", "# Deploy\n\nRead the commit log first.\n"),
        ])
        .unwrap();
    assert_eq!(ids(&index.search(&q("now")).unwrap()), Vec::<&str>::new());
    assert_eq!(ids(&index.search(&q("mmit")).unwrap()), Vec::<&str>::new());
    let r = index.search(&q("commit")).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["glued", "latin"]);
    assert_eq!(hit(&r, "glued").matches, vec![MatchReason::Term]);
    assert_eq!(hit(&r, "latin").matches, vec![MatchReason::Term]);
    // `know` begins `knowledge`: a prefix hit, in any script's note.
    let r = index.search(&q("know")).unwrap();
    assert_eq!(ids(&r), vec!["heading"]);
    assert_eq!(hit(&r, "heading").matches, vec![MatchReason::Prefix]);
}

/// Design 09 §6.5.2: a term carrying an unsegmented character is never a
/// prefix; it is a substring wherever it stands in a run, the start
/// included, and a mixed Latin and Chinese chunk keeps its body in the
/// trigram table.
#[test]
fn a_cjk_term_is_a_substring_wherever_it_stands_in_a_run() {
    let index = ready_index();
    index
        .upsert_many(&[
            record("style", "回答风格", "# 回答风格\n\n用户喜欢简洁的回答\n"),
            record(
                "mixed",
                "Release notes",
                "# Release notes\n\nFor every version, 发布说明写成要点 and link the tag.\n",
            ),
        ])
        .unwrap();
    for (query, id) in [
        ("用户喜欢", "style"),
        ("简洁的回答", "style"),
        ("说明写成", "mixed"),
        ("发布说明写成要点", "mixed"),
    ] {
        let r = index.search(&q(query)).unwrap();
        assert_eq!(ids(&r), vec![id], "{query}");
        let want = if query == "发布说明写成要点" {
            vec![MatchReason::Term]
        } else {
            vec![MatchReason::Trigram]
        };
        assert_eq!(hit(&r, id).matches, want, "{query}");
    }
}

/// Design 09 §6.5.2: a term's last token is a prefix from three
/// characters — exactly three included — and, since repair round 2 of
/// LQ17, when it is a number after another token, whatever its length
/// (`a_literal_ending_in_a_number_finds_the_text_it_begins`): `memory_file`
/// finds `memory_files`, `x_abc` finds `x_abcd` and `tea` finds `teams`,
/// while `partner's` stays the phrase `partner s` and `x_ab*` (a caller's
/// `*`) never finds `x_abc`. At a threshold of four, `x_abc` and `tea`
/// would find nothing (the L8 close-out's B1).
#[test]
fn a_last_token_is_a_prefix_from_three_characters_or_as_a_number() {
    let index = ready_index();
    index
        .upsert_many(&[
            record("files", "Storage crate", "Read memory_files before you edit a note.\n"),
            record("birthday", "Birthday", "My partner's birthday is in May.\n"),
            record("store", "Shopping", "The partner store opens at nine.\n"),
            record("flag", "Linker flag", "Pass x_abc to the linker.\n"),
            record("longer", "Other flag", "Pass x_abcd when it links twice.\n"),
            record("team", "Work", "The user's teams meet on Mondays.\n"),
        ])
        .unwrap();
    let r = index.search(&q("memory_file")).unwrap();
    assert_eq!(ids(&r), vec!["files"]);
    assert_eq!(hit(&r, "files").matches, vec![MatchReason::Phrase]);
    assert_eq!(ids(&index.search(&q("partner's")).unwrap()), vec!["birthday"]);
    assert_eq!(ids(&index.search(&q("x_ab*")).unwrap()), Vec::<&str>::new());
    let r = index.search(&q("x_abc")).unwrap();
    assert_eq!(ids(&r)[0], "flag");
    assert!(ids(&r).contains(&"longer"), "{:?}", ids(&r));
    assert_eq!(hit(&r, "longer").matches, vec![MatchReason::Phrase]);
    let r = index.search(&q("tea")).unwrap();
    assert_eq!(ids(&r), vec!["team"]);
    assert_eq!(hit(&r, "team").matches, vec![MatchReason::Prefix]);
}

/// Design 09 §6.5.2 (repair round 2 of LQ17): a literal counts as found —
/// so its parts are not searched — when a note holds its tokens in sequence,
/// a substring the trigram tables find, or a longer form that holds the
/// literal verbatim (`memory_file` in `memory_files`). A longer form alone is
/// not the literal (`dark modes` for `dark-mode`): its parts are searched, so
/// a note holding one of them whole is still found, as schema 3 found it.
/// The file scan agrees.
#[test]
fn a_literal_found_only_in_a_longer_form_still_routes_its_parts() {
    use MatchReason::{Phrase, Prefix, Term};
    let index = ready_index();
    let records = vec![
        record(
            "dm-a",
            "Editor setup",
            "Dark modes are enabled in every app the user installs.\n",
        ),
        record("dm-b", "Theme", "The user prefers a dark theme at night.\n"),
        record(
            "cr-a",
            "Team habits",
            "The team does code reviews on Fridays.\n",
        ),
        record(
            "cr-b",
            "Pull requests",
            "The user wants a careful review of every change.\n",
        ),
        record("sp-a", "Hobbies", "Side projects are for weekends only.\n"),
        record(
            "sp-b",
            "Rust game",
            "The user's current project is a small roguelike.\n",
        ),
        record(
            "tz-a",
            "Remote team",
            "The user tracks time zones for the remote team.\n",
        ),
        record(
            "tz-b",
            "Location",
            "The user's timezone is Europe/Berlin.\n",
        ),
        record(
            "oc-a",
            "Platform rota",
            "The platform team keeps on calls short.\n",
        ),
        record("oc-b", "Pager", "The user is on call every third week.\n"),
        record(
            "files",
            "Storage crate",
            "Read memory_files before you edit a note.\n",
        ),
        record(
            "trip",
            "Trip",
            "The memory of that trip stays with the user.\n",
        ),
    ];
    index.rebuild(&records).unwrap();
    let cases: [(&str, &[&str], &str, &[MatchReason]); 7] = [
        ("dark-mode", &["dm-a", "dm-b"], "dm-b", &[Term]),
        ("do I like dark-mode?", &["dm-a", "dm-b"], "dm-b", &[Term]),
        ("code-review", &["cr-a", "cr-b"], "cr-b", &[Term]),
        ("side-project", &["sp-a", "sp-b"], "sp-b", &[Term]),
        ("time-zone", &["tz-a", "tz-b"], "tz-b", &[Prefix]),
        // The note holds the phrase itself: found, parts not searched.
        ("on-call", &["oc-a", "oc-b"], "oc-b", &[Phrase]),
        // A longer form holding the literal verbatim: found, so `memory`
        // is not searched and the trip note stays out.
        ("memory_file", &["files"], "files", &[Phrase]),
    ];
    for (query, want, id, reasons) in cases {
        let r = index.search(&q(query)).unwrap();
        let mut got = ids(&r);
        got.sort();
        assert_eq!(got, want, "{query}");
        assert_eq!(hit(&r, id).matches, reasons, "{query} {id}");
        let scan = index.scan_search(&records, &q(query), false).unwrap();
        let mut from_files = ids(&scan);
        from_files.sort();
        assert_eq!(
            from_files, got,
            "{query}: the file scan finds what the index finds"
        );
        for h in &scan.hits {
            assert_eq!(h.matches, hit(&r, &h.id).matches, "{query} {}", h.id);
        }
    }
}

/// Design 09 §6.5.2 (repair round 2 of LQ17): a literal whose last token is
/// a number is a prefix whatever its length, so a time, a version or a date
/// finds the text it begins (`10:30` in `10:30am`, `v1.2` in `v1.2beta`,
/// `2026-09-15` in `2026-09-15T10:00Z`), and counts as found there. A number
/// inside a word is still not found (`1.2` in `v1.2beta`, design 09 §6.5.4).
#[test]
fn a_literal_ending_in_a_number_finds_the_text_it_begins() {
    let index = ready_index();
    let records = vec![
        record(
            "time",
            "Standup",
            "The daily standup is at 10:30am on weekdays.\n",
        ),
        record("rc", "Python", "The project pins python 3.12rc1 for now.\n"),
        record("beta", "App version", "The desktop app is on v1.2beta.\n"),
        record(
            "iso",
            "Deploy",
            "The last deploy ran at 2026-09-15T10:00Z without errors.\n",
        ),
        record(
            "next",
            "Next deploy",
            "The next deploy is planned for 2026-10-01.\n",
        ),
    ];
    index.rebuild(&records).unwrap();
    for (query, want) in [
        ("10:30", vec!["time"]),
        ("what is at 10:30?", vec!["time"]),
        ("3.12", vec!["rc"]),
        ("v1.2", vec!["beta"]),
        ("2026-09-15", vec!["iso"]),
        ("1.2", vec![]),
    ] {
        let r = index.search(&q(query)).unwrap();
        assert_eq!(ids(&r), want, "{query}");
        for h in &r.hits {
            assert_eq!(h.matches, vec![MatchReason::Phrase], "{query} {}", h.id);
        }
        let scan = index.scan_search(&records, &q(query), false).unwrap();
        assert_eq!(
            ids(&scan),
            want,
            "{query}: the file scan finds what the index finds"
        );
    }
}

/// Design 09 §6.5.2: when the word table's query fills its candidate cap
/// with notes a prefix found (`work*` → `worker`, `workspace`), the terms
/// sent as prefixes are searched again as whole words in titles, aliases
/// and tags — so a note whose one match is the tag `work` is still a
/// candidate. The trigram header table cannot save it: `work` is a
/// substring of every `Worker` title, and those fill its cap too. A note
/// only that query kept takes no word relevance (design 09 §6.5.3): its
/// coverage and its authority score it.
#[test]
fn a_whole_word_in_a_header_is_not_crowded_out_by_a_prefix_that_fills_the_cap() {
    let index = ready_index();
    let mut records: Vec<IndexRecord> = (0..CANDIDATE_CAP + 40)
        .map(|i| {
            record(
                &format!("w{i:03}"),
                &format!("Worker rota {i}"),
                "The worker rota lives in the shared workspace; workers swap shifts.\n",
            )
        })
        .collect();
    let mut job = record("job", "Current employer", "Quillpay, since March, as a designer.\n");
    job.tags = vec!["work".into()];
    records.push(job);
    index.rebuild(&records).unwrap();
    let mut sq = q("where do I work?");
    sq.limit = MAX_LIMIT;
    let r = index.search(&sq).unwrap();
    assert!(r.partial, "the word table's query filled its cap");
    assert_eq!(ids(&r)[0], "job", "{:?}", ids(&r));
    assert_eq!(hit(&r, "job").matches, vec![MatchReason::Term]);
    assert_eq!(
        hit(&r, "job").score,
        search::W_TERM + search::W_AUTHORITY * authority_rank("user") as f64,
        "coverage and authority alone: no relevance borrowed from the query at the cap"
    );
}

/// Design 09 §6.5.2 (repair round 1 of LQ17): the same at the cap for a
/// body. Each term sent as a prefix is searched again alone as a whole word
/// in bodies, unranked, and its chunks join the candidates when they fit
/// within the cap — so a note whose one match is `work` in its body is not
/// lost among the `worker` and `workspace` bodies the prefix found. A note
/// only that query kept takes no word relevance: its coverage and its
/// authority score it (§6.5.3), and a whole word still ranks above every
/// beginning.
#[test]
fn a_whole_word_in_a_body_is_not_crowded_out_by_a_prefix_that_fills_the_cap() {
    let index = ready_index();
    let mut records: Vec<IndexRecord> = (0..CANDIDATE_CAP + 40)
        .map(|i| {
            record(
                &format!("w{i:03}"),
                &format!("Worker rota {i}"),
                "The worker rota lives in the shared workspace; workers swap shifts.\n",
            )
        })
        .collect();
    records.push(record(
        "commute",
        "Commute",
        "The user rides to work on a bicycle.\n",
    ));
    index.rebuild(&records).unwrap();
    let mut sq = q("where do I work?");
    sq.limit = MAX_LIMIT;
    let r = index.search(&sq).unwrap();
    assert!(r.partial, "the word table's query filled its cap");
    assert_eq!(ids(&r)[0], "commute", "{:?}", ids(&r));
    let commute = hit(&r, "commute");
    assert_eq!(commute.matches, vec![MatchReason::Term]);
    assert_eq!(
        commute.score,
        search::W_TERM + search::W_AUTHORITY * authority_rank("user") as f64,
        "coverage and authority alone: no relevance borrowed from the query at the cap"
    );
}

/// Design 09 §6.5.2 and §6.5.4 (repair round 2 of LQ17): the body query at
/// the cap adds a word's chunks only when there are at most as many as the
/// cap. A word held whole by more bodies than that adds none of them, so the
/// candidates are the prefix query's own 256 — which can hold none of those
/// notes whenever more than 256 chunks holding a longer form outrank them
/// on BM25, by a header column as here or by a shorter row alone (the L8
/// close-out's B2 corrects §6.5.4, which named titles only). This pins the
/// cliff the design names; adding the first 256 by rowid instead would rank
/// arbitrary notes above every ranked prefix hit.
#[test]
fn a_body_word_held_by_more_than_the_cap_adds_none_at_the_cap() {
    let index = ready_index();
    let mut records: Vec<IndexRecord> = (0..CANDIDATE_CAP + 44)
        .map(|i| {
            record(
                &format!("w{i:03}"),
                &format!("Worker rota {i}"),
                "The worker rota lives in the shared workspace; workers swap shifts.\n",
            )
        })
        .collect();
    for i in 0..=CANDIDATE_CAP {
        records.push(record(
            &format!("c{i:03}"),
            &format!("Day {i}"),
            "Today I rode along the river to work.\n",
        ));
    }
    index.rebuild(&records).unwrap();
    let mut sq = q("where do I work?");
    sq.limit = MAX_LIMIT;
    let r = index.search(&sq).unwrap();
    assert!(r.partial, "the word table's query filled its cap");
    assert_eq!(r.count, MAX_LIMIT);
    for h in &r.hits {
        assert!(h.id.starts_with('w'), "{}: a whole-word note joined", h.id);
        assert_eq!(h.matches, vec![MatchReason::Prefix], "{}", h.id);
    }
}

/// Design 09 §6.5.3: trigram relevance is the larger of the header table's
/// and the unsegmented-body table's normalized BM25, not their sum — a term
/// found inside a run of both a note's title and its body earns one
/// trigram relevance weight.
#[test]
fn trigram_relevance_is_the_larger_of_the_two_tables_not_their_sum() {
    let index = ready_index();
    index
        .upsert(&record(
            "both",
            "项目发布说明",
            "每次发布说明都写成要点。\n",
        ))
        .unwrap();
    let r = index.search(&q("发布说明")).unwrap();
    assert_eq!(ids(&r), vec!["both"]);
    let both = hit(&r, "both");
    assert_eq!(both.matches, vec![MatchReason::Trigram]);
    assert_eq!(
        both.score,
        search::W_TRIGRAM
            + search::W_BM25_TRIGRAM
            + search::W_AUTHORITY * authority_rank("user") as f64,
        "one trigram relevance, the larger of the two tables' (each 1.0 here)"
    );
}

/// A query word glued across scripts (`python版本`) goes to the word table
/// as the phrase of its split tokens, so it finds text written spaced
/// (`Python 版本`) as well as glued; the ranker and the file scan read the
/// same split tokens in sequence and call it a whole-word match, `term`, so
/// an answer given during a rebuild finds what the index finds (design 09
/// §6.5.3). The tokens must stand in that sequence: a note holding both
/// apart is found by neither.
#[test]
fn a_query_word_glued_across_scripts_is_a_term_in_spaced_and_glued_text() {
    let index = ready_index();
    let records = vec![
        record("spaced", "Runtime", "Python 版本: 3.12\n"),
        record("glued", "Toolchain", "我们的python版本 是 3.11。\n"),
        record("apart", "Notes", "版本 是 3.11，工具用 python 写。\n"),
        record("other", "Garden", "Tomatoes ripen in August.\n"),
    ];
    index.rebuild(&records).unwrap();
    for query in ["python版本", "Python版本"] {
        let r = index.search(&q(query)).unwrap();
        let mut got = ids(&r);
        got.sort();
        assert_eq!(got, vec!["glued", "spaced"], "{query}");
        for id in ["glued", "spaced"] {
            assert_eq!(hit(&r, id).matches, vec![MatchReason::Term], "{query} {id}");
        }
        let scan = index.scan_search(&records, &q(query), false).unwrap();
        let mut from_files = ids(&scan);
        from_files.sort();
        assert_eq!(from_files, got, "{query}: the file scan finds what the index finds");
        for h in &scan.hits {
            assert_eq!(h.matches, hit(&r, &h.id).matches, "{query} {}", h.id);
        }
    }
}

/// Design 09 §6.5.1 (repair round 1 of LQ17): `unicode61` breaks a word of
/// Devanagari, Tamil or Bengali at every vowel sign, and reads a Yi or
/// Javanese run as one token, so that text keeps substring search on the
/// trigram table. A stem finds its inflected form — as a phrase, since the
/// virama makes the query a literal — and a word inside a run is a
/// substring, `trigram`; the file scan agrees.
#[test]
fn text_unicode61_breaks_at_vowel_signs_or_reads_as_one_run_keeps_substring_search() {
    let index = ready_index();
    let records = vec![
        record("hindi", "Weekend", "हर शनिवार दोस्तों के साथ क्रिकेट।\n"),
        record("tamil", "Home", "அவர் சென்னையில் வசிக்கிறார்.\n"),
        record("bengali", "Friends", "বন্ধুদের সাথে আড্ডা।\n"),
        record("yi", "Yi note", "ꆈꌠꁱꂷꀋꉬ\n"),
        record("javanese", "Javanese note", "ꦲꦏꦸꦱꦼꦩꦤ\n"),
    ];
    index.rebuild(&records).unwrap();
    for (query, id, reason) in [
        ("दोस्त", "hindi", MatchReason::Phrase),
        ("சென்னை", "tamil", MatchReason::Phrase),
        ("বন্ধু", "bengali", MatchReason::Phrase),
        ("ꌠꁱꂷ", "yi", MatchReason::Trigram),
        ("ꦏꦸꦱꦼ", "javanese", MatchReason::Trigram),
    ] {
        let r = index.search(&q(query)).unwrap();
        assert_eq!(ids(&r), vec![id], "{query}");
        assert_eq!(hit(&r, id).matches, vec![reason], "{query}");
        let scan = index.scan_search(&records, &q(query), false).unwrap();
        assert_eq!(ids(&scan), vec![id], "{query}: the file scan agrees");
        assert_eq!(scan.hits[0].matches, vec![reason], "{query}");
    }
}

/// Design 09 §6.5.1 (repair round 1 of LQ17): `unicode61` counts letters,
/// numbers and private-use characters as token characters, by its own
/// Unicode tables, and breaks a word at everything else; `query::tokens`
/// keeps every alphanumeric character. Where the two disagree inside a
/// word, the word table holds fragments that neither the ranker nor the
/// file scan reads, so that text must reach the trigram table: every such
/// character — as the bundled SQLite reads it — is unsegmented, but for the
/// ones listed here, which no living script writes inside every word: the
/// vowel points of Hebrew, Arabic, Syriac and Samaritan (their ordinary
/// text is unpointed), and a few combining letters and symbols.
#[test]
fn every_character_unicode61_breaks_a_word_at_is_unsegmented_or_listed() {
    const LISTED: &[(u32, u32)] = &[
        (0x0590, 0x05FF),   // Hebrew points
        (0x0600, 0x06FF),   // Arabic harakat
        (0x0700, 0x074F),   // Syriac vowels
        (0x0800, 0x083F),   // Samaritan vowels
        (0x08A0, 0x08FF),   // Arabic extended-A marks
        (0x1800, 0x18AF),   // Mongolian ali gali
        (0x1DC0, 0x1DFF),   // combining Latin letters
        (0x2DE0, 0x2DFF),   // combining Cyrillic letters
        (0xA640, 0xA69F),   // combining Cyrillic letters
        (0xFB1D, 0xFB4F),   // Hebrew presentation forms
        (0x1F100, 0x1F1FF), // enclosed Latin letters
    ];
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE t USING fts5(x, tokenize='unicode61 remove_diacritics 2');
         CREATE VIRTUAL TABLE v USING fts5vocab(t, 'instance');",
    )
    .unwrap();
    let mut probe: std::collections::BTreeSet<char> = std::collections::BTreeSet::new();
    for c in (0u32..=0x10FFFF).filter_map(char::from_u32) {
        if c.is_ascii() || !c.is_alphanumeric() {
            continue;
        }
        for d in query::normalize(&c.to_string()).chars() {
            if d.is_alphanumeric() && !d.is_ascii() && !query::is_unsegmented(d) {
                probe.insert(d);
            }
        }
    }
    let tx = conn.unchecked_transaction().unwrap();
    {
        let mut insert = tx.prepare("INSERT INTO t(rowid, x) VALUES (?1, ?2)").unwrap();
        for c in &probe {
            insert
                .execute(rusqlite::params![*c as i64, format!("q{c}q")])
                .unwrap();
        }
    }
    tx.commit().unwrap();
    // `q<c>q` is one token when `c` is a token character, two otherwise.
    let mut stmt = conn
        .prepare("SELECT doc FROM v GROUP BY doc HAVING COUNT(*) > 1")
        .unwrap();
    let breaking: Vec<u32> = stmt
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap() as u32)
        .collect();
    assert!(breaking.len() > 100, "the probe saw the marks: {}", breaking.len());
    let unlisted: Vec<String> = breaking
        .iter()
        .filter(|&&u| !LISTED.iter().any(|&(a, b)| (a..=b).contains(&u)))
        .map(|u| format!("U+{u:04X}"))
        .collect();
    assert!(
        unlisted.is_empty(),
        "unicode61 breaks a word at these, and is_unsegmented does not cover them: {unlisted:?}"
    );
}

/// Every way a chunk row goes takes its full-text rows with it — a cascade
/// from `note` too — so a reused chunk id never inherits a deleted note's
/// terms (design 09 §6.5.1: the `AFTER DELETE ON chunk` trigger).
#[test]
fn a_note_row_deleted_by_cascade_leaves_no_full_text_row() {
    let index = ready_index();
    index
        .upsert(&record("gone", "Zebra crossing", "斑马线在学校门口 zebra.\n"))
        .unwrap();
    {
        let conn = index.lock();
        let deleted = conn
            .execute("DELETE FROM note WHERE id = 'gone'", [])
            .unwrap();
        assert_eq!(deleted, 1);
        for table in ["chunk", "fts_word", "fts_tri_head", "fts_tri"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} keeps no row of the deleted note");
        }
    }
    // The next chunk takes the freed id and none of the old terms.
    index
        .upsert(&record("next", "Garden", "Tomatoes ripen in August.\n"))
        .unwrap();
    for query in ["zebra", "斑马线", "crossing"] {
        assert_eq!(ids(&index.search(&q(query)).unwrap()), Vec::<&str>::new(), "{query}");
    }
    assert_eq!(ids(&index.search(&q("tomatoes")).unwrap()), vec!["next"]);
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

/// The text of the chunk a hit names, read from the note's body — what the
/// service delivers from the re-read file (design 09 §6.5.1: the index
/// serves no excerpt).
fn chunk_text(record: &IndexRecord, ordinal: u32) -> String {
    let chunks = chunker::chunk_body(&record.body);
    chunks
        .iter()
        .find(|c| c.ordinal == ordinal)
        .map(|c| c.text.clone())
        .unwrap_or_default()
}

#[test]
fn identifiers_match_as_phrases_and_the_hit_names_the_chunk_holding_them() {
    let index = ready_index();
    let env = record(
        "env",
        "Scratch store location",
        "Set `OPENALPACA_HOME_STORE` to an absolute scratch path; the library lives in `openalpaca_storage::memory_files`.\n",
    );
    index
        .upsert_many(&[
            env.clone(),
            record("decoy", "Fruit", "Apples are in season in October.\n"),
        ])
        .unwrap();
    let r = index.search(&q("OPENALPACA_HOME_STORE")).unwrap();
    assert_eq!(ids(&r), vec!["env"]);
    assert!(hit(&r, "env").matches.contains(&MatchReason::Phrase));
    assert!(chunk_text(&env, hit(&r, "env").chunk_ordinal).contains("OPENALPACA_HOME_STORE"));
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
            record("tabs", "Tabs and spaces", "Use tabs for indentation and spaces for alignment; never one or the other alone. Quote a\"b and 简\"洁 exactly.\n"),
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
        // A quote inside a term reaches FTS5 doubled, never as a delimiter.
        "a\"b",
        "简\"洁",
        "tabs a\"b",
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

/// Design 09 §13.3 (MD-16): the automatic path never returns a
/// conversation summary — an `episode` of origin `extracted_other` — while
/// an explicit search does; an episode of any other origin and another
/// kind of `extracted_other` note are not summaries and stay recallable.
#[test]
fn the_automatic_path_excludes_conversation_summaries() {
    let index = ready_index();
    let mut summary = record(
        "summary",
        "Conversation on 2026-09-20: the release",
        "Discussed: the release checklist.\n",
    );
    summary.kind = "episode".into();
    summary.origin = "extracted_other".into();
    let mut saved = record("saved", "The release party", "We celebrated the release.\n");
    saved.kind = "episode".into();
    let mut learned = record("learned", "Release day", "The release ships on Tuesdays.\n");
    learned.kind = "episode".into();
    learned.origin = "extracted_user".into();
    let mut inferred = record("inferred", "Release owner", "Mina owns the release.\n");
    inferred.origin = "extracted_other".into();
    index
        .upsert_many(&[summary, saved, learned, inferred])
        .unwrap();

    let mut explicit = q("release");
    explicit.limit = 20;
    let mut got = ids(&index.search(&explicit).unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(got, vec!["inferred", "learned", "saved", "summary"]);

    let mut auto = explicit.clone();
    auto.mode = SearchMode::Automatic;
    let mut got = ids(&index.search(&auto).unwrap())
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(got, vec!["inferred", "learned", "saved"]);
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

/// Design §4 *Times*: current recall excludes a note whose interval has
/// **ended**; one that has not begun yet — a trip planned for next month —
/// is still what the user told us, and a current search finds it. `as_of`
/// evaluates the whole half-open interval at that instant.
#[test]
fn a_current_search_keeps_a_note_whose_window_has_not_started() {
    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let now = at("2026-09-24T12:00:00Z");
    let index = MemoryIndex::open_in_memory()
        .unwrap()
        .with_clock(move || now);
    index.rebuild(&[]).unwrap();
    let mut planned = record(
        "planned",
        "Lisbon trip",
        "The user travels to Lisbon for a conference, 5 to 19 October.\n",
    );
    planned.valid_from = Some("2026-10-05T00:00:00Z".into());
    planned.valid_to = Some("2026-10-20T00:00:00Z".into());
    let mut ended = record(
        "ended",
        "Porto trip",
        "The user travelled to Porto for a conference, 1 to 5 September.\n",
    );
    ended.valid_from = Some("2026-09-01T00:00:00Z".into());
    ended.valid_to = Some("2026-09-06T00:00:00Z".into());
    index.upsert_many(&[planned, ended]).unwrap();

    // Now is 24 September: the planned trip is current, the ended one is not.
    assert_eq!(
        ids(&index.search(&q("conference trip")).unwrap()),
        vec!["planned"]
    );
    let mut automatic = q("conference trip");
    automatic.mode = SearchMode::Automatic;
    assert_eq!(ids(&index.search(&automatic).unwrap()), vec!["planned"]);
    // `as_of` inside the window finds it; at its (exclusive) end it is gone.
    let mut during = q("conference trip");
    during.as_of = Some(at("2026-10-10T00:00:00Z"));
    assert_eq!(ids(&index.search(&during).unwrap()), vec!["planned"]);
    let mut after = q("conference trip");
    after.as_of = Some(at("2026-10-20T00:00:00Z"));
    assert_eq!(index.search(&after).unwrap().count, 0);
    // `as_of` before the window still evaluates the whole interval.
    let mut before = q("conference trip");
    before.as_of = Some(at("2026-09-24T12:00:00Z"));
    assert_eq!(index.search(&before).unwrap().count, 0);
    // `historical` with no `as_of` applies no validity filter at all.
    let mut all = q("conference trip");
    all.historical = true;
    let r = index.search(&all).unwrap();
    let mut got = ids(&r);
    got.sort();
    assert_eq!(got, vec!["ended", "planned"]);
}

/// Design 09 §6.1: every hit and every pinned row says whether its window
/// has not begun yet, on the index's own clock — the instant its validity
/// filter used — so the recall header and the filter cannot disagree. A
/// note with no `valid_from`, or one whose window has begun, is not
/// upcoming; with `as_of` the instant is that one.
#[test]
fn hits_and_pinned_rows_say_upcoming_on_the_index_clock() {
    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let now = at("2026-09-24T12:00:00Z");
    let index = MemoryIndex::open_in_memory()
        .unwrap()
        .with_clock(move || now);
    index.rebuild(&[]).unwrap();
    let windowed = |id: &str, from: Option<&str>, to: Option<&str>| {
        let mut r = record(id, id, "The user's conference trip.\n");
        r.pinned = true;
        r.valid_from = from.map(str::to_string);
        r.valid_to = to.map(str::to_string);
        r
    };
    index
        .upsert_many(&[
            windowed(
                "planned",
                Some("2026-10-05T00:00:00Z"),
                Some("2026-10-20T00:00:00Z"),
            ),
            windowed(
                "current",
                Some("2026-09-01T00:00:00Z"),
                Some("2026-10-01T00:00:00Z"),
            ),
            windowed("open", None, None),
            windowed("until", None, Some("2026-12-31T00:00:00Z")),
        ])
        .unwrap();

    let r = index.search(&q("conference trip")).unwrap();
    assert_eq!(r.count, 4);
    for (id, upcoming) in [
        ("planned", true),
        ("current", false),
        ("open", false),
        ("until", false),
    ] {
        assert_eq!(hit(&r, id).upcoming, upcoming, "hit {id}");
    }
    let pinned: Vec<(String, bool)> = index
        .list_pinned(&["home-p".to_string()])
        .unwrap()
        .into_iter()
        .map(|n| (n.id, n.upcoming))
        .collect();
    assert_eq!(pinned.len(), 4);
    for (id, upcoming) in &pinned {
        assert_eq!(*upcoming, id == "planned", "pinned row {id}");
    }
    assert!(index.get("home-p", "planned").unwrap().unwrap().upcoming);
    assert!(!index.get("home-p", "current").unwrap().unwrap().upcoming);

    // Evaluated inside the window, the same note has begun.
    let mut during = q("conference trip");
    during.as_of = Some(at("2026-10-10T00:00:00Z"));
    let r = index.search(&during).unwrap();
    assert!(!hit(&r, "planned").upcoming);
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
    assert!(chunk_text(&rec, h.chunk_ordinal).contains("blue-green script"));
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
            record("phone", "Phone", "My phone runs Android 15.\n"),
        ])
        .unwrap();
    // A stop word routed alone is matched exactly, never as a prefix
    // (design 09 §6.5.2): `and` does not find `Android`.
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

/// `stamp_scan` records exactly the `(last_scan_at, last_scan_files)` a
/// reconcile of the same scan records, and changes no row (scale slice S4:
/// a write's re-index stamps its own scan without reading every row back).
#[test]
fn stamp_scan_records_what_a_reconcile_records() {
    use chrono::TimeZone;
    let at = Utc.with_ymd_and_hms(2026, 9, 26, 8, 0, 0).unwrap();
    let index = ready_index().with_clock(move || at);
    index
        .upsert_many(&[record("coffee", "Coffee", "Oat flat white.\n")])
        .unwrap();
    index
        .reconcile("home-p", || {
            Ok(vec![
                entry("notes/a.md", 1, 1, 1),
                entry("notes/b.md", 1, 1, 2),
            ])
        })
        .unwrap();
    let reconciled = index.last_scan("home-p").unwrap();
    index.stamp_scan("home-q", 2).unwrap();
    assert_eq!(index.last_scan("home-q").unwrap(), reconciled);
    assert_eq!(reconciled, Some(("2026-09-26T08:00:00Z".to_string(), 2)));
    index.stamp_scan("home-p", 5).unwrap();
    assert_eq!(
        index.last_scan("home-p").unwrap(),
        Some(("2026-09-26T08:00:00Z".to_string(), 5))
    );
    assert!(
        index.get("home-p", "coffee").unwrap().is_some(),
        "no row changed"
    );
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
/// A hit's id, reasons, chunk ordinal, heading, line range, superseded-by
/// list and conflict flag.
type HitShape = (
    String,
    Vec<MatchReason>,
    u32,
    Option<String>,
    (u32, u32),
    Vec<String>,
    bool,
);

fn shape(h: &SearchHit) -> HitShape {
    (
        h.id.clone(),
        h.matches.clone(),
        h.chunk_ordinal,
        h.heading.clone(),
        h.line_range,
        h.superseded_by.clone(),
        h.conflict,
    )
}

/// What each full-text table holds, term by term — `fts5vocab`'s
/// `instance` rows (term, column, offset), each keyed by the note id and
/// chunk ordinal of its rowid — sorted. Contentless tables hold no text, so
/// comparing their row counts would pass with text written into the wrong
/// column; this compares what they index (design 09 §6.5.7).
pub(super) fn fts_contents(conn: &rusqlite::Connection) -> Vec<String> {
    let mut out = Vec::new();
    for table in ["fts_word", "fts_tri_head", "fts_tri"] {
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS temp.vocab_{table};
             CREATE VIRTUAL TABLE temp.vocab_{table} USING fts5vocab(main, {table}, instance);"
        ))
        .unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT n.partition_key, n.id, c.ordinal, v.term, v.col, v.offset
                 FROM temp.vocab_{table} v
                 JOIN chunk c ON c.id = v.doc
                 JOIN note n ON n.rowid = c.note_rowid"
            ))
            .unwrap();
        let mut rows: Vec<String> = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{table} {} {} {} {:?} {} {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
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
        rows.sort();
        out.push(format!("-- {table}: {} instances", rows.len()));
        out.extend(rows);
    }
    out
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
    // The full-text tables index the same terms in the same columns, for
    // the same chunks, and no orphaned row is left on either side.
    let x = fts_contents(&incremental.lock());
    let y = fts_contents(&rebuilt.lock());
    assert_eq!(x, y);
    assert!(
        x.iter().any(|l| l.starts_with("fts_tri ")),
        "the Chinese note's body is in the trigram table"
    );
    assert!(
        !x.iter().any(|l| l.starts_with("fts_tri ") && l.contains(" c ")),
        "an English body is not"
    );
}

/// Design 09 §3.5 (LR18): the pending suggestions are read from the catalog
/// — effective candidates whose own `supersedes` names a note, newest
/// `updated_at` first, of the named partitions only — and `catalog_note`
/// reads one row whatever its status.
#[test]
fn suggestion_rows_are_candidates_that_name_a_note_newest_first() {
    let index = ready_index();
    let candidate = |id: &str, updated: &str, supersedes: &[&str]| {
        let mut r = record(id, "Commute", "by bike");
        r.status = "candidate".into();
        r.effective_status = EffectiveStatus::Candidate;
        r.origin = "extracted_user".into();
        r.updated_at = updated.into();
        r.supersedes = supersedes.iter().map(|s| s.to_string()).collect();
        r
    };
    let older = candidate("s1", "2026-09-20T10:00:00Z", &["x"]);
    let newer = candidate("s2", "2026-09-22T10:00:00Z", &["x"]);
    let plain = candidate("c1", "2026-09-23T10:00:00Z", &[]);
    let mut elsewhere = candidate("s3", "2026-09-23T10:00:00Z", &["y"]);
    elsewhere.partition_key = "proj-q".into();
    let mut active = record("a1", "Editor", "helix");
    active.supersedes = vec!["old".into()];
    let target = record("x", "Commute", "by bus");
    index
        .upsert_many(&[older, newer, plain, elsewhere, active, target])
        .unwrap();

    let ids = |rows: Vec<CatalogNote>| rows.into_iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(
        ids(index.suggestion_rows(&["home-p".into()]).unwrap()),
        vec!["s2", "s1"]
    );
    assert_eq!(
        ids(index
            .suggestion_rows(&["home-p".into(), "proj-q".into()])
            .unwrap()),
        vec!["s3", "s2", "s1"]
    );
    assert!(index.suggestion_rows(&[]).unwrap().is_empty());
    let row = index.catalog_note("home-p", "x").unwrap().unwrap();
    assert_eq!(
        (row.title.as_str(), row.effective_status),
        ("Commute", EffectiveStatus::Active)
    );
    assert!(index.catalog_note("proj-q", "x").unwrap().is_none());
}

// ============================================================================
// Dates, order and the timeline (design 09 §13.1, MD-14)
// ============================================================================

fn instant(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

/// A note about the office, first written at `created`, observed at
/// `observed` when given.
fn dated(id: &str, created: &str, observed: Option<&str>) -> IndexRecord {
    let mut r = record(id, &format!("Office {id}"), "The office moved again.\n");
    r.created_at = created.to_string();
    r.updated_at = created.to_string();
    r.observed_at = observed.map(str::to_string);
    r
}

/// Design 09 §13.1: the index row's `date` is `observed_at` when the note
/// has one, else `created_at` — never `updated_at`, which any edit moves.
#[test]
fn a_hit_is_dated_by_observed_at_else_created_at() {
    let index = ready_index();
    let mut edited = dated("edited", "2026-03-01T09:00:00Z", None);
    edited.updated_at = "2026-09-20T09:00:00Z".into();
    index
        .upsert_many(&[
            dated(
                "seen",
                "2026-09-01T09:00:00Z",
                Some("2026-05-10T08:30:00+02:00"),
            ),
            edited,
        ])
        .unwrap();
    let r = index.search(&q("office")).unwrap();
    assert_eq!(hit(&r, "seen").date, "2026-05-10T06:30:00Z");
    assert_eq!(hit(&r, "edited").date, "2026-03-01T09:00:00Z");
}

/// Design 09 §13.1: `date_from` and `date_to` bound the note's date,
/// inclusive at both ends, before the limit.
#[test]
fn date_from_and_date_to_bound_the_date_inclusively() {
    let index = ready_index();
    index
        .upsert_many(&[
            dated("jan", "2026-01-15T00:00:00Z", None),
            // Created in September, observed in March: March is its date.
            dated("mar", "2026-09-01T00:00:00Z", Some("2026-03-10T12:00:00Z")),
            dated("jun", "2026-06-30T23:59:59Z", None),
            dated("sep", "2026-09-10T00:00:00Z", None),
        ])
        .unwrap();
    let between = |from: Option<&str>, to: Option<&str>| {
        let mut query = q("office");
        query.date_from = from.map(instant);
        query.date_to = to.map(instant);
        let mut got: Vec<String> = index
            .search(&query)
            .unwrap()
            .hits
            .into_iter()
            .map(|h| h.id)
            .collect();
        got.sort();
        got
    };
    assert_eq!(
        between(Some("2026-03-10T12:00:00Z"), Some("2026-06-30T23:59:59Z")),
        vec!["jun", "mar"],
        "both bounds are inclusive"
    );
    assert_eq!(between(Some("2026-07-01T00:00:00Z"), None), vec!["sep"]);
    assert_eq!(
        between(None, Some("2026-03-31T00:00:00Z")),
        vec!["jan", "mar"],
        "`mar` is dated by what it observed, not when it was written"
    );
    // The bound is applied before the limit: one hit asked for, and the
    // one in range is the one returned.
    let mut one = q("office");
    one.limit = 1;
    one.date_from = Some(instant("2026-09-05T00:00:00Z"));
    assert_eq!(ids(&index.search(&one).unwrap()), vec!["sep"]);
}

/// Design 09 §13.1: `order_by` newest and oldest sort by the date, not by
/// relevance, and the limit takes the newest (or oldest) of every match.
#[test]
fn order_newest_and_oldest_sort_by_the_date() {
    let index = ready_index();
    let mut strong = dated("strong", "2026-02-01T00:00:00Z", None);
    strong.title = "office".into();
    index
        .upsert_many(&[
            strong,
            dated("mid", "2026-05-01T00:00:00Z", None),
            dated("late", "2026-08-01T00:00:00Z", Some("2026-04-01T00:00:00Z")),
            dated("new", "2026-09-01T00:00:00Z", None),
        ])
        .unwrap();
    let relevance = index.search(&q("office")).unwrap();
    assert_eq!(
        relevance.hits[0].id, "strong",
        "the exact title wins on relevance"
    );

    let mut newest = q("office");
    newest.order = SearchOrder::Newest;
    assert_eq!(
        ids(&index.search(&newest).unwrap()),
        vec!["new", "mid", "late", "strong"]
    );
    let mut oldest = q("office");
    oldest.order = SearchOrder::Oldest;
    assert_eq!(
        ids(&index.search(&oldest).unwrap()),
        vec!["strong", "late", "mid", "new"]
    );
    newest.limit = 2;
    assert_eq!(ids(&index.search(&newest).unwrap()), vec!["new", "mid"]);
}

/// Design 09 §13.1: a search page reaches 50 hits.
#[test]
fn the_limit_reaches_fifty() {
    assert_eq!(MAX_LIMIT, 50);
    let index = ready_index();
    let records: Vec<IndexRecord> = (0..60)
        .map(|i| {
            record(
                &format!("n{i:02}"),
                &format!("Parking {i}"),
                "Parking level.\n",
            )
        })
        .collect();
    index.upsert_many(&records).unwrap();
    let mut query = q("parking");
    query.limit = 50;
    assert_eq!(index.search(&query).unwrap().count, 50);
    query.limit = 51;
    assert!(matches!(
        index.search(&query).unwrap_err(),
        IndexError::InvalidInput(m) if m.contains("limit")
    ));
}

/// Design 09 §13.1: a historical hit whose window has ended says so, on the
/// index clock; the same note is not ended when evaluated inside it.
#[test]
fn a_hit_says_ended_on_the_index_clock() {
    let now = instant("2026-09-24T12:00:00Z");
    let index = MemoryIndex::open_in_memory()
        .unwrap()
        .with_clock(move || now);
    index.rebuild(&[]).unwrap();
    let mut over = dated("over", "2026-01-01T00:00:00Z", None);
    over.valid_to = Some("2026-06-01T00:00:00Z".into());
    let mut running = dated("running", "2026-01-01T00:00:00Z", None);
    running.valid_to = Some("2026-12-01T00:00:00Z".into());
    index.upsert_many(&[over, running]).unwrap();
    let mut historical = q("office");
    historical.historical = true;
    let r = index.search(&historical).unwrap();
    assert!(hit(&r, "over").ended);
    assert!(!hit(&r, "running").ended);
    let mut then = q("office");
    then.as_of = Some(instant("2026-05-01T00:00:00Z"));
    assert!(!hit(&index.search(&then).unwrap(), "over").ended);
}

/// Schema 3 adds the note's dates. A cache written at schema 2 is recreated
/// on open and reports `rebuilding` until a rebuild has run.
#[test]
fn an_index_at_schema_2_is_rebuilt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index.rebuild(&[record("n1", "One", "one")]).unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '2' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0, "nothing of the v2 cache is served");
    let conn = index.lock();
    for column in ["date", "created_at", "observed_at"] {
        let present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('note') WHERE name = ?1)",
                [column],
                |r| r.get(0),
            )
            .unwrap();
        assert!(present, "note.{column}");
    }
}

/// Schema 4 (design 09 §6.5, LQ17) narrows the trigram index and keeps one
/// copy of the text: a cache written at schema 3 is recreated on open —
/// nothing of it is served — and the fresh one has the new tables: the
/// header trigram table, `chunk` without its copies, and three contentless
/// full-text tables.
#[test]
fn an_index_at_schema_3_is_rebuilt_with_the_revised_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index.rebuild(&[record("n1", "One", "one")]).unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '3' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0, "nothing of the v3 cache is served");
    let conn = index.lock();
    let column = |table: &str, column: &str| -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            [table, column],
            |r| r.get(0),
        )
        .unwrap()
    };
    for gone in ["excerpt", "title", "aliases", "tags"] {
        assert!(!column("chunk", gone), "chunk.{gone} is gone");
    }
    assert!(column("chunk", "body"));
    assert!(column("note", "norm_aliases") && column("note", "norm_tags"));
    for table in ["fts_word", "fts_tri_head", "fts_tri"] {
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [table],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            sql.contains("content=''") && sql.contains("contentless_delete=1"),
            "{table} is contentless: {sql}"
        );
    }
}

/// Repair round 1 of LQ17 widens `query::is_unsegmented` (design 09
/// §6.5.1), which decides what `fts_tri` and the script split hold: a cache
/// written at schema 4 — a Devanagari body kept out of `fts_tri` — is
/// recreated, never served.
#[test]
fn an_index_at_schema_4_is_rebuilt_for_the_widened_scripts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index
            .rebuild(&[record("n1", "Weekend", "हर शनिवार दोस्तों के साथ।\n")])
            .unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '4' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0, "nothing of the v4 cache is served");
    index
        .rebuild(&[record("n1", "Weekend", "हर शनिवार दोस्तों के साथ।\n")])
        .unwrap();
    let conn = index.lock();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM fts_tri", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "a Devanagari body is in the trigram table");
}

/// R21 (storage design): every project partition's key changed from its
/// directory's hash to its project id's, so a cache written at schema 5 —
/// rows under the old keys — is recreated and rebuilt from the notes, never
/// served.
#[test]
fn an_index_at_schema_5_is_rebuilt_for_the_project_id_keys() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index
            .rebuild(&[record(
                "n1",
                "Release step",
                "Tag, then run the release script.\n",
            )])
            .unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '5' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, SCHEMA_VERSION);
    assert_eq!(report.notes, 0, "nothing of the v5 cache is served");
}

/// The FRESHNESS-SWEEP's S12: unicode-normalization 0.1.26 reads Unicode
/// 18, and `query::normalize` folds its subscript `w` (U+209D) to `w`, so a
/// row a schema-6 cache holds for a body carrying it is not what the same
/// bytes give now: the cache is recreated, never served, and the rebuilt
/// row is found by the folded word.
#[test]
fn an_index_at_schema_6_is_rebuilt_for_unicode_18() {
    let note = || record("n1", "Release step", "Flip the x\u{209D}y switch.\n");
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    {
        let index = MemoryIndex::open(&path).unwrap();
        index.rebuild(&[note()]).unwrap();
        let conn = index.lock();
        conn.execute(
            "UPDATE meta SET value = '6' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    let report = index.index_state().unwrap();
    assert_eq!(SCHEMA_VERSION, 7);
    assert_eq!(report.state, IndexState::Rebuilding);
    assert_eq!(report.schema_version, 7);
    assert_eq!(report.notes, 0, "nothing of the v6 cache is served");
    index.rebuild(&[note()]).unwrap();
    assert_eq!(query::normalize("x\u{209D}y"), "xwy");
    let r = index.search(&q("xwy")).unwrap();
    assert_eq!(ids(&r), vec!["n1"]);
}

/// Five dated notes a day apart, one per status the timeline shows or hides.
fn timeline_notes(index: &MemoryIndex) {
    let mut superseded = dated("d2", "2026-06-02T00:00:00Z", None);
    superseded.effective_status = EffectiveStatus::Superseded;
    superseded.superseded_by = vec!["d4".into()];
    let mut candidate = dated("d3", "2026-06-03T00:00:00Z", None);
    candidate.status = "candidate".into();
    candidate.effective_status = EffectiveStatus::Candidate;
    let mut archived = dated("d5", "2026-06-05T00:00:00Z", None);
    archived.status = "archived".into();
    archived.effective_status = EffectiveStatus::Archived;
    let mut ended = dated("d6", "2026-06-06T00:00:00Z", None);
    ended.valid_to = Some("2026-07-01T00:00:00Z".into());
    let mut elsewhere = dated("d7", "2026-06-07T00:00:00Z", None);
    elsewhere.partition_key = "home-other".into();
    index
        .upsert_many(&[
            dated("d1", "2026-06-01T00:00:00Z", None),
            superseded,
            candidate,
            // Written in September about the fourth of June.
            dated("d4", "2026-09-01T00:00:00Z", Some("2026-06-04T00:00:00Z")),
            archived,
            ended,
            elsewhere,
            dated("d8", "2026-06-08T00:00:00Z", None),
        ])
        .unwrap();
}

fn row_ids(rows: &[TimelineRow]) -> Vec<&str> {
    rows.iter().map(|r| r.id.as_str()).collect()
}

/// Design 09 §13.1: around a note, the notes dated before it and after it,
/// each side in time order; superseded, archived and ended notes are listed
/// and marked, candidates and other partitions are not.
#[test]
fn a_timeline_around_a_note_lists_its_neighbours_in_time_order() {
    let now = instant("2026-09-24T12:00:00Z");
    let index = MemoryIndex::open_in_memory()
        .unwrap()
        .with_clock(move || now);
    index.rebuild(&[]).unwrap();
    timeline_notes(&index);
    let t = index
        .timeline(&TimelineQuery {
            partition_keys: vec!["home-p".into()],
            point: TimelinePoint::Note {
                partition_key: "home-p".into(),
                id: "d4".into(),
            },
            before: 5,
            after: 5,
        })
        .unwrap();
    let anchor = t.anchor.as_ref().expect("the anchor is a timeline note");
    assert_eq!(anchor.id, "d4");
    assert_eq!(anchor.date, "2026-06-04T00:00:00Z");
    assert_eq!(row_ids(&t.before), vec!["d1", "d2"], "no candidate");
    assert_eq!(
        row_ids(&t.after),
        vec!["d5", "d6", "d8"],
        "no other partition"
    );
    let by = |id: &str| {
        t.before
            .iter()
            .chain(t.after.iter())
            .find(|r| r.id == id)
            .unwrap()
    };
    assert_eq!(by("d2").effective_status, EffectiveStatus::Superseded);
    assert_eq!(by("d5").effective_status, EffectiveStatus::Archived);
    assert!(by("d6").ended, "a window that ended by the index clock");
    assert!(!by("d8").ended);
    assert_eq!(t.index_state, IndexState::Ready);

    // `before` and `after` take the nearest ones.
    let t = index
        .timeline(&TimelineQuery {
            partition_keys: vec!["home-p".into()],
            point: TimelinePoint::Note {
                partition_key: "home-p".into(),
                id: "d4".into(),
            },
            before: 1,
            after: 2,
        })
        .unwrap();
    assert_eq!(row_ids(&t.before), vec!["d2"]);
    assert_eq!(row_ids(&t.after), vec!["d5", "d6"]);
}

/// Design 09 §13.1: around an instant, `before` notes dated earlier and
/// `after` notes dated at it or later; a candidate or unknown anchor note
/// has no row, and no neighbours are listed around nothing.
#[test]
fn a_timeline_around_an_instant_and_around_no_note() {
    let index = ready_index();
    timeline_notes(&index);
    let t = index
        .timeline(&TimelineQuery {
            partition_keys: vec!["home-p".into(), "home-other".into()],
            point: TimelinePoint::Instant(instant("2026-06-05T00:00:00Z")),
            before: 2,
            after: 20,
        })
        .unwrap();
    assert!(t.anchor.is_none());
    assert_eq!(row_ids(&t.before), vec!["d2", "d4"]);
    assert_eq!(row_ids(&t.after), vec!["d5", "d6", "d7", "d8"]);

    for id in ["d3", "nope"] {
        let t = index
            .timeline(&TimelineQuery {
                partition_keys: vec!["home-p".into()],
                point: TimelinePoint::Note {
                    partition_key: "home-p".into(),
                    id: id.into(),
                },
                before: 5,
                after: 5,
            })
            .unwrap();
        assert!(t.anchor.is_none(), "{id}");
        assert!(t.before.is_empty() && t.after.is_empty(), "{id}");
    }
}

// ============================================================================
// Scale slice S6: no integrity check at open; the file scan while rebuilding
// ============================================================================

/// Open reads the catalog and the schema version, and no more: a cache
/// damaged below its catalog opens as it recorded itself — `quick_check`
/// read the whole file at every boot (2.4 s at 100K notes) — and the first
/// use of a damaged page reports corruption, which is what a caller
/// rebuilds on.
#[test]
fn a_cache_damaged_below_its_catalog_opens_and_says_corrupt_on_use() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    let (root, page_size) = {
        let index = MemoryIndex::open(&path).unwrap();
        let records: Vec<IndexRecord> = (0..40)
            .map(|i| record(&format!("n{i}"), &format!("Note {i}"), "Some body words.\n"))
            .collect();
        index.rebuild(&records).unwrap();
        let conn = index.lock();
        let root: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = 'note'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
        (root, page_size)
    };
    // Scribble over the `note` table's root page.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(((root - 1) * page_size) as u64))
            .unwrap();
        file.write_all(&vec![0xAB; page_size as usize]).unwrap();
    }
    let index = MemoryIndex::open(&path).unwrap();
    assert_eq!(
        index.state().unwrap(),
        IndexState::Ready,
        "opened as it recorded itself: no integrity check"
    );
    let err = index
        .reconcile("home-p", || Ok(Vec::new()))
        .expect_err("the note table's page is damaged");
    assert!(err.is_corruption(), "{err}");
    assert!(!IndexError::InvalidInput("x".into()).is_corruption());
}

/// A whole rebuild starts from a new, empty file — no free pages left
/// behind, no damaged page read — and indexes in batches.
#[test]
fn a_rebuild_starts_from_a_new_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&path).unwrap();
    let big: Vec<IndexRecord> = (0..600)
        .map(|i| record(&format!("b{i}"), &format!("Big {i}"), &"word ".repeat(400)))
        .collect();
    index.rebuild(&big).unwrap();
    let pages = |index: &MemoryIndex| -> i64 {
        index
            .lock()
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap()
    };
    let before = pages(&index);
    index.rebuild(&[record("s", "Small", "tiny")]).unwrap();
    assert!(
        pages(&index) * 4 < before,
        "the file shrank with its contents: {} pages of {before}",
        pages(&index)
    );
    let state = index.index_state().unwrap();
    assert_eq!(state.state, IndexState::Ready);
    assert_eq!(state.notes, 1);
    assert_eq!(index.state().unwrap(), IndexState::Ready);
}

/// The file scan a search falls back to while the index is rebuilt answers
/// with the index's own filters and evidence: the same notes and reasons,
/// for word, word-beginning, header-substring, glued-word, CJK, literal,
/// short CJK, exact-title, glued-query and Devanagari queries — and nothing
/// for a term inside a body word — on both filter sets and with the
/// historical, kind, validity and date switches (design 09 §6.5.3). It says
/// `rebuilding`, and `partial` when the caller left notes out.
#[test]
fn the_file_scan_finds_what_the_index_finds() {
    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let now = at("2026-09-26T12:00:00Z");
    let index = MemoryIndex::open_in_memory().unwrap().with_clock(move || now);
    let mut records = vec![
        record("vim", "Editor preference", "The user prefers vim for editing.\n"),
        record("tabs", "Indentation", "Tabs, never spaces; `rustfmt::skip` is fine.\n"),
        record("tea", "Afternoon drink", "简洁的回答 and green tea.\n"),
        record("old", "Old editor", "The user used emacs.\n"),
        record("loser", "Editor key loser", "Nano for editing.\n"),
        record("forgot", "Forgotten source", "Editing notes from a forgotten row.\n"),
        record("summary", "Conversation summary", "We talked about editing.\n"),
        record("ended", "Ended plan", "Editing sprint that is over.\n"),
        record("soon", "Upcoming plan", "Editing sprint next month.\n"),
        record("other", "Other partition", "Editing elsewhere.\n"),
        record("glued", "发版 checklist", "发版前先看一遍commit 记录。\n## Knowledge\n周五不发版。\n"),
        record("espresso", "Espresso machine", "Descale the espresso machine monthly; `memory_files` too.\n"),
        record("spaced", "Runtime", "Python 版本: 3.12\n"),
        record("hindi", "Weekend", "हर शनिवार दोस्तों के साथ क्रिकेट।\n"),
    ];
    records[3].effective_status = EffectiveStatus::Superseded;
    records[4].same_key_loser = true;
    records[4].conflict = true;
    records[5].source_forgotten = true;
    records[6].kind = "episode".into();
    records[6].origin = "extracted_other".into();
    records[7].valid_to = Some("2026-09-01T00:00:00Z".into());
    records[8].valid_from = Some("2026-10-01T00:00:00Z".into());
    records[9].partition_key = "home-q".into();
    records[1].kind = "preference".into();
    records[2].observed_at = Some("2026-01-05T00:00:00Z".into());
    index.rebuild(&records).unwrap();

    let keys = vec!["home-p".to_string()];
    let mut queries: Vec<SearchQuery> = Vec::new();
    for text in [
        "editing",
        "prefer",
        "rustfmt::skip",
        "简洁",
        "Editor preference",
        "sprint",
        "green tea",
        "nothing matches this",
        "commit",
        "now",
        "presso",
        "check",
        "发版前",
        "memory_file",
        "machine descale",
        "python版本",
        "दोस्त",
        "शनिवार",
    ] {
        for mode in [SearchMode::Explicit, SearchMode::Automatic] {
            let mut q = SearchQuery::new(text, keys.clone());
            q.limit = MAX_LIMIT;
            q.mode = mode;
            queries.push(q.clone());
            q.historical = true;
            queries.push(q.clone());
            q.historical = false;
            q.as_of = Some(at("2026-08-01T00:00:00Z"));
            queries.push(q.clone());
            q.as_of = None;
            q.kind = Some("preference".into());
            queries.push(q.clone());
            q.kind = None;
            q.date_from = Some(at("2026-09-01T00:00:00Z"));
            queries.push(q);
        }
    }
    let mut found = 0;
    for q in &queries {
        let from_index = index.search(q).unwrap();
        found += from_index.hits.len();
        let from_files = index.scan_search(&records, q, false).unwrap();
        let mut a: Vec<&str> = ids(&from_index);
        let mut b: Vec<&str> = ids(&from_files);
        a.sort();
        b.sort();
        assert_eq!(a, b, "{q:?}");
        assert_eq!(from_files.index_state, IndexState::Rebuilding);
        assert!(!from_files.partial);
        for hit in &from_files.hits {
            let twin = from_index.hits.iter().find(|h| h.id == hit.id).unwrap();
            assert_eq!(hit.matches, twin.matches, "{q:?} {}", hit.id);
            assert_eq!(hit.chunk_ordinal, twin.chunk_ordinal);
            assert_eq!(hit.upcoming, twin.upcoming);
            assert_eq!(hit.same_key_loser, twin.same_key_loser);
        }
    }
    assert!(found > 60, "the queries exercise the filters ({found} hits)");
    // The beginning of a word is found as a prefix, below whole tokens; a
    // term inside a body word is not found, in a title it is.
    let q = SearchQuery::new("prefer", keys.clone());
    let hits = index.scan_search(&records, &q, true).unwrap();
    assert_eq!(ids(&hits), ["vim"]);
    assert_eq!(hits.hits[0].matches, [MatchReason::Prefix]);
    assert!(hits.partial, "a caller's bound is reported");
    for (text, want) in [("now", vec![]), ("presso", vec!["espresso"]), ("check", vec!["glued"])] {
        let q = SearchQuery::new(text, keys.clone());
        let from_files = index.scan_search(&records, &q, false).unwrap();
        assert_eq!(ids(&from_files), want, "{text}");
    }

    // Pinned notes, as the index lists them.
    records[0].pinned = true;
    records[2].pinned = true;
    records[2].updated_at = "2026-09-24T10:00:00Z".into();
    records[7].pinned = true;
    records[9].pinned = true;
    index.rebuild(&records).unwrap();
    assert_eq!(
        index.scan_pinned(&records, &keys),
        index.list_pinned(&keys).unwrap()
    );
}

// ============================================================================
// Scale slice S5: the index is maintained
// ============================================================================

fn wal_bytes(path: &std::path::Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::metadata(std::path::PathBuf::from(wal))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Bulky notes, every one carrying a Chinese sentence, so all three
/// full-text tables take rows (design 09 §6.5.1).
fn bulky(prefix: &str, n: usize, partition: &str) -> Vec<IndexRecord> {
    (0..n)
        .map(|i| {
            let mut r = record(
                &format!("{prefix}{i}"),
                &format!("Bulky {prefix} {i}"),
                &format!(
                    "{} {i}\n\n用户每天早上喝一杯咖啡 {i}\n",
                    "lorem ipsum dolor sit amet ".repeat(60)
                ),
            );
            r.partition_key = partition.to_string();
            r.relative_path = format!("notes/{prefix}{i}--bulky.md");
            r
        })
        .collect()
}

/// A full replace of a partition — the one bulk write left outside a
/// rebuild — ends like a rebuild: its FTS indexes merged and its WAL folded
/// back and truncated, where it used to leave a WAL as large as everything
/// it wrote (896 MiB after a first tick at 100K notes).
#[test]
fn a_full_replace_leaves_the_wal_truncated() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&path).unwrap();
    index.rebuild(&[]).unwrap();
    index
        .replace_partition("home-p", &bulky("r", 300, "home-p"))
        .unwrap();
    assert_eq!(wal_bytes(&path), 0, "the WAL is truncated");
    assert_eq!(index.index_state().unwrap().notes, 300);
}

/// How many segments each full-text index holds, `(fts_word, fts_tri_head,
/// fts_tri)`: one after a rebuild's `optimize`, more after later writes.
fn fts_segments(index: &MemoryIndex) -> (i64, i64, i64) {
    let conn = index.lock();
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

/// Maintenance merges the FTS segments writes left, folds the WAL back and
/// truncates it, and says how the file's size compares with its size after
/// the last rebuild.
#[test]
fn maintenance_merges_truncates_and_reports_the_growth() {
    let dir = tempdir().unwrap();
    let path = dir.path().join(INDEX_FILE_NAME);
    let index = MemoryIndex::open(&path).unwrap();
    index.rebuild(&bulky("a", 20, "home-p")).unwrap();
    let rebuilt = index.maintain(std::time::Duration::from_secs(5)).unwrap();
    assert!(rebuilt.rebuilt_bytes > 0);
    assert_eq!(rebuilt.bytes, rebuilt.rebuilt_bytes, "nothing written since");
    let writes = index.writes();
    for round in 0..6 {
        for r in bulky("a", 20, "home-p").iter_mut() {
            r.body = format!("{} round {round}\n", r.body);
            r.revision = format!("{:0>64}", round + 1);
            index.upsert(r).unwrap();
        }
    }
    index.upsert_many(&bulky("b", 60, "home-p")).unwrap();
    // Contentless-delete tables fold a replacement's delete into the old
    // segment as a tombstone and merge a segment most of whose rows are
    // gone, so the replacements alone can leave one segment; notes added in
    // their own transactions leave one new segment each.
    for batch in 0..3 {
        index
            .upsert_many(&bulky(&format!("c{batch}-"), 5, "home-p"))
            .unwrap();
    }
    assert!(index.writes() > writes, "writes are counted");
    assert!(wal_bytes(&path) > 0);
    let (word, head, tri) = fts_segments(&index);
    assert!(
        word > 1 && head > 1 && tri > 1,
        "the writes left segments to merge: {word}, {head}, {tri}"
    );
    let after = index.maintain(std::time::Duration::from_secs(5)).unwrap();
    assert!(after.merge_steps > 0, "the merge did work: {after:?}");
    assert_eq!(
        fts_segments(&index),
        (1, 1, 1),
        "each full-text index is one segment again"
    );
    assert_eq!(wal_bytes(&path), 0, "the WAL is truncated");
    assert!(after.bytes > rebuilt.rebuilt_bytes, "{after:?}");
    assert_eq!(after.rebuilt_bytes, rebuilt.rebuilt_bytes);
    assert_eq!(index.index_state().unwrap().notes, 95);
    // A search still finds what the writes left.
    let found = index.search(&q("round 5")).unwrap();
    assert!(!found.hits.is_empty());
}


/// The L8 close-out's B1: a word held whole by **exactly** the cap's number
/// of bodies is added at the cap (§6.5.2: "only when there are at most
/// 256") — every top hit then holds `work` whole. At `< CANDIDATE_CAP` it
/// would add none, and the top hits would be the `worker` titles.
#[test]
fn a_body_word_held_by_exactly_the_cap_is_added_at_the_cap() {
    let index = ready_index();
    let mut records: Vec<IndexRecord> = (0..CANDIDATE_CAP + 44)
        .map(|i| {
            record(
                &format!("w{i:03}"),
                &format!("Worker rota {i}"),
                "The worker rota lives in the shared workspace; workers swap shifts.\n",
            )
        })
        .collect();
    for i in 0..CANDIDATE_CAP {
        records.push(record(
            &format!("c{i:03}"),
            &format!("Day {i}"),
            "Today I rode along the river to work.\n",
        ));
    }
    index.rebuild(&records).unwrap();
    let mut sq = q("where do I work?");
    sq.limit = MAX_LIMIT;
    let r = index.search(&sq).unwrap();
    assert!(r.partial, "the word table's query filled its cap");
    assert_eq!(r.count, MAX_LIMIT);
    for h in &r.hits {
        assert!(h.id.starts_with('c'), "{}: every top hit holds `work` whole", h.id);
        assert_eq!(h.matches, vec![MatchReason::Term], "{}", h.id);
    }
}

/// A body long enough that the chunker cuts at the next heading, ending in
/// `last`.
fn long_section(last: &str) -> String {
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!(
            "Line {i} of filler text that says nothing in particular here. "
        ));
    }
    s.push_str(last);
    s.push('\n');
    s
}

/// The L8 close-out's B1 (design 09 §6.5.3: "a word matched as the beginning
/// of such a token, and nowhere as a whole token, reports `prefix`"): a note
/// holding `prefer` whole in one chunk and only as the beginning of
/// `prefers` in another reports `term` alone — in the index and in the file
/// scan. The chunk-level evidence says `prefix` in one chunk; the
/// note-level rule drops it, whichever chunk comes first (the close-out's
/// repair round 1 added the order with the whole word first, which pins
/// the prefix branch's own check).
#[test]
fn a_word_whole_in_one_chunk_drops_prefix_from_another() {
    let (beginning, whole) = (
        "The user prefers vim.",
        "The user said prefer, exactly that word.",
    );
    for (one, two) in [(beginning, whole), (whole, beginning)] {
        let index = ready_index();
        let body = format!(
            "# Editor\n\n## One\n{}\n## Two\n{}",
            long_section(one),
            long_section(two)
        );
        assert!(chunker::chunk_body(&body).len() >= 2, "two chunks");
        let records = vec![record("two", "Editor", &body)];
        index.rebuild(&records).unwrap();
        let r = index.search(&q("prefer")).unwrap();
        assert_eq!(hit(&r, "two").matches, vec![MatchReason::Term], "{one}");
        let scan = index.scan_search(&records, &q("prefer"), false).unwrap();
        assert_eq!(hit(&scan, "two").matches, vec![MatchReason::Term], "{one}");
    }
}

/// The L8 close-out's B1 (design 09 §6.5.2, repair round 2 of LQ17): the
/// word table's probes of a literal are split at script boundaries, as the
/// word table indexes text — so `看一遍commit-log` counts as found in a note
/// holding it spaced (`看一遍 commit-log`), and its part `log` is not
/// searched to drag in a decoy. Unsplit, the index and the file scan
/// disagree.
#[test]
fn a_glued_literal_found_spaced_hides_its_parts() {
    let literal = query::parse_query("看一遍commit-log").unwrap().literals[0].clone();
    // Split, and exact: a term carrying an unsegmented character is never a
    // prefix.
    assert_eq!(
        literal.word_probes(),
        ("\"看一遍 commit-log\"".to_string(), None)
    );
    let index = ready_index();
    let records = vec![
        record("spaced", "发版", "每次 看一遍 commit-log 再发版。\n"),
        record("decoy", "Deploy", "The deploy log is kept for a week.\n"),
    ];
    index.rebuild(&records).unwrap();
    let r = index.search(&q("看一遍commit-log")).unwrap();
    assert_eq!(ids(&r), vec!["spaced"]);
    let scan = index
        .scan_search(&records, &q("看一遍commit-log"), false)
        .unwrap();
    assert_eq!(ids(&scan), vec!["spaced"]);
}
