//! Live memory-quality eval harness (memory redesign — plan slice 1, §5;
//! rewritten for the new engine by plan commit 3.11).
//!
//! Seeds the gold notes of `scripts/eval/memory/notes-gold.json` through the
//! memory service on the user-directed surface (`MemoryService::store` with a
//! `Surface::UserSurface` context — the call `POST /v1/memories` and
//! `openalpaca memory add` make), asks every question of the held-out set
//! through the front door (`Orchestrator::handle_message`, the entry
//! `POST /v1/chat` uses) and judges each answer against its gold answer with
//! the frozen judge prompt, through the configured model at temperature 0.
//! One system under test per run, chosen by `OPENALPACA_MEMORY_EVAL_SYSTEM`;
//! the report is one JSON file at `OPENALPACA_MEMORY_EVAL_REPORT`, and the
//! paired comparison across two reports is `scripts/eval/memory/compare.py`.
//!
//! This whole file compiles only with `--features live-eval`, and the live
//! test is `#[ignore]`d AND gated on `OPENALPACA_LIVE_EVAL=1`, so plain
//! `cargo test` neither builds nor runs it and it can never spend money or
//! reach a model by accident. Everything it touches on disk is the scratch
//! store `OPENALPACA_HOME_STORE` names, a tempdir, and the report path.
//!
//! Run (see `scripts/eval/memory/README.md` for the full runbook):
//!
//! ```sh
//! OPENALPACA_LIVE_EVAL=1 OPENALPACA_MEMORY_EVAL_SYSTEM=D \
//! OPENALPACA_MEMORY_EVAL_REPORT=/path/to/report-D.json \
//! OPENALPACA_HOME_STORE=/path/to/scratch/store \
//! OPENALPACA_CONFIG_DIR=/path/to/scratch/config \
//!     cargo test -p openalpaca_core --features live-eval \
//!     --test memory_eval -- --ignored memory_eval --nocapture
//! ```
//!
//! Systems: `none` (no memory service — the context-cost reference), `D`
//! (the new service as the daemon runs it: the per-turn recall on, the read
//! tools `memory_search`/`memory_read`/`memory_list` registered the way the
//! daemon registers them, under the main loop's shipped round and tool caps)
//! and `E` (D under design §5's bound — the per-turn recall plus at most two
//! model-directed follow-up searches — expressed with the main loop's
//! existing tool budget; see [`MainLoopBudget`] for the approximation). `A`
//! and `B` measured the engine slice 3 deleted: their reports are archived
//! in `scripts/eval/memory/reports/`, and a D or E report is compared against
//! them with `compare.py` as before — the report schema is unchanged, only
//! added to.
#![cfg(feature = "live-eval")]

use openalpaca_core::daemon_config::RoutingConfig;
use openalpaca_core::memory::service::{
    Attribution, Capabilities, MemoryService, RecallBudget, StoreInput, Surface, UsedReason,
    WorkspaceHint,
};
use openalpaca_storage::memory_files::{Dedup, NoteId, NoteKind, NoteScope};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The prefix of the one owner every note is stored under and every
/// question is asked as. Each run appends its own suffix, so a scratch store
/// reused across runs never lets one run's notes (or the notes a model
/// stored during it) into another's partition.
const OWNER_PREFIX: &str = "memory-eval-owner";
/// The router-side agent id of the judge calls (cost accounting).
const JUDGE_AGENT_ID: &str = "memory_eval_judge";
const JUDGE_MAX_TOKENS: u32 = 400;
const JUDGE_TEMPERATURE: f32 = 0.0;
/// An answer longer than this is cut before it reaches the judge (bounds the
/// judge's cost; the full answer is still in the report).
const JUDGE_ANSWER_CAP_CHARS: usize = 6_000;
/// Unchanged from the slice-1 reports: every field they carry is still here
/// with the same meaning, so `compare.py` pairs a D or E report with the
/// archived A and B ones. New fields are additions only.
const REPORT_SCHEMA: u32 = 1;
/// Design §5: "the running agent may issue up to two revised searches".
const FOLLOW_UP_SEARCHES: usize = 2;
/// The memory tools the main loop offers when a service is installed: the
/// three read tools the daemon registers globally
/// (`apps/openalpacad/src/services/tools.rs`, `memory_read_tools`) and the
/// three writers `main_loop_tool_set` adds per request.
const MEMORY_TOOLS: [&str; 6] = [
    "memory_search",
    "memory_read",
    "memory_list",
    "memory_store",
    "memory_update",
    "memory_forget",
];
/// Tool calls that change memory. A model that stores or edits a note
/// during the run changes what later questions can recall, so every row
/// records its tool calls and the totals count these.
const MEMORY_WRITE_TOOLS: [&str; 3] = ["memory_store", "memory_update", "memory_forget"];

// ─── Data files ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Category {
    ExactRecall,
    Paraphrase,
    CrossLanguage,
    ChangedFactsTime,
    ProjectProcedures,
    RejectedFailedActions,
    Abstention,
    SourceConflicts,
}

impl Category {
    const ALL: [Category; 8] = [
        Category::ExactRecall,
        Category::Paraphrase,
        Category::CrossLanguage,
        Category::ChangedFactsTime,
        Category::ProjectProcedures,
        Category::RejectedFailedActions,
        Category::Abstention,
        Category::SourceConflicts,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Category::ExactRecall => "exact_recall",
            Category::Paraphrase => "paraphrase",
            Category::CrossLanguage => "cross_language",
            Category::ChangedFactsTime => "changed_facts_time",
            Category::ProjectProcedures => "project_procedures",
            Category::RejectedFailedActions => "rejected_failed_actions",
            Category::Abstention => "abstention",
            Category::SourceConflicts => "source_conflicts",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Language {
    En,
    Zh,
    Mixed,
}

impl Language {
    fn as_str(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Zh => "zh",
            Language::Mixed => "mixed",
        }
    }
}

/// One gold note, in the shape of design §4's frontmatter plus the two
/// eval-only fields `project` and `language`.
#[derive(Debug, Clone, Deserialize)]
struct GoldNote {
    id: String,
    title: String,
    kind: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    language: Language,
    #[serde(default)]
    fact_key: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    body: String,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_to: Option<String>,
    #[serde(default)]
    supersedes: Vec<String>,
    created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Question {
    id: String,
    category: Category,
    language: Language,
    question: String,
    gold_answer: String,
    #[serde(default)]
    relies_on: Vec<String>,
    #[serde(default)]
    expect_abstain: bool,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn eval_dir() -> PathBuf {
    repo_root().join("scripts/eval/memory")
}

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Parse a JSON data file and return it with the SHA-256 of its exact bytes.
fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> (T, String) {
    let bytes = read_bytes(path);
    let parsed = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
    (parsed, sha256_hex(&bytes))
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.hyphenated().to_string() == s)
}

fn parse_instant(
    field: &str,
    value: &Option<String>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    value
        .as_deref()
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|t| t.with_timezone(&chrono::Utc))
                .map_err(|e| format!("{field} {v:?} is not RFC 3339: {e}"))
        })
        .transpose()
}

// ─── Seeding through the service ────────────────────────────────────────────

/// The order the gold notes are stored in: the order a user would have told
/// the assistant — oldest `created_at` first, ties in file order — except
/// that a note whose `supersedes` names another waits for it, then goes as
/// early as it can: the service's `supersedes` needs the target's id and
/// revision, which exist only once the target is stored. The service stamps
/// its own `created_at` and `updated_at` at write time, and design §3 rule
/// 3 breaks a same-`fact_key` tie by them, so writing a March note after a
/// June one would make March the newer knowledge. `Err` names a
/// `created_at` that is not RFC 3339 or a link that can never be satisfied
/// (an unknown target, or a cycle).
fn seed_order(notes: &[GoldNote]) -> Result<Vec<usize>, String> {
    let index: HashMap<&str, usize> = notes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();
    for n in notes {
        for target in &n.supersedes {
            if !index.contains_key(target.as_str()) {
                return Err(format!("note {} supersedes unknown note {target}", n.id));
            }
        }
    }
    let mut chronological = Vec::with_capacity(notes.len());
    for (i, n) in notes.iter().enumerate() {
        let created = parse_instant("created_at", &Some(n.created_at.clone()))
            .map_err(|e| format!("note {}: {e}", n.id))?;
        chronological.push((created, i));
    }
    chronological.sort();
    let mut placed = vec![false; notes.len()];
    let mut order = Vec::with_capacity(notes.len());
    while order.len() < notes.len() {
        // The oldest note not yet stored whose targets all are.
        let next = chronological.iter().map(|&(_, i)| i).find(|&i| {
            !placed[i]
                && notes[i]
                    .supersedes
                    .iter()
                    .all(|t| placed[index[t.as_str()]])
        });
        let Some(i) = next else {
            return Err("the supersedes links form a cycle".to_string());
        };
        placed[i] = true;
        order.push(i);
    }
    Ok(order)
}

