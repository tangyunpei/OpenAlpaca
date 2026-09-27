//! The invariant fixtures (`tests/fixtures/memory/*.json`, README there),
//! driven through the index alone — the slice-2 reading of the README's
//! *Slice 2 has no service*: `search` is the index query with the context's
//! partitions pre-resolved, `recall` the same with the automatic-path filter
//! set, `erase_cache`/`restart` the cache file deleted and the index reopened
//! and rebuilt. Steps the index cannot run are skipped by name when they read
//! (`read`, `list`, `doctor`) and **stop the file** when they mutate through
//! `memory_files` (`store`, `update`, `forget`, `external_*`, `scan`), since a
//! later expectation may depend on them.
//!
//! The fields the index takes pre-computed — `effective_status`, `conflict`,
//! `same_key_loser`, `source_forgotten`, `superseded_by` — are derived here
//! from the fixture's notes and snapshot by design §6's rules (same
//! partition, at-least-equal authority, not a candidate; tombstones count) and
//! §3 rule 3 (same `fact_key`: highest authority, then newest). This is a
//! test-side stand-in for `memory_files`' computation, kept minimal; the
//! revision is a stand-in too (a hash over id and body), since no step below
//! reads one. Nothing here is the design's authority on either.
//!
//! An `excerpt_contains` expectation is read from the note's own body, at
//! the chunk the hit names — as the service reads it from the re-read file
//! (`memory::service::recall`, `chunk_at`): the index keeps the text only
//! normalized, for ranking, and serves no excerpt (design 09 §6.5.1).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

const FIXTURE_DIR: &str = "tests/fixtures/memory";

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(FIXTURE_DIR)
        .join(name)
}

fn owner_key(owner_id: &str) -> String {
    let digest = Sha256::digest(owner_id.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..32].to_string()
}

