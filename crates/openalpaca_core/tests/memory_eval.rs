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
/// four read tools the daemon registers globally
/// (`apps/openalpacad/src/services/tools.rs`, `memory_read_tools`) and the
/// three writers `main_loop_tool_set` adds per request.
const MEMORY_TOOLS: [&str; 7] = [
    "memory_search",
    "memory_read",
    "memory_list",
    "memory_timeline",
    "memory_store",
    "memory_update",
    "memory_forget",
];
/// Tool calls that change memory. A model that stores or edits a note
/// during the run changes what later questions can recall, so every row
/// records its tool calls and the totals count these.
const MEMORY_WRITE_TOOLS: [&str; 3] = ["memory_store", "memory_update", "memory_forget"];

// ─── Data files ─────────────────────────────────────────────────────────────
//
// The data files' types, the corpus's included (design 09 §7.1), are shared
// with `tests/memory_eval_corpus.rs`, which runs in default CI.

#[path = "common/eval.rs"]
mod eval;

use eval::{
    Category, ChangePair, Corpus, CorpusSession, CorpusTurn, GoldNote, Language, Question,
    TurnMode, change_pairs, eval_dir, is_uuid, load_json, parse_instant, read_bytes, relied_on,
    repo_root, sha256_hex,
};

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

/// The group a same-`fact_key` tie is decided in: one partition, one key.
/// The harness has one workspace, so the scope names the partition.
fn same_key(note: &GoldNote) -> Option<(String, String)> {
    let key = note.fact_key.clone()?;
    Some((note.scope.clone(), key))
}

/// How long to wait so the next write is stamped in a later second than
/// `previous` — an instant taken after the previous write returned, so at or
/// after its stamp. `None` once that second is over.
fn wait_past_second(
    previous: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<std::time::Duration> {
    let next = chrono::DateTime::from_timestamp(previous.timestamp() + 1, 0)?;
    (now < next).then(|| (next - now).to_std().expect("next is later than now"))
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
    // When the last note of each same-`fact_key` group was written, so the
    // next one is stamped in a later second (see `wait_past_second`).
    let mut key_written: HashMap<(String, String), chrono::DateTime<chrono::Utc>> = HashMap::new();
    let mut out = SeedOutcome::default();
    for i in order {
        let note = &notes[i];
        let key = same_key(note);
        if let Some(previous) = key.as_ref().and_then(|k| key_written.get(k))
            && let Some(wait) = wait_past_second(*previous, chrono::Utc::now())
        {
            std::thread::sleep(wait);
        }
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
                if let Some(key) = key {
                    key_written.insert(key, chrono::Utc::now());
                }
            }
            Err(e) => {
                out.failed += 1;
                eprintln!("memory_eval: note {}: store refused: {e}", note.id);
            }
        }
    }
    out
}

/// Four of the change pairs' old gold notes are written in hindsight — "Until
/// June 2026 …", "Replaced by Helix", a `valid_to` — because the gold set
/// records a history. L-S seeds them as the user would have saved them at
/// the time instead (review of L7, finding 2): a saved note that already
/// records its own change lets a correct extractor read the change turn as a
/// restatement of it (§2.5), which would count against MD-11. Per note: the
/// gold id, its key, and the present-tense title and body; everything else
/// is the gold note's, with no `valid_to`.
const LS_SEEDED_AS_SAVED_THEN: [(&str, &str, &str, &str); 4] = [
    (
        "eca96db9-98b9-5def-a63d-c3c39a10ea0e",
        "user.editor",
        "Editor: vim",
        "## Knowledge\nLin Wei's editor is vim, with a plain vimrc and no plugin manager.\n\
         ## Evidence and limits\nStated by the user.\n\
         ## Recheck when\nShe mentions switching editors.",
    ),
    (
        "17310b78-2539-5802-a542-9aa6380ca979",
        "user.laptop",
        "Laptop: ThinkPad X1 Carbon on Fedora",
        "## Knowledge\nLin Wei's laptop is a ThinkPad X1 Carbon Gen 9 running Fedora.\n\
         ## Evidence and limits\nStated by the user.\n\
         ## Recheck when\nShe mentions a new machine.",
    ),
    (
        "73c3f368-d646-5e3f-845e-b319ed050190",
        "user.commute",
        "开一辆本田飞度",
        "## Knowledge\n林薇开一辆 2018 年的本田飞度。\n\
         ## Evidence and limits\n用户说的。\n\
         ## Recheck when\n她换车或不再开车。",
    ),
    (
        "005e519a-6a01-5e8d-bf96-1492badedcf4",
        "moss.output_dir",
        "Moss output directory: _site",
        "## Knowledge\nMoss writes its build output to `_site/`.\n\
         ## Evidence and limits\nStated by the user.\n\
         ## Recheck when\nThe build tool changes.",
    ),
];