/// A gold note as the service's store input, every field native (design
/// §7a `memory_store`): kind, scope, aliases, tags, `fact_key`, the validity
/// bounds, and `supersedes` resolved to the stored target's id and current
/// revision. `created_at` is not an input — the service stamps its own —
/// and `project` has no field: the harness has one workspace, so both
/// fictional projects' notes share its partition (see the README).
fn store_input_for(
    note: &GoldNote,
    stored: &BTreeMap<String, (NoteId, String)>,
) -> Result<StoreInput, String> {
    let kind = NoteKind::parse(&note.kind).ok_or_else(|| format!("kind {:?}", note.kind))?;
    let scope = NoteScope::parse(&note.scope).ok_or_else(|| format!("scope {:?}", note.scope))?;
    let supersedes = match note.supersedes.as_slice() {
        [] => None,
        [target] => {
            let (id, revision) = stored
                .get(target)
                .ok_or_else(|| format!("supersedes {target}, which is not stored yet"))?;
            Some((*id, revision.clone()))
        }
        more => {
            return Err(format!(
                "supersedes {} notes; the service's `supersedes` names one",
                more.len()
            ));
        }
    };
    Ok(StoreInput {
        title: note.title.clone(),
        body: note.body.clone(),
        kind: Some(kind),
        scope: Some(scope),
        aliases: note.aliases.clone(),
        tags: note.tags.clone(),
        fact_key: note.fact_key.clone(),
        valid_from: parse_instant("valid_from", &note.valid_from)?,
        valid_to: parse_instant("valid_to", &note.valid_to)?,
        supersedes,
        ..Default::default()
    })
}

/// What seeding did: the gold id → the id the service minted, and the counts
/// the report's conditions carry.
#[derive(Debug, Default)]
struct SeedOutcome {
    ids: BTreeMap<String, String>,
    seeded: usize,
    duplicate: usize,
    failed: usize,
    superseding: usize,
}

fn seed_notes(
    service: &MemoryService,
    owner_id: &str,
    project: &Path,
    notes: &[GoldNote],
) -> SeedOutcome {
    // The user-directed surface: origin `user`, status `active` — gold notes
    // are what the user said, not what a model inferred.
    let ctx = service
        .context(
            owner_id,
            WorkspaceHint::Root(project),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution {
                source: Some("memory_eval".to_string()),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("memory_eval: no memory context for {owner_id}: {e}"));
    let order = seed_order(notes).unwrap_or_else(|e| panic!("memory_eval: {e}"));
    let mut stored: BTreeMap<String, (NoteId, String)> = BTreeMap::new();
    let mut out = SeedOutcome::default();
    for i in order {
        let note = &notes[i];
        let input = match store_input_for(note, &stored) {
            Ok(input) => input,
            Err(e) => {
                out.failed += 1;
                eprintln!("memory_eval: note {}: {e}", note.id);
                continue;
            }
        };
        let superseding = input.supersedes.is_some();
        match service.store(&ctx, input) {
            Ok(created) => {
                if created.dedup == Dedup::Existing {
                    out.duplicate += 1;
                    eprintln!(
                        "memory_eval: note {} is an exact duplicate of {} — not stored twice",
                        note.id, created.id
                    );
                } else {
                    out.seeded += 1;
                    out.superseding += usize::from(superseding);
                }
                out.ids.insert(note.id.clone(), created.id.to_string());
                stored.insert(note.id.clone(), (created.id, created.revision));
            }
            Err(e) => {
                out.failed += 1;
                eprintln!("memory_eval: note {}: store refused: {e}", note.id);
            }
        }
    }
    out
}

/// Always runs under `cargo test --features live-eval` (no model, no
/// network, no keys): the set v0 must load and be structurally sound — and
/// storable by the service as it is — so a bad edit is caught long before a
/// paid run.
#[test]
fn memory_eval_set_v0_is_well_formed() {
    let dir = eval_dir();
    let (notes, _) = load_json::<Vec<GoldNote>>(&dir.join("notes-gold.json"));
    let (heldout, _) = load_json::<Vec<Question>>(&dir.join("questions-heldout.json"));
    let (tuning, _) = load_json::<Vec<Question>>(&dir.join("questions-tuning.json"));
    let judge = String::from_utf8(read_bytes(&dir.join("judge-prompt.md"))).expect("utf-8");

    // Notes: unique UUID ids, known kinds and scopes, a project only on a
    // project note, every supersedes link resolvable, a fact_key in grammar.
    assert!(
        notes.len() >= 60,
        "gold set should hold >= 60 notes (got {})",
        notes.len()
    );
    let mut note_ids = BTreeSet::new();
    for n in &notes {
        assert!(
            is_uuid(&n.id),
            "note {}: id is not a hyphenated lowercase UUID",
            n.id
        );
        assert!(note_ids.insert(n.id.clone()), "duplicate note id {}", n.id);
        assert!(!n.title.trim().is_empty(), "note {}: empty title", n.id);
        assert!(!n.body.trim().is_empty(), "note {}: empty body", n.id);
        assert!(
            NoteKind::parse(&n.kind).is_some(),
            "note {}: kind {}",
            n.id,
            n.kind
        );
        assert!(
            NoteScope::parse(&n.scope).is_some(),
            "note {}: scope {}",
            n.id,
            n.scope
        );
        assert_eq!(
            n.project.is_some(),
            n.scope == "project",
            "note {}: a project is named exactly when scope is project",
            n.id
        );
        assert!(
            !n.created_at.trim().is_empty(),
            "note {}: empty created_at",
            n.id
        );
        parse_instant("created_at", &Some(n.created_at.clone()))
            .unwrap_or_else(|e| panic!("note {}: {e}", n.id));
        if let Some(key) = &n.fact_key {
            let ok = key.len() <= 64
                && key.bytes().enumerate().all(|(i, b)| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || (i > 0 && (b == b'.' || b == b'_' || b == b'-'))
                });
            assert!(
                ok,
                "note {}: fact_key {key:?} is outside the §4 grammar",
                n.id
            );
        }
        assert!(n.aliases.len() <= 16, "note {}: more than 16 aliases", n.id);
        assert!(n.tags.len() <= 16, "note {}: more than 16 tags", n.id);
        parse_instant("valid_from", &n.valid_from).unwrap_or_else(|e| panic!("note {}: {e}", n.id));
        parse_instant("valid_to", &n.valid_to).unwrap_or_else(|e| panic!("note {}: {e}", n.id));
    }
    let by_id: HashMap<&str, &GoldNote> = notes.iter().map(|n| (n.id.as_str(), n)).collect();
    for n in &notes {
        // The service's `supersedes` names one note, in the same partition
        // (design §6), so a gold link must too.
        assert!(
            n.supersedes.len() <= 1,
            "note {}: supersedes more than one note",
            n.id
        );
        for s in &n.supersedes {
            assert!(
                note_ids.contains(s),
                "note {}: supersedes unknown note {s}",
                n.id
            );
            assert_ne!(s, &n.id, "note {}: supersedes itself", n.id);
            let target = by_id[s.as_str()];
            assert_eq!(
                (&target.scope, &target.project),
                (&n.scope, &n.project),
                "note {}: supersedes a note in another scope or project",
                n.id
            );
        }
    }
    for lang in [Language::En, Language::Zh] {
        assert!(
            notes.iter().filter(|n| n.language == lang).count() >= 10,
            "gold set needs >= 10 {} notes",
            lang.as_str()
        );
    }
    // Every note maps to a store input once the notes it supersedes are
    // stored (the ids and revisions here stand in for the service's).
    let order = seed_order(&notes).expect("a seed order exists");
    assert_eq!(order.len(), notes.len());
    // The seed order is the order a user would have told the assistant:
    // oldest `created_at` first, and never a replacement before the note it
    // replaces (in this set every replacement is also the later one).
    for pair in order.windows(2) {
        let (a, b) = (&notes[pair[0]], &notes[pair[1]]);
        let when = |n: &GoldNote| {
            parse_instant("created_at", &Some(n.created_at.clone())).expect("checked above")
        };
        assert!(
            when(a) <= when(b),
            "note {} ({}) is seeded before note {} ({}): not chronological",
            a.id,
            a.created_at,
            b.id,
            b.created_at
        );
    }
    let position: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(at, &i)| (notes[i].id.as_str(), at))
        .collect();
    for n in &notes {
        for target in &n.supersedes {
            assert!(
                position[target.as_str()] < position[n.id.as_str()],
                "note {} is seeded before {target}, the note it supersedes",
                n.id
            );
        }
    }
    let mut stored = BTreeMap::new();
    for i in order {
        store_input_for(&notes[i], &stored).unwrap_or_else(|e| panic!("note {}: {e}", notes[i].id));
        stored.insert(notes[i].id.clone(), (NoteId::new(), "0".repeat(64)));
    }

    // Questions: ids unique across both splits, categories stratified, every
    // language present, abstention rows marked and nothing else.
    let mut seen = BTreeSet::new();
    let held_counts = validate_questions("heldout", &heldout, &note_ids, &mut seen);
    let tune_counts = validate_questions("tuning", &tuning, &note_ids, &mut seen);
    assert!(
        heldout.len() >= 450,
        "held-out set should stay near 470 questions (got {})",
        heldout.len()
    );
    assert!(
        tuning.len() >= 120,
        "tuning set should stay near 150 questions (got {})",
        tuning.len()
    );
    for cat in Category::ALL {
        assert!(
            held_counts.get(&cat).copied().unwrap_or(0) >= 30,
            "held-out needs >= 30 {} questions (got {held_counts:?})",
            cat.as_str()
        );
        assert!(
            tune_counts.get(&cat).copied().unwrap_or(0) >= 8,
            "tuning needs >= 8 {} questions (got {tune_counts:?})",
            cat.as_str()
        );
    }
    for lang in [Language::En, Language::Zh, Language::Mixed] {
        assert!(
            heldout.iter().filter(|q| q.language == lang).count() >= 40,
            "held-out needs >= 40 {} questions",
            lang.as_str()
        );
    }

    // Judge prompt: every placeholder present exactly once, verdict words named.
    for p in JUDGE_PLACEHOLDERS {
        assert_eq!(
            judge.matches(p).count(),
            1,
            "judge prompt must contain {p} exactly once"
        );
    }
    assert!(judge.contains("CORRECT") && judge.contains("INCORRECT"));
}