fn dir_key(name: &str) -> String {
    let digest = Sha256::digest(name.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

fn authority(origin: &str) -> i64 {
    authority_rank(origin)
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

struct Partition {
    key: String,
    scope: String,
    owner: String,
    workspace: Option<String>,
}

/// One fixture, loaded: its records and the partitions each context sees.
struct Loaded {
    records: Vec<IndexRecord>,
    partitions: Vec<Partition>,
    contexts: HashMap<String, Vec<String>>,
    operations: Vec<Value>,
}

fn load(name: &str) -> Loaded {
    let text = std::fs::read_to_string(fixture_path(name)).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    assert!(
        str_list(doc.get("loads_in")).contains(&"slice-2".to_string()),
        "{name} is not a slice-2 fixture"
    );
    // The fixture's owner keys are the design's derivation; check it.
    for owner in doc["owners"].as_array().unwrap() {
        assert_eq!(
            owner_key(owner["id"].as_str().unwrap()),
            owner["owner_key"].as_str().unwrap(),
            "owner key derivation"
        );
    }

    let mut partitions = Vec::new();
    let mut records = Vec::new();
    for part in doc["partitions"].as_array().unwrap() {
        if part.get("lazy").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let owner = part["owner"].as_str().unwrap().to_string();
        let okey = owner_key(&owner);
        let scope_word = part["scope"].as_str().unwrap();
        let workspace = part
            .get("workspace")
            .and_then(Value::as_str)
            .map(str::to_string);
        let (key, scope) = match scope_word {
            "home" => (format!("home-{okey}"), "global"),
            "project" => (
                format!("proj-{okey}-{}", dir_key(workspace.as_deref().unwrap())),
                "project",
            ),
            other => panic!("scope {other}"),
        };
        let notes: Vec<&Value> = part
            .get("notes")
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        let tombstones: Vec<&Value> = part
            .get("snapshot")
            .and_then(|s| s.get("tombstones"))
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        records.extend(derive_records(&key, scope, &notes, &tombstones));
        partitions.push(Partition {
            key,
            scope: scope.to_string(),
            owner,
            workspace,
        });
    }

    let mut contexts = HashMap::new();
    if let Some(ctxs) = doc.get("contexts").and_then(Value::as_object) {
        for (name, ctx) in ctxs {
            let owner = ctx["owner_id"].as_str().unwrap_or("");
            let workspace = ctx.get("workspace").and_then(Value::as_str);
            let ws_name = workspace.and_then(|w| w.strip_prefix("ws:"));
            let keys: Vec<String> = partitions
                .iter()
                .filter(|p| p.owner == owner)
                .filter(|p| {
                    p.scope == "global" || (ws_name.is_some() && p.workspace.as_deref() == ws_name)
                })
                .map(|p| p.key.clone())
                .collect();
            contexts.insert(name.clone(), keys);
        }
    }

    Loaded {
        records,
        partitions,
        contexts,
        operations: doc["operations"].as_array().unwrap().clone(),
    }
}

/// Design §6 derived supersession and §3 rule 3, over one partition.
fn derive_records(
    key: &str,
    scope: &str,
    notes: &[&Value],
    tombstones: &[&Value],
) -> Vec<IndexRecord> {
    let ids: HashSet<&str> = notes.iter().map(|n| n["id"].as_str().unwrap()).collect();
    let origin_of: HashMap<&str, &str> = notes
        .iter()
        .map(|n| (n["id"].as_str().unwrap(), n["origin"].as_str().unwrap()))
        .collect();
    let mut superseded_by: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut conflict: HashSet<&str> = HashSet::new();

    for note in notes {
        let id = note["id"].as_str().unwrap();
        if note["status"].as_str() == Some("candidate") {
            continue; // a candidate supersedes nothing
        }
        for target in str_list(note.get("supersedes")) {
            let Some(target_origin) = ids
                .contains(target.as_str())
                .then(|| origin_of[target.as_str()])
            else {
                continue; // another partition: ignored
            };
            if authority(origin_of[id]) >= authority(target_origin) {
                superseded_by
                    .entry(ids.get(target.as_str()).copied().unwrap())
                    .or_default()
                    .push(id.to_string());
            } else {
                conflict.insert(id);
            }
        }
    }
    let mut suppressed: HashSet<String> = HashSet::new();
    for tomb in tombstones {
        let origin = tomb["origin"].as_str().unwrap();
        for target in str_list(tomb.get("supersedes")) {
            if let Some(t) = ids.get(target.as_str()).copied()
                && authority(origin) >= authority(origin_of[t])
            {
                superseded_by
                    .entry(t)
                    .or_default()
                    .push(tomb["id"].as_str().unwrap().to_string());
            }
        }
        suppressed.extend(str_list(tomb.get("suppressed_sources")));
    }

    let effective = |note: &Value| -> EffectiveStatus {
        let id = note["id"].as_str().unwrap();
        match note["status"].as_str().unwrap() {
            "candidate" => EffectiveStatus::Candidate,
            _ if superseded_by.get(id).is_some_and(|v| !v.is_empty()) => {
                EffectiveStatus::Superseded
            }
            "archived" => EffectiveStatus::Archived,
            _ => EffectiveStatus::Active,
        }
    };

    // Same-fact_key losers among the active notes.
    let mut by_key: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for note in notes {
        if effective(note) == EffectiveStatus::Active
            && let Some(k) = note.get("fact_key").and_then(Value::as_str)
        {
            by_key.entry(k).or_default().push(note);
        }
    }
    let mut losers: HashSet<&str> = HashSet::new();
    for group in by_key.values() {
        if group.len() < 2 {
            continue;
        }
        let winner = group
            .iter()
            .max_by_key(|n| {
                (
                    authority(n["origin"].as_str().unwrap()),
                    n["updated_at"].as_str().unwrap().to_string(),
                )
            })
            .unwrap();
        for n in group {
            if n["id"] != winner["id"] {
                losers.insert(n["id"].as_str().unwrap());
            }
        }
    }

    notes
        .iter()
        .map(|note| {
            let id = note["id"].as_str().unwrap();
            let body = note["body"].as_str().unwrap_or("");
            let sources: Vec<String> = note
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|s| {
                            format!(
                                "{}:{}",
                                s["type"].as_str().unwrap(),
                                s["id"].as_str().unwrap()
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let digest = Sha256::digest(format!("{id}\n{body}").as_bytes());
            IndexRecord {
                id: id.to_string(),
                revision: digest.iter().map(|b| format!("{b:02x}")).collect(),
                partition_key: key.to_string(),
                scope: scope.to_string(),
                relative_path: format!("notes/{id}--note.md"),
                title: note["title"].as_str().unwrap().to_string(),
                aliases: str_list(note.get("aliases")),
                tags: str_list(note.get("tags")),
                kind: note["kind"].as_str().unwrap().to_string(),
                status: note["status"].as_str().unwrap().to_string(),
                effective_status: effective(note),
                origin: note["origin"].as_str().unwrap().to_string(),
                fact_key: note
                    .get("fact_key")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                valid_from: note
                    .get("valid_from")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                valid_to: note
                    .get("valid_to")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                updated_at: note["updated_at"].as_str().unwrap().to_string(),
                created_at: note
                    .get("created_at")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| note["updated_at"].as_str().unwrap())
                    .to_string(),
                observed_at: note
                    .get("observed_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                pinned: note.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                source_forgotten: sources.iter().any(|s| suppressed.contains(s)),
                conflict: conflict.contains(id) || losers.contains(id),
                same_key_loser: losers.contains(id),
                superseded_by: superseded_by.get(id).cloned().unwrap_or_default(),
                supersedes: str_list(note.get("supersedes")),
                sources: sources.clone(),
                body: body.to_string(),
                body_line_offset: 0,
                file: FileStat {
                    mtime_ns: 1,
                    size: body.len() as u64,
                    inode: 1,
                },
            }
        })
        .collect()
}

/// What a run did, so a test can assert the file was really exercised.
#[derive(Debug, Default)]
struct RunSummary {
    searched: usize,
    recalled: usize,
    restarted: usize,
    skipped: Vec<String>,
    stopped_at: Option<String>,
}

const READ_ONLY_SERVICE_OPS: &[&str] = &["read", "list", "doctor", "recently_used"];

fn run(name: &str) -> RunSummary {
    let loaded = load(name);
    let home = TempDir::new().unwrap();
    let cache = home
        .path()
        .join("home")
        .join("state")
        .join("cache")
        .join(INDEX_FILE_NAME);
    let mut index = Some(MemoryIndex::open(&cache).unwrap());
    index.as_ref().unwrap().rebuild(&loaded.records).unwrap();
    let mut summary = RunSummary::default();

    for op in &loaded.operations {
        let step = op["step"].to_string();
        let verb = op["op"].as_str().unwrap();
        let input = &op["input"];
        let expect = op.get("expect").cloned().unwrap_or(Value::Null);
        let label = format!("{name} step {step} ({verb})");
        match verb {
            "search" | "recall" => {
                let ctx = op["context"].as_str().unwrap();
                let partitions = loaded.contexts[ctx].clone();
                let query = if verb == "search" {
                    match build_query(input, &partitions, &loaded.partitions) {
                        Some(q) => q,
                        None => {
                            summary
                                .skipped
                                .push(format!("{label}: service-level schema validation"));
                            continue;
                        }
                    }
                } else {
                    let mut q = SearchQuery::new(input["message"].as_str().unwrap(), partitions);
                    q.mode = SearchMode::Automatic;
                    q
                };
                let result = index.as_ref().unwrap().search(&query);
                check_search(&label, result, &expect, &loaded.records);
                if verb == "search" {
                    summary.searched += 1;
                } else {
                    summary.recalled += 1;
                }
            }
            "erase_cache" => {
                index = None;
                std::fs::remove_file(&cache).unwrap();
                for suffix in ["-wal", "-shm"] {
                    let mut p = cache.as_os_str().to_owned();
                    p.push(suffix);
                    let _ = std::fs::remove_file(PathBuf::from(p));
                }
                if let Some(absent) = expect.get("files_absent") {
                    for f in absent.as_array().unwrap() {
                        assert!(
                            f.as_str().unwrap().ends_with(INDEX_FILE_NAME),
                            "{label}: only the cache file is expected absent"
                        );
                        assert!(!cache.exists(), "{label}");
                    }
                }
            }
            "restart" => {
                let reopened = MemoryIndex::open(&cache).unwrap();
                assert_eq!(
                    reopened.index_state().unwrap().state,
                    IndexState::Rebuilding,
                    "{label}: a missing cache reports rebuilding until rebuilt"
                );
                reopened.rebuild(&loaded.records).unwrap();
                if let Some(state) = expect.get("index_state").and_then(Value::as_str) {
                    assert_eq!(
                        reopened.index_state().unwrap().state.as_str(),
                        state,
                        "{label}"
                    );
                }
                if let Some(exists) = expect.get("files_exist") {
                    for f in exists.as_array().unwrap() {
                        if f.as_str().unwrap().ends_with(INDEX_FILE_NAME) {
                            assert!(cache.exists(), "{label}");
                        }
                    }
                }
                index = Some(reopened);
                summary.restarted += 1;
            }
            v if READ_ONLY_SERVICE_OPS.contains(&v) => {
                summary.skipped.push(format!("{label}: slice-3 read"));
            }
            _ => {
                summary.stopped_at = Some(label);
                break;
            }
        }
    }
    summary
}

/// The `memory_search` input as a `SearchQuery`; `None` when the input
/// carries a key the schema does not have (the service refuses those).
fn build_query(input: &Value, partitions: &[String], all: &[Partition]) -> Option<SearchQuery> {
    const KNOWN: &[&str] = &["query", "kind", "scope", "limit", "historical", "as_of"];
    if input
        .as_object()
        .unwrap()
        .keys()
        .any(|k| !KNOWN.contains(&k.as_str()))
    {
        return None;
    }
    let mut keys = partitions.to_vec();
    if let Some(scope) = input.get("scope").and_then(Value::as_str) {
        keys.retain(|k| all.iter().any(|p| &p.key == k && p.scope == scope));
    }
    let mut q = SearchQuery::new(input["query"].as_str().unwrap_or(""), keys);
    q.kind = input
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(limit) = input.get("limit").and_then(Value::as_u64) {
        q.limit = limit as usize;
    }
    q.historical = input
        .get("historical")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    q.as_of = input.get("as_of").and_then(Value::as_str).map(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    });
    Some(q)
}

fn resolve_id(v: &Value) -> String {
    let s = v.as_str().unwrap();
    assert!(!s.starts_with('@'), "unresolvable placeholder {s}");
    s.to_string()
}

fn check_search(
    label: &str,
    result: Result<SearchResults>,
    expect: &Value,
    records: &[IndexRecord],
) {
    if let Some(class) = expect.get("refused").and_then(Value::as_str) {
        assert_eq!(
            class, "invalid_input",
            "{label}: only invalid_input is an index refusal"
        );
        assert!(
            matches!(result, Err(IndexError::InvalidInput(_))),
            "{label}: expected an invalid_input refusal"
        );
        return;
    }
    let result = result.unwrap_or_else(|e| panic!("{label}: {e}"));
    let got: Vec<String> = result.hits.iter().map(|h| h.id.clone()).collect();

    if let Some(ids) = expect.get("ids") {
        let want: Vec<String> = ids.as_array().unwrap().iter().map(resolve_id).collect();
        assert_eq!(got, want, "{label}: ids (ordered)");
    }
    if let Some(ids) = expect.get("ids_unordered") {
        let mut want: Vec<String> = ids.as_array().unwrap().iter().map(resolve_id).collect();
        want.sort();
        let mut have = got.clone();
        have.sort();
        assert_eq!(have, want, "{label}: ids_unordered");
    }
    if let Some(ids) = expect.get("ids_absent") {
        for id in ids.as_array().unwrap().iter().map(resolve_id) {
            assert!(
                !got.contains(&id),
                "{label}: {id} must be absent, got {got:?}"
            );
        }
    }
    let mut allowed: Vec<String> = Vec::new();
    if let Some(ids) = expect.get("ids_include") {
        for id in ids.as_array().unwrap().iter().map(resolve_id) {
            assert!(
                got.contains(&id),
                "{label}: {id} must be present, got {got:?}"
            );
            allowed.push(id);
        }
    }
    if let Some(ids) = expect.get("ids_may_include") {
        allowed.extend(ids.as_array().unwrap().iter().map(resolve_id));
        for id in &got {
            assert!(
                allowed.contains(id),
                "{label}: {id} is not allowed, got {got:?}"
            );
        }
    }
    if let Some(count) = expect.get("count").and_then(Value::as_u64) {
        assert_eq!(result.count as u64, count, "{label}: count");
    }
    if let Some(partial) = expect.get("partial").and_then(Value::as_bool) {
        assert_eq!(result.partial, partial, "{label}: partial");
    }
    if let Some(state) = expect.get("index_state").and_then(Value::as_str) {
        assert_eq!(result.index_state.as_str(), state, "{label}: index_state");
    }
    if let Some(results) = expect.get("results").and_then(Value::as_object) {
        for (id, fields) in results {
            let id = resolve_id(&Value::String(id.clone()));
            let hit = result
                .hits
                .iter()
                .find(|h| h.id == id)
                .unwrap_or_else(|| panic!("{label}: results names {id}, got {got:?}"));
            let record = records
                .iter()
                .find(|r| r.id == hit.id && r.partition_key == hit.partition_key)
                .unwrap_or_else(|| panic!("{label}: no record for {id}"));
            check_hit(&format!("{label} result {id}"), hit, record, fields);
        }
    }
}

/// The text of the chunk a hit names, from the note's body: what the
/// service delivers after re-reading the file.
fn chunk_text(record: &IndexRecord, ordinal: u32) -> String {
    let chunks = chunker::chunk_body(&record.body);
    chunks
        .iter()
        .find(|c| c.ordinal == ordinal)
        .or(chunks.last())
        .map(|c| c.text.clone())
        .unwrap_or_default()
}

fn check_hit(label: &str, hit: &SearchHit, record: &IndexRecord, fields: &Value) {
    for (key, value) in fields.as_object().unwrap() {
        match key.as_str() {
            "match_includes" => {
                let have: Vec<&str> = hit.matches.iter().map(|m| m.as_str()).collect();
                for reason in value.as_array().unwrap() {
                    let reason = reason.as_str().unwrap();
                    assert!(
                        have.contains(&reason),
                        "{label}: match {reason} missing from {have:?}"
                    );
                }
            }
            "conflict" => assert_eq!(hit.conflict, value.as_bool().unwrap(), "{label}: conflict"),
            "status" => assert_eq!(hit.status, value.as_str().unwrap(), "{label}: status"),
            "origin" => assert_eq!(hit.origin, value.as_str().unwrap(), "{label}: origin"),
            "scope" => assert_eq!(hit.scope, value.as_str().unwrap(), "{label}: scope"),
            "excerpt_contains" => {
                let excerpt = chunk_text(record, hit.chunk_ordinal);
                assert!(
                    excerpt.contains(value.as_str().unwrap()),
                    "{label}: excerpt {excerpt:?} (chunk {}) lacks {value:?}",
                    hit.chunk_ordinal
                );
            }
            "superseded_by" => {
                let want: Vec<String> = value.as_array().unwrap().iter().map(resolve_id).collect();
                assert_eq!(hit.superseded_by, want, "{label}: superseded_by");
            }
            "valid_from" => assert_eq!(
                hit.valid_from.as_deref(),
                value.as_str(),
                "{label}: valid_from"
            ),
            "valid_to" => assert_eq!(hit.valid_to.as_deref(), value.as_str(), "{label}: valid_to"),
            // Verification (freshness, not_served_from_cache) and the
            // sources_status derivation are the service's, not the index's.
            "freshness" | "sources_status" => {}
            other => panic!("{label}: unhandled result expectation {other}"),
        }
    }
}

// ============================================================================
// The files
// ============================================================================

#[test]
fn fixture_21_natural_question_recalls_prefer_vim_through_a_word_beginning() {
    let s = run("21-query-natural-question-prefer-vim.json");
    assert_eq!((s.searched, s.recalled), (4, 1), "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_22_literal_and_case_never_reaches_fts_syntax() {
    let s = run("22-query-literal-and.json");
    assert_eq!(s.searched, 20, "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_23_cjk_queries_of_one_two_and_more_characters() {
    let s = run("23-query-cjk-lengths.json");
    assert_eq!(s.searched, 13, "{s:?}");
    assert_eq!(s.skipped.len(), 1, "{s:?}"); // step 12 is a read
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_26_a_word_beginning_in_a_body_and_the_boundary_of_a_body_search() {
    let s = run("26-query-prefix-in-body.json");
    assert_eq!((s.searched, s.recalled), (8, 1), "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_24_forgotten_or_archived_replacement_does_not_revive_its_predecessor() {
    let s = run("24-forgotten-replacement-not-revived.json");
    // Steps 1, 2, 8, 9 search; step 3 carries include_candidates, which the
    // service refuses and the loader skips; 1b recalls; 6–7 erase and restart.
    assert_eq!((s.searched, s.recalled, s.restarted), (4, 1, 1), "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_12_short_cjk_identifier_negation_and_punctuation() {
    let s = run("12-query-handling-cjk-identifier-negation-punctuation.json");
    assert_eq!(s.searched, 17, "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_11_validity_x_before_june_now_y() {
    let s = run("11-validity-time-x-before-june-now-y.json");
    assert_eq!((s.searched, s.recalled), (7, 1), "{s:?}");
    assert!(s.stopped_at.is_none(), "{s:?}");
}

#[test]
fn fixture_10_authority_order_search_steps() {
    let s = run("10-authority-order-correction.json");
    assert_eq!(s.searched, 2, "{s:?}");
    assert!(
        s.stopped_at
            .as_deref()
            .is_some_and(|l| l.contains("step 3 (store)")),
        "{s:?}"
    );
}

#[test]
fn fixture_09_same_phrase_two_owners_search_steps() {
    let s = run("09-same-phrase-two-owners.json");
    assert_eq!(s.searched, 3, "{s:?}");
    assert!(
        s.stopped_at
            .as_deref()
            .is_some_and(|l| l.contains("step 4 (store)")),
        "{s:?}"
    );
}