/// The notes L-S seeds (design 09 §7.2): the change pairs' old notes, in
/// file order, the four in [`LS_SEEDED_AS_SAVED_THEN`] as the user saved
/// them then.
fn ls_seed_notes(notes: &[GoldNote], pairs: &[ChangePair]) -> Vec<GoldNote> {
    let old: BTreeSet<&str> = pairs.iter().map(|p| p.old.as_str()).collect();
    notes
        .iter()
        .filter(|n| old.contains(n.id.as_str()))
        .map(|n| {
            let mut n = n.clone();
            if let Some((_, key, title, body)) =
                LS_SEEDED_AS_SAVED_THEN.iter().find(|(id, ..)| *id == n.id)
            {
                assert_eq!(
                    n.fact_key.as_deref(),
                    Some(*key),
                    "memory_eval: the gold note {} changed key",
                    n.id
                );
                n.title = title.to_string();
                n.body = body.to_string();
                n.valid_to = None;
            }
            n
        })
        .collect()
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

/// The service stamps `created_at` to the second, and design §3 rule 3
/// breaks a same-`fact_key` tie by it: two notes written in one second tie,
/// and the pair falls to the lowest of the two ids the service minted — a
/// coin flip. A note sharing its `(scope, fact_key)` with one already stored
/// therefore waits until the second that note was written in is over, and
/// no longer.
#[test]
fn a_same_key_note_waits_for_the_next_second() {
    let at = |s: &str| {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let previous = at("2026-09-24T10:00:00.300Z");
    let ms = std::time::Duration::from_millis;
    assert_eq!(
        wait_past_second(previous, at("2026-09-24T10:00:00.300Z")),
        Some(ms(700))
    );
    assert_eq!(
        wait_past_second(previous, at("2026-09-24T10:00:00.999Z")),
        Some(ms(1))
    );
    assert_eq!(wait_past_second(previous, at("2026-09-24T10:00:01Z")), None);
    assert_eq!(
        wait_past_second(previous, at("2026-09-24T10:00:02.5Z")),
        None
    );

    let mut note = gold("n", &[]);
    assert_eq!(same_key(&note), None, "no fact_key, nothing to order");
    note.fact_key = Some("lantern.staging.db".to_string());
    note.scope = "project".to_string();
    assert_eq!(
        same_key(&note),
        Some(("project".to_string(), "lantern.staging.db".to_string()))
    );
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

/// L-S seeds the change pairs' old notes as the user would have saved them
/// then: a seeded note that already records its own change (a `valid_to`,
/// "until June 2026", "replaced by Helix") lets a correct extractor read the
/// change turn as a restatement of it (§2.5: a listed saved target with no
/// `replaces` is `unchanged`), so the "8 suggestions" would measure the
/// seeding, not MD-11. Each keeps the gold note's id, kind, scope, key,
/// aliases and tags, and stores as it is.
#[test]
fn l_s_seeds_the_old_notes_as_the_user_saved_them_then() {
    let (notes, _) = load_json::<Vec<GoldNote>>(&eval_dir().join("notes-gold.json"));
    let pairs = change_pairs(&notes).unwrap();
    let gold: HashMap<&str, &GoldNote> = notes.iter().map(|n| (n.id.as_str(), n)).collect();
    let seeded = ls_seed_notes(&notes, &pairs);
    assert_eq!(seeded.len(), 8, "the 8 pairs' old notes");
    for n in &seeded {
        let g = gold[n.id.as_str()];
        assert!(pairs.iter().any(|p| p.old == n.id), "{}", n.id);
        assert_eq!(
            (&n.kind, &n.scope, &n.project),
            (&g.kind, &g.scope, &g.project),
            "{}",
            n.id
        );
        assert_eq!(
            (&n.fact_key, &n.aliases, &n.tags),
            (&g.fact_key, &g.aliases, &g.tags),
            "{}",
            n.id
        );
        assert_eq!(n.valid_to, None, "{} records its own end", n.id);
        let text = format!("{}\n{}", n.title, n.body).to_lowercase();
        for hindsight in ["until", "replaced", "historical", "以前", "卖掉", "历史"] {
            assert!(
                !text.contains(hindsight),
                "{} ({:?}) is written in hindsight: {hindsight:?}",
                n.id,
                n.fact_key
            );
        }
        let input = store_input_for(n, &BTreeMap::new()).expect("storable as it is");
        assert!(input.body.starts_with("## Knowledge\n"), "{}", n.id);
    }
    // Only the hindsight notes are rewritten; the other four are the gold
    // text, and every rewritten one is seeded.
    let rewritten: BTreeSet<&str> = seeded
        .iter()
        .filter(|n| n.body != gold[n.id.as_str()].body)
        .map(|n| n.id.as_str())
        .collect();
    let listed: BTreeSet<&str> = LS_SEEDED_AS_SAVED_THEN.iter().map(|(id, ..)| *id).collect();
    assert_eq!(rewritten, listed);
    let hindsight: BTreeSet<&str> = seeded
        .iter()
        .filter(|n| gold[n.id.as_str()].valid_to.is_some())
        .map(|n| n.id.as_str())
        .collect();
    assert_eq!(hindsight, listed, "the four with a gold `valid_to`");
}

// ─── Settings ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum System {
    #[serde(rename = "none")]
    NoMemory,
    D,
    E,
    /// Conversations in, learning on (design 09 §7.2).
    L,
    /// Conversations in, learning off: only the reader's own `memory_store`
    /// calls learn.
    L0,
    /// The MD-11/MD-12 check: suggestions against notes the user saved.
    #[serde(rename = "L-S")]
    LS,
}

impl System {
    fn from_label(s: &str) -> Result<Self, String> {
        match s.trim() {
            "none" | "NONE" | "None" => Ok(System::NoMemory),
            "D" | "d" => Ok(System::D),
            "E" | "e" => Ok(System::E),
            "L" | "l" => Ok(System::L),
            "L0" | "l0" => Ok(System::L0),
            "L-S" | "l-s" | "LS" | "ls" => Ok(System::LS),
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
            other => Err(format!(
                "{other:?} is not one of none | D | E | L | L0 | L-S"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            System::NoMemory => "none",
            System::D => "D",
            System::E => "E",
            System::L => "L",
            System::L0 => "L0",
            System::LS => "L-S",
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
            System::L => {
                "conversations in, learning on: the corpus fed through the gateway, which \
                 persists every turn and feeds a manual learning queue, flushed after every \
                 10th user turn of a session and at its end; then the questions, asked as D \
                 asks them with learning off"
            }
            System::L0 => {
                "conversations in, learning off: the corpus fed through the gateway and never \
                 flushed, so only the reader's own memory_store calls learn; then the \
                 questions, asked as D asks them"
            }
            System::LS => {
                "the MD-11/MD-12 check: the 8 change pairs' old notes seeded as the user's own, \
                 the suggestion sessions fed with learning on, and each suggestion confirmed \
                 through the user-directed confirm; no questions"
            }
        }
    }

    /// The corpus is fed through the gateway before anything is measured.
    fn feeds_corpus(self) -> bool {
        matches!(self, System::L | System::L0 | System::LS)
    }

    /// The learning queue flushes what was fed.
    fn learns(self) -> bool {
        matches!(self, System::L | System::LS)
    }

    /// The held-out questions are asked: every system but L-S.
    fn asks_questions(self) -> bool {
        self != System::LS
    }
}

/// The main loop's round and per-round tool caps a system runs under, and
/// the follow-up search bound they express.
///
/// D (and the learning systems) run the shipped caps (`[orchestrator.routing]`,
/// 8 rounds × 4 tool calls), so their model-directed searches are bounded only
/// by them. E narrows the same two caps to design §5's bound — no new
/// mechanism, the main loop's existing tool budget: one tool call per round
/// and `FOLLOW_UP_SEARCHES + 1` rounds, so at most two tool calls can inform
/// an answer written in the last round. The approximation, recorded in every
/// E report:
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
            System::NoMemory | System::D | System::L | System::L0 | System::LS => Self {
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
    for system in [
        System::NoMemory,
        System::D,
        System::L,
        System::L0,
        System::LS,
    ] {
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

/// The learning systems (design 09 §7.2): what each feeds, whether it
/// learns, and whether it asks the questions.
#[test]
fn the_learning_systems_feed_the_corpus() {
    assert_eq!(System::from_label("L"), Ok(System::L));
    assert_eq!(System::from_label("l0"), Ok(System::L0));
    assert_eq!(System::from_label("L-S"), Ok(System::LS));
    assert_eq!(System::from_label("LS"), Ok(System::LS));
    let json = |s: System| serde_json::to_value(s).unwrap();
    assert_eq!(json(System::L), "L");
    assert_eq!(json(System::L0), "L0");
    assert_eq!(json(System::LS), "L-S");
    assert_eq!(json(System::NoMemory), "none");
    for s in [System::L, System::L0, System::LS] {
        assert!(s.feeds_corpus(), "{}", s.as_str());
    }
    for s in [System::NoMemory, System::D, System::E] {
        assert!(!s.feeds_corpus() && !s.learns(), "{}", s.as_str());
        assert!(s.asks_questions());
    }
    assert!(System::L.learns() && System::L.asks_questions());
    assert!(!System::L0.learns() && System::L0.asks_questions());
    assert!(System::LS.learns() && !System::LS.asks_questions());
}

struct Settings {
    /// `None` only for a re-judge, which carries the archived report's.
    system: Option<System>,
    report_path: PathBuf,
    /// `None` only for a re-judge, which touches no store.
    home_store: Option<PathBuf>,
    llm_toml: PathBuf,
    questions_path: PathBuf,
    limit: Option<usize>,
    category: Option<Category>,
    judge: JudgeRequest,
    keep_scratch: bool,
    /// L, L0, L-S: feed only the first N sessions — a smoke run.
    session_limit: Option<usize>,
    /// Replay (design 09 §7.2): an L run's kept scratch directory.
    learn_from: Option<PathBuf>,
    /// Re-judge (design 09 §7.4): an archived report to grade again.
    rejudge_from: Option<PathBuf>,
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).as_deref() == Ok("1")
}

fn env_number(name: &str) -> Option<usize> {
    env_nonempty(name).map(|v| {
        v.trim()
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("memory_eval: {name}={v} is not a number"))
    })
}

fn env_absolute(name: &str) -> Option<PathBuf> {
    env_nonempty(name).map(|v| {
        let path = PathBuf::from(v);
        assert!(
            path.is_absolute(),
            "memory_eval: {name} must be an absolute path"
        );
        path
    })
}

impl Settings {
    /// Every pre-flight check happens here, before a router exists, so a
    /// misconfigured run refuses without a single model call.
    fn from_env() -> Self {
        let rejudge_from = env_absolute("OPENALPACA_MEMORY_EVAL_REJUDGE");
        if let Some(path) = &rejudge_from {
            assert!(
                path.is_file(),
                "memory_eval: OPENALPACA_MEMORY_EVAL_REJUDGE names no file: {}",
                path.display()
            );
        }
        let system = match env_nonempty("OPENALPACA_MEMORY_EVAL_SYSTEM") {
            Some(label) => {
                Some(System::from_label(&label).unwrap_or_else(|e| panic!("memory_eval: {e}")))
            }
            None if rejudge_from.is_some() => None,
            None => panic!(
                "memory_eval: OPENALPACA_MEMORY_EVAL_SYSTEM must be one of none | D | E | L | L0 | L-S \
                 (or set OPENALPACA_MEMORY_EVAL_REJUDGE to re-grade an archived report)"
            ),
        };
        let report_path = env_absolute("OPENALPACA_MEMORY_EVAL_REPORT").unwrap_or_else(|| {
            panic!("memory_eval: OPENALPACA_MEMORY_EVAL_REPORT must name the JSON report to write")
        });

        // The scratch store: where the memory partitions, their control
        // snapshots and the index cache go. It has to be named explicitly,
        // absolute, and must not be the real one. A re-judge touches none.
        let home_store = env_absolute("OPENALPACA_HOME_STORE");
        match &home_store {
            Some(home_store) => {
                if let Some(real) =
                    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".openalpaca"))
                {
                    let same = *home_store == real
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
                openalpaca_storage::store::home_root().unwrap_or_else(|e| {
                    panic!("memory_eval: OPENALPACA_HOME_STORE is not usable: {e}")
                });
            }
            None => assert!(
                rejudge_from.is_some(),
                "memory_eval: OPENALPACA_HOME_STORE must name a scratch store directory \
                 (never the real ~/.openalpaca); see scripts/eval/memory/README.md"
            ),
        }

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
        let category = env_nonempty("OPENALPACA_MEMORY_EVAL_CATEGORY").map(|v| {
            Category::ALL
                .into_iter()
                .find(|c| c.as_str() == v.trim())
                .unwrap_or_else(|| {
                    panic!("memory_eval: OPENALPACA_MEMORY_EVAL_CATEGORY={v} is not a category")
                })
        });
        let learn_from = env_absolute("OPENALPACA_MEMORY_EVAL_LEARN_FROM");
        if let Some(dir) = &learn_from {
            assert_eq!(
                system,
                Some(System::L),
                "memory_eval: OPENALPACA_MEMORY_EVAL_LEARN_FROM replays L's learning phase; set OPENALPACA_MEMORY_EVAL_SYSTEM=L"
            );
            for file in [SCRATCH_DB, TRANSCRIPT_FILE] {
                assert!(
                    dir.join(file).is_file(),
                    "memory_eval: {} has no {file}: name the scratch directory an L run kept",
                    dir.display()
                );
            }
        }

        Settings {
            system,
            report_path,
            home_store,
            llm_toml,
            questions_path,
            limit: env_number("OPENALPACA_MEMORY_EVAL_LIMIT"),
            category,
            judge: JudgeRequest::from_env_value(env_nonempty("OPENALPACA_MEMORY_EVAL_JUDGE_MODEL")),
            keep_scratch: env_flag("OPENALPACA_MEMORY_EVAL_KEEP"),
            session_limit: env_number("OPENALPACA_MEMORY_EVAL_SESSIONS"),
            learn_from,
            rejudge_from,
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
    /// No learning ran at any point of the run: `[orchestrator.learning]
    /// enabled = false` throughout (D, E, none, L0). Derived from `learning`
    /// since the learning evaluation (design 09 §7.2); it was hard-coded.
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
    /// `[orchestrator.memory] recall_mode` (design 09 §13.2): how the
    /// per-turn recall delivered its matches, `index` or `excerpts`.
    recall_mode: &'static str,
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
    // ── Added with the learning evaluation (design 09 §7) ─────────────────
    /// The judge every call pinned — the reader's own model unless
    /// `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` names another (owner answer LQ1);
    /// `null` for a dry run with no judge.
    judge_model: Option<String>,
    /// LQ1: the reader graded its own answers. Recorded, never refused.
    judge_is_reader: Option<bool>,
    /// The judge's family (its id before `:`) is the reader's.
    judge_same_family: Option<bool>,
    /// The judge is the model the learning extractor answers with.
    judge_is_extractor: Option<bool>,
    /// The learning phase's conditions (L, L0, L-S, a replay); `null` for D,
    /// E and none.
    learning: Option<LearningConditions>,
}

/// How the learning phase ran (design 09 §7.2).
#[derive(Debug, Serialize)]
struct LearningConditions {
    corpus_path: String,
    corpus_sha256: String,
    sessions_fed: usize,
    turns_fed: usize,
    /// `OPENALPACA_MEMORY_EVAL_SESSIONS`: only the first N sessions (a smoke
    /// run).
    session_limit: Option<usize>,
    /// The queue flushed what was fed (L, L-S, a replay). L0 feeds with
    /// learning off: only the reader's own `memory_store` calls learn.
    learning_on: bool,
    /// Always `manual` (design 09 §1.5): no sweeper and no trigger starts a
    /// flush; the harness replays the triggers deterministically.
    queue_mode: &'static str,
    /// A `turns` flush after every this-many user turns of a session, and a
    /// `switch` flush at its end. A replay flushes each session whole.
    turns_per_flush: u32,
    /// `[orchestrator.costs] extraction_model`; `null` is the reader's model.
    extraction_model: Option<String>,
    /// The daily extraction caps, lifted so a cap never stops the phase:
    /// `0` calls is no cap.
    extract_max_calls_per_day: u32,
    extract_max_daily_cost_usd: f64,
    /// The questions run with `[orchestrator.learning] enabled = false`.
    learning_during_questions: bool,
    /// Every fed turn is unattended, as every question is (M6).
    unattended: bool,
    /// L-S: the change pairs' old notes seeded as the user's own.
    seeded_old_notes: usize,
    /// L-S: the old notes seeded as the user saved them then — a
    /// present-tense title and body, no `valid_to` — instead of the gold
    /// note's hindsight text (`LS_SEEDED_AS_SAVED_THEN`).
    seeded_as_saved_then: Vec<String>,
    /// A replay: the L run's kept scratch directory it learned from.
    replayed_from: Option<String>,
    /// L: where the scratch database, its project and the transcript map
    /// were kept, for `OPENALPACA_MEMORY_EVAL_LEARN_FROM`.
    scratch_kept: Option<String>,
    coverage_judge_prompt_path: String,
    coverage_judge_prompt_sha256: String,
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
    /// L, L0, L-S and a replay: what the learning phase did and what the
    /// store holds after it (design 09 §7.3). `null` for D, E and none.
    learning: Option<LearningResults>,
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
fn write_report(path: &Path, report: &impl Serialize) {
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

// ─── Judge (design 09 §7.4) ─────────────────────────────────────────────────
//
// Owner answer LQ1: the reader's own model judges, and the report says so.
// The guard therefore refuses only a judge the router cannot route — the
// ladder would silently answer with another model in its place — and
// records a judge equal to the reader, or of the reader's family, as
// `judge_is_reader` / `judge_same_family`. After the first judge call it
// asserts that the model that answered is the model requested.

/// What `OPENALPACA_MEMORY_EVAL_JUDGE_MODEL` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JudgeRequest {
    /// Unset: the reader's own model judges (LQ1).
    Reader,
    /// A model id the eval's `llm.toml` serves.
    Model(String),
    /// `none`: no judge call is made and every row is `not_judged` — the dry
    /// run of the README, where no provider is enabled.
    Off,
}

impl JudgeRequest {
    fn from_env_value(value: Option<String>) -> Self {
        match value.as_deref().map(str::trim) {
            None | Some("") => JudgeRequest::Reader,
            Some("none") | Some("off") => JudgeRequest::Off,
            Some(model) => JudgeRequest::Model(model.to_string()),
        }
    }

    fn requested(&self) -> Option<&str> {
        match self {
            JudgeRequest::Model(model) => Some(model),
            JudgeRequest::Reader | JudgeRequest::Off => None,
        }
    }
}

/// The judge a run pins every call to, and what the guard recorded about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct JudgeChoice {
    model: String,
    reader: Option<String>,
    judge_is_reader: bool,
    judge_same_family: bool,
    judge_is_extractor: bool,
}

/// A model's family: its id before the tag (`qwen3.8` of `qwen3.8:27b-mlx`).
fn model_family(id: &str) -> &str {
    id.split_once(':').map_or(id, |(family, _)| family)
}

/// The judge guard, before any call (design 09 §7.4 as LQ1 rewrote it): the
/// requested judge, or the reader's model, must be routable; being the
/// reader, of its family, or the extractor is recorded, never refused.
/// `extraction_model` `None` is the reader's model (the extractor answers
/// with the effective default).
fn judge_guard(
    router: &openalpaca_llm::LlmRouter,
    requested: Option<&str>,
    reader: Option<&str>,
    extraction_model: Option<&str>,
) -> Result<JudgeChoice, String> {
    let Some(model) = requested.or(reader) else {
        return Err(
            "no judge: no reader model is routable and OPENALPACA_MEMORY_EVAL_JUDGE_MODEL names none"
                .to_string(),
        );
    };
    if !router.is_routable(model) {
        return Err(format!(
            "the judge model {model} is not routable: no loaded provider serves it, and the \
             router would answer with another model in its place. Name a model the eval's \
             llm.toml serves in OPENALPACA_MEMORY_EVAL_JUDGE_MODEL, or `none` for a dry run"
        ));
    }
    Ok(JudgeChoice {
        model: model.to_string(),
        reader: reader.map(str::to_string),
        judge_is_reader: reader == Some(model),
        judge_same_family: reader.is_some_and(|r| model_family(r) == model_family(model)),
        judge_is_extractor: extraction_model.or(reader) == Some(model),
    })
}

/// The post-call half of the guard: the model that answered is the judge
/// requested — never a rung of the ladder standing in for it.
fn answering_judge_is_requested(requested: &str, answered: &str) -> Result<(), String> {
    if requested == answered {
        Ok(())
    } else {
        Err(format!(
            "the judge that answered was {answered}, not {requested}, the judge requested: the \
             router substituted another model"
        ))
    }
}

/// The judge every call pins, with the guard's post-call assertion: the
/// first call's mismatch stops the run; a later one fails its own row.
struct JudgeGate {
    choice: JudgeChoice,
    checked: bool,
}

impl JudgeGate {
    fn new(choice: JudgeChoice) -> Self {
        Self {
            choice,
            checked: false,
        }
    }

    fn check(&mut self, answered: &str) -> Result<(), String> {
        let result = answering_judge_is_requested(&self.choice.model, answered);
        if !std::mem::replace(&mut self.checked, true)
            && let Err(e) = &result
        {
            panic!("memory_eval: {e}");
        }
        result
    }
}

/// Resolve the run's judge, refusing an unroutable one before any call.
fn resolve_judge(
    request: &JudgeRequest,
    router: &openalpaca_llm::LlmRouter,
    reader: Option<&str>,
    extraction_model: Option<&str>,
) -> Option<JudgeGate> {
    if *request == JudgeRequest::Off {
        eprintln!(
            "memory_eval: no judge (OPENALPACA_MEMORY_EVAL_JUDGE_MODEL=none): every row is not_judged"
        );
        return None;
    }
    let choice = judge_guard(router, request.requested(), reader, extraction_model)
        .unwrap_or_else(|e| panic!("memory_eval: {e}"));
    eprintln!(
        "memory_eval: judge {} (judge_is_reader={}, judge_same_family={})",
        choice.model, choice.judge_is_reader, choice.judge_same_family
    );
    Some(JudgeGate::new(choice))
}

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

/// The last non-empty line, with the decoration a model adds around it
/// (`**CORRECT**`, `Verdict: …` is not accepted) trimmed off.
fn last_line(raw: &str) -> Option<String> {
    raw.lines()
        .rev()
        .map(|l| l.trim().trim_matches(|c: char| !c.is_alphanumeric()))
        .find(|l| !l.is_empty())
        .map(str::to_ascii_uppercase)
}

/// The verdict is the last non-empty line, one of the two words; anything
/// else is `unparsed` and counted as not correct.
fn parse_verdict(raw: &str) -> Option<bool> {
    match last_line(raw)?.as_str() {
        "INCORRECT" => Some(false),
        "CORRECT" => Some(true),
        _ => None,
    }
}

struct JudgeReply {
    raw: String,
    model: String,
    tokens_in: u32,
    tokens_out: u32,
}

/// One judge call at temperature 0 with reasoning off, pinned to the gate's
/// model; the answering model is checked against it.
async fn judge_call(
    router: &openalpaca_llm::LlmRouter,
    gate: &mut JudgeGate,
    agent_id: &str,
    prompt: &str,
) -> Result<JudgeReply, String> {
    use openalpaca_llm::{ChatMessage, RequestContext, RouterRequest, ThinkingConfig};
    let request = RouterRequest {
        model: Some(gate.choice.model.clone()),
        messages: Arc::new(vec![ChatMessage::user(prompt)]),
        tools: Arc::new(Vec::new()),
        temperature: Some(JUDGE_TEMPERATURE),
        max_tokens: Some(JUDGE_MAX_TOKENS),
        context: RequestContext {
            agent_id: Some(agent_id.to_string()),
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
    gate.check(&response.model)?;
    Ok(JudgeReply {
        raw: response.content,
        model: response.model,
        tokens_in: response.usage.input_tokens,
        tokens_out: response.usage.output_tokens,
    })
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
    gate: &mut JudgeGate,
    template: &str,
    q: &Question,
    answer: &str,
) -> Result<JudgeOutcome, String> {
    let prompt = fill_judge_prompt(template, q, answer);
    let reply = judge_call(router, gate, JUDGE_AGENT_ID, &prompt).await?;
    Ok(JudgeOutcome {
        verdict: parse_verdict(&reply.raw),
        raw: reply.raw,
        model: reply.model,
        tokens_in: reply.tokens_in,
        tokens_out: reply.tokens_out,
    })
}

/// Cohen's κ of two binary gradings of the same rows; `None` when chance
/// agreement is total (both always said the same one word).
fn cohen_kappa(pairs: &[(bool, bool)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let n = pairs.len() as f64;
    let observed = pairs.iter().filter(|(a, b)| a == b).count() as f64 / n;
    let a_yes = pairs.iter().filter(|(a, _)| *a).count() as f64 / n;
    let b_yes = pairs.iter().filter(|(_, b)| *b).count() as f64 / n;
    let chance = a_yes * b_yes + (1.0 - a_yes) * (1.0 - b_yes);
    (chance < 1.0).then(|| (observed - chance) / (1.0 - chance))
}

// ─── The coverage judge (design 09 §7.3) ────────────────────────────────────

/// The coverage judge's placeholders: the frozen prompt carries each once.
const COVERAGE_PLACEHOLDERS: [&str; 3] = ["{{gold_title}}", "{{gold_knowledge}}", "{{candidates}}"];
/// "A historical `memory_search` on its title and aliases returns at most 5
/// notes."
const COVERAGE_HITS: usize = 5;
/// A returned note's knowledge is cut here before the judge sees it.
const COVERAGE_KNOWLEDGE_CHARS: usize = 1_200;

/// The coverage search's query: the gold note's title and aliases, within
/// the service's query cap. The index ORs its terms (`memory_index::query`).
fn coverage_query(note: &GoldNote) -> String {
    let mut query = note.title.clone();
    for alias in &note.aliases {
        query.push(' ');
        query.push_str(alias);
    }
    query
        .chars()
        .take(openalpaca_storage::memory_index::MAX_QUERY_CHARS)
        .collect()
}

/// A note body's `## Knowledge` section (design 09 §2.6), trimmed; the whole
/// body when it has none.
fn knowledge_section(body: &str) -> String {
    let mut inside = false;
    let mut found = false;
    let mut lines = Vec::new();
    for line in body.lines() {
        let heading = line.trim_end();
        if heading.starts_with("## ") || heading == "##" {
            if inside {
                break;
            }
            if heading[2..].trim().eq_ignore_ascii_case("knowledge") {
                inside = true;
                found = true;
            }
            continue;
        }
        if inside {
            lines.push(line);
        }
    }
    if found {
        lines.join("\n").trim().to_string()
    } else {
        body.trim().to_string()
    }
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut} […]")
}

fn fill_coverage_prompt(template: &str, gold: &GoldNote, hits: &[(String, String)]) -> String {
    let candidates = hits
        .iter()
        .enumerate()
        .map(|(i, (title, knowledge))| {
            format!(
                "[{}] {title}\n{}",
                i + 1,
                cut_chars(knowledge, COVERAGE_KNOWLEDGE_CHARS)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    template
        .replace("{{gold_title}}", &gold.title)
        .replace("{{gold_knowledge}}", &knowledge_section(&gold.body))
        .replace("{{candidates}}", &candidates)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageVerdict {
    /// The 1-based number of the matching note.
    Match(usize),
    NoMatch,
    Unparsed,
}

/// The last line: `MATCH <n>` naming one of the `candidates` notes, or
/// `NONE`; anything else is unparsed.
fn parse_coverage_verdict(raw: &str, candidates: usize) -> CoverageVerdict {
    let Some(line) = last_line(raw) else {
        return CoverageVerdict::Unparsed;
    };
    if line == "NONE" {
        return CoverageVerdict::NoMatch;
    }
    if let Some(rest) = line.strip_prefix("MATCH") {
        let number = rest.trim().trim_start_matches([':', '#', '[', ' ']);
        if let Ok(k) = number.trim_end_matches(']').trim().parse::<usize>()
            && (1..=candidates).contains(&k)
        {
            return CoverageVerdict::Match(k);
        }
    }
    CoverageVerdict::Unparsed
}

// ─── The learning phase (design 09 §7.2) ────────────────────────────────────

/// The scratch database's file name, in the run's tempdir.
const SCRATCH_DB: &str = "eval.db";
/// What an L run keeps beside its database for a replay: which session holds
/// which corpus session, and each turn's user row.
const TRANSCRIPT_FILE: &str = "learning-transcript.json";
/// The flush labels (design 09 §7.2): after every 10th user turn, at a
/// session's end, a replay's whole-session flush, and the forced re-run.
const FLUSH_TURNS: &str = "turns";
const FLUSH_SWITCH: &str = "switch";
const FLUSH_REPLAY: &str = "replay";
const FLUSH_FORCED_RERUN: &str = "forced_rerun";
/// The daily extraction cost cap the harness sets: never binding.
const LIFTED_DAILY_COST_USD: f64 = 1_000_000.0;
const COVERAGE_JUDGE_AGENT_ID: &str = "memory_eval_coverage_judge";

/// The eval's daemon config for a run: no workflow can dispatch, task-output
/// extraction is off, and — for a system that learns — the extraction's
/// daily caps are lifted, so a cap never stops a learning phase part-way.
fn eval_daemon_config(system: System) -> openalpaca_core::daemon_config::DaemonConfig {
    let mut cfg = openalpaca_core::daemon_config::DaemonConfig::default();
    // A stray `start_workflow` is refused at the cap instead of dispatching a
    // real (costly) lead-agent run; the refusal still shows as a tool call.
    cfg.orchestrator.routing.max_workflows_per_lane = 0;
    cfg.orchestrator.costs.task_extract_enabled = false;
    // Learning runs only in the learning phase of a system that learns; the
    // questions switch it off (`learning_off`).
    cfg.orchestrator.learning.enabled = system.learns();
    if system.learns() {
        cfg.orchestrator.costs.extract_max_calls_per_day = 0;
        cfg.orchestrator.costs.extract_max_daily_cost_usd = LIFTED_DAILY_COST_USD;
    }
    let budget = MainLoopBudget::for_system(system, &cfg.orchestrator.routing);
    cfg.orchestrator.routing.main_loop_max_rounds = budget.max_rounds;
    cfg.orchestrator.routing.main_loop_max_tools_per_round = budget.max_tools_per_round;
    cfg
}

/// Swap `[orchestrator.learning] enabled` in the live config.
fn set_learning(
    config: &arc_swap::ArcSwap<openalpaca_core::daemon_config::DaemonConfig>,
    enabled: bool,
) {
    let mut cfg = (**config.load()).clone();
    cfg.orchestrator.learning.enabled = enabled;
    config.store(Arc::new(cfg));
}

fn new_owner_id() -> String {
    format!(
        "{OWNER_PREFIX}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// A scratch project: a directory with a `.git` marker, canonical.
fn scratch_project(under: &Path) -> PathBuf {
    let project = under.join("project");
    std::fs::create_dir_all(project.join(".git")).expect("scratch project marker");
    project.canonicalize().expect("canonical scratch project")
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("a blocking task panicked")
}

/// The daemon's `OrchestratorHandler` (`apps/openalpacad/src/gateway_bridge.rs`),
/// ten lines of it: the turn, then the per-request maps the bridge drains.
struct EvalHandler(Arc<openalpaca_core::orchestrator::Orchestrator>);

#[async_trait::async_trait]
impl openalpaca_core::gateway::MessageHandler for EvalHandler {
    async fn handle(
        &self,
        request: openalpaca_core::gateway::HandleRequest,
    ) -> Result<openalpaca_core::gateway::HandleResult, String> {
        let request_id = request.request_id;
        let content = self.0.handle_message(request).await;
        let meta = self.0.llm_metadata_map.remove(&request_id).map(|(_, v)| v);
        self.0.delegation_map.remove(&request_id);
        self.0.attachments_skipped_map.remove(&request_id);
        let memory_used = self.0.take_memory_used(request_id);
        Ok(openalpaca_core::gateway::HandleResult {
            model: meta.as_ref().map(|m| m.model.clone()),
            tokens_in: meta.as_ref().map(|m| m.tokens_in),
            tokens_out: meta.as_ref().map(|m| m.tokens_out),
            memory_used,
            ..openalpaca_core::gateway::HandleResult::text(content?)
        })
    }
}

/// The orchestrator as every system builds it: the eval's config, the reader's
/// loop at its defaults, no skills, and — with a memory service — the service
/// installed and its read tools registered as the daemon registers them.
struct EvalStack {
    orch: Arc<openalpaca_core::orchestrator::Orchestrator>,
    shared: Arc<openalpaca_core::context::SharedContext>,
    lanes: Arc<openalpaca_core::lane::LaneManager>,
}

fn eval_stack(
    router: Arc<openalpaca_llm::LlmRouter>,
    db: &openalpaca_storage::Database,
    bus: &openalpaca_core::bus::EventBus,
    service: Option<&Arc<MemoryService>>,
    daemon_config: Arc<arc_swap::ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
) -> EvalStack {
    use openalpaca_core::context::SharedContext;
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::middleware::prompt::SystemPersona;
    use openalpaca_core::orchestrator::{Orchestrator, skill_catalog, skill_router};
    use openalpaca_core::runner::LoopConfig;
    use openalpaca_core::security::gate::SecurityGate;
    use openalpaca_core::security::sandbox::SandboxManager;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_core::tools::builtins::memory_read_tools;

    let shared = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let registry = Arc::new(ToolRegistry::default());
    let sandbox = Arc::new(SandboxManager::with_defaults(registry.clone(), bus.clone()));
    let gate = Arc::new(SecurityGate::new(sandbox));
    if let Some(service) = service {
        for tool in memory_read_tools(service.clone()) {
            registry
                .register(tool)
                .expect("register a memory read tool");
        }
    }
    let orch = Arc::new(Orchestrator::new(
        shared.clone(),
        lanes.clone(),
        bus.clone(),
        SystemPersona::default(),
        Some(router),
        LoopConfig::default(),
        gate,
        registry,
        Some(db.clone()),
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        daemon_config,
    ));
    if let Some(service) = service {
        orch.set_memory_service(service.clone());
    }
    EvalStack {
        orch,
        shared,
        lanes,
    }
}

/// One note a flush wrote, from its `memory_changed` event.
#[derive(Debug, Clone, Serialize)]
struct WrittenNote {
    note: String,
    change: String,
    origin: String,
    surface: Option<String>,
}

/// One flush (design 09 §7.2): what `flush_session` reported, its latency,
/// and the notes its bus events say it wrote.
#[derive(Debug, Clone, Serialize)]
struct FlushRecord {
    session: String,
    /// `turns` | `switch` | `replay` | `forced_rerun`.
    label: &'static str,
    latency_ms: u64,
    calls: usize,
    chunks: usize,
    input_tokens: u64,
    output_tokens: u64,
    stored: usize,
    active: usize,
    candidates: usize,
    replaced: usize,
    suggestions: usize,
    deduplicated: usize,
    dropped_over_cap: usize,
    dropped_below_threshold: usize,
    dropped_invalid: usize,
    dropped_suppressed: usize,
    dropped_duplicate_key: usize,
    dropped_already_learned: usize,
    dropped_unchanged: usize,
    dropped_conflict: usize,
    replaces_unknown: usize,
    failed: usize,
    skipped_short: usize,
    skipped_no_owner: usize,
    skipped_unusable: usize,
    skipped_root_gone: usize,
    /// Why the flush stopped before its window's end; `null` when it did not.
    stopped: Option<String>,
    written: Vec<WrittenNote>,
    /// Bus events the subscription missed (a lagged receiver).
    events_lagged: u64,
}

impl FlushRecord {
    fn new(
        session: &str,
        label: &'static str,
        latency_ms: u64,
        r: &openalpaca_core::memory::learning::FlushReport,
        written: Vec<WrittenNote>,
        events_lagged: u64,
    ) -> Self {
        Self {
            session: session.to_string(),
            label,
            latency_ms,
            calls: r.calls,
            chunks: r.chunks,
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            stored: r.stored,
            active: r.active,
            candidates: r.candidates,
            replaced: r.replaced,
            suggestions: r.suggestions,
            deduplicated: r.deduplicated,
            dropped_over_cap: r.dropped_over_cap,
            dropped_below_threshold: r.dropped_below_threshold,
            dropped_invalid: r.dropped_invalid,
            dropped_suppressed: r.dropped_suppressed,
            dropped_duplicate_key: r.dropped_duplicate_key,
            dropped_already_learned: r.dropped_already_learned,
            dropped_unchanged: r.dropped_unchanged,
            dropped_conflict: r.dropped_conflict,
            replaces_unknown: r.replaces_unknown,
            failed: r.failed,
            skipped_short: r.skipped_short,
            skipped_no_owner: r.skipped_no_owner,
            skipped_unusable: r.skipped_unusable,
            skipped_root_gone: r.skipped_root_gone,
            stopped: r.stopped.map(|s| format!("{s:?}")),
            written,
            events_lagged,
        }
    }
}

/// Every `memory_changed` event waiting on a receiver, and how many were
/// lost to lag.
fn drain_memory_changes(
    rx: &mut tokio::sync::broadcast::Receiver<openalpaca_core::events::SystemEvent>,
) -> (Vec<WrittenNote>, u64) {
    use openalpaca_core::events::SystemEvent;
    use tokio::sync::broadcast::error::TryRecvError;
    let mut written = Vec::new();
    let mut lagged = 0;
    loop {
        match rx.try_recv() {
            Ok(SystemEvent::MemoryChanged {
                note_id,
                change,
                origin,
                surface,
                ..
            }) => written.push(WrittenNote {
                note: note_id,
                change,
                origin,
                surface,
            }),
            Ok(_) => {}
            Err(TryRecvError::Lagged(n)) => lagged += n,
            Err(_) => break,
        }
    }
    (written, lagged)
}

/// `flush_session`, with the bus subscribed around it (design 09 §7.2,
/// *Counting the two learning paths*).
async fn flush_one(
    queue: &openalpaca_core::memory::learning::LearningQueue,
    bus: &openalpaca_core::bus::EventBus,
    session_id: &str,
    corpus_id: &str,
    label: &'static str,
) -> FlushRecord {
    let mut rx = bus.subscribe();
    let t0 = Instant::now();
    let report = queue.flush_session(session_id).await;
    let latency_ms = t0.elapsed().as_millis() as u64;
    let (written, lagged) = drain_memory_changes(&mut rx);
    let record = FlushRecord::new(corpus_id, label, latency_ms, &report, written, lagged);
    eprintln!(
        "memory_eval: flush {corpus_id} ({label}): {} call(s), {} chunk(s), {} stored ({} active, {} candidates, {} replaced, {} suggestions), {} ms{}",
        record.calls,
        record.chunks,
        record.stored,
        record.active,
        record.candidates,
        record.replaced,
        record.suggestions,
        latency_ms,
        record
            .stopped
            .as_deref()
            .map(|s| format!(", stopped: {s}"))
            .unwrap_or_default()
    );
    record
}

/// One fed turn: its corpus place, its user row and what the reader did.
#[derive(Debug, Serialize)]
struct TurnRecord {
    session: String,
    turn: usize,
    mode: TurnMode,
    states: Vec<String>,
    /// The user row the gateway persisted (`message:<row>` in a note's
    /// sources).
    row: Option<i64>,
    latency_ms: u64,
    is_error: bool,
    answer_chars: usize,
    reader_llm_calls: u32,
    reader_tokens_in: u64,
    reader_tokens_out: u64,
    reader_models: Vec<String>,
    tools: Vec<String>,
    /// The reader's own memory writes this turn (`memory_changed`, surface
    /// `model_tool`) — the learning path L0 keeps.
    model_tool_memory_changes: usize,
}

/// Which database session holds which corpus session, and each corpus
/// turn's user row: written beside an L run's kept database, read by a
/// replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Transcript {
    schema: u32,
    owner_id: String,
    /// The scratch project the sessions' rows name as their workspace.
    project: String,
    corpus_sha256: String,
    sessions: Vec<TranscriptSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TranscriptSession {
    corpus_id: String,
    session_id: String,
    /// The user rows, in turn order: the i-th is the corpus session's i-th
    /// turn.
    user_rows: Vec<i64>,
}

impl Transcript {
    /// The user row of a corpus turn.
    fn row_of(&self, corpus_id: &str, turn: usize) -> Option<i64> {
        self.sessions
            .iter()
            .find(|s| s.corpus_id == corpus_id)
            .and_then(|s| s.user_rows.get(turn).copied())
    }
}

fn write_json(path: &Path, value: &impl Serialize) {
    let bytes = serde_json::to_vec_pretty(value).expect("serialize");
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// The learning phase's front door (design 09 §7.2): the gateway persists
/// each turn and feeds a manual queue, as the daemon's gateway feeds its
/// automatic one; `Orchestrator::handle_message` alone persists nothing.
struct Learner {
    gateway: openalpaca_core::gateway::Gateway,
    queue: Arc<openalpaca_core::memory::learning::LearningQueue>,
    bus: openalpaca_core::bus::EventBus,
    db: openalpaca_storage::Database,
    owner_id: String,
    project: PathBuf,
    turns_per_flush: u32,
}

#[derive(Default)]
struct Fed {
    turns: Vec<TurnRecord>,
    flushes: Vec<FlushRecord>,
    transcript: Vec<TranscriptSession>,
    wall_ms: u64,
}

impl Learner {
    /// The gateway over `stack`, persisting every turn and feeding `queue`.
    fn new(
        stack: &EvalStack,
        queue: &Arc<openalpaca_core::memory::learning::LearningQueue>,
        bus: &openalpaca_core::bus::EventBus,
        db: &openalpaca_storage::Database,
        owner_id: &str,
        project: &Path,
        turns_per_flush: u32,
    ) -> Self {
        Self {
            gateway: openalpaca_core::gateway::Gateway::new(
                stack.shared.clone(),
                stack.lanes.clone(),
                Arc::new(EvalHandler(stack.orch.clone())),
                bus.clone(),
                Some(db.clone()),
            )
            .with_learning(queue.clone()),
            queue: queue.clone(),
            bus: bus.clone(),
            db: db.clone(),
            owner_id: owner_id.to_string(),
            project: project.to_path_buf(),
            turns_per_flush,
        }
    }

    fn lane(&self) -> String {
        format!("{}:gui", self.owner_id)
    }

    /// Feed sessions in order, each as "New chat" makes one; with `learn`,
    /// flush after every `turns_per_flush`-th user turn (`turns`) and at the
    /// session's end (`switch`) — the production triggers, replayed
    /// deterministically. The idle trigger has no stand-in (L1's tests).
    async fn feed(&self, sessions: &[&CorpusSession], learn: bool) -> Fed {
        use openalpaca_api::events::EventSource;
        use openalpaca_core::events::SystemEvent;
        use openalpaca_core::gateway::GatewayRequest;
        use openalpaca_core::orchestrator::MAIN_LOOP_AGENT_ID;
        use openalpaca_core::security::policy::{Principal, Scope};
        use openalpaca_storage::ConversationRepository;
        use tokio::sync::broadcast::error::TryRecvError;

        let started = Instant::now();
        let mut fed = Fed::default();
        let lane = self.lane();
        let project = self.project.to_string_lossy().into_owned();
        for (si, session) in sessions.iter().enumerate() {
            let workspace = session.workspace.map(|_| project.clone());
            let repo = ConversationRepository::new(&self.db);
            let created = repo
                .create_session(&lane, "gui", workspace.as_deref(), Some(&session.id))
                .unwrap_or_else(|e| panic!("memory_eval: cannot open session {}: {e}", session.id));
            repo.activate_session(&created.id)
                .unwrap_or_else(|e| panic!("memory_eval: cannot activate {}: {e}", session.id));
            eprintln!(
                "memory_eval: session {} ({}/{}, {}, {}, {} turns)",
                session.id,
                si + 1,
                sessions.len(),
                session.language.as_str(),
                session.workspace.map_or("no project", |w| w.as_str()),
                session.turns.len()
            );
            let first_turn = fed.turns.len();
            for (ti, turn) in session.turns.iter().enumerate() {
                let mut rx = self.bus.subscribe();
                let t0 = Instant::now();
                let response = self
                    .gateway
                    .handle_event(GatewayRequest {
                        source: EventSource::Gui {
                            connection_id: self.owner_id.clone(),
                        },
                        content: turn.user.clone(),
                        attachments: Vec::new(),
                        principal: Principal::User {
                            global_id: self.owner_id.clone(),
                        },
                        scope: Scope::Global,
                        workspace_path: workspace.clone(),
                        stream_id: None,
                        lane_override: None,
                        model_override: None,
                        // Nobody here can answer a confirmation (M6).
                        unattended: true,
                        turn_sink: None,
                    })
                    .await;
                let latency_ms = t0.elapsed().as_millis() as u64;
                let mut record = TurnRecord {
                    session: session.id.clone(),
                    turn: ti,
                    mode: turn.mode,
                    states: turn.states.clone(),
                    row: None,
                    latency_ms,
                    is_error: response.is_error,
                    answer_chars: response.content.chars().count(),
                    reader_llm_calls: 0,
                    reader_tokens_in: 0,
                    reader_tokens_out: 0,
                    reader_models: Vec::new(),
                    tools: Vec::new(),
                    model_tool_memory_changes: 0,
                };
                let mut models = BTreeSet::new();
                loop {
                    match rx.try_recv() {
                        Ok(SystemEvent::LlmCallCompleted {
                            agent_id,
                            model,
                            input_tokens,
                            output_tokens,
                            ..
                        }) if agent_id == MAIN_LOOP_AGENT_ID => {
                            record.reader_llm_calls += 1;
                            record.reader_tokens_in += u64::from(input_tokens);
                            record.reader_tokens_out += u64::from(output_tokens);
                            models.insert(model);
                        }
                        Ok(SystemEvent::ToolExecuted {
                            agent_id,
                            tool_name,
                            ..
                        }) if agent_id == MAIN_LOOP_AGENT_ID => record.tools.push(tool_name),
                        Ok(SystemEvent::MemoryChanged {
                            surface: Some(surface),
                            ..
                        }) if surface == "model_tool" => record.model_tool_memory_changes += 1,
                        Ok(_) => {}
                        Err(TryRecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                record.reader_models = models.into_iter().collect();
                println!(
                    "[turn] {} #{:<2} {:<10} latency={}ms llm_calls={} tools={:?}{}",
                    session.id,
                    ti,
                    turn.mode.as_str(),
                    latency_ms,
                    record.reader_llm_calls,
                    record.tools,
                    if record.is_error { " (error)" } else { "" }
                );
                fed.turns.push(record);
                if learn && (ti + 1) % self.turns_per_flush.max(1) as usize == 0 {
                    fed.flushes.push(
                        flush_one(
                            &self.queue,
                            &self.bus,
                            &created.id,
                            &session.id,
                            FLUSH_TURNS,
                        )
                        .await,
                    );
                }
            }
            if learn {
                fed.flushes.push(
                    flush_one(
                        &self.queue,
                        &self.bus,
                        &created.id,
                        &session.id,
                        FLUSH_SWITCH,
                    )
                    .await,
                );
            }
            // Each fed turn persisted one user row, in order.
            let user_rows: Vec<i64> = repo
                .learning_window(&created.id, 0, 100_000)
                .unwrap_or_else(|e| panic!("memory_eval: cannot read {}: {e}", session.id))
                .into_iter()
                .filter(|r| r.role == "user")
                .map(|r| r.id)
                .collect();
            if user_rows.len() != session.turns.len() {
                eprintln!(
                    "memory_eval: WARNING session {} persisted {} user rows for {} turns; its turns map to no row",
                    session.id,
                    user_rows.len(),
                    session.turns.len()
                );
            } else {
                for (record, row) in fed.turns[first_turn..].iter_mut().zip(&user_rows) {
                    record.row = Some(*row);
                }
            }
            fed.transcript.push(TranscriptSession {
                corpus_id: session.id.clone(),
                session_id: created.id,
                user_rows,
            });
        }
        fed.wall_ms = started.elapsed().as_millis() as u64;
        fed
    }
}

// ─── A replay (design 09 §7.2, *Replay mode*) ───────────────────────────────

/// An L run's kept scratch, copied and made fresh for a replay.
struct ReplayScratch {
    db: openalpaca_storage::Database,
    owner_id: String,
    project: PathBuf,
    transcript: Transcript,
}

/// Copy an L run's kept database into `into` and make it learn afresh: a
/// new owner (so a fresh home partition), a new scratch project (so a fresh
/// project partition, and the kept one untouched), every row that named the
/// old project naming the new one, and every cursor at 0. The kept scratch
/// is only read.
fn prepare_replay(from: &Path, into: &Path) -> Result<ReplayScratch, String> {
    use openalpaca_storage::{Database, IdentityRepository};
    let bytes = std::fs::read(from.join(TRANSCRIPT_FILE))
        .map_err(|e| format!("cannot read {}: {e}", from.join(TRANSCRIPT_FILE).display()))?;
    let mut transcript: Transcript =
        serde_json::from_slice(&bytes).map_err(|e| format!("{TRANSCRIPT_FILE}: {e}"))?;
    for suffix in ["", "-wal", "-shm"] {
        let source = from.join(format!("{SCRATCH_DB}{suffix}"));
        if source.is_file() {
            std::fs::copy(&source, into.join(format!("{SCRATCH_DB}{suffix}")))
                .map_err(|e| format!("cannot copy {}: {e}", source.display()))?;
        }
    }
    let db = Database::open(&into.join(SCRATCH_DB)).map_err(|e| format!("open the copy: {e}"))?;
    let owner_id = new_owner_id();
    IdentityRepository::new(&db)
        .create_global_user(&owner_id, None)
        .map_err(|e| format!("create the replay's owner: {e}"))?;
    let project = scratch_project(into);
    let project_str = project.to_string_lossy().into_owned();
    let old_lane = format!("{}:", transcript.owner_id);
    let new_lane = format!("{owner_id}:");
    let old_project = transcript.project.clone();
    db.with_connection(|c| {
        let rebase = "SET lane_key = ?2 || substr(lane_key, length(?1) + 1) \
                      WHERE substr(lane_key, 1, length(?1)) = ?1";
        c.execute(&format!("UPDATE session {rebase}"), (&old_lane, &new_lane))?;
        c.execute(
            &format!("UPDATE conversation_messages {rebase}"),
            (&old_lane, &new_lane),
        )?;
        c.execute(
            "UPDATE session SET workspace_id = ?2 WHERE workspace_id = ?1",
            (&old_project, &project_str),
        )?;
        c.execute(
            "UPDATE conversation_messages SET workspace_root = ?2 WHERE workspace_root = ?1",
            (&old_project, &project_str),
        )?;
        // "Resets every cursor to 0": a session with no row reads 0.
        c.execute("DELETE FROM learning_cursor", ())?;
        Ok(())
    })
    .map_err(|e| format!("rebase the copy: {e}"))?;
    transcript.owner_id = owner_id.clone();
    transcript.project = project_str;
    Ok(ReplayScratch {
        db,
        owner_id,
        project,
        transcript,
    })
}

/// Flush every stored session in order, whole, with no reader call.
async fn replay_learning(
    queue: &openalpaca_core::memory::learning::LearningQueue,
    bus: &openalpaca_core::bus::EventBus,
    transcript: &Transcript,
) -> Vec<FlushRecord> {
    let mut flushes = Vec::with_capacity(transcript.sessions.len());
    for s in &transcript.sessions {
        flushes.push(flush_one(queue, bus, &s.session_id, &s.corpus_id, FLUSH_REPLAY).await);
    }
    flushes
}

// ─── What is measured (design 09 §7.3) ──────────────────────────────────────

/// One note of the learned store, as the report dumps it — active,
/// superseded and candidate alike.
#[derive(Debug, Clone, Serialize)]
struct LearnedNote {
    id: String,
    title: String,
    kind: &'static str,
    scope: &'static str,
    /// The stored status: `active` | `candidate` | `archived`.
    status: &'static str,
    effective_status: &'static str,
    origin: &'static str,
    fact_key: Option<String>,
    aliases: Vec<String>,
    valid_from: Option<String>,
    valid_to: Option<String>,
    /// Suppression keys: `message:<row>` | `task:<id>`.
    sources: Vec<String>,
    supersedes: Vec<String>,
    superseded_by: Vec<String>,
    conflict: bool,
    source_forgotten: bool,
    /// Its `## Knowledge` section, cut to 2 000 characters.
    knowledge: String,
}

impl LearnedNote {
    fn from_read(r: &openalpaca_core::memory::service::ReadOutput) -> Self {
        let text = |key: &str| {
            r.frontmatter
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        Self {
            id: r.id.to_string(),
            title: r.title.clone(),
            kind: r.kind.as_str(),
            scope: r.scope.as_str(),
            status: r.status.as_str(),
            effective_status: r.effective_status.as_str(),
            origin: r.origin.as_str(),
            fact_key: text("fact_key"),
            aliases: r
                .frontmatter
                .get("aliases")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            valid_from: text("valid_from"),
            valid_to: text("valid_to"),
            sources: r.sources.iter().map(|s| s.suppression_key()).collect(),
            supersedes: r.supersedes.iter().map(ToString::to_string).collect(),
            superseded_by: r.superseded_by.iter().map(ToString::to_string).collect(),
            conflict: r.conflict,
            source_forgotten: r.source_forgotten,
            knowledge: cut_chars(&knowledge_section(&r.body), 2_000),
        }
    }

    /// Learning wrote it: from the user's words or inferred.
    fn is_learned(&self) -> bool {
        matches!(self.origin, "extracted_user" | "extracted_other")
    }

    fn is_active(&self) -> bool {
        self.effective_status == "active"
    }

    fn cites_row(&self, row: i64) -> bool {
        let key = format!("message:{row}");
        self.sources.contains(&key)
    }
}

/// The owner's store, read as the user reads it: both partitions, history
/// and candidates included, archived notes left out.
fn dump_learned(
    service: &MemoryService,
    ctx: &openalpaca_core::memory::service::MemoryAccessContext,
) -> Result<Vec<LearnedNote>, String> {
    use openalpaca_core::memory::service::{ListInput, ReadInput};
    let mut items = Vec::new();
    let mut cursor = None;
    loop {
        let page = service
            .list(
                ctx,
                ListInput {
                    include_history: true,
                    include_candidates: true,
                    limit: Some(100),
                    cursor: cursor.take(),
                    ..Default::default()
                },
            )
            .map_err(|e| format!("list: {e}"))?;
        items.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let mut notes = Vec::with_capacity(items.len());
    for item in items {
        let read = service
            .read(
                ctx,
                ReadInput {
                    id: item.id,
                    ..Default::default()
                },
            )
            .map_err(|e| format!("read {}: {e}", item.id))?;
        notes.push(LearnedNote::from_read(&read));
    }
    Ok(notes)
}

#[derive(Debug, Clone, Serialize)]
struct CoverageHit {
    note: String,
    title: String,
    effective_status: &'static str,
    same_key_loser: bool,
    origin: &'static str,
}

#[derive(Debug, Serialize)]
struct CoverageRow {
    gold: String,
    fact_key: Option<String>,
    title: String,
    query: String,
    hits: Vec<CoverageHit>,
    /// `match` | `none` | `not_found` (the search returned nothing, no call)
    /// | `unparsed` | `judge_error` | `not_judged`.
    verdict: &'static str,
    /// The matching note.
    matched: Option<String>,
    judge_raw: String,
    judge_error: Option<String>,
    judge_tokens_in: u32,
    judge_tokens_out: u32,
}

impl CoverageRow {
    fn matched_hit(&self) -> Option<&CoverageHit> {
        let id = self.matched.as_deref()?;
        self.hits.iter().find(|h| h.note == id)
    }
}

#[derive(Debug, Serialize)]
struct CoverageReport {
    gold_notes: usize,
    matched: usize,
    share: Option<f64>,
    judge_tokens_in: u64,
    judge_tokens_out: u64,
    rows: Vec<CoverageRow>,
}

/// Fact coverage (design 09 §7.3): for each relied-on gold note, a historical
/// search on its title and aliases returns at most 5 notes, and the coverage
/// judge answers whether any of them states the gold knowledge. One call per
/// gold note that the search found anything for.
async fn measure_coverage(
    router: &openalpaca_llm::LlmRouter,
    mut gate: Option<&mut JudgeGate>,
    template: &str,
    service: &Arc<MemoryService>,
    ctx: &openalpaca_core::memory::service::MemoryAccessContext,
    gold: &[&GoldNote],
    learned: &[LearnedNote],
) -> CoverageReport {
    use openalpaca_core::memory::service::{ReadInput, SearchInput};
    let by_id: HashMap<&str, &LearnedNote> = learned.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut rows = Vec::with_capacity(gold.len());
    for note in gold {
        let query = coverage_query(note);
        let (svc, c, q) = (service.clone(), ctx.clone(), query.clone());
        let found = blocking(move || {
            svc.search(
                &c,
                SearchInput {
                    query: q,
                    limit: Some(COVERAGE_HITS),
                    historical: true,
                    ..Default::default()
                },
            )
        })
        .await;
        let mut row = CoverageRow {
            gold: note.id.clone(),
            fact_key: note.fact_key.clone(),
            title: note.title.clone(),
            query,
            hits: Vec::new(),
            verdict: "not_found",
            matched: None,
            judge_raw: String::new(),
            judge_error: None,
            judge_tokens_in: 0,
            judge_tokens_out: 0,
        };
        let results = match found {
            Ok(out) => out.results,
            Err(e) => {
                row.verdict = "judge_error";
                row.judge_error = Some(format!("search: {e}"));
                rows.push(row);
                continue;
            }
        };
        let mut texts = Vec::new();
        for hit in results.iter().take(COVERAGE_HITS) {
            let id = hit.id.to_string();
            let knowledge = match by_id.get(id.as_str()) {
                Some(n) => n.knowledge.clone(),
                None => {
                    let (svc, c, nid) = (service.clone(), ctx.clone(), hit.id);
                    blocking(move || {
                        svc.read(
                            &c,
                            ReadInput {
                                id: nid,
                                ..Default::default()
                            },
                        )
                    })
                    .await
                    .map(|r| knowledge_section(&r.body))
                    .unwrap_or_else(|_| hit.excerpt.clone())
                }
            };
            texts.push((hit.title.clone(), knowledge));
            row.hits.push(CoverageHit {
                note: id,
                title: hit.title.clone(),
                effective_status: hit.effective_status.as_str(),
                same_key_loser: hit.same_key_loser,
                origin: hit.origin.as_str(),
            });
        }
        if !row.hits.is_empty() {
            match gate.as_deref_mut() {
                None => row.verdict = "not_judged",
                Some(gate) => {
                    let prompt = fill_coverage_prompt(template, note, &texts);
                    match judge_call(router, gate, COVERAGE_JUDGE_AGENT_ID, &prompt).await {
                        Ok(reply) => {
                            row.verdict = match parse_coverage_verdict(&reply.raw, row.hits.len()) {
                                CoverageVerdict::Match(k) => {
                                    row.matched = Some(row.hits[k - 1].note.clone());
                                    "match"
                                }
                                CoverageVerdict::NoMatch => "none",
                                CoverageVerdict::Unparsed => "unparsed",
                            };
                            row.judge_raw = reply.raw;
                            row.judge_tokens_in = reply.tokens_in;
                            row.judge_tokens_out = reply.tokens_out;
                        }
                        Err(e) => {
                            row.verdict = "judge_error";
                            row.judge_error = Some(e);
                        }
                    }
                }
            }
        }
        println!(
            "[coverage] {:<32} {:<10} hits={}",
            note.fact_key.as_deref().unwrap_or(&note.id),
            row.verdict,
            row.hits.len()
        );
        rows.push(row);
    }
    let matched = rows.iter().filter(|r| r.matched.is_some()).count();
    CoverageReport {
        gold_notes: rows.len(),
        matched,
        share: (!rows.is_empty()).then(|| matched as f64 / rows.len() as f64),
        judge_tokens_in: rows.iter().map(|r| u64::from(r.judge_tokens_in)).sum(),
        judge_tokens_out: rows.iter().map(|r| u64::from(r.judge_tokens_out)).sum(),
        rows,
    }
}

/// One changed fact (design 09 §7.3): a gold pair, or a correction.
#[derive(Debug, Serialize)]
struct ChangedFact {
    /// `pair` (one of the 8 gold change pairs) | `correction` (a wrong value
    /// corrected to the gold one in the same turn).
    kind: &'static str,
    fact_key: Option<String>,
    old_gold: Option<String>,
    new_gold: String,
    old_note: Option<String>,
    new_note: Option<String>,
    /// A pair: `new_active_old_superseded` | `new_active_old_same_key_loser`
    /// | `both_active_unlinked` | `new_not_active` | `new_missing` |
    /// `old_never_learned`. A correction: `new_active` | `new_not_active` |
    /// `new_missing`.
    outcome: &'static str,
    /// Both learned notes carry one `fact_key`.
    key_reused: Option<bool>,
    correct: bool,
}

/// A pair's outcome from what the coverage judge matched for its two gold
/// notes. The same learned note matching both means the old value was never
/// learned as its own note.
fn classify_pair(old: Option<&CoverageHit>, new: Option<&CoverageHit>) -> &'static str {
    let Some(new) = new else {
        return "new_missing";
    };
    let Some(old) = old.filter(|o| o.note != new.note) else {
        return "old_never_learned";
    };
    if new.effective_status != "active" || new.same_key_loser {
        "new_not_active"
    } else if old.effective_status == "superseded" {
        "new_active_old_superseded"
    } else if old.same_key_loser {
        "new_active_old_same_key_loser"
    } else {
        "both_active_unlinked"
    }
}

fn classify_correction(new: Option<&CoverageHit>) -> &'static str {
    match new {
        None => "new_missing",
        Some(n) if n.effective_status == "active" && !n.same_key_loser => "new_active",
        Some(_) => "new_not_active",
    }
}

fn measure_changed_facts(
    pairs: &[ChangePair],
    corrections: &[&CorpusTurn],
    coverage: &CoverageReport,
    learned: &[LearnedNote],
) -> Vec<ChangedFact> {
    let by_gold: HashMap<&str, &CoverageRow> =
        coverage.rows.iter().map(|r| (r.gold.as_str(), r)).collect();
    let hit = |gold: &str| by_gold.get(gold).and_then(|r| r.matched_hit()).cloned();
    let key_of = |id: &str| {
        learned
            .iter()
            .find(|n| n.id == id)
            .and_then(|n| n.fact_key.clone())
    };
    let mut out = Vec::new();
    for p in pairs {
        let (old, new) = (hit(&p.old), hit(&p.new));
        let outcome = classify_pair(old.as_ref(), new.as_ref());
        let key_reused = match (&old, &new) {
            (Some(o), Some(n)) if o.note != n.note => Some(matches!(
                (key_of(&o.note), key_of(&n.note)),
                (Some(a), Some(b)) if a == b
            )),
            _ => None,
        };
        out.push(ChangedFact {
            kind: "pair",
            fact_key: Some(p.fact_key.clone()),
            old_gold: Some(p.old.clone()),
            new_gold: p.new.clone(),
            old_note: old.map(|h| h.note),
            new_note: new.map(|h| h.note),
            outcome,
            key_reused,
            correct: matches!(
                outcome,
                "new_active_old_superseded" | "new_active_old_same_key_loser"
            ),
        });
    }
    for turn in corrections {
        for gold in &turn.states {
            let new = hit(gold);
            let outcome = classify_correction(new.as_ref());
            out.push(ChangedFact {
                kind: "correction",
                fact_key: by_gold.get(gold.as_str()).and_then(|r| r.fact_key.clone()),
                old_gold: None,
                new_gold: gold.clone(),
                old_note: None,
                new_note: new.map(|h| h.note),
                outcome,
                key_reused: None,
                correct: outcome == "new_active",
            });
        }
    }
    out
}

/// One `restate` turn (design 09 §7.3, *Restatements*).
#[derive(Debug, Serialize)]
struct Restatement {
    session: String,
    turn: usize,
    states: Vec<String>,
    row: Option<i64>,
    /// `nothing_written` (unchanged, already learned or a dedup) |
    /// `automatic_replacement` | `suggestion` | `duplicate_note` (an active
    /// note beside another with its key or title) | `first_learned` (the
    /// earlier statement was never learned) | `candidate` | `unknown_row`.
    outcome: &'static str,
    /// The notes citing the turn's row.
    notes: Vec<String>,
}

fn classify_restatement(row: Option<i64>, learned: &[LearnedNote]) -> (&'static str, Vec<String>) {
    let Some(row) = row else {
        return ("unknown_row", Vec::new());
    };
    let citing: Vec<&LearnedNote> = learned.iter().filter(|n| n.cites_row(row)).collect();
    let ids = citing.iter().map(|n| n.id.clone()).collect();
    let outcome = if citing.is_empty() {
        "nothing_written"
    } else if citing
        .iter()
        .any(|n| n.status == "candidate" && !n.supersedes.is_empty())
    {
        "suggestion"
    } else if citing
        .iter()
        .any(|n| n.status == "active" && !n.supersedes.is_empty())
    {
        "automatic_replacement"
    } else if citing.iter().any(|n| {
        n.status == "active"
            && learned.iter().any(|other| {
                other.id != n.id
                    && ((n.fact_key.is_some() && other.fact_key == n.fact_key)
                        || other.title.to_lowercase() == n.title.to_lowercase())
            })
    }) {
        "duplicate_note"
    } else if citing.iter().any(|n| n.status == "active") {
        "first_learned"
    } else {
        "candidate"
    };
    (outcome, ids)
}

/// Han, kana or Hangul in a string (`common/eval.rs`'s `is_cjk`).
fn has_cjk(text: &str) -> bool {
    text.chars().any(eval::is_cjk)
}

/// An alias in the script the title is not in (design 09 §7.3, *Aliases*):
/// CJK for a Latin title, Latin letters for a CJK one.
fn has_other_script_alias(title: &str, aliases: &[String]) -> bool {
    if has_cjk(title) {
        aliases
            .iter()
            .any(|a| a.chars().any(|c| c.is_ascii_alphabetic()))
    } else {
        aliases.iter().any(|a| has_cjk(a))
    }
}

#[derive(Debug, Serialize)]
struct AliasReport {
    active_learned: usize,
    with_any_alias: usize,
    with_other_script_alias: usize,
    share_any: Option<f64>,
    share_other_script: Option<f64>,
}

#[derive(Debug, Serialize)]
struct FactKeyReport {
    active_learned: usize,
    with_fact_key: usize,
    share: Option<f64>,
    /// Change pairs whose two learned notes share one `fact_key`, of those
    /// with two distinct notes.
    pairs_key_reused: usize,
    pairs_with_two_notes: usize,
}

#[derive(Debug, Serialize)]
struct NoiseReport {
    active_learned: usize,
    /// Active learned notes no coverage call chose.
    never_matched: usize,
    never_matched_ids: Vec<String>,
    /// Candidates raised: inferred notes and suggestions.
    candidates: usize,
}

fn share(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

fn measure_aliases(learned: &[LearnedNote]) -> AliasReport {
    let active: Vec<&LearnedNote> = learned
        .iter()
        .filter(|n| n.is_learned() && n.is_active())
        .collect();
    let any = active.iter().filter(|n| !n.aliases.is_empty()).count();
    let other = active
        .iter()
        .filter(|n| has_other_script_alias(&n.title, &n.aliases))
        .count();
    AliasReport {
        active_learned: active.len(),
        with_any_alias: any,
        with_other_script_alias: other,
        share_any: share(any, active.len()),
        share_other_script: share(other, active.len()),
    }
}

fn measure_fact_keys(learned: &[LearnedNote], changed: &[ChangedFact]) -> FactKeyReport {
    let active: Vec<&LearnedNote> = learned
        .iter()
        .filter(|n| n.is_learned() && n.is_active())
        .collect();
    let keyed = active.iter().filter(|n| n.fact_key.is_some()).count();
    let pairs: Vec<&ChangedFact> = changed.iter().filter(|c| c.key_reused.is_some()).collect();
    FactKeyReport {
        active_learned: active.len(),
        with_fact_key: keyed,
        share: share(keyed, active.len()),
        pairs_key_reused: pairs.iter().filter(|c| c.key_reused == Some(true)).count(),
        pairs_with_two_notes: pairs.len(),
    }
}

fn measure_noise(learned: &[LearnedNote], coverage: &CoverageReport) -> NoiseReport {
    let matched: BTreeSet<&str> = coverage
        .rows
        .iter()
        .filter_map(|r| r.matched.as_deref())
        .collect();
    let active: Vec<&LearnedNote> = learned
        .iter()
        .filter(|n| n.is_learned() && n.is_active())
        .collect();
    let never: Vec<String> = active
        .iter()
        .filter(|n| !matched.contains(n.id.as_str()))
        .map(|n| n.id.clone())
        .collect();
    NoiseReport {
        active_learned: active.len(),
        never_matched: never.len(),
        never_matched_ids: never,
        candidates: learned
            .iter()
            .filter(|n| n.effective_status == "candidate")
            .count(),
    }
}

/// The forced re-run inside L (design 09 §7.2): forget one learned note,
/// reset its session's cursor, flush again — the forgotten note must not
/// come back (§1.4(d)).
#[derive(Debug, Default, Serialize)]
struct ForcedRerun {
    forgotten: Option<String>,
    title: Option<String>,
    /// The forgotten note's sources: suppressed by the forget.
    sources: Vec<String>,
    session: Option<String>,
    flush: Option<FlushRecord>,
    /// Notes the re-run wrote citing a row the forget suppressed: a
    /// resurrection. The hard invariant is that this is empty.
    resurrected: Vec<String>,
    /// Notes the re-run wrote with the forgotten note's title or key but
    /// citing other rows: not a resurrection, recorded for reading.
    rewritten_from_other_rows: Vec<String>,
    /// Why no check ran, when none did.
    skipped: Option<String>,
}

#[derive(Debug, Serialize)]
struct Invariants {
    /// User notes retired without a confirm: must be 0.
    user_notes_retired_without_confirm: usize,
    /// Forgotten notes the forced re-run brought back: must be 0.
    forgotten_resurrected: usize,
    /// The forced re-run forgot a note and its flush made an extraction
    /// call and read its session to the end (no `stopped`), so
    /// `forgotten_resurrected` measured something. For L and a
    /// replay `false` is a violation: a check that never ran is no pass.
    /// L0 and L-S run no re-run.
    forced_rerun_checked: bool,
    violated: bool,
}

#[derive(Debug, Serialize)]
struct LearningCost {
    flushes: usize,
    flushes_with_calls: usize,
    extraction_calls: usize,
    extraction_tokens_in: u64,
    extraction_tokens_out: u64,
    /// Over the flushes that made a call.
    flush_latency_p50_ms: u64,
    flush_latency_p95_ms: u64,
    flushes_by_trigger: BTreeMap<String, usize>,
    /// The two learning paths: `extraction` from the flushes' events,
    /// `model_tool` (the reader's own writes) from the fed turns'.
    memory_changes_by_surface: BTreeMap<String, usize>,
    reader_llm_calls: u32,
    reader_tokens_in: u64,
    reader_tokens_out: u64,
    feed_wall_ms: u64,
}

fn learning_cost(turns: &[TurnRecord], flushes: &[FlushRecord], feed_wall_ms: u64) -> LearningCost {
    let mut latencies: Vec<u64> = flushes
        .iter()
        .filter(|f| f.calls > 0)
        .map(|f| f.latency_ms)
        .collect();
    latencies.sort_unstable();
    let mut by_trigger = BTreeMap::new();
    for f in flushes {
        *by_trigger.entry(f.label.to_string()).or_insert(0) += 1;
    }
    let mut by_surface = BTreeMap::new();
    for w in flushes.iter().flat_map(|f| &f.written) {
        *by_surface
            .entry(w.surface.clone().unwrap_or_else(|| "none".to_string()))
            .or_insert(0) += 1;
    }
    let model_tool: usize = turns.iter().map(|t| t.model_tool_memory_changes).sum();
    *by_surface.entry("model_tool".to_string()).or_insert(0) += model_tool;
    LearningCost {
        flushes: flushes.len(),
        flushes_with_calls: latencies.len(),
        extraction_calls: flushes.iter().map(|f| f.calls).sum(),
        extraction_tokens_in: flushes.iter().map(|f| f.input_tokens).sum(),
        extraction_tokens_out: flushes.iter().map(|f| f.output_tokens).sum(),
        flush_latency_p50_ms: percentile(&latencies, 0.50),
        flush_latency_p95_ms: percentile(&latencies, 0.95),
        flushes_by_trigger: by_trigger,
        memory_changes_by_surface: by_surface,
        reader_llm_calls: turns.iter().map(|t| t.reader_llm_calls).sum(),
        reader_tokens_in: turns.iter().map(|t| t.reader_tokens_in).sum(),
        reader_tokens_out: turns.iter().map(|t| t.reader_tokens_out).sum(),
        feed_wall_ms,
    }
}

/// L-S (design 09 §7.2): one seeded old note and what its suggestions did.
#[derive(Debug, Serialize)]
struct SuggestionTargetCheck {
    old_gold: String,
    fact_key: String,
    target: String,
    /// Pending suggestions naming it: the newest listed, plus the older.
    suggestions: usize,
    suggestion: Option<String>,
    suggestion_title: Option<String>,
    suggestion_knowledge: Option<String>,
    /// Its effective status after the feed, before any confirm: must be
    /// `active` (nothing learned retires a note the user saved, G2).
    status_before_confirm: String,
    confirm_error: Option<String>,
    /// Its effective status after the user-directed confirm: `superseded`
    /// when the confirm retired it.
    status_after_confirm: Option<String>,
}

#[derive(Debug, Serialize)]
struct SuggestionCheck {
    targets: Vec<SuggestionTargetCheck>,
    /// Seeded notes at least one suggestion names (expected 8).
    targets_with_a_suggestion: usize,
    suggestions_total: usize,
    /// Suggestions naming a note that is not one of the seeded ones.
    suggestions_elsewhere: usize,
    confirmed_and_superseded: usize,
}

/// The learning side of a report (design 09 §7.3).
#[derive(Debug, Default, Serialize)]
struct LearningResults {
    turns: Vec<TurnRecord>,
    flushes: Vec<FlushRecord>,
    learned: Vec<LearnedNote>,
    coverage: Option<CoverageReport>,
    noise: Option<NoiseReport>,
    changed_facts: Vec<ChangedFact>,
    /// Of `changed_facts.len()`: a pair whose old note was retired or lost
    /// its key to the new one, a correction whose gold value is active.
    changed_facts_correct: Option<usize>,
    restatements: Vec<Restatement>,
    restatements_by_outcome: BTreeMap<String, usize>,
    aliases: Option<AliasReport>,
    fact_keys: Option<FactKeyReport>,
    forced_rerun: Option<ForcedRerun>,
    suggestion_check: Option<SuggestionCheck>,
    invariants: Option<Invariants>,
    cost: Option<LearningCost>,
}

fn measure_restatements(
    sessions: &[&CorpusSession],
    transcript: &Transcript,
    learned: &[LearnedNote],
) -> Vec<Restatement> {
    let mut out = Vec::new();
    for s in sessions {
        for (ti, t) in s.turns.iter().enumerate() {
            if t.mode != TurnMode::Restate {
                continue;
            }
            let row = transcript.row_of(&s.id, ti);
            let (outcome, notes) = classify_restatement(row, learned);
            out.push(Restatement {
                session: s.id.clone(),
                turn: ti,
                states: t.states.clone(),
                row,
                outcome,
                notes,
            });
        }
    }
    out
}

/// Everything §7.3 measures on a store the corpus taught (L, L0, a replay):
/// the dump, coverage, noise, the changed facts, the restatements, aliases
/// and keys. The forced re-run and the invariants come after.
#[allow(clippy::too_many_arguments)]
async fn measure_learning(
    router: &openalpaca_llm::LlmRouter,
    gate: Option<&mut JudgeGate>,
    coverage_template: &str,
    service: &Arc<MemoryService>,
    ctx: &openalpaca_core::memory::service::MemoryAccessContext,
    notes: &[GoldNote],
    relied: &BTreeSet<String>,
    sessions: &[&CorpusSession],
    transcript: &Transcript,
    results: &mut LearningResults,
) {
    let (svc, c) = (service.clone(), ctx.clone());
    results.learned = blocking(move || dump_learned(&svc, &c))
        .await
        .unwrap_or_else(|e| panic!("memory_eval: cannot dump the learned store: {e}"));
    let gold: Vec<&GoldNote> = notes.iter().filter(|n| relied.contains(&n.id)).collect();
    let coverage = measure_coverage(
        router,
        gate,
        coverage_template,
        service,
        ctx,
        &gold,
        &results.learned,
    )
    .await;
    let pairs = change_pairs(notes).unwrap_or_else(|e| panic!("memory_eval: {e}"));
    let corrections: Vec<&CorpusTurn> = sessions
        .iter()
        .flat_map(|s| s.turns.iter())
        .filter(|t| t.mode == TurnMode::Correction)
        .collect();
    results.changed_facts =
        measure_changed_facts(&pairs, &corrections, &coverage, &results.learned);
    results.changed_facts_correct =
        Some(results.changed_facts.iter().filter(|c| c.correct).count());
    results.restatements = measure_restatements(sessions, transcript, &results.learned);
    for r in &results.restatements {
        *results
            .restatements_by_outcome
            .entry(r.outcome.to_string())
            .or_insert(0) += 1;
    }
    results.aliases = Some(measure_aliases(&results.learned));
    results.fact_keys = Some(measure_fact_keys(&results.learned, &results.changed_facts));
    results.noise = Some(measure_noise(&results.learned, &coverage));
    results.coverage = Some(coverage);
}

/// The forced re-run (design 09 §7.2): forget the first learned note that is
/// still active and cites a message, move its session's cursor back to 0,
/// flush the session again, and look for a note citing a forgotten row.
#[allow(clippy::too_many_arguments)]
async fn forced_rerun(
    queue: &openalpaca_core::memory::learning::LearningQueue,
    bus: &openalpaca_core::bus::EventBus,
    db: &openalpaca_storage::Database,
    service: &Arc<MemoryService>,
    ctx: &openalpaca_core::memory::service::MemoryAccessContext,
    transcript: &Transcript,
    flushes: &[FlushRecord],
    learned: &[LearnedNote],
) -> ForcedRerun {
    use openalpaca_core::memory::service::{ForgetInput, ReadInput};
    use openalpaca_storage::{ConversationRepository, LearningCursorRepository};
    let mut out = ForcedRerun::default();
    // The first note an extraction flush created that is still active and
    // cites a message of a fed session.
    let by_id: HashMap<&str, &LearnedNote> = learned.iter().map(|n| (n.id.as_str(), n)).collect();
    let row_session: HashMap<i64, &TranscriptSession> = {
        let mut map = HashMap::new();
        let repo = ConversationRepository::new(db);
        for s in &transcript.sessions {
            if let Ok(rows) = repo.learning_window(&s.session_id, 0, 100_000) {
                for r in rows {
                    map.insert(r.id, s);
                }
            }
        }
        map
    };
    let candidate = flushes
        .iter()
        .flat_map(|f| &f.written)
        .filter(|w| w.change == "created" && w.surface.as_deref() == Some("extraction"))
        .filter_map(|w| by_id.get(w.note.as_str()).copied())
        .find(|n| {
            n.is_learned()
                && n.is_active()
                && n.sources.iter().any(|s| {
                    s.strip_prefix("message:")
                        .and_then(|r| r.parse::<i64>().ok())
                        .is_some_and(|r| row_session.contains_key(&r))
                })
        });
    let Some(note) = candidate else {
        out.skipped = Some("no active learned note cites a fed message".to_string());
        return out;
    };
    let session = note
        .sources
        .iter()
        .filter_map(|s| {
            s.strip_prefix("message:")
                .and_then(|r| r.parse::<i64>().ok())
        })
        .find_map(|r| row_session.get(&r).copied())
        .expect("the candidate cites a fed message");
    out.forgotten = Some(note.id.clone());
    out.title = Some(note.title.clone());
    out.sources = note.sources.clone();
    out.session = Some(session.corpus_id.clone());

    let (svc, c, nid) = (service.clone(), ctx.clone(), note.id.clone());
    let forgot = blocking(move || -> Result<(), String> {
        let id = NoteId::parse(&nid)?;
        let current = svc
            .read(
                &c,
                ReadInput {
                    id,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
        svc.forget(
            &c,
            ForgetInput {
                targets: vec![(id, current.revision)],
                reason: Some("memory_eval: the forced re-run (design 09 §7.2)".to_string()),
            },
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
    .await;
    if let Err(e) = forgot {
        out.skipped = Some(format!("the forget failed: {e}"));
        return out;
    }
    let cursors = LearningCursorRepository::new(db);
    let reset = cursors
        .get(&session.session_id)
        .and_then(|c| cursors.advance_cas(&session.session_id, 0, c.extracted_through));
    if !matches!(reset, Ok(true)) {
        out.skipped = Some(format!("the cursor could not be reset: {reset:?}"));
        return out;
    }
    let flush = flush_one(
        queue,
        bus,
        &session.session_id,
        &session.corpus_id,
        FLUSH_FORCED_RERUN,
    )
    .await;
    let suppressed: BTreeSet<&str> = note.sources.iter().map(String::as_str).collect();
    for w in flush.written.iter().filter(|w| w.change == "created") {
        let (svc, c, wid) = (service.clone(), ctx.clone(), w.note.clone());
        let read = blocking(move || {
            let id = NoteId::parse(&wid).ok()?;
            svc.read(
                &c,
                ReadInput {
                    id,
                    ..Default::default()
                },
            )
            .ok()
        })
        .await;
        let Some(read) = read else { continue };
        let written = LearnedNote::from_read(&read);
        if written
            .sources
            .iter()
            .any(|s| suppressed.contains(s.as_str()))
        {
            out.resurrected.push(written.id.clone());
        } else if written.title.to_lowercase() == note.title.to_lowercase()
            || (note.fact_key.is_some() && written.fact_key == note.fact_key)
        {
            out.rewritten_from_other_rows.push(written.id.clone());
        }
    }
    out.flush = Some(flush);
    out
}

// ─── Re-judge (design 09 §7.4, *Re-judge mode*) ─────────────────────────────

/// The fields of an archived row the judge reads (`memory_eval.rs`'s `Row`,
/// the same in every schema-1 report since slice 1).
#[derive(Debug, Deserialize)]
struct ArchivedRow {
    id: String,
    category: Category,
    language: Language,
    question: String,
    gold_answer: String,
    #[serde(default)]
    answer: String,
    #[serde(default)]
    expect_abstain: bool,
    #[serde(default)]
    judge_verdict: Option<String>,
    #[serde(default)]
    judge_model: Option<String>,
}

/// The reader an archived report records: the model that answered its
/// turns, else its configured default.
fn archived_reader(report: &serde_json::Value) -> Option<String> {
    report["totals"]["reader_models"]
        .as_array()
        .and_then(|m| m.first())
        .and_then(|m| m.as_str())
        .or_else(|| report["conditions"]["reader_default_model"].as_str())
        .map(str::to_string)
}

/// Grade an archived report's rows again with `gate`'s judge and the frozen
/// prompt, with no reader call: every row keeps its question, gold answer
/// and answer, gains `original_judge_verdict` / `original_judge_model`, and
/// takes the new verdict; totals are recomputed; `conditions` records the new
/// judge; the report records `rejudged_from`, and the agreement between the
/// two judges (raw and Cohen's κ) over the rows both judged. `progress` is
/// handed the report after every row.
#[allow(clippy::too_many_arguments)]
async fn rejudge_report(
    router: &openalpaca_llm::LlmRouter,
    gate: &mut JudgeGate,
    template: &str,
    template_sha256: &str,
    mut report: serde_json::Value,
    archived: &Path,
    archived_sha256: &str,
    progress: &mut dyn FnMut(&serde_json::Value),
) -> Result<serde_json::Value, String> {
    let rows = report["rows"]
        .as_array()
        .cloned()
        .ok_or("the archived report has no rows")?;
    let started = Instant::now();
    report["rejudged_from"] = archived.to_string_lossy().into_owned().into();
    report["rejudged_from_sha256"] = archived_sha256.into();
    report["rejudged_at"] = now_rfc3339().into();
    report["status"] = "running".into();
    let original_judges = report["totals"]["judge_models"].clone();
    let conditions = report["conditions"]
        .as_object_mut()
        .ok_or("the archived report has no conditions")?;
    conditions.insert("original_judge_models".into(), original_judges);
    conditions.insert("judge_model".into(), gate.choice.model.clone().into());
    conditions.insert(
        "judge_model_requested".into(),
        gate.choice.model.clone().into(),
    );
    conditions.insert("judge_is_reader".into(), gate.choice.judge_is_reader.into());
    conditions.insert(
        "judge_same_family".into(),
        gate.choice.judge_same_family.into(),
    );
    conditions.insert(
        "judge_is_extractor".into(),
        gate.choice.judge_is_extractor.into(),
    );
    conditions.insert(
        "judge_temperature".into(),
        f64::from(JUDGE_TEMPERATURE).into(),
    );
    conditions.insert("judge_max_tokens".into(), JUDGE_MAX_TOKENS.into());
    conditions.insert("judge_prompt_sha256".into(), template_sha256.into());

    let mut regraded = Vec::with_capacity(rows.len());
    let mut agreement = Vec::new();
    let (mut correct, mut incorrect, mut unparsed, mut errors) = (0usize, 0usize, 0usize, 0usize);
    let (mut tokens_in, mut tokens_out) = (0u64, 0u64);
    let mut judges = BTreeSet::new();
    let mut by_category: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_language: BTreeMap<String, Bucket> = BTreeMap::new();
    for mut row in rows {
        let archived_row: ArchivedRow =
            serde_json::from_value(row.clone()).map_err(|e| format!("a row: {e}"))?;
        let q = Question {
            id: archived_row.id.clone(),
            category: archived_row.category,
            language: archived_row.language,
            question: archived_row.question.clone(),
            gold_answer: archived_row.gold_answer.clone(),
            relies_on: Vec::new(),
            expect_abstain: archived_row.expect_abstain,
        };
        let outcome = judge(router, gate, template, &q, &archived_row.answer).await;
        let obj = row.as_object_mut().ok_or("a row is not an object")?;
        obj.insert(
            "original_judge_verdict".into(),
            archived_row.judge_verdict.clone().into(),
        );
        obj.insert(
            "original_judge_model".into(),
            archived_row.judge_model.clone().into(),
        );
        let verdict = match outcome {
            Ok(out) => {
                tokens_in += u64::from(out.tokens_in);
                tokens_out += u64::from(out.tokens_out);
                judges.insert(out.model.clone());
                obj.insert("correct".into(), out.verdict.into());
                obj.insert("judge_raw".into(), out.raw.into());
                obj.insert("judge_model".into(), out.model.into());
                obj.insert("judge_tokens_in".into(), out.tokens_in.into());
                obj.insert("judge_tokens_out".into(), out.tokens_out.into());
                obj.insert("judge_error".into(), serde_json::Value::Null);
                out.verdict
            }
            Err(e) => {
                obj.insert("correct".into(), serde_json::Value::Null);
                obj.insert("judge_raw".into(), "".into());
                obj.insert("judge_model".into(), serde_json::Value::Null);
                obj.insert("judge_tokens_in".into(), 0.into());
                obj.insert("judge_tokens_out".into(), 0.into());
                obj.insert("judge_error".into(), e.into());
                errors += 1;
                None
            }
        };
        let word = match (verdict, obj["judge_error"].is_null()) {
            (Some(true), _) => {
                correct += 1;
                "correct"
            }
            (Some(false), _) => {
                incorrect += 1;
                "incorrect"
            }
            (None, true) => {
                unparsed += 1;
                "unparsed"
            }
            (None, false) => "judge_error",
        };
        obj.insert("judge_verdict".into(), word.into());
        let ok = verdict == Some(true);
        for (map, key) in [
            (&mut by_category, archived_row.category.as_str()),
            (&mut by_language, archived_row.language.as_str()),
        ] {
            let b = map.entry(key.to_string()).or_default();
            b.n += 1;
            b.correct += usize::from(ok);
        }
        let original = match archived_row.judge_verdict.as_deref() {
            Some("correct") => Some(true),
            Some("incorrect") => Some(false),
            _ => None,
        };
        if let (Some(a), Some(b)) = (original, verdict) {
            agreement.push((a, b));
        }
        regraded.push(row);
        report["rows"] = serde_json::Value::Array(regraded.clone());
        progress(&report);
    }
    by_category.values_mut().for_each(bucket_accuracy);
    by_language.values_mut().for_each(bucket_accuracy);
    let n = regraded.len();
    if !report["totals"].is_object() {
        report["totals"] = serde_json::json!({});
    }
    let totals = report["totals"]
        .as_object_mut()
        .expect("totals is an object");
    totals.insert("correct".into(), correct.into());
    totals.insert("incorrect".into(), incorrect.into());
    totals.insert("unparsed".into(), unparsed.into());
    totals.insert("judge_errors".into(), errors.into());
    totals.insert(
        "accuracy".into(),
        (n > 0).then(|| correct as f64 / n as f64).into(),
    );
    totals.insert(
        "by_category".into(),
        serde_json::to_value(&by_category).expect("buckets"),
    );
    totals.insert(
        "by_language".into(),
        serde_json::to_value(&by_language).expect("buckets"),
    );
    totals.insert("judge_tokens_in".into(), tokens_in.into());
    totals.insert("judge_tokens_out".into(), tokens_out.into());
    totals.insert(
        "judge_models".into(),
        judges.into_iter().collect::<Vec<_>>().into(),
    );
    let agree = agreement.iter().filter(|(a, b)| a == b).count();
    totals.insert(
        "agreement_with_original".into(),
        serde_json::json!({
            "rows": agreement.len(),
            "agree": agree,
            "raw": share(agree, agreement.len()),
            "cohen_kappa": cohen_kappa(&agreement),
        }),
    );
    totals.insert(
        "rejudge_wall_ms".into(),
        (started.elapsed().as_millis() as u64).into(),
    );
    report["status"] = "complete".into();
    report["run_finished_at"] = now_rfc3339().into();
    Ok(report)
}

async fn run_rejudge(settings: &Settings, router: &openalpaca_llm::LlmRouter, archived: &Path) {
    let (report, archived_sha256) = load_json::<serde_json::Value>(archived);
    // The judge is this run's — the eval's default model unless named — and
    // it is recorded against the archived report's reader.
    assert!(
        settings.judge != JudgeRequest::Off,
        "memory_eval: a re-judge needs a judge (OPENALPACA_MEMORY_EVAL_JUDGE_MODEL is none)"
    );
    let reader = archived_reader(&report);
    let requested = settings
        .judge
        .requested()
        .map(str::to_string)
        .or_else(|| router.effective_default_model());
    let choice = judge_guard(router, requested.as_deref(), reader.as_deref(), None)
        .unwrap_or_else(|e| panic!("memory_eval: {e}"));
    eprintln!(
        "memory_eval: re-judging with {} (the archived reader: {}; judge_is_reader={}, judge_same_family={})",
        choice.model,
        reader.as_deref().unwrap_or("unknown"),
        choice.judge_is_reader,
        choice.judge_same_family
    );
    let mut gate = JudgeGate::new(choice);
    let template_path = eval_dir().join("judge-prompt.md");
    let bytes = read_bytes(&template_path);
    let template_sha256 = sha256_hex(&bytes);
    let template = String::from_utf8(bytes).expect("judge prompt is UTF-8");
    let out_path = settings.report_path.clone();
    let mut progress = |r: &serde_json::Value| write_report(&out_path, r);
    let report = rejudge_report(
        router,
        &mut gate,
        &template,
        &template_sha256,
        report,
        archived,
        &archived_sha256,
        &mut progress,
    )
    .await
    .unwrap_or_else(|e| panic!("memory_eval: cannot re-judge {}: {e}", archived.display()));
    write_report(&settings.report_path, &report);
    println!(
        "re-judged {} with {}: accuracy {:?}, agreement {}",
        archived.display(),
        gate.choice.model,
        report["totals"]["accuracy"],
        report["totals"]["agreement_with_original"]
    );
    println!("report: {}", settings.report_path.display());
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
    // Double gate: never make live calls unless the operator opted in.
    if !env_flag("OPENALPACA_LIVE_EVAL") {
        eprintln!(
            "memory_eval: skipped — set OPENALPACA_LIVE_EVAL=1 to run the live eval \
             (no LLM calls were made)."
        );
        return;
    }
    let settings = Settings::from_env();
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
    if let Some(archived) = &settings.rejudge_from {
        run_rejudge(&settings, &router, archived).await;
        return;
    }
    let system = settings.system.expect("a system, checked by from_env");
    if settings.learn_from.is_some() {
        run_replay(&settings, router).await;
    } else {
        run_system(&settings, system, router).await;
    }
}

/// The scratch directory a run works in: deleted at the end unless kept.
enum Scratch {
    Temp(tempfile::TempDir),
    Kept(PathBuf),
}

impl Scratch {
    fn path(&self) -> &Path {
        match self {
            Scratch::Temp(t) => t.path(),
            Scratch::Kept(p) => p,
        }
    }
}

fn new_report(system: System) -> Report {
    Report {
        schema: REPORT_SCHEMA,
        status: "running",
        system,
        unavailable_reason: None,
        run_started_at: now_rfc3339(),
        run_finished_at: None,
        machine: machine(),
        build: build_info(),
        conditions: None,
        embedder: None,
        totals: None,
        rows: Vec::new(),
        learning: None,
    }
}

fn corpus_sessions(corpus: &Corpus, system: System, limit: Option<usize>) -> Vec<&CorpusSession> {
    let wanted: BTreeSet<&str> = corpus
        .suggestion_sessions
        .iter()
        .map(String::as_str)
        .collect();
    corpus
        .sessions
        .iter()
        .filter(|s| system != System::LS || wanted.contains(s.id.as_str()))
        .take(limit.unwrap_or(usize::MAX))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn learning_conditions(
    cfg: &openalpaca_core::daemon_config::DaemonConfig,
    corpus_path: &Path,
    corpus_sha256: &str,
    sessions: &[&CorpusSession],
    settings: &Settings,
    learning_on: bool,
    seeded_old_notes: usize,
    coverage_path: &Path,
    coverage_sha256: &str,
) -> LearningConditions {
    LearningConditions {
        corpus_path: corpus_path.to_string_lossy().into_owned(),
        corpus_sha256: corpus_sha256.to_string(),
        sessions_fed: sessions.len(),
        turns_fed: sessions.iter().map(|s| s.turns.len()).sum(),
        session_limit: settings.session_limit,
        learning_on,
        queue_mode: "manual",
        turns_per_flush: cfg.orchestrator.learning.turns_per_flush,
        extraction_model: cfg.orchestrator.costs.extraction_model.clone(),
        extract_max_calls_per_day: cfg.orchestrator.costs.extract_max_calls_per_day,
        extract_max_daily_cost_usd: cfg.orchestrator.costs.extract_max_daily_cost_usd,
        learning_during_questions: false,
        unattended: true,
        seeded_old_notes,
        seeded_as_saved_then: Vec::new(),
        replayed_from: settings
            .learn_from
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        scratch_kept: None,
        coverage_judge_prompt_path: coverage_path.to_string_lossy().into_owned(),
        coverage_judge_prompt_sha256: coverage_sha256.to_string(),
    }
}

/// None, D, E, L, L0 and L-S: the scratch database, owner and project, the
/// orchestrator, the seeding, the learning phase for a learning system, then
/// the questions (every system but L-S).
async fn run_system(settings: &Settings, system: System, router: Arc<openalpaca_llm::LlmRouter>) {
    use arc_swap::ArcSwap;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    use openalpaca_core::runner::LoopConfig;
    use openalpaca_storage::IdentityRepository;

    let started = Instant::now();
    let mut report = new_report(system);
    eprintln!(
        "memory_eval: system {} — router loaded from {} (default model: {}; effective: {})",
        system.as_str(),
        settings.llm_toml.display(),
        router.default_model(),
        router
            .effective_default_model()
            .unwrap_or_else(|| "none".to_string())
    );

    // ── Data ────────────────────────────────────────────────────────────
    let dir = eval_dir();
    let notes_path = dir.join("notes-gold.json");
    let judge_prompt_path = dir.join("judge-prompt.md");
    let coverage_prompt_path = dir.join("coverage-judge-prompt.md");
    let corpus_path = dir.join("conversations-learning.json");
    let (notes, notes_sha256) = load_json::<Vec<GoldNote>>(&notes_path);
    let (all_questions, questions_sha256) = load_json::<Vec<Question>>(&settings.questions_path);
    let judge_bytes = read_bytes(&judge_prompt_path);
    let judge_prompt_sha256 = sha256_hex(&judge_bytes);
    let judge_template = String::from_utf8(judge_bytes).expect("judge prompt is UTF-8");
    let coverage_bytes = read_bytes(&coverage_prompt_path);
    let coverage_prompt_sha256 = sha256_hex(&coverage_bytes);
    let coverage_template = String::from_utf8(coverage_bytes).expect("coverage prompt is UTF-8");
    let llm_toml_sha256 = sha256_hex(&read_bytes(&settings.llm_toml));
    let corpus = system
        .feeds_corpus()
        .then(|| load_json::<Corpus>(&corpus_path));
    let questions: Vec<Question> = if system.asks_questions() {
        all_questions
            .iter()
            .filter(|q| settings.category.is_none_or(|c| q.category == c))
            .take(settings.limit.unwrap_or(usize::MAX))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    assert!(
        !system.asks_questions() || !questions.is_empty(),
        "memory_eval: no questions selected"
    );

    // ── Scratch DB, owner and project (so the turn's workspace is never
    //    this checkout, whatever the test process's CWD is). L keeps its
    //    scratch from the start: a replay learns from it. ──────────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let scratch = if system == System::L {
        Scratch::Kept(tmp.keep())
    } else {
        Scratch::Temp(tmp)
    };
    let project = scratch_project(scratch.path());
    let db = openalpaca_storage::Database::open(&scratch.path().join(SCRATCH_DB)).expect("open db");
    // The service answers only for a `global_user` row (design §3); a fresh
    // owner per run keeps a reused scratch store's earlier partitions out.
    let owner_id = new_owner_id();
    IdentityRepository::new(&db)
        .create_global_user(&owner_id, None)
        .expect("create the eval owner");

    // ── Orchestrator ────────────────────────────────────────────────────
    let bus = EventBus::default();
    let cfg = eval_daemon_config(system);
    let budget = MainLoopBudget::for_system(system, &cfg.orchestrator.routing);
    let workflow_cap = cfg.orchestrator.routing.max_workflows_per_lane;
    let tool_result_inline_bytes = cfg.orchestrator.sessions.tool_result_inline_bytes;
    let autocompact_buffer_ratio = cfg.execution.context.autocompact_buffer_ratio;
    let recall_mode = cfg.orchestrator.memory.recall_mode;
    let extraction_model = cfg.orchestrator.costs.extraction_model.clone();
    let daemon_config = Arc::new(ArcSwap::from_pointee(cfg.clone()));

    // Every system with memory: the daemon's own service
    // (`MemoryService::open_default` — the index cache under the scratch
    // store), its read tools registered as the daemon registers them.
    let service = (system != System::NoMemory)
        .then(|| Arc::new(MemoryService::open_default(Some(db.clone())).with_bus(bus.clone())));
    let reader_max_rounds = LoopConfig::default().max_rounds;
    let reader_max_cost = LoopConfig::default().max_cost;
    let stack = eval_stack(
        router.clone(),
        &db,
        &bus,
        service.as_ref(),
        daemon_config.clone(),
    );
    let orch = stack.orch.clone();

    // ── The judge, before any call (design 09 §7.4) ─────────────────────
    let reader = router.effective_default_model();
    let mut judge_gate = resolve_judge(
        &settings.judge,
        &router,
        reader.as_deref(),
        extraction_model.as_deref(),
    );

    // ── Seed through the service: every gold note for D and E, the change
    //    pairs' old notes for L-S, nothing for the others ────────────────
    let pairs = change_pairs(&notes).unwrap_or_else(|e| panic!("memory_eval: {e}"));
    let to_seed: Vec<GoldNote> = match system {
        System::D | System::E => notes.clone(),
        System::LS => ls_seed_notes(&notes, &pairs),
        System::NoMemory | System::L | System::L0 => Vec::new(),
    };
    let seed = match &service {
        Some(service) if !to_seed.is_empty() => {
            let (service, owner, root) = (service.clone(), owner_id.clone(), project.clone());
            let total = to_seed.len();
            // The service's file work is synchronous (it says so); an async
            // caller runs it on the blocking pool, as the tools do.
            let seed = blocking(move || seed_notes(&service, &owner, &root, &to_seed)).await;
            eprintln!(
                "memory_eval: seeded {} of {total} notes ({} as a replacement, {} duplicate, {} failed)",
                seed.seeded, seed.superseding, seed.duplicate, seed.failed
            );
            seed
        }
        _ => SeedOutcome::default(),
    };
    let gold_by_service_id: HashMap<String, String> = seed
        .ids
        .iter()
        .map(|(gold, service)| (service.clone(), gold.clone()))
        .collect();

    let fed_sessions: Vec<&CorpusSession> = corpus
        .as_ref()
        .map(|(c, _)| corpus_sessions(c, system, settings.session_limit))
        .unwrap_or_default();
    let recall = RecallBudget::for_mode(recall_mode);
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
        judge_model_requested: settings.judge.requested().map(str::to_string),
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
        extraction_disabled: !system.learns(),
        workflow_cap,
        home_store: settings
            .home_store
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        scratch_dir: (settings.keep_scratch || system == System::L)
            .then(|| scratch.path().to_string_lossy().into_owned()),
        system_description: system.description(),
        memory_tools_offered: if service.is_some() {
            MEMORY_TOOLS.to_vec()
        } else {
            Vec::new()
        },
        follow_up_search_bound: budget.follow_up_search_bound,
        recall_mode: recall_mode.as_str(),
        recall_total_bytes: recall.total_bytes,
        recall_pinned_bytes: recall.pinned_bytes,
        recall_max_items: recall.max_items,
        read_max_bytes: openalpaca_core::memory::service::READ_MAX_BYTES,
        notes_scope_applied: service.is_some(),
        notes_superseding: seed.superseding,
        seeded_ids: seed.ids.clone(),
        judge_model: judge_gate.as_ref().map(|g| g.choice.model.clone()),
        judge_is_reader: judge_gate.as_ref().map(|g| g.choice.judge_is_reader),
        judge_same_family: judge_gate.as_ref().map(|g| g.choice.judge_same_family),
        judge_is_extractor: judge_gate.as_ref().map(|g| g.choice.judge_is_extractor),
        learning: corpus.as_ref().map(|(_, corpus_sha256)| {
            let mut c = learning_conditions(
                &cfg,
                &corpus_path,
                corpus_sha256,
                &fed_sessions,
                settings,
                system.learns(),
                if system == System::LS { seed.seeded } else { 0 },
                &coverage_prompt_path,
                &coverage_prompt_sha256,
            );
            if let Scratch::Kept(path) = &scratch {
                c.scratch_kept = Some(path.to_string_lossy().into_owned());
            }
            if system == System::LS {
                c.seeded_as_saved_then = LS_SEEDED_AS_SAVED_THEN
                    .iter()
                    .map(|(id, ..)| id.to_string())
                    .collect();
            }
            c
        }),
    });
    write_report(&settings.report_path, &report);

    // ── The learning phase (L, L0, L-S) ─────────────────────────────────
    // The queue runs in `Manual` mode (design 09 §1.5): nothing starts a
    // flush on its own, so reader timing cannot change what is learned when.
    let queue = corpus.as_ref().and(service.as_ref()).map(|service| {
        LearningQueue::new(
            LearningDeps {
                db: db.clone(),
                router: router.clone(),
                daemon_config: daemon_config.clone(),
                memory_service: service.clone(),
            },
            QueueMode::Manual,
        )
    });
    let mut learning = LearningResults::default();
    let mut transcript = None;
    let user_ctx = service.as_ref().map(|service| {
        service
            .context(
                &owner_id,
                WorkspaceHint::Root(&project),
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution {
                    source: Some("memory_eval".to_string()),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|e| panic!("memory_eval: no memory context for {owner_id}: {e}"))
    });
    if let (Some((_, corpus_sha256)), Some(queue)) = (&corpus, &queue) {
        let learner = Learner::new(
            &stack,
            queue,
            &bus,
            &db,
            &owner_id,
            &project,
            cfg.orchestrator.learning.turns_per_flush,
        );
        let fed = learner.feed(&fed_sessions, system.learns()).await;
        let t = Transcript {
            schema: 1,
            owner_id: owner_id.clone(),
            project: project.to_string_lossy().into_owned(),
            corpus_sha256: corpus_sha256.clone(),
            sessions: fed.transcript,
        };
        if system == System::L {
            write_json(&scratch.path().join(TRANSCRIPT_FILE), &t);
        }
        learning.cost = Some(learning_cost(&fed.turns, &fed.flushes, fed.wall_ms));
        learning.turns = fed.turns;
        learning.flushes = fed.flushes;
        transcript = Some(t);
        report.learning = Some(std::mem::take(&mut learning));
        write_report(&settings.report_path, &report);
        learning = report.learning.take().expect("just set");

        let service = service.as_ref().expect("a learning system has a service");
        let ctx = user_ctx.as_ref().expect("a learning system has a context");
        let transcript = transcript.as_ref().expect("just set");
        if system == System::LS {
            learning.suggestion_check = Some(
                suggestion_check(
                    service,
                    ctx,
                    &pairs,
                    &seed,
                    &fed_sessions,
                    transcript,
                    &mut learning,
                )
                .await,
            );
        } else {
            let (heldout, _) =
                load_json::<Vec<Question>>(&eval_dir().join("questions-heldout.json"));
            measure_learning(
                &router,
                judge_gate.as_mut(),
                &coverage_template,
                service,
                ctx,
                &notes,
                &relied_on(&heldout),
                &fed_sessions,
                transcript,
                &mut learning,
            )
            .await;
        }
        report.learning = Some(learning);
        write_report(&settings.report_path, &report);
    }

    // ── Ask every question through the front door (learning off) ────────
    if system.asks_questions() {
        set_learning(&daemon_config, false);
        ask_questions(
            &orch,
            &bus,
            &router,
            judge_gate.as_mut(),
            &judge_template,
            &questions,
            &project,
            &owner_id,
            &gold_by_service_id,
            &mut report,
            &settings.report_path,
        )
        .await;
    }

    // ── L: the forced re-run, after the questions so it changes nothing
    //    they read ───────────────────────────────────────────────────────
    if let (System::L, Some(service), Some(ctx), Some(t), Some(queue)) =
        (system, &service, &user_ctx, &transcript, &queue)
    {
        set_learning(&daemon_config, true);
        let learning = report.learning.as_mut().expect("L learned");
        let rerun = forced_rerun(
            queue,
            &bus,
            &db,
            service,
            ctx,
            t,
            &learning.flushes,
            &learning.learned,
        )
        .await;
        learning.forced_rerun = Some(rerun);
    }
    if let Some(learning) = report.learning.as_mut() {
        learning.invariants = Some(invariants(learning, system == System::L));
    }

    // ── Totals ──────────────────────────────────────────────────────────
    let wall_ms = started.elapsed().as_millis() as u64;
    if system.asks_questions() {
        let judge_cost = router
            .cost_tracker
            .get_agent_usage(JUDGE_AGENT_ID)
            .await
            .map(|u| u.total_cost_usd)
            .unwrap_or(0.0);
        let t = totals(&report.rows, judge_cost, wall_ms);
        print_totals(system, &t);
        report.totals = Some(t);
    }
    if let Some(learning) = &report.learning {
        print_learning(learning);
    }
    report.status = "complete";
    report.run_finished_at = Some(now_rfc3339());
    write_report(&settings.report_path, &report);
    println!("report: {}", settings.report_path.display());

    match scratch {
        Scratch::Kept(path) => println!("scratch kept at {}", path.display()),
        Scratch::Temp(tmp) if settings.keep_scratch => {
            println!("scratch kept at {}", tmp.keep().display())
        }
        Scratch::Temp(_) => {}
    }
    // The report is written; now the guard's row check and the invariants
    // may stop the run (design 09 §7.3: "A violation blocks").
    if let Some(reader) = reader.as_deref() {
        let others: Vec<&str> = report
            .rows
            .iter()
            .filter(|r| r.reader_models.iter().any(|m| m != reader))
            .map(|r| r.id.as_str())
            .collect();
        assert!(
            others.is_empty(),
            "memory_eval: rows answered by a model other than the reader {reader}: {others:?}"
        );
    }
    if let Some(inv) = report.learning.as_ref().and_then(|l| l.invariants.as_ref()) {
        assert!(
            !inv.violated,
            "memory_eval: a learning invariant was violated: {inv:?}"
        );
    }
    // No accuracy bar here: a single run is a measurement, and the gate is the
    // paired comparison of two reports (`scripts/eval/memory/compare.py`).
}

/// The hard invariants (design 09 §7.3): no user note retired without a
/// confirm, no forgotten note resurrected — and, where a forced re-run is
/// `required` (L, a replay), that it really checked: its flush made a call
/// and read its session to the end (`calls` counts an answer before it is
/// parsed, so a call alone proves nothing was examined).
fn invariants(learning: &LearningResults, rerun_required: bool) -> Invariants {
    let retired = match &learning.suggestion_check {
        Some(check) => check
            .targets
            .iter()
            .filter(|t| t.status_before_confirm != "active")
            .count(),
        None => learning
            .learned
            .iter()
            .filter(|n| n.origin == "user" && n.effective_status == "superseded")
            .count(),
    };
    let resurrected = learning
        .forced_rerun
        .as_ref()
        .map_or(0, |r| r.resurrected.len());
    let rerun_checked = learning.forced_rerun.as_ref().is_some_and(|r| {
        r.skipped.is_none()
            && r.flush
                .as_ref()
                .is_some_and(|f| f.calls > 0 && f.stopped.is_none())
    });
    Invariants {
        user_notes_retired_without_confirm: retired,
        forgotten_resurrected: resurrected,
        forced_rerun_checked: rerun_checked,
        violated: retired > 0 || resurrected > 0 || (rerun_required && !rerun_checked),
    }
}

/// L-S (design 09 §7.2): after the suggestion sessions, the seeded notes'
/// suggestions, the notes retired before any confirm, the restatements, and
/// then each suggestion confirmed through the user-directed confirm with its
/// `target_revision`.
async fn suggestion_check(
    service: &Arc<MemoryService>,
    ctx: &openalpaca_core::memory::service::MemoryAccessContext,
    pairs: &[ChangePair],
    seed: &SeedOutcome,
    sessions: &[&CorpusSession],
    transcript: &Transcript,
    learning: &mut LearningResults,
) -> SuggestionCheck {
    use openalpaca_core::memory::service::{
        ReadInput, SUGGESTIONS_MAX_LIMIT, UpdateAction, UpdateInput,
    };
    let (svc, c) = (service.clone(), ctx.clone());
    learning.learned = blocking(move || dump_learned(&svc, &c))
        .await
        .unwrap_or_else(|e| panic!("memory_eval: cannot dump the store: {e}"));
    learning.restatements = measure_restatements(sessions, transcript, &learning.learned);
    for r in &learning.restatements {
        *learning
            .restatements_by_outcome
            .entry(r.outcome.to_string())
            .or_insert(0) += 1;
    }
    let (svc, c) = (service.clone(), ctx.clone());
    let suggestions = blocking(move || svc.suggestions(&c, Some(SUGGESTIONS_MAX_LIMIT)))
        .await
        .unwrap_or_else(|e| panic!("memory_eval: cannot list suggestions: {e}"));
    let seeded: BTreeMap<String, &ChangePair> = pairs
        .iter()
        .filter_map(|p| seed.ids.get(&p.old).map(|id| (id.clone(), p)))
        .collect();
    let status_of = |id: String| {
        let (svc, c) = (service.clone(), ctx.clone());
        async move {
            blocking(move || {
                let nid = NoteId::parse(&id).ok()?;
                svc.read(
                    &c,
                    ReadInput {
                        id: nid,
                        ..Default::default()
                    },
                )
                .ok()
                .map(|r| r.effective_status.as_str().to_string())
            })
            .await
            .unwrap_or_else(|| "missing".to_string())
        }
    };
    let mut targets = Vec::new();
    for (target, pair) in &seeded {
        let found = suggestions
            .iter()
            .find(|s| s.target.id.to_string() == *target);
        targets.push(SuggestionTargetCheck {
            old_gold: pair.old.clone(),
            fact_key: pair.fact_key.clone(),
            target: target.clone(),
            suggestions: found.map_or(0, |s| 1 + s.older_count),
            suggestion: found.map(|s| s.id.to_string()),
            suggestion_title: found.map(|s| s.title.clone()),
            suggestion_knowledge: found.map(|s| s.knowledge.clone()),
            status_before_confirm: status_of(target.clone()).await,
            confirm_error: None,
            status_after_confirm: None,
        });
    }
    // The user's confirm, one per target: the listed (newest) suggestion,
    // against the target revision the listing verified (S5).
    for check in targets.iter_mut() {
        let Some(s) = suggestions
            .iter()
            .find(|s| s.target.id.to_string() == check.target)
        else {
            continue;
        };
        let (svc, c) = (service.clone(), ctx.clone());
        let input = UpdateInput {
            id: s.id,
            expected_revision: s.revision.clone(),
            action: UpdateAction::Confirm {
                target_revision: Some(s.target.revision.clone()),
            },
            reason: Some("memory_eval: L-S confirms the suggestion".to_string()),
        };
        if let Err(e) = blocking(move || svc.update(&c, input)).await {
            check.confirm_error = Some(e.to_string());
        }
        check.status_after_confirm = Some(status_of(check.target.clone()).await);
    }
    let with = targets.iter().filter(|t| t.suggestions > 0).count();
    SuggestionCheck {
        targets_with_a_suggestion: with,
        suggestions_total: suggestions.iter().map(|s| 1 + s.older_count).sum(),
        suggestions_elsewhere: suggestions
            .iter()
            .filter(|s| !seeded.contains_key(&s.target.id.to_string()))
            .map(|s| 1 + s.older_count)
            .sum(),
        confirmed_and_superseded: targets
            .iter()
            .filter(|t| t.status_after_confirm.as_deref() == Some("superseded"))
            .count(),
        targets,
    }
}

/// A replay (design 09 §7.2): an L run's kept transcript learned again by a
/// fresh store, with no reader call; the learning side measured as L's is.
async fn run_replay(settings: &Settings, router: Arc<openalpaca_llm::LlmRouter>) {
    use arc_swap::ArcSwap;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    let from = settings.learn_from.as_ref().expect("a replay has a source");
    let started = Instant::now();
    let mut report = new_report(System::L);
    let dir = eval_dir();
    let notes_path = dir.join("notes-gold.json");
    let corpus_path = dir.join("conversations-learning.json");
    let coverage_prompt_path = dir.join("coverage-judge-prompt.md");
    let (notes, notes_sha256) = load_json::<Vec<GoldNote>>(&notes_path);
    let (corpus, corpus_sha256) = load_json::<Corpus>(&corpus_path);
    let (heldout, heldout_sha256) = load_json::<Vec<Question>>(&dir.join("questions-heldout.json"));
    let coverage_bytes = read_bytes(&coverage_prompt_path);
    let coverage_prompt_sha256 = sha256_hex(&coverage_bytes);
    let coverage_template = String::from_utf8(coverage_bytes).expect("coverage prompt is UTF-8");

    let tmp = tempfile::tempdir().expect("tempdir");
    let replay = prepare_replay(from, tmp.path()).unwrap_or_else(|e| panic!("memory_eval: {e}"));
    if replay.transcript.corpus_sha256 != corpus_sha256 {
        eprintln!(
            "memory_eval: WARNING the kept transcript was fed from another corpus ({}); the turn map is read as it is",
            replay.transcript.corpus_sha256
        );
    }
    let cfg = eval_daemon_config(System::L);
    let daemon_config = Arc::new(ArcSwap::from_pointee(cfg.clone()));
    let bus = EventBus::default();
    let service =
        Arc::new(MemoryService::open_default(Some(replay.db.clone())).with_bus(bus.clone()));
    let queue = LearningQueue::new(
        LearningDeps {
            db: replay.db.clone(),
            router: router.clone(),
            daemon_config,
            memory_service: service.clone(),
        },
        QueueMode::Manual,
    );
    let reader = router.effective_default_model();
    let mut judge_gate = resolve_judge(
        &settings.judge,
        &router,
        reader.as_deref(),
        cfg.orchestrator.costs.extraction_model.as_deref(),
    );
    let fed: Vec<&CorpusSession> = replay
        .transcript
        .sessions
        .iter()
        .filter_map(|t| corpus.sessions.iter().find(|s| s.id == t.corpus_id))
        .collect();
    let mut conditions = learning_conditions(
        &cfg,
        &corpus_path,
        &corpus_sha256,
        &fed,
        settings,
        true,
        0,
        &coverage_prompt_path,
        &coverage_prompt_sha256,
    );
    // A replay flushes each stored session whole.
    conditions.turns_per_flush = 0;
    report.conditions = Some(replay_conditions(
        settings,
        &replay,
        &router,
        &notes_path,
        notes_sha256,
        &heldout_sha256,
        judge_gate.as_ref(),
        conditions,
    ));
    write_report(&settings.report_path, &report);

    let flushes = replay_learning(&queue, &bus, &replay.transcript).await;
    let mut learning = LearningResults {
        cost: Some(learning_cost(
            &[],
            &flushes,
            started.elapsed().as_millis() as u64,
        )),
        flushes,
        ..Default::default()
    };
    let ctx = service
        .context(
            &replay.owner_id,
            WorkspaceHint::Root(&replay.project),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution {
                source: Some("memory_eval".to_string()),
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("memory_eval: no memory context: {e}"));
    measure_learning(
        &router,
        judge_gate.as_mut(),
        &coverage_template,
        &service,
        &ctx,
        &notes,
        &relied_on(&heldout),
        &fed,
        &replay.transcript,
        &mut learning,
    )
    .await;
    let rerun = forced_rerun(
        &queue,
        &bus,
        &replay.db,
        &service,
        &ctx,
        &replay.transcript,
        &learning.flushes,
        &learning.learned,
    )
    .await;
    learning.forced_rerun = Some(rerun);
    learning.invariants = Some(invariants(&learning, true));
    print_learning(&learning);
    let violated = learning.invariants.as_ref().is_some_and(|i| i.violated);
    report.learning = Some(learning);
    report.status = "complete";
    report.run_finished_at = Some(now_rfc3339());
    write_report(&settings.report_path, &report);
    println!("report: {}", settings.report_path.display());
    if settings.keep_scratch {
        println!("scratch kept at {}", tmp.keep().display());
    }
    assert!(!violated, "memory_eval: a learning invariant was violated");
}

/// A replay's conditions: no questions, no seeding, the replay's own owner
/// and scratch.
#[allow(clippy::too_many_arguments)]
fn replay_conditions(
    settings: &Settings,
    replay: &ReplayScratch,
    router: &openalpaca_llm::LlmRouter,
    notes_path: &Path,
    notes_sha256: String,
    heldout_sha256: &str,
    judge: Option<&JudgeGate>,
    learning: LearningConditions,
) -> Conditions {
    let cfg = eval_daemon_config(System::L);
    let recall = RecallBudget::for_mode(cfg.orchestrator.memory.recall_mode);
    Conditions {
        owner_id: replay.owner_id.clone(),
        llm_toml: settings.llm_toml.to_string_lossy().into_owned(),
        llm_toml_sha256: sha256_hex(&read_bytes(&settings.llm_toml)),
        reader_default_model: router.default_model(),
        reader_temperature: None,
        reader_max_rounds: openalpaca_core::runner::LoopConfig::default().max_rounds,
        reader_max_cost_usd: openalpaca_core::runner::LoopConfig::default().max_cost,
        main_loop_max_rounds: cfg.orchestrator.routing.main_loop_max_rounds,
        main_loop_max_tools_per_round: cfg.orchestrator.routing.main_loop_max_tools_per_round,
        tool_result_inline_bytes: cfg.orchestrator.sessions.tool_result_inline_bytes,
        autocompact_buffer_ratio: cfg.execution.context.autocompact_buffer_ratio,
        judge_model_requested: settings.judge.requested().map(str::to_string),
        judge_temperature: JUDGE_TEMPERATURE,
        judge_max_tokens: JUDGE_MAX_TOKENS,
        judge_prompt_path: eval_dir()
            .join("judge-prompt.md")
            .to_string_lossy()
            .into_owned(),
        judge_prompt_sha256: sha256_hex(&read_bytes(&eval_dir().join("judge-prompt.md"))),
        notes_path: notes_path.to_string_lossy().into_owned(),
        notes_sha256,
        notes_total: 0,
        notes_seeded: 0,
        notes_duplicate: 0,
        notes_failed: 0,
        questions_path: eval_dir()
            .join("questions-heldout.json")
            .to_string_lossy()
            .into_owned(),
        questions_sha256: heldout_sha256.to_string(),
        questions_total: 0,
        questions_selected: 0,
        limit: None,
        category_filter: None,
        per_turn_fts_active: false,
        memory_search_tool_offered: false,
        unattended: true,
        extraction_disabled: false,
        workflow_cap: cfg.orchestrator.routing.max_workflows_per_lane,
        home_store: settings
            .home_store
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        scratch_dir: None,
        system_description: "a replay of L's learning phase (design 09 §7.2): the kept transcript \
                             learned again by a fresh store, with no reader call; no questions",
        memory_tools_offered: Vec::new(),
        follow_up_search_bound: None,
        recall_mode: cfg.orchestrator.memory.recall_mode.as_str(),
        recall_total_bytes: recall.total_bytes,
        recall_pinned_bytes: recall.pinned_bytes,
        recall_max_items: recall.max_items,
        read_max_bytes: openalpaca_core::memory::service::READ_MAX_BYTES,
        notes_scope_applied: false,
        notes_superseding: 0,
        seeded_ids: BTreeMap::new(),
        judge_model: judge.map(|g| g.choice.model.clone()),
        judge_is_reader: judge.map(|g| g.choice.judge_is_reader),
        judge_same_family: judge.map(|g| g.choice.judge_same_family),
        judge_is_extractor: judge.map(|g| g.choice.judge_is_extractor),
        learning: Some(learning),
    }
}

fn print_totals(system: System, t: &Totals) {
    println!(
        "\n== memory_eval: system {} — {}/{} correct ({:.1}%), {} unparsed, {} judge errors, {} turn errors ==",
        system.as_str(),
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
}

fn print_learning(l: &LearningResults) {
    if let Some(c) = &l.coverage {
        println!(
            "coverage: {}/{} ({:.1}%)",
            c.matched,
            c.gold_notes,
            c.share.unwrap_or(0.0) * 100.0
        );
    }
    if let Some(correct) = l.changed_facts_correct {
        println!("changed facts: {correct}/{} correct", l.changed_facts.len());
    }
    if !l.restatements_by_outcome.is_empty() {
        println!("restatements: {:?}", l.restatements_by_outcome);
    }
    if let Some(a) = &l.aliases {
        println!(
            "aliases: other script {:.1}%, any {:.1}% of {} active learned notes",
            a.share_other_script.unwrap_or(0.0) * 100.0,
            a.share_any.unwrap_or(0.0) * 100.0,
            a.active_learned
        );
    }
    if let Some(s) = &l.suggestion_check {
        println!(
            "L-S: {}/{} seeded notes have a suggestion ({} in all, {} elsewhere), {} confirmed and superseded",
            s.targets_with_a_suggestion,
            s.targets.len(),
            s.suggestions_total,
            s.suggestions_elsewhere,
            s.confirmed_and_superseded
        );
    }
    if let Some(c) = &l.cost {
        println!(
            "learning: {} flushes ({:?}), {} extraction calls, {} in / {} out tokens, flush p50={}ms p95={}ms",
            c.flushes,
            c.flushes_by_trigger,
            c.extraction_calls,
            c.extraction_tokens_in,
            c.extraction_tokens_out,
            c.flush_latency_p50_ms,
            c.flush_latency_p95_ms
        );
    }
    if let Some(r) = &l.forced_rerun {
        println!(
            "forced re-run: forgot {}, {} extraction call(s), {} resurrected{}{}",
            r.forgotten.as_deref().unwrap_or("nothing"),
            r.flush.as_ref().map_or(0, |f| f.calls),
            r.resurrected.len(),
            r.flush
                .as_ref()
                .and_then(|f| f.stopped.as_deref())
                .map(|s| format!(", stopped: {s}"))
                .unwrap_or_default(),
            r.skipped
                .as_deref()
                .map(|s| format!(", skipped: {s}"))
                .unwrap_or_default()
        );
    }
    if let Some(i) = &l.invariants {
        println!("invariants: {i:?}");
    }
}

/// The question phase: every question on a fresh lane, as D asks it —
/// `Orchestrator::handle_message`, which persists nothing — and judged.
#[allow(clippy::too_many_arguments)]
async fn ask_questions(
    orch: &openalpaca_core::orchestrator::Orchestrator,
    bus: &openalpaca_core::bus::EventBus,
    router: &openalpaca_llm::LlmRouter,
    mut gate: Option<&mut JudgeGate>,
    judge_template: &str,
    questions: &[Question],
    project: &Path,
    owner_id: &str,
    gold_by_service_id: &HashMap<String, String>,
    report: &mut Report,
    report_path: &Path,
) {
    use openalpaca_core::events::SystemEvent;
    use openalpaca_core::gateway::HandleRequest;
    use openalpaca_core::orchestrator::MAIN_LOOP_AGENT_ID;
    use openalpaca_core::security::policy::{Principal, Scope};
    use tokio::sync::broadcast::error::TryRecvError;
    use uuid::Uuid;

    for (i, q) in questions.iter().enumerate() {
        // A fresh lane per question: no cross-question history, one owner.
        let lane_key = format!("memory-eval-q{i:04}:cli");
        let request_id = Uuid::new_v4();
        let mut rx = bus.subscribe();
        let t0 = Instant::now();
        let reply = orch
            .handle_message(HandleRequest {
                workspace_path: Some(project.to_string_lossy().into_owned()),
                // Nobody here can answer a confirmation prompt: a tool that
                // needs one is refused at once instead of hanging (M6).
                unattended: true,
                ..HandleRequest::new(
                    request_id,
                    "cli",
                    q.question.clone(),
                    Principal::User {
                        global_id: owner_id.to_string(),
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
        if let Some(gate) = gate.as_deref_mut() {
            match judge(router, gate, judge_template, q, &answer).await {
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
        write_report(report_path, &*report);
    }
}

// ─── The harness's own tests: no model, no network, no key ──────────────────

/// A model that answers from the request itself and keeps every request: the
/// harness tests' offline seam. It never opens a socket — the router hands
/// the request to whatever provider is registered for the model's type.
struct StubModel {
    seen: std::sync::Mutex<Vec<openalpaca_llm::ChatRequest>>,
    answer: Box<dyn Fn(&openalpaca_llm::ChatRequest) -> String + Send + Sync>,
}

impl StubModel {
    fn requests(&self) -> Vec<openalpaca_llm::ChatRequest> {
        self.seen.lock().unwrap().clone()
    }
}

/// The whole prompt of a request, every message joined.
fn prompt_of(request: &openalpaca_llm::ChatRequest) -> String {
    request
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait::async_trait]
impl openalpaca_llm::LlmProvider for StubModel {
    fn name(&self) -> &str {
        "stub"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn requires_key(&self) -> bool {
        false
    }

    async fn chat(
        &self,
        request: openalpaca_llm::ChatRequest,
    ) -> Result<openalpaca_llm::ChatResponse, openalpaca_llm::LlmError> {
        self.seen.lock().unwrap().push(request.clone());
        Ok(openalpaca_llm::ChatResponse {
            content: (self.answer)(&request),
            tool_calls: Vec::new(),
            model: request.model.clone().unwrap_or_else(|| "stub".to_string()),
            usage: openalpaca_llm::Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            },
            finish_reason: openalpaca_llm::FinishReason::Stop,
            thinking: None,
            parts: None,
        })
    }
}

/// The test model of the design (owner, 2026-09-24).
const TEST_MODEL: &str = "qwen3.8:27b-mlx";

/// A router serving `models` from one stub Ollama provider, the first the
/// default: `LlmRouter::single_provider`'s shape, with rate limits no test
/// reaches — at the default 40 000 TPM a fed turn's reader prompt waits
/// seconds for the bucket, which would time the limiter, not the harness.
fn stub_router(
    models: &[&str],
    answer: impl Fn(&openalpaca_llm::ChatRequest) -> String + Send + Sync + 'static,
) -> (Arc<openalpaca_llm::LlmRouter>, Arc<StubModel>) {
    use openalpaca_llm::{
        ApiKey, CostTracker, KeyPool, LlmRouter, LlmRuntimeConfig, ModelInfo, ModelRegistry,
        ProviderEntry, ProviderType, RateLimitConfig, SelectionStrategy,
    };
    let stub = Arc::new(StubModel {
        seen: std::sync::Mutex::new(Vec::new()),
        answer: Box::new(answer),
    });
    let key = ApiKey::new("default".to_string(), ProviderType::Ollama, String::new());
    let entry = ProviderEntry {
        provider: stub.clone(),
        key_pool: Arc::new(arc_swap::ArcSwap::from_pointee(KeyPool::new(
            vec![key],
            SelectionStrategy::RoundRobin,
        ))),
    };
    let router = LlmRouter::new_with_runtime(
        HashMap::from([(ProviderType::Ollama, entry)]),
        ModelRegistry::with_defaults(),
        HashMap::new(),
        Arc::new(CostTracker::new(ModelRegistry::with_defaults())),
        models[0].to_string(),
        LlmRuntimeConfig::default(),
        RateLimitConfig {
            default_rpm: 1_000_000,
            default_tpm: 1_000_000_000,
            ..Default::default()
        },
    );
    for model in models {
        router.model_registry().register(
            model.to_string(),
            ModelInfo {
                provider: ProviderType::Ollama,
                input_price_per_million: 0.0,
                output_price_per_million: 0.0,
                context_window: 32_768,
                discovered: true,
                supports_image: false,
                supports_audio: false,
                supports_document: false,
                supports_reasoning: false,
                supports_tools: true,
                declared: false,
            },
        );
    }
    (Arc::new(router), stub)
}

/// LQ1: an unroutable judge is refused before any call — the ladder would
/// answer in its place — whether named or defaulted to the reader.
#[test]
fn an_unroutable_judge_is_refused() {
    let (router, stub) = stub_router(&[TEST_MODEL], |_| String::new());
    let err = judge_guard(
        &router,
        Some("claude-opus-4-1-nonexistent"),
        Some(TEST_MODEL),
        None,
    )
    .unwrap_err();
    assert!(err.contains("not routable"), "{err}");
    // A model the registry knows whose provider is not loaded.
    let err = judge_guard(&router, Some("gpt-4o"), Some(TEST_MODEL), None).unwrap_err();
    assert!(err.contains("not routable"), "{err}");
    let err = judge_guard(&router, None, None, None).unwrap_err();
    assert!(err.contains("no reader model is routable"), "{err}");
    assert!(stub.requests().is_empty(), "the guard makes no call");
}

/// LQ1: the reader judging itself is accepted and recorded, not refused.
#[test]
fn a_judge_equal_to_the_reader_is_accepted_and_recorded() {
    let (router, _) = stub_router(&[TEST_MODEL], |_| String::new());
    let choice = judge_guard(&router, None, Some(TEST_MODEL), None).expect("accepted");
    assert_eq!(
        choice,
        JudgeChoice {
            model: TEST_MODEL.to_string(),
            reader: Some(TEST_MODEL.to_string()),
            judge_is_reader: true,
            judge_same_family: true,
            judge_is_extractor: true,
        }
    );
    // Named explicitly, the same.
    assert_eq!(
        judge_guard(&router, Some(TEST_MODEL), Some(TEST_MODEL), None),
        Ok(choice)
    );
}

/// LQ1: a judge of the reader's family (the other `qwen3.8` build) is
/// accepted and recorded as such; another family is recorded as neither.
#[test]
fn a_judge_of_the_readers_family_is_accepted_and_recorded() {
    let (router, _) = stub_router(&[TEST_MODEL, "qwen3.8:27b-mtp-q8_0", "llama4:8b"], |_| {
        String::new()
    });
    let sibling = judge_guard(
        &router,
        Some("qwen3.8:27b-mtp-q8_0"),
        Some(TEST_MODEL),
        None,
    )
    .expect("accepted");
    assert!(!sibling.judge_is_reader);
    assert!(sibling.judge_same_family);
    assert!(!sibling.judge_is_extractor);
    let other = judge_guard(
        &router,
        Some("llama4:8b"),
        Some(TEST_MODEL),
        Some("llama4:8b"),
    )
    .expect("accepted");
    assert!(!other.judge_is_reader && !other.judge_same_family);
    assert!(other.judge_is_extractor, "the extractor named it");
    assert_eq!(model_family("qwen3.8:27b-mlx"), "qwen3.8");
    assert_eq!(model_family("claude-sonnet-4-5"), "claude-sonnet-4-5");
}

#[test]
fn the_judge_model_env_value_reads_reader_model_or_none() {
    assert_eq!(JudgeRequest::from_env_value(None), JudgeRequest::Reader);
    assert_eq!(
        JudgeRequest::from_env_value(Some(" ".into())),
        JudgeRequest::Reader
    );
    assert_eq!(
        JudgeRequest::from_env_value(Some("none".into())),
        JudgeRequest::Off
    );
    assert_eq!(
        JudgeRequest::from_env_value(Some(TEST_MODEL.into())),
        JudgeRequest::Model(TEST_MODEL.into())
    );
}

/// The post-call half of the guard: a later mismatch fails its own row…
#[test]
fn a_judge_that_answers_as_another_model_fails_its_row() {
    assert!(answering_judge_is_requested(TEST_MODEL, TEST_MODEL).is_ok());
    let err = answering_judge_is_requested(TEST_MODEL, "qwen3.8:27b-mtp-q8_0").unwrap_err();
    assert!(err.contains("substituted"), "{err}");
    let (router, _) = stub_router(&[TEST_MODEL], |_| String::new());
    let mut gate = JudgeGate::new(judge_guard(&router, None, Some(TEST_MODEL), None).unwrap());
    assert!(
        gate.check(TEST_MODEL).is_ok(),
        "the first call answered as itself"
    );
    assert!(gate.check("llama4:8b").is_err());
}

/// … and the first call's mismatch stops the run.
#[test]
#[should_panic(expected = "the judge that answered was llama4:8b")]
fn the_first_judge_call_answering_as_another_model_stops_the_run() {
    let (router, _) = stub_router(&[TEST_MODEL], |_| String::new());
    let mut gate = JudgeGate::new(judge_guard(&router, None, Some(TEST_MODEL), None).unwrap());
    let _ = gate.check("llama4:8b");
}

/// The frozen coverage-judge prompt carries each placeholder once and names
/// both verdicts; its hash goes in every learning report.
#[test]
fn the_coverage_judge_prompt_is_complete() {
    let prompt =
        String::from_utf8(read_bytes(&eval_dir().join("coverage-judge-prompt.md"))).expect("utf-8");
    for p in COVERAGE_PLACEHOLDERS {
        assert_eq!(prompt.matches(p).count(), 1, "{p} exactly once");
    }
    assert!(prompt.contains("MATCH") && prompt.contains("NONE"));
    let gold = GoldNote {
        body: "# Editor\n\n## Knowledge\nShe edits with Helix.\n\n## Evidence and limits\nSaid in June."
            .to_string(),
        ..gold("g", &[])
    };
    let filled = fill_coverage_prompt(
        &prompt,
        &gold,
        &[(
            "Editor".to_string(),
            "The user edits with Helix.".to_string(),
        )],
    );
    assert!(!filled.contains("{{"), "every placeholder filled");
    assert!(filled.contains("She edits with Helix."));
    assert!(
        !filled.contains("Said in June"),
        "only the knowledge section"
    );
    assert!(filled.contains("[1] Editor\nThe user edits with Helix."));
}

#[test]
fn the_coverage_verdict_is_the_last_line() {
    use CoverageVerdict::*;
    assert_eq!(
        parse_coverage_verdict("It says Helix.\nMATCH 2", 3),
        Match(2)
    );
    assert_eq!(
        parse_coverage_verdict("reason\n**MATCH [1]**\n", 1),
        Match(1)
    );
    assert_eq!(parse_coverage_verdict("reason\nmatch: 3", 3), Match(3));
    assert_eq!(parse_coverage_verdict("reason\nNONE.", 3), NoMatch);
    assert_eq!(
        parse_coverage_verdict("MATCH 4", 3),
        Unparsed,
        "past the list"
    );
    assert_eq!(parse_coverage_verdict("MATCH 0", 3), Unparsed);
    assert_eq!(parse_coverage_verdict("MATCH 1\nbecause", 3), Unparsed);
    assert_eq!(parse_coverage_verdict("", 3), Unparsed);
}

#[test]
fn the_knowledge_section_is_read_alone() {
    let body = "# Title\n\n## Knowledge\nLine one.\n### A sub-heading\nLine two.\n\n## Evidence and limits\nNot this.";
    assert_eq!(
        knowledge_section(body),
        "Line one.\n### A sub-heading\nLine two."
    );
    assert_eq!(knowledge_section("Just a body."), "Just a body.");
    assert_eq!(
        knowledge_section("## knowledge\nlower case\n## Recheck when\nx"),
        "lower case"
    );
}

#[test]
fn the_coverage_query_is_the_title_and_aliases_within_the_cap() {
    let mut note = gold("g", &[]);
    note.title = "Editor: Helix".to_string();
    note.aliases = vec!["hx".to_string(), "编辑器".to_string()];
    assert_eq!(coverage_query(&note), "Editor: Helix hx 编辑器");
    note.aliases = vec!["x".repeat(600)];
    assert_eq!(
        coverage_query(&note).chars().count(),
        openalpaca_storage::memory_index::MAX_QUERY_CHARS
    );
}

fn hit(note: &str, status: &'static str, loser: bool) -> CoverageHit {
    CoverageHit {
        note: note.to_string(),
        title: note.to_string(),
        effective_status: status,
        same_key_loser: loser,
        origin: "extracted_user",
    }
}

/// Design 09 §7.3's five outcomes of a change pair, from the notes the
/// coverage judge matched for its old and new gold notes.
#[test]
fn a_change_pair_is_classified_by_what_the_coverage_judge_matched() {
    let new = hit("n", "active", false);
    assert_eq!(classify_pair(None, None), "new_missing");
    assert_eq!(
        classify_pair(Some(&hit("o", "active", false)), None),
        "new_missing"
    );
    assert_eq!(classify_pair(None, Some(&new)), "old_never_learned");
    assert_eq!(
        classify_pair(Some(&hit("n", "active", false)), Some(&new)),
        "old_never_learned",
        "one note matched both"
    );
    assert_eq!(
        classify_pair(Some(&hit("o", "superseded", false)), Some(&new)),
        "new_active_old_superseded"
    );
    assert_eq!(
        classify_pair(Some(&hit("o", "active", true)), Some(&new)),
        "new_active_old_same_key_loser"
    );
    assert_eq!(
        classify_pair(Some(&hit("o", "active", false)), Some(&new)),
        "both_active_unlinked"
    );
    assert_eq!(
        classify_pair(
            Some(&hit("o", "active", false)),
            Some(&hit("n", "active", true))
        ),
        "new_not_active",
        "the new note lost its key"
    );
    assert_eq!(classify_correction(Some(&new)), "new_active");
    assert_eq!(classify_correction(None), "new_missing");
    assert_eq!(
        classify_correction(Some(&hit("n", "superseded", false))),
        "new_not_active"
    );
}

fn learned(
    id: &str,
    status: &'static str,
    key: Option<&str>,
    sources: &[i64],
    supersedes: &[&str],
) -> LearnedNote {
    LearnedNote {
        id: id.to_string(),
        title: format!("title {id}"),
        kind: "fact",
        scope: "global",
        status,
        effective_status: if status == "candidate" {
            "candidate"
        } else {
            "active"
        },
        origin: "extracted_user",
        fact_key: key.map(str::to_string),
        aliases: Vec::new(),
        valid_from: None,
        valid_to: None,
        sources: sources.iter().map(|r| format!("message:{r}")).collect(),
        supersedes: supersedes.iter().map(|s| s.to_string()).collect(),
        superseded_by: Vec::new(),
        conflict: false,
        source_forgotten: false,
        knowledge: String::new(),
    }
}

/// Design 09 §7.3's restatement outcomes, from the notes citing the
/// restating turn's row.
#[test]
fn a_restatement_is_classified_by_the_notes_citing_its_row() {
    let earlier = learned("a", "active", Some("user.drink"), &[1], &[]);
    let classify = |notes: &[LearnedNote]| classify_restatement(Some(9), notes).0;
    assert_eq!(classify(std::slice::from_ref(&earlier)), "nothing_written");
    assert_eq!(
        classify(&[
            earlier.clone(),
            learned("b", "candidate", Some("user.drink"), &[9], &["a"])
        ]),
        "suggestion"
    );
    assert_eq!(
        classify(&[
            earlier.clone(),
            learned("b", "active", Some("user.drink"), &[9], &["a"])
        ]),
        "automatic_replacement"
    );
    assert_eq!(
        classify(&[
            earlier.clone(),
            learned("b", "active", Some("user.drink"), &[9], &[])
        ]),
        "duplicate_note"
    );
    assert_eq!(
        classify(&[learned("b", "active", Some("user.drink"), &[9], &[])]),
        "first_learned",
        "the earlier statement was never learned"
    );
    assert_eq!(
        classify(&[learned("b", "candidate", None, &[9], &[])]),
        "candidate"
    );
    assert_eq!(classify_restatement(None, &[earlier]).0, "unknown_row");
}

/// Design 09 §7.3's alias measure: an alias in the other script than the
/// title's — CJK for a Latin title, Latin for a CJK one.
#[test]
fn other_script_aliases_are_read_against_the_titles_script() {
    let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    assert!(has_other_script_alias(
        "Editor: Helix",
        &s(&["hx", "编辑器"])
    ));
    assert!(!has_other_script_alias(
        "Editor: Helix",
        &s(&["hx", "editor"])
    ));
    assert!(has_other_script_alias("用户住在杭州", &s(&["Hangzhou"])));
    assert!(!has_other_script_alias(
        "用户住在杭州",
        &s(&["杭州", "时区"])
    ));
    assert!(has_other_script_alias("日本語の勉強", &s(&["Japanese"])));
    assert!(!has_other_script_alias("Title", &[]));
}

#[test]
fn cohens_kappa_reads_two_binary_gradings() {
    assert_eq!(cohen_kappa(&[]), None);
    assert_eq!(
        cohen_kappa(&[(true, true), (true, true)]),
        None,
        "no variance"
    );
    let perfect = [(true, true), (false, false), (true, true), (false, false)];
    assert_eq!(cohen_kappa(&perfect), Some(1.0));
    // Observed 0.5; chance 0.5 → κ 0.
    let coin = [(true, true), (true, false), (false, true), (false, false)];
    assert_eq!(cohen_kappa(&coin), Some(0.0));
}

/// The learning systems' config: no workflow, no task extraction, learning
/// on only where it learns, and the extraction caps lifted there.
#[test]
fn a_learning_system_lifts_the_extraction_caps() {
    let l = eval_daemon_config(System::L);
    assert!(l.orchestrator.learning.enabled);
    assert_eq!(l.orchestrator.costs.extract_max_calls_per_day, 0, "no cap");
    assert_eq!(
        l.orchestrator.costs.extract_max_daily_cost_usd,
        LIFTED_DAILY_COST_USD
    );
    assert_eq!(l.orchestrator.routing.max_workflows_per_lane, 0);
    assert!(!l.orchestrator.costs.task_extract_enabled);
    for system in [System::D, System::E, System::NoMemory, System::L0] {
        let cfg = eval_daemon_config(system);
        assert!(!cfg.orchestrator.learning.enabled, "{}", system.as_str());
        assert_eq!(
            cfg.orchestrator.costs.extract_max_calls_per_day,
            openalpaca_core::daemon_config::DaemonConfig::default()
                .orchestrator
                .costs
                .extract_max_calls_per_day
        );
    }
    let e = eval_daemon_config(System::E);
    assert_eq!(e.orchestrator.routing.main_loop_max_tools_per_round, 1);
}

/// L-S feeds the suggestion sessions only; L and L0 feed everything, and a
/// session limit takes the first N of what the system feeds.
#[test]
fn the_fed_sessions_follow_the_system() {
    let (corpus, _) = load_json::<Corpus>(&eval_dir().join("conversations-learning.json"));
    let ids = |v: Vec<&CorpusSession>| v.into_iter().map(|s| s.id.clone()).collect::<Vec<_>>();
    assert_eq!(
        ids(corpus_sessions(&corpus, System::L, None)).len(),
        corpus.sessions.len()
    );
    assert_eq!(
        ids(corpus_sessions(&corpus, System::LS, None)),
        corpus.suggestion_sessions
    );
    assert_eq!(
        ids(corpus_sessions(&corpus, System::LS, Some(2))),
        corpus.suggestion_sessions[..2].to_vec()
    );
    assert_eq!(ids(corpus_sessions(&corpus, System::L0, Some(3))).len(), 3);
}

/// §7.3's resurrection invariant is hard, so a forced re-run that checked
/// nothing — skipped before its flush, a flush that made no extraction call,
/// or one that stopped before its session's end — is not a pass for L or a
/// replay.
#[test]
fn a_forced_rerun_that_checked_nothing_violates_the_invariant() {
    use openalpaca_core::memory::learning::{FlushReport, FlushStop};
    let stopped_flush = |calls, stopped| {
        FlushRecord::new(
            "cs-001",
            FLUSH_FORCED_RERUN,
            0,
            &FlushReport {
                calls,
                stopped,
                ..Default::default()
            },
            Vec::new(),
            0,
        )
    };
    let flush = |calls| stopped_flush(calls, None);
    let with = |rerun: ForcedRerun| LearningResults {
        forced_rerun: Some(rerun),
        ..Default::default()
    };
    let skipped = with(ForcedRerun {
        skipped: Some("no active learned note cites a fed message".to_string()),
        ..Default::default()
    });
    let no_call = with(ForcedRerun {
        forgotten: Some("note".to_string()),
        flush: Some(flush(0)),
        ..Default::default()
    });
    let checked = with(ForcedRerun {
        forgotten: Some("note".to_string()),
        flush: Some(flush(1)),
        ..Default::default()
    });
    let resurrected = with(ForcedRerun {
        forgotten: Some("note".to_string()),
        flush: Some(flush(1)),
        resurrected: vec!["again".to_string()],
        ..Default::default()
    });
    // `calls` counts an answer before it is parsed: an unusable first answer
    // stops the flush with nothing examined, and a later chunk that finds no
    // model leaves the rest of the session unread.
    let bad_output = with(ForcedRerun {
        forgotten: Some("note".to_string()),
        flush: Some(stopped_flush(1, Some(FlushStop::BadOutput))),
        ..Default::default()
    });
    let unavailable = with(ForcedRerun {
        forgotten: Some("note".to_string()),
        flush: Some(stopped_flush(2, Some(FlushStop::Unavailable))),
        ..Default::default()
    });
    for (what, learning) in [
        ("a skipped re-run", &skipped),
        ("no call", &no_call),
        ("an unusable answer", &bad_output),
        ("a later chunk with no model", &unavailable),
    ] {
        let inv = invariants(learning, true);
        assert!(!inv.forced_rerun_checked, "{what}: {inv:?}");
        assert_eq!(inv.forgotten_resurrected, 0, "{what}");
        assert!(inv.violated, "{what}: 0 resurrected is not a pass");
    }
    let inv = invariants(&checked, true);
    assert!(inv.forced_rerun_checked && !inv.violated, "{inv:?}");
    let inv = invariants(&resurrected, true);
    assert!(inv.forced_rerun_checked && inv.violated, "{inv:?}");
    // L with no re-run at all is not a pass; L0 and L-S run none.
    assert!(invariants(&LearningResults::default(), true).violated);
    let inv = invariants(&LearningResults::default(), false);
    assert!(!inv.forced_rerun_checked && !inv.violated, "{inv:?}");
}

/// Re-judge mode (design 09 §7.4) on a fixture-sized archived report: every
/// row is graded again with no reader call, keeps its original verdict
/// beside the new one, the totals and the agreement are recomputed, and the
/// report records where it came from and which judge graded it.
#[tokio::test]
async fn rejudge_reads_a_fixture_sized_archived_report() {
    let archived_dir = tempfile::tempdir().unwrap();
    let archived_path = archived_dir.path().join("report-D.json");
    let row = |id: &str, answer: &str, verdict: &str| {
        serde_json::json!({
            "id": id, "category": "changed_facts_time", "language": "en",
            "question": format!("question {id}"), "gold_answer": "Helix",
            "expect_abstain": false, "relies_on": [], "answer": answer,
            "turn_error": null, "correct": verdict == "correct", "judge_verdict": verdict,
            "judge_raw": "…", "judge_model": "qwen3.8:27b-mtp-q8_0",
            "judge_tokens_in": 1, "judge_tokens_out": 1, "judge_error": null,
            "latency_ms": 1, "reader_models": [TEST_MODEL],
        })
    };
    let archived = serde_json::json!({
        "schema": 1, "status": "complete", "system": "D",
        "conditions": {"reader_default_model": TEST_MODEL, "judge_prompt_sha256": "old"},
        "totals": {"correct": 3, "reader_models": [TEST_MODEL], "judge_models": ["qwen3.8:27b-mtp-q8_0"]},
        "rows": [
            row("q1", "She uses Helix now.", "correct"),
            row("q2", "She uses vim.", "correct"),
            row("q3", "Helix, since June.", "correct"),
        ],
    });
    std::fs::write(&archived_path, serde_json::to_vec(&archived).unwrap()).unwrap();

    let (router, stub) = stub_router(&[TEST_MODEL], |request| {
        let prompt = prompt_of(request);
        let answer = prompt
            .split("Answer under evaluation:")
            .nth(1)
            .unwrap_or_default();
        if answer.contains("Helix") {
            "It names Helix.\nCORRECT".to_string()
        } else {
            "It names vim.\nINCORRECT".to_string()
        }
    });
    let (loaded, sha) = load_json::<serde_json::Value>(&archived_path);
    let mut gate = JudgeGate::new(
        judge_guard(&router, None, archived_reader(&loaded).as_deref(), None).unwrap(),
    );
    let template = String::from_utf8(read_bytes(&eval_dir().join("judge-prompt.md"))).unwrap();
    let mut progress_calls = 0;
    let out = rejudge_report(
        &router,
        &mut gate,
        &template,
        "new-prompt-sha",
        loaded,
        &archived_path,
        &sha,
        &mut |_| progress_calls += 1,
    )
    .await
    .expect("re-judged");

    assert_eq!(progress_calls, 3);
    let requests = stub.requests();
    assert_eq!(requests.len(), 3, "one judge call a row");
    assert!(
        requests
            .iter()
            .all(|r| prompt_of(r).starts_with("You are grading one answer")),
        "no reader call: every request is the judge prompt"
    );
    assert_eq!(
        out["rejudged_from"],
        archived_path.to_string_lossy().as_ref()
    );
    assert_eq!(out["rejudged_from_sha256"], sha.as_str());
    assert_eq!(out["status"], "complete");
    assert_eq!(out["system"], "D", "the archived report's system");
    let rows = out["rows"].as_array().unwrap();
    let verdicts: Vec<&str> = rows
        .iter()
        .map(|r| r["judge_verdict"].as_str().unwrap())
        .collect();
    assert_eq!(verdicts, ["correct", "incorrect", "correct"]);
    assert!(
        rows.iter()
            .all(|r| r["original_judge_verdict"] == "correct")
    );
    assert!(rows.iter().all(|r| r["judge_model"] == TEST_MODEL));
    assert!(
        rows.iter()
            .all(|r| r["original_judge_model"] == "qwen3.8:27b-mtp-q8_0")
    );
    let totals = &out["totals"];
    assert_eq!(totals["correct"], 2);
    assert_eq!(totals["incorrect"], 1);
    assert_eq!(totals["judge_models"], serde_json::json!([TEST_MODEL]));
    assert_eq!(
        totals["reader_models"],
        serde_json::json!([TEST_MODEL]),
        "kept"
    );
    assert_eq!(totals["agreement_with_original"]["rows"], 3);
    assert_eq!(totals["agreement_with_original"]["agree"], 2);
    let conditions = &out["conditions"];
    assert_eq!(conditions["judge_model"], TEST_MODEL);
    assert_eq!(conditions["judge_is_reader"], true);
    assert_eq!(conditions["judge_same_family"], true);
    assert_eq!(conditions["judge_prompt_sha256"], "new-prompt-sha");
    assert_eq!(
        conditions["original_judge_models"],
        serde_json::json!(["qwen3.8:27b-mtp-q8_0"])
    );
}

/// `OPENALPACA_HOME_STORE` pointed at a tempdir for one test — the memory
/// service resolves the home partition from it, and nothing may reach the
/// real `~/.openalpaca`. Serialized: this binary's only writer of the
/// variable.
struct HomeStoreGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

static HOME_STORE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl HomeStoreGuard {
    fn set(path: &Path) -> Self {
        let lock = HOME_STORE_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(openalpaca_storage::store::HOME_STORE_ENV);
        // SAFETY: serialized by HOME_STORE_ENV_LOCK — this binary's only writer.
        unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, path) };
        Self { _lock: lock, prev }
    }
}

impl Drop for HomeStoreGuard {
    fn drop(&mut self) {
        // SAFETY: as above — still holding the lock.
        match self.prev.take() {
            Some(v) => unsafe { std::env::set_var(openalpaca_storage::store::HOME_STORE_ENV, v) },
            None => unsafe { std::env::remove_var(openalpaca_storage::store::HOME_STORE_ENV) },
        }
    }
}

/// An L run's kept scratch, fixture-sized: a database with the owner, two
/// sessions — one with no project, one in the scratch project — whose turns
/// were learned (their cursors at the last row), and the transcript map.
fn fixture_scratch(dir: &Path) -> (String, PathBuf, Transcript) {
    use openalpaca_storage::{
        ConversationMessage, ConversationRepository, Database, IdentityRepository,
        LearningCursorRepository,
    };
    let db = Database::open(&dir.join(SCRATCH_DB)).unwrap();
    let owner = new_owner_id();
    IdentityRepository::new(&db)
        .create_global_user(&owner, None)
        .unwrap();
    let project = scratch_project(dir);
    let project_str = project.to_string_lossy().into_owned();
    let lane = format!("{owner}:gui");
    let repo = ConversationRepository::new(&db);
    let mut sessions = Vec::new();
    for (corpus_id, root, user) in [
        (
            "cs-001",
            None,
            "I only drink tea, Longjing mostly, never coffee.",
        ),
        (
            "cs-002",
            Some(project_str.as_str()),
            "Lantern listens on port 8790 locally now.",
        ),
    ] {
        let session = repo
            .create_session(&lane, "gui", root, Some(corpus_id))
            .unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let row = |role: &str, content: &str, root: Option<&str>| {
            repo.insert(&ConversationMessage {
                lane_key: lane.clone(),
                role: role.to_string(),
                content: content.to_string(),
                source: Some("gui".to_string()),
                session_id: Some(session.id.clone()),
                request_id: Some(request.clone()),
                workspace_root: root.map(str::to_string),
                ..Default::default()
            })
            .unwrap()
        };
        let q = row("user", user, root);
        let a = row("assistant", "Noted.", None);
        // The L run learned it: the cursor sits at the answer.
        assert!(
            LearningCursorRepository::new(&db)
                .advance_cas(&session.id, a, 0)
                .unwrap()
        );
        sessions.push(TranscriptSession {
            corpus_id: corpus_id.to_string(),
            session_id: session.id,
            user_rows: vec![q],
        });
    }
    let transcript = Transcript {
        schema: 1,
        owner_id: owner.clone(),
        project: project_str,
        corpus_sha256: "fixture".to_string(),
        sessions,
    };
    write_json(&dir.join(TRANSCRIPT_FILE), &transcript);
    (owner, project, transcript)
}

/// Replay mode (design 09 §7.2) on a fixture-sized scratch database: a
/// fresh owner and project learn every stored session again from cursor 0,
/// with no reader call — only extraction requests reach the model — and the
/// kept scratch is left as it was.
#[tokio::test]
async fn replay_learns_from_a_fixture_sized_scratch_database_with_no_reader_call() {
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    use openalpaca_core::memory::service::ListInput;
    use openalpaca_storage::memory_index::MemoryIndex;
    use openalpaca_storage::{Database, LearningCursorRepository};

    let home = tempfile::tempdir().unwrap();
    let _env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
    let kept = tempfile::tempdir().unwrap();
    let (old_owner, old_project, old_transcript) = fixture_scratch(kept.path());

    let into = tempfile::tempdir().unwrap();
    let replay = prepare_replay(kept.path(), into.path()).expect("prepared");
    assert_ne!(
        replay.owner_id, old_owner,
        "a fresh owner: a fresh home partition"
    );
    assert_ne!(replay.project, old_project, "a fresh project partition");
    assert_eq!(replay.transcript.sessions.len(), 2);

    let (router, stub) = stub_router(&[TEST_MODEL], |request| {
        let prompt = prompt_of(request);
        let item = |title: &str, content: &str| {
            serde_json::json!({"items": [{
                "title": title, "content": content, "kind": "fact",
                "stated_by": "user", "confidence": 0.9, "rows": [1],
            }]})
            .to_string()
        };
        // The transcript alone: the known notes may list the other session's.
        let conversation = prompt
            .split("<conversation>")
            .nth(1)
            .and_then(|rest| rest.split("</conversation>").next())
            .unwrap_or_default();
        if conversation.contains("Longjing") {
            item(
                "Drinks tea, not coffee",
                "The user drinks only tea, mostly Longjing.",
            )
        } else {
            item("Lantern port", "Lantern listens on port 8790 locally.")
        }
    });
    // As the live replay builds it: the flush's writes reach the bus, which
    // is how the report counts the two learning paths.
    let bus = openalpaca_core::bus::EventBus::default();
    let service = MemoryService::new(
        Some(replay.db.clone()),
        MemoryIndex::open_in_memory().unwrap(),
    )
    .with_bus(bus.clone());
    service.rebuild_index().unwrap();
    let service = Arc::new(service);
    let queue = LearningQueue::new(
        LearningDeps {
            db: replay.db.clone(),
            router,
            daemon_config: Arc::new(arc_swap::ArcSwap::from_pointee(eval_daemon_config(
                System::L,
            ))),
            memory_service: service.clone(),
        },
        QueueMode::Manual,
    );
    let flushes = replay_learning(&queue, &bus, &replay.transcript).await;

    assert_eq!(flushes.len(), 2);
    for f in &flushes {
        assert_eq!(f.label, FLUSH_REPLAY);
        assert_eq!(
            (f.calls, f.stored, f.stopped.as_deref()),
            (1, 1, None),
            "{f:?}"
        );
        assert_eq!(f.written.len(), 1, "the bus saw the write: {f:?}");
    }
    let requests = stub.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| prompt_of(r).contains("You are the memory extractor")),
        "no reader call: every request is an extraction"
    );
    let ctx = service
        .context(
            &replay.owner_id,
            WorkspaceHint::Root(&replay.project),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let learned = service.list(&ctx, ListInput::default()).unwrap().items;
    let mut scopes: Vec<(&str, String)> = learned
        .iter()
        .map(|i| (i.scope.as_str(), i.title.clone()))
        .collect();
    scopes.sort();
    assert_eq!(
        scopes,
        [
            ("global", "Drinks tea, not coffee".to_string()),
            ("project", "Lantern port".to_string()),
        ],
        "each row learned into its own root, rebased to the replay's project"
    );
    // The row map carried over: the dump reads each turn's row as a source.
    let dumped = dump_learned(&service, &ctx).unwrap();
    for s in &replay.transcript.sessions {
        assert!(dumped.iter().any(|n| n.cites_row(s.user_rows[0])));
    }

    // The kept scratch is untouched: its cursors still sit where L left them,
    // and its project never got a store.
    let original = Database::open(&kept.path().join(SCRATCH_DB)).unwrap();
    for s in &old_transcript.sessions {
        assert!(
            LearningCursorRepository::new(&original)
                .get(&s.session_id)
                .unwrap()
                .extracted_through
                > 0
        );
    }
    assert!(!old_project.join(".openalpaca").exists());
}

/// The live learning phase's own path, offline (review of L7, finding 3):
/// two corpus sessions fed through the gateway as `run_system` builds it —
/// one of 11 turns with no project, one of 2 in the scratch project — each
/// turn persisted and mapped to its user row, the flushes at the 10th turn
/// and at each session's end, the reader's calls read off the bus, and then
/// the forced re-run: it forgets a learned note, re-reads its session from
/// cursor 0 with an extraction call, and brings nothing back.
#[tokio::test(flavor = "multi_thread")]
async fn the_gateway_feed_maps_every_turn_and_the_forced_rerun_checks() {
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    use openalpaca_storage::{Database, IdentityRepository};

    let home = tempfile::tempdir().unwrap();
    let _env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch_project(scratch.path());
    let db = Database::open(&scratch.path().join(SCRATCH_DB)).unwrap();
    let owner = new_owner_id();
    IdentityRepository::new(&db)
        .create_global_user(&owner, None)
        .unwrap();

    // The reader answers every turn with no tool call; the extractor keeps
    // the chunk's first own user row (context rows are numbered first).
    let (router, stub) = stub_router(&[TEST_MODEL], |request| {
        let prompt = prompt_of(request);
        if !prompt.contains("You are the memory extractor") {
            return "Noted.".to_string();
        }
        let conversation = prompt
            .split("<conversation>")
            .nth(1)
            .and_then(|rest| rest.split("</conversation>").next())
            .unwrap_or_default();
        let first_own = conversation.lines().find_map(|line| {
            let (n, rest) = line.strip_prefix('R')?.split_once(' ')?;
            let (_, text) = rest.strip_prefix("user ")?.split_once(": ")?;
            Some((n.parse::<u32>().ok()?, text.to_string()))
        });
        let Some((n, text)) = first_own else {
            return r#"{"items": []}"#.to_string();
        };
        serde_json::json!({"items": [{
            "title": text, "content": format!("The user said: {text}"),
            "kind": "fact", "stated_by": "user", "confidence": 0.9, "rows": [n],
        }]})
        .to_string()
    });
    let bus = EventBus::default();
    let cfg = eval_daemon_config(System::L);
    let daemon_config = Arc::new(arc_swap::ArcSwap::from_pointee(cfg.clone()));
    let service = Arc::new(MemoryService::open_default(Some(db.clone())).with_bus(bus.clone()));
    let stack = eval_stack(
        router.clone(),
        &db,
        &bus,
        Some(&service),
        daemon_config.clone(),
    );
    let queue = LearningQueue::new(
        LearningDeps {
            db: db.clone(),
            router,
            daemon_config,
            memory_service: service.clone(),
        },
        QueueMode::Manual,
    );
    let learner = Learner::new(
        &stack,
        &queue,
        &bus,
        &db,
        &owner,
        &project,
        cfg.orchestrator.learning.turns_per_flush,
    );
    assert_eq!(learner.turns_per_flush, 10, "the production cadence");
    let session = |id: &str, workspace: Option<eval::Project>, turns: usize| CorpusSession {
        id: id.to_string(),
        workspace,
        language: Language::En,
        narrative_date: "2026-03-02".to_string(),
        turns: (1..=turns)
            .map(|i| CorpusTurn {
                user: format!("Note {id}-{i}: my locker number this week is {i}{i}{i}."),
                states: Vec::new(),
                mode: TurnMode::Explicit,
            })
            .collect(),
    };
    let long = session("cs-t01", None, 11);
    let short = session("cs-t02", Some(eval::Project::Lantern), 2);
    let fed = learner.feed(&[&long, &short], true).await;

    let labels: Vec<(&str, &str)> = fed
        .flushes
        .iter()
        .map(|f| (f.session.as_str(), f.label))
        .collect();
    assert_eq!(
        labels,
        [
            ("cs-t01", FLUSH_TURNS),
            ("cs-t01", FLUSH_SWITCH),
            ("cs-t02", FLUSH_SWITCH),
        ]
    );
    for f in &fed.flushes {
        assert_eq!(
            (f.calls, f.stored, f.active, f.stopped.as_deref()),
            (1, 1, 1, None),
            "{f:?}"
        );
        assert!(
            f.written
                .iter()
                .any(|w| w.change == "created" && w.surface.as_deref() == Some("extraction")),
            "the bus saw the flush's write: {f:?}"
        );
    }
    assert_eq!(fed.turns.len(), 13);
    let rows: Vec<i64> = fed
        .turns
        .iter()
        .map(|t| t.row.unwrap_or_else(|| panic!("no row for {t:?}")))
        .collect();
    assert!(
        rows.windows(2).all(|w| w[0] < w[1]),
        "one row per turn, in order"
    );
    for t in &fed.turns {
        assert!(!t.is_error, "{t:?}");
        assert!(
            t.reader_llm_calls >= 1,
            "the reader's calls on the bus: {t:?}"
        );
        assert_eq!(t.reader_models, [TEST_MODEL], "{t:?}");
        assert_eq!(t.model_tool_memory_changes, 0, "{t:?}");
    }
    let transcript = Transcript {
        schema: 1,
        owner_id: owner.clone(),
        project: project.to_string_lossy().into_owned(),
        corpus_sha256: "test".to_string(),
        sessions: fed.transcript,
    };
    assert_eq!(transcript.row_of("cs-t01", 10), fed.turns[10].row);
    assert_eq!(transcript.row_of("cs-t02", 1), fed.turns[12].row);

    let ctx = service
        .context(
            &owner,
            WorkspaceHint::Root(&project),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let learned = dump_learned(&service, &ctx).unwrap();
    let cited = |turn: usize| {
        learned
            .iter()
            .find(|n| n.cites_row(rows[turn]))
            .unwrap_or_else(|| panic!("no note cites turn {turn}: {learned:?}"))
    };
    assert_eq!(cited(0).scope, "global", "the first `turns` chunk");
    assert_eq!(cited(10).scope, "global", "the `switch` chunk after it");
    assert_eq!(cited(11).scope, "project", "a project turn learns there");

    let rerun = forced_rerun(
        &queue,
        &bus,
        &db,
        &service,
        &ctx,
        &transcript,
        &fed.flushes,
        &learned,
    )
    .await;
    assert_eq!(rerun.skipped, None, "{rerun:?}");
    assert_eq!(rerun.forgotten.as_deref(), Some(cited(0).id.as_str()));
    assert_eq!(rerun.session.as_deref(), Some("cs-t01"));
    let flush = rerun.flush.clone().expect("the re-run flushed");
    assert_eq!(flush.label, FLUSH_FORCED_RERUN);
    assert!(flush.calls > 0, "it re-read the session: {flush:?}");
    assert!(flush.dropped_suppressed > 0, "{flush:?}");
    assert!(rerun.resurrected.is_empty(), "{rerun:?}");
    let results = LearningResults {
        forced_rerun: Some(rerun),
        ..Default::default()
    };
    let inv = invariants(&results, true);
    assert!(inv.forced_rerun_checked && !inv.violated, "{inv:?}");

    let extractions = stub
        .requests()
        .iter()
        .filter(|r| prompt_of(r).contains("You are the memory extractor"))
        .count();
    assert_eq!(extractions, 3 + flush.calls, "one call per flush chunk");
}

/// L-S's own path, offline (review of L7, finding 3): an old note seeded as
/// the user's own, a change turn fed through the gateway, the extractor's
/// change raised as a suggestion that retires nothing, and the user-directed
/// confirm against the target's revision retiring the old note.
#[tokio::test(flavor = "multi_thread")]
async fn l_s_raises_a_suggestion_and_the_users_confirm_retires_the_old_note() {
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::memory::learning::{LearningDeps, LearningQueue, QueueMode};
    use openalpaca_storage::{Database, IdentityRepository};

    let home = tempfile::tempdir().unwrap();
    let _env = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let project = scratch_project(scratch.path());
    let db = Database::open(&scratch.path().join(SCRATCH_DB)).unwrap();
    let owner = new_owner_id();
    IdentityRepository::new(&db)
        .create_global_user(&owner, None)
        .unwrap();

    // The extractor states the change and names the saved note when the
    // prompt lists it; the reader answers with no tool call.
    let (router, _stub) = stub_router(&[TEST_MODEL], |request| {
        let prompt = prompt_of(request);
        if !prompt.contains("You are the memory extractor") {
            return "Noted.".to_string();
        }
        let section = |tag: &str| {
            prompt
                .split(&format!("<{tag}>"))
                .nth(1)
                .and_then(|rest| rest.split(&format!("</{tag}>")).next())
                .unwrap_or_default()
                .to_string()
        };
        let row = section("conversation").lines().find_map(|line| {
            let (n, rest) = line.strip_prefix('R')?.split_once(' ')?;
            let text = rest.strip_prefix("user ")?;
            text.contains("Helix").then(|| n.parse::<u32>().ok())?
        });
        let Some(row) = row else {
            return r#"{"items": []}"#.to_string();
        };
        let replaces = section("known_notes").lines().find_map(|line| {
            let (n, rest) = line.split_once(' ')?;
            rest.starts_with("key=user.editor ").then(|| n.to_string())
        });
        serde_json::json!({"items": [{
            "title": "Editor: Helix", "kind": "preference", "stated_by": "user",
            "content": "Since June 2026 the user's editor is Helix; it replaced vim.",
            "confidence": 0.9, "rows": [row], "fact_key": "user.editor",
            "replaces": replaces,
        }]})
        .to_string()
    });
    let bus = EventBus::default();
    let cfg = eval_daemon_config(System::LS);
    let daemon_config = Arc::new(arc_swap::ArcSwap::from_pointee(cfg.clone()));
    let service = Arc::new(MemoryService::open_default(Some(db.clone())).with_bus(bus.clone()));

    let (notes, _) = load_json::<Vec<GoldNote>>(&eval_dir().join("notes-gold.json"));
    let pairs = change_pairs(&notes).unwrap();
    let old: Vec<GoldNote> = ls_seed_notes(&notes, &pairs)
        .into_iter()
        .filter(|n| n.fact_key.as_deref() == Some("user.editor"))
        .collect();
    let seed = {
        let (service, owner, root) = (service.clone(), owner.clone(), project.clone());
        blocking(move || seed_notes(&service, &owner, &root, &old)).await
    };
    assert_eq!((seed.seeded, seed.failed), (1, 0));

    let stack = eval_stack(
        router.clone(),
        &db,
        &bus,
        Some(&service),
        daemon_config.clone(),
    );
    let queue = LearningQueue::new(
        LearningDeps {
            db: db.clone(),
            router,
            daemon_config,
            memory_service: service.clone(),
        },
        QueueMode::Manual,
    );
    let learner = Learner::new(
        &stack,
        &queue,
        &bus,
        &db,
        &owner,
        &project,
        cfg.orchestrator.learning.turns_per_flush,
    );
    let editor = pairs
        .iter()
        .find(|p| p.fact_key == "user.editor")
        .expect("the editor pair");
    let turn = |user: &str, states: &[&str], mode| CorpusTurn {
        user: user.to_string(),
        states: states.iter().map(|s| s.to_string()).collect(),
        mode,
    };
    let session = CorpusSession {
        id: "cs-t03".to_string(),
        workspace: None,
        language: Language::En,
        narrative_date: "2026-06-03".to_string(),
        turns: vec![
            turn(
                "Plain vimrc, no plugin manager, same as ever.",
                &[&editor.old],
                TurnMode::Restate,
            ),
            turn(
                "I switched my editor to Helix at the start of June.",
                &[&editor.new],
                TurnMode::Change,
            ),
        ],
    };
    let fed = learner.feed(&[&session], true).await;
    assert_eq!(fed.flushes.len(), 1);
    assert_eq!(
        (fed.flushes[0].suggestions, fed.flushes[0].active),
        (1, 0),
        "{:?}",
        fed.flushes[0]
    );
    let transcript = Transcript {
        schema: 1,
        owner_id: owner.clone(),
        project: project.to_string_lossy().into_owned(),
        corpus_sha256: "test".to_string(),
        sessions: fed.transcript,
    };
    let ctx = service
        .context(
            &owner,
            WorkspaceHint::Root(&project),
            Surface::UserSurface,
            Capabilities::ALL,
            Attribution::default(),
        )
        .unwrap();
    let mut learning = LearningResults::default();
    let check = suggestion_check(
        &service,
        &ctx,
        &pairs,
        &seed,
        &[&session],
        &transcript,
        &mut learning,
    )
    .await;
    assert_eq!(check.targets.len(), 1);
    let target = &check.targets[0];
    assert_eq!(target.old_gold, editor.old);
    assert_eq!(target.suggestions, 1, "{check:?}");
    assert_eq!(target.status_before_confirm, "active", "nothing retired it");
    assert_eq!(target.confirm_error, None);
    assert_eq!(target.status_after_confirm.as_deref(), Some("superseded"));
    assert_eq!(
        (
            check.targets_with_a_suggestion,
            check.suggestions_elsewhere,
            check.confirmed_and_superseded
        ),
        (1, 0, 1)
    );
    let restated: Vec<&str> = learning.restatements.iter().map(|r| r.outcome).collect();
    assert_eq!(restated, ["nothing_written"]);
    learning.suggestion_check = Some(check);
    let inv = invariants(&learning, false);
    assert_eq!(inv.user_notes_retired_without_confirm, 0);
    assert!(!inv.violated, "{inv:?}");
}