/// The judge prompt's placeholders. The validation test pins them so a prompt
/// edit cannot silently stop filling one.
const JUDGE_PLACEHOLDERS: [&str; 5] = [
    "{{question}}",
    "{{gold_answer}}",
    "{{answer}}",
    "{{category}}",
    "{{expect_abstain}}",
];

fn validate_questions(
    which: &str,
    questions: &[Question],
    note_ids: &BTreeSet<String>,
    seen: &mut BTreeSet<String>,
) -> BTreeMap<Category, usize> {
    let mut counts = BTreeMap::new();
    for q in questions {
        assert!(!q.id.trim().is_empty(), "{which}: empty question id");
        assert!(
            seen.insert(q.id.clone()),
            "{which}: duplicate question id {} (across both files)",
            q.id
        );
        assert!(
            !q.question.trim().is_empty(),
            "{which}/{}: empty question",
            q.id
        );
        assert!(
            !q.gold_answer.trim().is_empty(),
            "{which}/{}: empty gold answer",
            q.id
        );
        assert_eq!(
            q.expect_abstain,
            q.category == Category::Abstention,
            "{which}/{}: expect_abstain must be true exactly for the abstention category",
            q.id
        );
        if q.category != Category::Abstention {
            assert!(
                !q.relies_on.is_empty(),
                "{which}/{}: a non-abstention question must name the notes it relies on",
                q.id
            );
        }
        for id in &q.relies_on {
            assert!(
                note_ids.contains(id),
                "{which}/{}: relies_on names an unknown note {id}",
                q.id
            );
        }
        *counts.entry(q.category).or_insert(0) += 1;
    }
    counts
}

fn gold(id: &str, supersedes: &[&str]) -> GoldNote {
    GoldNote {
        id: id.to_string(),
        title: format!("title {id}"),
        kind: "fact".to_string(),
        scope: "global".to_string(),
        project: None,
        language: Language::En,
        fact_key: None,
        aliases: vec![],
        tags: vec![],
        body: format!("body {id}"),
        valid_from: None,
        valid_to: None,
        supersedes: supersedes.iter().map(|s| s.to_string()).collect(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

/// A replacement listed before the note it replaces is stored after it; a
/// link to nothing, or a cycle, is an error rather than a silent skip.
#[test]
fn a_superseded_note_is_seeded_before_its_successor() {
    let notes = vec![gold("new", &["old"]), gold("plain", &[]), gold("old", &[])];
    assert_eq!(seed_order(&notes).unwrap(), vec![1, 2, 0]);
    let chain = vec![gold("c", &["b"]), gold("b", &["a"]), gold("a", &[])];
    assert_eq!(seed_order(&chain).unwrap(), vec![2, 1, 0]);
    assert!(seed_order(&[gold("x", &["missing"])]).is_err());
    assert!(seed_order(&[gold("x", &["y"]), gold("y", &["x"])]).is_err());
}

/// Notes are seeded oldest `created_at` first, ties in file order — the
/// order a user would have said them — and a note still waits for the note
/// it supersedes, then goes as early as it can. A `created_at` that is not
/// RFC 3339 is an error, not a guess.
#[test]
fn gold_notes_are_seeded_oldest_first_and_after_what_they_replace() {
    let dated = |id: &str, created_at: &str, supersedes: &[&str]| GoldNote {
        created_at: created_at.to_string(),
        ..gold(id, supersedes)
    };
    // File order June, March, January: seeded January, March, June.
    let notes = vec![
        dated("june", "2026-06-20T10:00:00Z", &[]),
        dated("march", "2026-03-02T10:00:00Z", &[]),
        dated("january", "2026-01-05T08:00:00Z", &[]),
    ];
    assert_eq!(seed_order(&notes).unwrap(), vec![2, 1, 0]);
    // The same instant keeps file order.
    let tie = vec![
        dated("b", "2026-02-01T00:00:00Z", &[]),
        dated("a", "2026-02-01T00:00:00Z", &[]),
    ];
    assert_eq!(seed_order(&tie).unwrap(), vec![0, 1]);
    // A replacement dated before its target waits for it, and no longer.
    let notes = vec![
        dated("early-successor", "2026-01-01T00:00:00Z", &["target"]),
        dated("february", "2026-02-01T00:00:00Z", &[]),
        dated("target", "2026-03-01T00:00:00Z", &[]),
        dated("april", "2026-04-01T00:00:00Z", &[]),
    ];
    assert_eq!(seed_order(&notes).unwrap(), vec![1, 2, 0, 3]);
    assert!(seed_order(&[dated("x", "June 2026", &[])]).is_err());
}

/// Every field the design's `memory_store` takes arrives natively — nothing
/// is flattened into the body the way the old one-string tool needed.
#[test]
fn a_gold_note_maps_to_the_store_input_field_by_field() {
    let mut note = gold("new", &["old"]);
    note.kind = "procedure".to_string();
    note.scope = "project".to_string();
    note.project = Some("moss".to_string());
    note.fact_key = Some("moss.output_dir".to_string());
    note.aliases = vec!["build dir".to_string()];
    note.tags = vec!["moss".to_string()];
    note.valid_from = Some("2026-05-10T00:00:00Z".to_string());
    let old_id = NoteId::new();
    let old_rev = "a".repeat(64);
    let stored = BTreeMap::from([("old".to_string(), (old_id, old_rev.clone()))]);

    let input = store_input_for(&note, &stored).unwrap();
    assert_eq!(input.title, "title new");
    assert_eq!(input.body, "body new");
    assert_eq!(input.kind, Some(NoteKind::Procedure));
    assert_eq!(input.scope, Some(NoteScope::Project));
    assert_eq!(input.fact_key.as_deref(), Some("moss.output_dir"));
    assert_eq!(input.aliases, vec!["build dir".to_string()]);
    assert_eq!(input.tags, vec!["moss".to_string()]);
    assert_eq!(
        input.valid_from.map(|t| t.to_rfc3339()),
        Some("2026-05-10T00:00:00+00:00".to_string())
    );
    assert_eq!(input.valid_to, None);
    assert_eq!(input.supersedes, Some((old_id, old_rev)));
    assert!(input.sources.is_empty() && input.provenance.is_none());

    assert!(
        store_input_for(&note, &BTreeMap::new()).is_err(),
        "target not stored yet"
    );
    note.kind = "memo".to_string();
    assert!(store_input_for(&note, &stored).is_err(), "unknown kind");
}

// ─── Settings ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum System {
    #[serde(rename = "none")]
    NoMemory,
    D,
    E,
}

impl System {
    fn from_label(s: &str) -> Result<Self, String> {
        match s.trim() {
            "none" | "NONE" | "None" => Ok(System::NoMemory),
            "D" | "d" => Ok(System::D),
            "E" | "e" => Ok(System::E),
            label @ ("A" | "a" | "B" | "b") => {
                let upper = label.to_ascii_uppercase();
                Err(format!(
                    "system {upper} measured the memory engine slice 3 deleted; its report is \
                     archived at scripts/eval/memory/reports/baseline-{upper}-2026-09-24.json — \
                     compare a D or E report against it with compare.py"
                ))
            }
            "C" | "c" => Err(
                "system C (one bounded MEMORY.md) was never built; nothing measures it".to_string(),
            ),
            other => Err(format!("{other:?} is not one of none | D | E")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            System::NoMemory => "none",
            System::D => "D",
            System::E => "E",
        }
    }

    fn description(self) -> &'static str {
        match self {
            System::NoMemory => {
                "no memory service: no recall, no memory tool — the context-cost reference"
            }
            System::D => {
                "the memory service as the daemon runs it: the per-turn recall on, the read \
                 tools and the main loop's writers offered, under the shipped main-loop caps"
            }
            System::E => {
                "D bounded to the per-turn recall plus at most two model-directed follow-up \
                 searches, expressed with the main loop's existing caps: 3 rounds, 1 tool call \
                 per round (the bound counts every tool call; the model is not told it)"
            }
        }
    }
}

/// The main loop's round and per-round tool caps a system runs under, and
/// the follow-up search bound they express.
///
/// D runs the shipped caps (`[orchestrator.routing]`, 8 rounds × 4 tool
/// calls), so its model-directed searches are bounded only by them. E
/// narrows the same two caps to design §5's bound — no new mechanism, the
/// main loop's existing tool budget: one tool call per round and
/// `FOLLOW_UP_SEARCHES + 1` rounds, so at most two tool calls can inform an
/// answer written in the last round. The approximation, recorded in every E
/// report:
///
/// - the bound counts **every** tool call (`memory_read`, `task_status`, …),
///   not only `memory_search`;
/// - nothing tells the model which round is its last, so a model that spends
///   the third round on a tool exits `MaxRounds` and the turn answers with
///   the runtime's no-answer line (V3), which the judge scores like any
///   other answer;
/// - a second tool call in one round is answered "max tools per round
///   exceeded" instead of running;
/// - the answer guard's one corrective round (round 13, H3) can add a round
///   to a turn that states a task id nothing answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MainLoopBudget {
    max_rounds: usize,
    max_tools_per_round: usize,
    follow_up_search_bound: Option<usize>,
}

impl MainLoopBudget {
    fn for_system(system: System, shipped: &RoutingConfig) -> Self {
        match system {
            System::NoMemory | System::D => Self {
                max_rounds: shipped.main_loop_max_rounds,
                max_tools_per_round: shipped.main_loop_max_tools_per_round,
                follow_up_search_bound: None,
            },
            System::E => Self {
                max_rounds: FOLLOW_UP_SEARCHES + 1,
                max_tools_per_round: 1,
                follow_up_search_bound: Some(FOLLOW_UP_SEARCHES),
            },
        }
    }
}

#[test]
fn e_bounds_follow_up_searches_with_the_main_loops_own_caps() {
    let shipped = RoutingConfig::default();
    for system in [System::NoMemory, System::D] {
        let budget = MainLoopBudget::for_system(system, &shipped);
        assert_eq!(budget.max_rounds, shipped.main_loop_max_rounds);
        assert_eq!(
            budget.max_tools_per_round,
            shipped.main_loop_max_tools_per_round
        );
        assert_eq!(budget.follow_up_search_bound, None);
    }
    let e = MainLoopBudget::for_system(System::E, &shipped);
    // Every round but the last may carry one tool call; the last answers.
    assert_eq!(e.max_tools_per_round, 1);
    assert_eq!(
        (e.max_rounds - 1) * e.max_tools_per_round,
        FOLLOW_UP_SEARCHES
    );
    assert_eq!(e.follow_up_search_bound, Some(2));
    assert!(e.max_rounds < shipped.main_loop_max_rounds);
}

#[test]
fn the_old_engines_systems_point_at_their_archived_reports() {
    assert_eq!(System::from_label("none"), Ok(System::NoMemory));
    assert_eq!(System::from_label(" d "), Ok(System::D));
    assert_eq!(System::from_label("E"), Ok(System::E));
    for label in ["A", "b"] {
        let err = System::from_label(label).unwrap_err();
        assert!(err.contains("reports/baseline-"), "{err}");
        assert!(err.contains("compare.py"), "{err}");
    }
    assert!(System::from_label("C").unwrap_err().contains("never built"));
    assert!(System::from_label("F").is_err());
    for (label, dir) in [("A", "A"), ("B", "B")] {
        let err = System::from_label(label).unwrap_err();
        let path = err
            .split_whitespace()
            .find(|w| w.starts_with("scripts/eval/memory/reports/"))
            .unwrap();
        assert!(
            repo_root().join(path).is_file(),
            "{dir}: {path} is not archived"
        );
    }
}

struct Settings {
    system: System,
    report_path: PathBuf,
    home_store: PathBuf,
    llm_toml: PathBuf,
    questions_path: PathBuf,
    limit: Option<usize>,
    category: Option<Category>,
    judge_model: Option<String>,
    keep_scratch: bool,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

impl Settings {
    /// Every pre-flight check happens here, before a router exists, so a
    /// misconfigured run refuses without a single model call.
    fn from_env() -> Self {
        let label = env_nonempty("OPENALPACA_MEMORY_EVAL_SYSTEM").unwrap_or_else(|| {
            panic!("memory_eval: OPENALPACA_MEMORY_EVAL_SYSTEM must be one of none | D | E")
        });
        let system = System::from_label(&label).unwrap_or_else(|e| panic!("memory_eval: {e}"));
        let report_path = PathBuf::from(
            env_nonempty("OPENALPACA_MEMORY_EVAL_REPORT").unwrap_or_else(|| {
                panic!(
                    "memory_eval: OPENALPACA_MEMORY_EVAL_REPORT must name the JSON report to write"
                )
            }),
        );
        assert!(
            report_path.is_absolute(),
            "memory_eval: OPENALPACA_MEMORY_EVAL_REPORT must be an absolute path"
        );

        // The scratch store: where the memory partitions, their control
        // snapshots and the index cache go. It has to be named explicitly,
        // absolute, and must not be the real one.
        let home_store =
            PathBuf::from(env_nonempty("OPENALPACA_HOME_STORE").unwrap_or_else(|| {
                panic!(
                    "memory_eval: OPENALPACA_HOME_STORE must name a scratch store directory \
                 (never the real ~/.openalpaca); see scripts/eval/memory/README.md"
                )
            }));
        assert!(
            home_store.is_absolute(),
            "memory_eval: OPENALPACA_HOME_STORE must be an absolute path"
        );
        if let Some(real) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".openalpaca")) {
            let same = home_store == real
                || matches!(
                    (home_store.canonicalize(), real.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                );
            assert!(
                !same,
                "memory_eval: OPENALPACA_HOME_STORE points at the real store {}; use a scratch directory",
                real.display()
            );
        }
        openalpaca_storage::store::home_root()
            .unwrap_or_else(|e| panic!("memory_eval: OPENALPACA_HOME_STORE is not usable: {e}"));

        // The eval's own llm.toml — never the repo's config/, never a default.
        let config_dir = PathBuf::from(env_nonempty("OPENALPACA_CONFIG_DIR").unwrap_or_else(|| {
            panic!(
                "memory_eval: OPENALPACA_CONFIG_DIR must name the directory holding the eval's own llm.toml"
            )
        }));
        let llm_toml = config_dir.join("llm.toml");
        assert!(
            llm_toml.is_file(),
            "memory_eval: no llm.toml at {} (OPENALPACA_CONFIG_DIR)",
            llm_toml.display()
        );

        let questions_path = env_nonempty("OPENALPACA_MEMORY_EVAL_QUESTIONS")
            .map(PathBuf::from)
            .unwrap_or_else(|| eval_dir().join("questions-heldout.json"));
        let limit = env_nonempty("OPENALPACA_MEMORY_EVAL_LIMIT").map(|v| {
            v.parse::<usize>().unwrap_or_else(|_| {
                panic!("memory_eval: OPENALPACA_MEMORY_EVAL_LIMIT={v} is not a number")
            })
        });
        let category = env_nonempty("OPENALPACA_MEMORY_EVAL_CATEGORY").map(|v| {
            Category::ALL
                .into_iter()
                .find(|c| c.as_str() == v.trim())
                .unwrap_or_else(|| {
                    panic!("memory_eval: OPENALPACA_MEMORY_EVAL_CATEGORY={v} is not a category")
                })
        });

        Settings {
            system,
            report_path,
            home_store,
            llm_toml,
            questions_path,
            limit,
            category,
            judge_model: env_nonempty("OPENALPACA_MEMORY_EVAL_JUDGE_MODEL"),
            keep_scratch: env_flag("OPENALPACA_MEMORY_EVAL_KEEP"),
        }
    }
}

// ─── Report ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct Machine {
    hostname: Option<String>,
    os: &'static str,
    arch: &'static str,
    rustc: Option<String>,
}

#[derive(Debug, Serialize)]
struct Build {
    git_head: Option<String>,
    lockfile_sha256: String,
    crate_version: &'static str,
    features: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct Conditions {
    owner_id: String,
    llm_toml: String,
    llm_toml_sha256: String,
    reader_default_model: String,
    /// The main loop sets no temperature (`runner/agentic_loop/backend.rs`),
    /// so the reader runs at the provider's default; recorded as `null`.
    reader_temperature: Option<f32>,
    reader_max_rounds: usize,
    reader_max_cost_usd: f64,
    /// Plan §5 controls, from the effective `DaemonConfig`. The main loop
    /// applies `orchestrator.routing`'s caps in place of the `LoopConfig`
    /// defaults (`simple_query_handler.rs`, `override_max_rounds`), so these
    /// two are the reader's real round and tool-call caps and
    /// `reader_max_rounds` is the default they replace. E narrows both
    /// ([`MainLoopBudget`]).
    main_loop_max_rounds: usize,
    main_loop_max_tools_per_round: usize,
    /// The byte cap on a tool result handed to the model
    /// (`orchestrator.sessions.tool_result_inline_bytes`): a `memory_search`
    /// answer is cut here.
    tool_result_inline_bytes: usize,
    /// The per-turn context assembly's budget is the model window minus the
    /// reserved tokens minus `window × this ratio`
    /// (`execution.context.autocompact_buffer_ratio`, `prompt_ctx/manager.rs`).
    autocompact_buffer_ratio: f64,
    judge_model_requested: Option<String>,
    judge_temperature: f32,
    judge_max_tokens: u32,
    judge_prompt_path: String,
    judge_prompt_sha256: String,
    notes_path: String,
    notes_sha256: String,
    notes_total: usize,
    notes_seeded: usize,
    notes_duplicate: usize,
    notes_failed: usize,
    questions_path: String,
    questions_sha256: String,
    questions_total: usize,
    questions_selected: usize,
    limit: Option<usize>,
    category_filter: Option<Category>,
    /// The automatic per-turn recall (`prompt_ctx::sources::memory::MemorySource`
    /// over `MemoryService::recall` — lexical FTS5, no model) is on.
    per_turn_fts_active: bool,
    memory_search_tool_offered: bool,
    unattended: bool,
    extraction_disabled: bool,
    workflow_cap: usize,
    /// The scratch store (`OPENALPACA_HOME_STORE`): the home partition, the
    /// control snapshots and the index cache.
    home_store: String,
    scratch_dir: Option<String>,
    // ── Added with the new engine (plan 3.11) ─────────────────────────────
    /// What the system is, in one sentence.
    system_description: &'static str,
    /// The memory tools on the main loop's surface.
    memory_tools_offered: Vec<&'static str>,
    /// E: the model-directed follow-up searches the main-loop caps allow
    /// before the answering round; `null` when only the shipped caps bound
    /// them.
    follow_up_search_bound: Option<usize>,
    /// The retrieved byte budget (plan §5 control; design §4): the per-turn
    /// recall's total and pinned bytes and item cap, and a `memory_read`'s
    /// body cap.
    recall_total_bytes: usize,
    recall_pinned_bytes: usize,
    recall_max_items: usize,
    read_max_bytes: usize,
    /// Gold notes carry `scope` natively: global notes in the owner's home
    /// partition, project notes in the scratch project's partition (both
    /// fictional projects share it — the harness asks from one workspace).
    notes_scope_applied: bool,
    /// Stored as a `supersedes` successor of an earlier gold note.
    notes_superseding: usize,
    /// Gold id → the id the service minted, so a row's `memory_used` reads
    /// back as gold ids.
    seeded_ids: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Default)]
struct Bucket {
    n: usize,
    correct: usize,
    accuracy: Option<f64>,
}

#[derive(Debug, Serialize)]
struct Latency {
    p50_ms: u64,
    p95_ms: u64,
    mean_ms: f64,
    max_ms: u64,
}

#[derive(Debug, Serialize)]
struct Totals {
    questions: usize,
    correct: usize,
    incorrect: usize,
    unparsed: usize,
    judge_errors: usize,
    turn_errors: usize,
    accuracy: Option<f64>,
    by_category: BTreeMap<String, Bucket>,
    by_language: BTreeMap<String, Bucket>,
    reader_llm_calls: u32,
    reader_tokens_in: u64,
    reader_tokens_out: u64,
    reader_cost_usd: f64,
    reader_models: Vec<String>,
    judge_tokens_in: u64,
    judge_tokens_out: u64,
    judge_cost_usd: f64,
    judge_models: Vec<String>,
    memory_search_calls: u32,
    rows_with_per_turn_fts_hits: usize,
    latency: Latency,
    wall_ms: u64,
    // ── Added with the new engine (plan 3.11) ─────────────────────────────
    /// Rows with a non-empty `relies_on` whose every relied-on note reached
    /// the model (recall or tool) — retrieval, separately from the answer.
    rows_with_every_relied_note_delivered: usize,
    /// Tool calls that changed memory mid-run (`memory_store`,
    /// `memory_update`, `memory_forget`); non-zero means later questions saw
    /// a store the gold set does not describe.
    memory_write_calls: u32,
}

#[derive(Debug, Serialize)]
struct UsedNote {
    /// The gold id when the note is a seeded one, else the service's id (a
    /// note a model stored during the run).
    note: String,
    gold: bool,
    /// `recall` | `pinned` | `tool` (design §7a `memory_used`).
    reason: &'static str,
}

#[derive(Debug, Serialize)]
struct Row {
    id: String,
    category: Category,
    language: Language,
    question: String,
    gold_answer: String,
    expect_abstain: bool,
    relies_on: Vec<String>,
    answer: String,
    turn_error: Option<String>,
    /// `Some(true)` correct, `Some(false)` incorrect, `None` when the judge
    /// could not decide (`judge_verdict` says why).
    correct: Option<bool>,
    /// `correct` | `incorrect` | `unparsed` | `judge_error` | `not_judged`
    judge_verdict: &'static str,
    judge_raw: String,
    judge_model: Option<String>,
    judge_tokens_in: u32,
    judge_tokens_out: u32,
    judge_error: Option<String>,
    latency_ms: u64,
    ack_ms: u64,
    mode: String,
    /// The ids the router reported answering (V4), one per distinct model.
    reader_models: Vec<String>,
    reader_llm_calls: u32,
    reader_tokens_in: u64,
    reader_tokens_out: u64,
    reader_cost_usd: f64,
    tools: Vec<String>,
    memory_search_calls: u32,
    /// How many notes the automatic per-turn recall delivered for this
    /// question (`memory_used` entries with reason `recall` or `pinned`).
    per_turn_fts_hits: usize,
    // ── Added with the new engine (plan 3.11) ─────────────────────────────
    /// The turn's `memory_used` (design §7a), as `take_memory_used` hands it
    /// to the daemon's bridge: every note delivered, by recall or by tool.
    memory_used: Vec<UsedNote>,
    /// How many of `relies_on` are in `memory_used`.
    relied_notes_delivered: usize,
}

#[derive(Debug, Serialize)]
struct Report {
    schema: u32,
    /// `running` while rows are still being added, then `complete`.
    status: &'static str,
    system: System,
    unavailable_reason: Option<String>,
    run_started_at: String,
    run_finished_at: Option<String>,
    machine: Machine,
    build: Build,
    conditions: Option<Conditions>,
    /// Baseline B's E5 record in the archived reports. The new engine has no
    /// embedder, so it is always `null` here; kept so every report has the
    /// same keys.
    embedder: Option<serde_json::Value>,
    totals: Option<Totals>,
    rows: Vec<Row>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn command_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn machine() -> Machine {
    Machine {
        hostname: command_stdout("hostname", &[]),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        rustc: command_stdout("rustc", &["-V"]),
    }
}

fn build_info() -> Build {
    let root = repo_root();
    let root_str = root.to_string_lossy().into_owned();
    let lockfile = read_bytes(&root.join("Cargo.lock"));
    Build {
        git_head: command_stdout("git", &["-C", &root_str, "rev-parse", "HEAD"]),
        lockfile_sha256: sha256_hex(&lockfile),
        crate_version: env!("CARGO_PKG_VERSION"),
        features: vec!["live-eval"],
    }
}

/// Atomic write: the report is rewritten after every question, so a run that
/// dies mid-way leaves a readable `running` report rather than half a file.
fn write_report(path: &Path, report: &Report) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
    }
    let json = serde_json::to_vec_pretty(report).expect("serialize report");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).unwrap_or_else(|e| panic!("cannot write {}: {e}", tmp.display()));
    std::fs::rename(&tmp, path)
        .unwrap_or_else(|e| panic!("cannot move {} to {}: {e}", tmp.display(), path.display()));
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn bucket_accuracy(b: &mut Bucket) {
    b.accuracy = (b.n > 0).then(|| b.correct as f64 / b.n as f64);
}

fn totals(rows: &[Row], judge_cost_usd: f64, wall_ms: u64) -> Totals {
    let mut by_category: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_language: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut correct = 0;
    let mut incorrect = 0;
    let mut unparsed = 0;
    let mut judge_errors = 0;
    let mut turn_errors = 0;
    let mut reader_models = BTreeSet::new();
    let mut judge_models = BTreeSet::new();
    let mut latencies: Vec<u64> = rows.iter().map(|r| r.latency_ms).collect();
    latencies.sort_unstable();
    for r in rows {
        match r.judge_verdict {
            "correct" => correct += 1,
            "incorrect" => incorrect += 1,
            "unparsed" => unparsed += 1,
            "judge_error" => judge_errors += 1,
            _ => {}
        }
        if r.turn_error.is_some() {
            turn_errors += 1;
        }
        let ok = r.correct == Some(true);
        let c = by_category
            .entry(r.category.as_str().to_string())
            .or_default();
        c.n += 1;
        c.correct += usize::from(ok);
        let l = by_language
            .entry(r.language.as_str().to_string())
            .or_default();
        l.n += 1;
        l.correct += usize::from(ok);
        reader_models.extend(r.reader_models.iter().cloned());
        judge_models.extend(r.judge_model.iter().cloned());
    }
    by_category.values_mut().for_each(bucket_accuracy);
    by_language.values_mut().for_each(bucket_accuracy);
    let n = rows.len();
    Totals {
        questions: n,
        correct,
        incorrect,
        unparsed,
        judge_errors,
        turn_errors,
        accuracy: (n > 0).then(|| correct as f64 / n as f64),
        by_category,
        by_language,
        reader_llm_calls: rows.iter().map(|r| r.reader_llm_calls).sum(),
        reader_tokens_in: rows.iter().map(|r| r.reader_tokens_in).sum(),
        reader_tokens_out: rows.iter().map(|r| r.reader_tokens_out).sum(),
        reader_cost_usd: rows.iter().map(|r| r.reader_cost_usd).sum(),
        reader_models: reader_models.into_iter().collect(),
        judge_tokens_in: rows.iter().map(|r| u64::from(r.judge_tokens_in)).sum(),
        judge_tokens_out: rows.iter().map(|r| u64::from(r.judge_tokens_out)).sum(),
        judge_cost_usd,
        judge_models: judge_models.into_iter().collect(),
        memory_search_calls: rows.iter().map(|r| r.memory_search_calls).sum(),
        rows_with_per_turn_fts_hits: rows.iter().filter(|r| r.per_turn_fts_hits > 0).count(),
        latency: Latency {
            p50_ms: percentile(&latencies, 0.50),
            p95_ms: percentile(&latencies, 0.95),
            mean_ms: if n == 0 {
                0.0
            } else {
                latencies.iter().sum::<u64>() as f64 / n as f64
            },
            max_ms: latencies.last().copied().unwrap_or(0),
        },
        wall_ms,
        rows_with_every_relied_note_delivered: rows
            .iter()
            .filter(|r| !r.relies_on.is_empty() && r.relied_notes_delivered == r.relies_on.len())
            .count(),
        memory_write_calls: rows
            .iter()
            .flat_map(|r| r.tools.iter())
            .filter(|t| MEMORY_WRITE_TOOLS.contains(&t.as_str()))
            .count() as u32,
    }
}

// ─── Judge ──────────────────────────────────────────────────────────────────

fn fill_judge_prompt(template: &str, q: &Question, answer: &str) -> String {
    let answer: String = if answer.chars().count() > JUDGE_ANSWER_CAP_CHARS {
        let cut: String = answer.chars().take(JUDGE_ANSWER_CAP_CHARS).collect();
        format!("{cut}\n[answer cut at {JUDGE_ANSWER_CAP_CHARS} characters]")
    } else {
        answer.to_string()
    };
    template
        .replace("{{question}}", &q.question)
        .replace("{{gold_answer}}", &q.gold_answer)
        .replace("{{answer}}", &answer)
        .replace("{{category}}", q.category.as_str())
        .replace(
            "{{expect_abstain}}",
            if q.expect_abstain { "yes" } else { "no" },
        )
}

/// The verdict is the last non-empty line, one of the two words; anything
/// else is `unparsed` and counted as not correct.
fn parse_verdict(raw: &str) -> Option<bool> {
    let last = raw
        .lines()
        .rev()
        .map(|l| l.trim().trim_matches(|c: char| !c.is_alphanumeric()))
        .find(|l| !l.is_empty())?
        .to_ascii_uppercase();
    if last == "INCORRECT" {
        Some(false)
    } else if last == "CORRECT" {
        Some(true)
    } else {
        None
    }
}

struct JudgeOutcome {
    verdict: Option<bool>,
    raw: String,
    model: String,
    tokens_in: u32,
    tokens_out: u32,
}

async fn judge(
    router: &openalpaca_llm::LlmRouter,
    template: &str,
    model: Option<String>,
    q: &Question,
    answer: &str,
) -> Result<JudgeOutcome, String> {
    use openalpaca_llm::{ChatMessage, RequestContext, RouterRequest, ThinkingConfig};
    let prompt = fill_judge_prompt(template, q, answer);
    let request = RouterRequest {
        model,
        messages: Arc::new(vec![ChatMessage::user(&prompt)]),
        tools: Arc::new(Vec::new()),
        temperature: Some(JUDGE_TEMPERATURE),
        max_tokens: Some(JUDGE_MAX_TOKENS),
        context: RequestContext {
            agent_id: Some(JUDGE_AGENT_ID.to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: Some(ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    };
    let response = router.complete(request).await.map_err(|e| e.to_string())?;
    Ok(JudgeOutcome {
        verdict: parse_verdict(&response.content),
        raw: response.content.clone(),
        model: response.model,
        tokens_in: response.usage.input_tokens,
        tokens_out: response.usage.output_tokens,
    })
}

// ─── Live eval ──────────────────────────────────────────────────────────────

/// What the bus said happened during one turn.
#[derive(Default)]
struct TurnObservation {
    mode: String,
    ack_ms: u64,
    llm_calls: u32,
    tokens_in: u64,
    tokens_out: u64,
    cost_usd: f64,
    models: BTreeSet<String>,
    tools: Vec<String>,
}

/// Live memory eval. `#[ignore]`d: even `cargo test -- --ignored` exits early
/// (without spending money) unless `OPENALPACA_LIVE_EVAL=1`.
///
/// Requires the eval's own `llm.toml` under `OPENALPACA_CONFIG_DIR`, a scratch
/// `OPENALPACA_HOME_STORE`, and `--features live-eval`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live LLM eval — set OPENALPACA_LIVE_EVAL=1, OPENALPACA_MEMORY_EVAL_SYSTEM, OPENALPACA_MEMORY_EVAL_REPORT, a scratch OPENALPACA_HOME_STORE and OPENALPACA_CONFIG_DIR; build with --features live-eval (runbook: scripts/eval/memory/README.md)"]
async fn memory_eval() {
    use arc_swap::ArcSwap;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::daemon_config::DaemonConfig;
    use openalpaca_core::events::SystemEvent;
    use openalpaca_core::gateway::HandleRequest;
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::middleware::prompt::SystemPersona;
    use openalpaca_core::orchestrator::{
        MAIN_LOOP_AGENT_ID, Orchestrator, skill_catalog, skill_router,
    };
    use openalpaca_core::runner::LoopConfig;
    use openalpaca_core::security::gate::SecurityGate;
    use openalpaca_core::security::policy::{Principal, Scope};
    use openalpaca_core::security::sandbox::SandboxManager;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_core::tools::builtins::memory_read_tools;
    use openalpaca_storage::IdentityRepository;
    use tokio::sync::broadcast::error::TryRecvError;
    use uuid::Uuid;

    // Double gate: never make live calls unless the operator opted in.
    if !env_flag("OPENALPACA_LIVE_EVAL") {
        eprintln!(
            "memory_eval: skipped — set OPENALPACA_LIVE_EVAL=1 to run the live eval \
             (no LLM calls were made)."
        );
        return;
    }
    let settings = Settings::from_env();
    let started = Instant::now();
    let mut report = Report {
        schema: REPORT_SCHEMA,
        status: "running",
        system: settings.system,
        unavailable_reason: None,
        run_started_at: now_rfc3339(),
        run_finished_at: None,
        machine: machine(),
        build: build_info(),
        conditions: None,
        embedder: None,
        totals: None,
        rows: Vec::new(),
    };

    // ── Data ────────────────────────────────────────────────────────────
    let dir = eval_dir();
    let notes_path = dir.join("notes-gold.json");
    let judge_prompt_path = dir.join("judge-prompt.md");
    let (notes, notes_sha256) = load_json::<Vec<GoldNote>>(&notes_path);
    let (all_questions, questions_sha256) = load_json::<Vec<Question>>(&settings.questions_path);
    let judge_bytes = read_bytes(&judge_prompt_path);
    let judge_prompt_sha256 = sha256_hex(&judge_bytes);
    let judge_template = String::from_utf8(judge_bytes).expect("judge prompt is UTF-8");
    let llm_toml_sha256 = sha256_hex(&read_bytes(&settings.llm_toml));

    let questions: Vec<Question> = all_questions
        .iter()
        .filter(|q| settings.category.is_none_or(|c| q.category == c))
        .take(settings.limit.unwrap_or(usize::MAX))
        .cloned()
        .collect();
    assert!(!questions.is_empty(), "memory_eval: no questions selected");

    // ── Router ──────────────────────────────────────────────────────────
    let router = Arc::new(
        openalpaca_llm::build_router(&settings.llm_toml).unwrap_or_else(|e| {
            panic!(
                "memory_eval: failed to build LLM router from {}: {e}\n\
             (providers are feature-gated — build with `--features live-eval`; \
             for secret_ref keys use secret_env in the eval's own llm.toml)",
                settings.llm_toml.display()
            )
        }),
    );
    // The daemon discovers each enabled provider's models at boot
    // (`refresh_models`, apps/openalpacad/src/services/llm.rs); a router built
    // from the file alone knows no Ollama model and answers every turn with
    // "No model is available".
    router.refresh_models().await;
    eprintln!(
        "memory_eval: system {} — router loaded from {} (default model: {}; effective: {})",
        settings.system.as_str(),
        settings.llm_toml.display(),
        router.default_model(),
        router
            .effective_default_model()
            .unwrap_or_else(|| "none".to_string())
    );

    // ── Scratch DB, owner and project (so the turn's workspace is never
    //    this checkout, whatever the test process's CWD is) ──────────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let scratch_project = tmp.path().join("project");
    std::fs::create_dir_all(scratch_project.join(".git")).expect("scratch project marker");
    let scratch_project = scratch_project
        .canonicalize()
        .expect("canonical scratch project");
    let db = openalpaca_storage::Database::open(&tmp.path().join("eval.db")).expect("open db");
    // The service answers only for a `global_user` row (design §3); a fresh
    // owner per run keeps a reused scratch store's earlier partitions out.
    let owner_id = format!(
        "{OWNER_PREFIX}-{}",
        &Uuid::new_v4().simple().to_string()[..8]
    );
    IdentityRepository::new(&db)
        .create_global_user(&owner_id, None)
        .expect("create the eval owner");

    // ── Orchestrator ────────────────────────────────────────────────────
    let shared = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let registry = Arc::new(ToolRegistry::default());
    let sandbox = Arc::new(SandboxManager::with_defaults(registry.clone(), bus.clone()));
    let gate = Arc::new(SecurityGate::new(sandbox));

    let mut cfg = DaemonConfig::default();
    // A stray `start_workflow` is refused at the cap instead of dispatching a
    // real (costly) lead-agent run; the refusal still shows as a tool call.
    cfg.orchestrator.routing.max_workflows_per_lane = 0;
    // The background user-trait extraction is a real model call per turn and
    // writes memories of its own; the eval measures recall, not learning.
    cfg.orchestrator.costs.extract_every_n_turns = usize::MAX;
    cfg.orchestrator.costs.task_extract_enabled = false;
    let budget = MainLoopBudget::for_system(settings.system, &cfg.orchestrator.routing);
    cfg.orchestrator.routing.main_loop_max_rounds = budget.max_rounds;
    cfg.orchestrator.routing.main_loop_max_tools_per_round = budget.max_tools_per_round;
    let workflow_cap = cfg.orchestrator.routing.max_workflows_per_lane;
    let tool_result_inline_bytes = cfg.orchestrator.sessions.tool_result_inline_bytes;
    let autocompact_buffer_ratio = cfg.execution.context.autocompact_buffer_ratio;
    let daemon_config = Arc::new(ArcSwap::from_pointee(cfg));

    // D and E: the daemon's own service (`MemoryService::open_default`,
    // `apps/openalpacad/src/services/mod.rs` — the index cache under the
    // scratch store) and its read tools registered exactly as the daemon
    // registers them (`services/tools.rs`); the orchestrator then runs the
    // per-turn recall and offers the main loop's writers. `none` installs
    // nothing: no recall, no memory tool.
    let service = (settings.system != System::NoMemory).then(|| {
        let service = Arc::new(MemoryService::open_default(Some(db.clone())).with_bus(bus.clone()));
        for tool in memory_read_tools(service.clone()) {
            registry
                .register(tool)
                .expect("register a memory read tool");
        }
        service
    });

    let loop_config = LoopConfig::default();
    let reader_max_rounds = loop_config.max_rounds;
    let reader_max_cost = loop_config.max_cost;
    let orch = Orchestrator::new(
        shared.clone(),
        lanes,
        bus.clone(),
        SystemPersona::default(),
        Some(router.clone()),
        loop_config,
        gate,
        registry,
        Some(db.clone()),
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        daemon_config,
    );
    if let Some(service) = &service {
        orch.set_memory_service(service.clone());
    }

    // ── Seed the gold notes through the service ─────────────────────────
    let seed = match &service {
        Some(service) => {
            let (service, owner, project, gold_notes) = (
                service.clone(),
                owner_id.clone(),
                scratch_project.clone(),
                notes.clone(),
            );
            // The service's file work is synchronous (it says so); an async
            // caller runs it on the blocking pool, as the tools do.
            let seed = tokio::task::spawn_blocking(move || {
                seed_notes(&service, &owner, &project, &gold_notes)
            })
            .await
            .expect("seeding task");
            eprintln!(
                "memory_eval: seeded {} of {} notes ({} as a replacement, {} duplicate, {} failed)",
                seed.seeded,
                notes.len(),
                seed.superseding,
                seed.duplicate,
                seed.failed
            );
            seed
        }
        None => SeedOutcome::default(),
    };
    let gold_by_service_id: HashMap<String, String> = seed
        .ids
        .iter()
        .map(|(gold, service)| (service.clone(), gold.clone()))
        .collect();

    let recall = RecallBudget::default();
    report.conditions = Some(Conditions {
        owner_id: owner_id.clone(),
        llm_toml: settings.llm_toml.to_string_lossy().into_owned(),
        llm_toml_sha256,
        reader_default_model: router.default_model(),
        reader_temperature: None,
        reader_max_rounds,
        reader_max_cost_usd: reader_max_cost,
        main_loop_max_rounds: budget.max_rounds,
        main_loop_max_tools_per_round: budget.max_tools_per_round,
        tool_result_inline_bytes,
        autocompact_buffer_ratio,
        judge_model_requested: settings.judge_model.clone(),
        judge_temperature: JUDGE_TEMPERATURE,
        judge_max_tokens: JUDGE_MAX_TOKENS,
        judge_prompt_path: judge_prompt_path.to_string_lossy().into_owned(),
        judge_prompt_sha256,
        notes_path: notes_path.to_string_lossy().into_owned(),
        notes_sha256,
        notes_total: notes.len(),
        notes_seeded: seed.seeded,
        notes_duplicate: seed.duplicate,
        notes_failed: seed.failed,
        questions_path: settings.questions_path.to_string_lossy().into_owned(),
        questions_sha256,
        questions_total: all_questions.len(),
        questions_selected: questions.len(),
        limit: settings.limit,
        category_filter: settings.category,
        per_turn_fts_active: service.is_some(),
        memory_search_tool_offered: service.is_some(),
        unattended: true,
        extraction_disabled: true,
        workflow_cap,
        home_store: settings.home_store.to_string_lossy().into_owned(),
        scratch_dir: settings
            .keep_scratch
            .then(|| tmp.path().to_string_lossy().into_owned()),
        system_description: settings.system.description(),
        memory_tools_offered: if service.is_some() {
            MEMORY_TOOLS.to_vec()
        } else {
            Vec::new()
        },
        follow_up_search_bound: budget.follow_up_search_bound,
        recall_total_bytes: recall.total_bytes,
        recall_pinned_bytes: recall.pinned_bytes,
        recall_max_items: recall.max_items,
        read_max_bytes: openalpaca_core::memory::service::READ_MAX_BYTES,
        notes_scope_applied: service.is_some(),
        notes_superseding: seed.superseding,
        seeded_ids: seed.ids.clone(),
    });
    write_report(&settings.report_path, &report);

    // ── Ask every question through the front door ───────────────────────
    for (i, q) in questions.iter().enumerate() {
        // A fresh lane per question: no cross-question history, one owner.
        let lane_key = format!("memory-eval-q{i:04}:cli");
        let request_id = Uuid::new_v4();
        let mut rx = bus.subscribe();
        let t0 = Instant::now();
        let reply = orch
            .handle_message(HandleRequest {
                workspace_path: Some(scratch_project.to_string_lossy().into_owned()),
                // Nobody here can answer a confirmation prompt: a tool that
                // needs one is refused at once instead of hanging (M6).
                unattended: true,
                ..HandleRequest::new(
                    request_id,
                    "cli",
                    q.question.clone(),
                    Principal::User {
                        global_id: owner_id.clone(),
                    },
                    Scope::Global,
                    lane_key,
                )
            })
            .await;
        let latency_ms = t0.elapsed().as_millis() as u64;
        // What the turn delivered — the daemon's bridge takes exactly this
        // after `handle_message` (`apps/openalpacad/src/gateway_bridge.rs`).
        let used = orch.take_memory_used(request_id);

        let mut obs = TurnObservation {
            mode: "?".to_string(),
            ..Default::default()
        };
        loop {
            match rx.try_recv() {
                Ok(SystemEvent::OrchestrationStage {
                    request_id: rid,
                    mode,
                    ack_ms,
                    ..
                }) if rid == request_id => {
                    obs.mode = mode;
                    obs.ack_ms = ack_ms;
                }
                Ok(SystemEvent::ToolExecuted {
                    agent_id,
                    tool_name,
                    ..
                }) if agent_id == MAIN_LOOP_AGENT_ID => obs.tools.push(tool_name),
                Ok(SystemEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    ..
                }) if agent_id == MAIN_LOOP_AGENT_ID => {
                    obs.llm_calls += 1;
                    obs.tokens_in += u64::from(input_tokens);
                    obs.tokens_out += u64::from(output_tokens);
                    obs.cost_usd += cost_usd;
                    obs.models.insert(model);
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }

        let memory_used: Vec<UsedNote> = used
            .iter()
            .map(|u| {
                let id = u.id.to_string();
                let gold = gold_by_service_id.get(&id);
                UsedNote {
                    gold: gold.is_some(),
                    note: gold.cloned().unwrap_or(id),
                    reason: u.reason.as_str(),
                }
            })
            .collect();
        let per_turn_fts_hits = used
            .iter()
            .filter(|u| u.reason != UsedReason::Tool)
            .map(|u| u.id)
            .collect::<BTreeSet<_>>()
            .len();
        let relied_notes_delivered = q
            .relies_on
            .iter()
            .filter(|r| memory_used.iter().any(|u| u.gold && &u.note == *r))
            .count();

        // `handle_message` answers with the turn's text — the daemon's
        // `OrchestratorHandler::handle` calls exactly this and wraps it.
        let (answer, turn_error) = match &reply {
            Ok(text) => (text.clone(), None),
            Err(e) => (String::new(), Some(e.clone())),
        };

        let mut row = Row {
            id: q.id.clone(),
            category: q.category,
            language: q.language,
            question: q.question.clone(),
            gold_answer: q.gold_answer.clone(),
            expect_abstain: q.expect_abstain,
            relies_on: q.relies_on.clone(),
            answer: answer.clone(),
            turn_error,
            correct: None,
            judge_verdict: "not_judged",
            judge_raw: String::new(),
            judge_model: None,
            judge_tokens_in: 0,
            judge_tokens_out: 0,
            judge_error: None,
            latency_ms,
            ack_ms: obs.ack_ms,
            mode: obs.mode,
            reader_models: obs.models.into_iter().collect(),
            reader_llm_calls: obs.llm_calls,
            reader_tokens_in: obs.tokens_in,
            reader_tokens_out: obs.tokens_out,
            reader_cost_usd: obs.cost_usd,
            memory_search_calls: obs
                .tools
                .iter()
                .filter(|t| t.as_str() == "memory_search")
                .count() as u32,
            tools: obs.tools,
            per_turn_fts_hits,
            memory_used,
            relied_notes_delivered,
        };

        // A turn that errored still gets judged: an empty answer to a
        // non-abstention question is simply incorrect, and the judge, not
        // the harness, decides what an abstention row deserves.
        match judge(
            &router,
            &judge_template,
            settings.judge_model.clone(),
            q,
            &answer,
        )
        .await
        {
            Ok(out) => {
                row.correct = out.verdict;
                row.judge_verdict = match out.verdict {
                    Some(true) => "correct",
                    Some(false) => "incorrect",
                    None => "unparsed",
                };
                row.judge_raw = out.raw;
                row.judge_model = Some(out.model);
                row.judge_tokens_in = out.tokens_in;
                row.judge_tokens_out = out.tokens_out;
            }
            Err(e) => {
                row.judge_verdict = "judge_error";
                row.judge_error = Some(e);
            }
        }

        println!(
            "[{}] {:<10} {:<24} {:<5} latency={:<6}ms llm_calls={} tokens={}/{} tools={:?} recalled={} relied={}/{}{}",
            match row.correct {
                Some(true) => " OK ",
                Some(false) => "MISS",
                None => " ?? ",
            },
            row.id,
            row.category.as_str(),
            row.language.as_str(),
            row.latency_ms,
            row.reader_llm_calls,
            row.reader_tokens_in,
            row.reader_tokens_out,
            row.tools,
            row.per_turn_fts_hits,
            row.relied_notes_delivered,
            row.relies_on.len(),
            match (&row.turn_error, &row.judge_error) {
                (Some(e), _) => format!(" (turn returned Err: {e})"),
                (None, Some(e)) => format!(" (judge failed: {e})"),
                _ => String::new(),
            }
        );
        report.rows.push(row);
        write_report(&settings.report_path, &report);
    }

    // ── Totals ──────────────────────────────────────────────────────────
    let judge_cost = router
        .cost_tracker
        .get_agent_usage(JUDGE_AGENT_ID)
        .await
        .map(|u| u.total_cost_usd)
        .unwrap_or(0.0);
    let wall_ms = started.elapsed().as_millis() as u64;
    let t = totals(&report.rows, judge_cost, wall_ms);
    println!(
        "\n== memory_eval: system {} — {}/{} correct ({:.1}%), {} unparsed, {} judge errors, {} turn errors ==",
        settings.system.as_str(),
        t.correct,
        t.questions,
        t.accuracy.unwrap_or(0.0) * 100.0,
        t.unparsed,
        t.judge_errors,
        t.turn_errors,
    );
    for (cat, b) in &t.by_category {
        println!(
            "  {cat:<24} {}/{} ({:.1}%)",
            b.correct,
            b.n,
            b.accuracy.unwrap_or(0.0) * 100.0
        );
    }
    println!(
        "reader: {} calls, {} in / {} out tokens, ${:.4}; judge: {} in / {} out tokens, ${:.4}; \
         latency p50={}ms p95={}ms; memory_search calls={}; rows with per-turn recall hits={}; \
         rows with every relied-on note delivered={}; memory writes={}; wall {}s",
        t.reader_llm_calls,
        t.reader_tokens_in,
        t.reader_tokens_out,
        t.reader_cost_usd,
        t.judge_tokens_in,
        t.judge_tokens_out,
        t.judge_cost_usd,
        t.latency.p50_ms,
        t.latency.p95_ms,
        t.memory_search_calls,
        t.rows_with_per_turn_fts_hits,
        t.rows_with_every_relied_note_delivered,
        t.memory_write_calls,
        t.wall_ms / 1000,
    );
    report.totals = Some(t);
    report.status = "complete";
    report.run_finished_at = Some(now_rfc3339());
    write_report(&settings.report_path, &report);
    println!("report: {}", settings.report_path.display());

    if settings.keep_scratch {
        let kept = tmp.keep();
        println!("scratch kept at {}", kept.display());
    }
    // No accuracy bar here: a single run is a measurement, and the gate is the
    // paired comparison of two reports (`scripts/eval/memory/compare.py`).
}
