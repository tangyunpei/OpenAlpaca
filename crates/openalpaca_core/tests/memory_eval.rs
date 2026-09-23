//! Live memory-quality eval harness (memory redesign — plan slice 1, §5).
//!
//! Seeds the gold notes of `scripts/eval/memory/notes-gold.json` through the
//! `memory_store` tool path, asks every question of the held-out set through
//! the front door (`Orchestrator::handle_message`, the entry `POST /v1/chat`
//! uses) and judges each answer against its gold answer with the frozen judge
//! prompt, through the configured model at temperature 0. One system under
//! test per run, chosen by `OPENALPACA_MEMORY_EVAL_SYSTEM`; the report is one
//! JSON file at `OPENALPACA_MEMORY_EVAL_REPORT`, and the paired comparison
//! across two reports is `scripts/eval/memory/compare.py`.
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
//! OPENALPACA_LIVE_EVAL=1 OPENALPACA_MEMORY_EVAL_SYSTEM=A \
//! OPENALPACA_MEMORY_EVAL_REPORT=/path/to/report-A.json \
//! OPENALPACA_HOME_STORE=/path/to/scratch/store \
//! OPENALPACA_CONFIG_DIR=/path/to/scratch/config \
//!     cargo test -p openalpaca_core --features live-eval \
//!     --test memory_eval -- --ignored memory_eval --nocapture
//! ```
//!
//! Systems today: `none` (no notes seeded — the context-cost reference), `A`
//! (the per-turn FTS path, `prompt_ctx::sources::memory::MemorySource`) and
//! `B` (the `memory_search` tool path, hybrid FTS + E5, `--features
//! live-eval-e5`). `C`, `D` and `E` arrive with slice 3 and are recorded as
//! `unavailable`. The harness is engine-agnostic on purpose: it drives the
//! store tool and the chat entry, never a repository, so the same file
//! measures D/E in slice 4.
#![cfg(feature = "live-eval")]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The one owner every note is stored under and every question is asked as.
const OWNER_ID: &str = "memory-eval-owner";
/// The router-side agent id of the judge calls (cost accounting).
const JUDGE_AGENT_ID: &str = "memory_eval_judge";
const JUDGE_MAX_TOKENS: u32 = 400;
const JUDGE_TEMPERATURE: f32 = 0.0;
/// An answer longer than this is cut before it reaches the judge (bounds the
/// judge's cost; the full answer is still in the report).
const JUDGE_ANSWER_CAP_CHARS: usize = 6_000;
const REPORT_SCHEMA: u32 = 1;

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

const NOTE_KINDS: [&str; 6] = [
    "fact",
    "preference",
    "decision",
    "procedure",
    "reference",
    "episode",
];

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

/// Always runs under `cargo test --features live-eval` (no model, no
/// network, no keys): the set v0 must load and be structurally sound, so a bad
/// edit is caught long before a paid run.
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
            NOTE_KINDS.contains(&n.kind.as_str()),
            "note {}: kind {}",
            n.id,
            n.kind
        );
        assert!(
            n.scope == "global" || n.scope == "project",
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
    }
    for n in &notes {
        for s in &n.supersedes {
            assert!(
                note_ids.contains(s),
                "note {}: supersedes unknown note {s}",
                n.id
            );
            assert_ne!(s, &n.id, "note {}: supersedes itself", n.id);
        }
    }
    for lang in [Language::En, Language::Zh] {
        assert!(
            notes.iter().filter(|n| n.language == lang).count() >= 10,
            "gold set needs >= 10 {} notes",
            lang.as_str()
        );
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

// ─── Settings ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum System {
    #[serde(rename = "none")]
    NoMemory,
    A,
    B,
    C,
    D,
    E,
}

impl System {
    fn from_label(s: &str) -> Option<Self> {
        match s.trim() {
            "none" | "NONE" | "None" => Some(System::NoMemory),
            "A" | "a" => Some(System::A),
            "B" | "b" => Some(System::B),
            "C" | "c" => Some(System::C),
            "D" | "d" => Some(System::D),
            "E" | "e" => Some(System::E),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            System::NoMemory => "none",
            System::A => "A",
            System::B => "B",
            System::C => "C",
            System::D => "D",
            System::E => "E",
        }
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
    allow_download: bool,
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
        let system = env_nonempty("OPENALPACA_MEMORY_EVAL_SYSTEM")
            .and_then(|s| System::from_label(&s))
            .unwrap_or_else(|| {
                panic!(
                    "memory_eval: OPENALPACA_MEMORY_EVAL_SYSTEM must be one of none | A | B | C | D | E"
                )
            });
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

        // The scratch store: the only place the embedder cache may go. It has
        // to be named explicitly, absolute, and must not be the real one.
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
            allow_download: env_flag("OPENALPACA_MEMORY_EVAL_ALLOW_DOWNLOAD"),
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
    owner_id: &'static str,
    llm_toml: String,
    llm_toml_sha256: String,
    reader_default_model: String,
    /// The main loop sets no temperature (`runner/agentic_loop/backend.rs`),
    /// so the reader runs at the provider's default; recorded as `null`.
    reader_temperature: Option<f32>,
    reader_max_rounds: usize,
    reader_max_cost_usd: f64,
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
    per_turn_fts_active: bool,
    memory_search_tool_offered: bool,
    unattended: bool,
    extraction_disabled: bool,
    workflow_cap: usize,
    /// The scratch store (`OPENALPACA_HOME_STORE`) — where B's E5 cache lives.
    home_store: String,
    scratch_dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct EmbedderInfo {
    provider: String,
    model: Option<String>,
    dimensions: u32,
    cache_dir: String,
    cache_was_empty: bool,
    download_authorized: bool,
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
    /// What the per-turn FTS path found for this question, measured with the
    /// same repository call `MemorySource` makes (observability only).
    per_turn_fts_hits: usize,
}

#[derive(Debug, Serialize)]
struct Report {
    schema: u32,
    /// `running` while rows are still being added, then `complete`; or
    /// `unavailable` when the system cannot be measured by this build.
    status: &'static str,
    system: System,
    unavailable_reason: Option<String>,
    run_started_at: String,
    run_finished_at: Option<String>,
    machine: Machine,
    build: Build,
    conditions: Option<Conditions>,
    embedder: Option<EmbedderInfo>,
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
    let mut features = vec!["live-eval"];
    if cfg!(feature = "live-eval-e5") {
        features.push("live-eval-e5");
    }
    Build {
        git_head: command_stdout("git", &["-C", &root_str, "rev-parse", "HEAD"]),
        lockfile_sha256: sha256_hex(&lockfile),
        crate_version: env!("CARGO_PKG_VERSION"),
        features,
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
    }
}

// ─── The old engine's flat note text ────────────────────────────────────────

/// What a gold note becomes on the way into today's `memory_store`, which
/// takes one `content` string: the title as a heading, the body as written,
/// then the aliases, tags and validity bounds as labelled lines — every field
/// the new service indexes, so B is not handicapped by the flattening. Slice 4
/// passes the fields natively once the tool takes them (design §7a).
fn flatten_for_store(note: &GoldNote) -> String {
    let mut text = format!("# {}\n{}", note.title.trim(), note.body.trim());
    if !note.aliases.is_empty() {
        text.push_str("\nAliases: ");
        text.push_str(&note.aliases.join(", "));
    }
    if !note.tags.is_empty() {
        text.push_str("\nTags: ");
        text.push_str(&note.tags.join(", "));
    }
    if let Some(from) = &note.valid_from {
        text.push_str("\nValid from: ");
        text.push_str(from);
    }
    if let Some(to) = &note.valid_to {
        text.push_str("\nValid to: ");
        text.push_str(to);
    }
    text
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
/// `OPENALPACA_HOME_STORE`, and `--features live-eval` (`live-eval-e5` for B).
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
    use openalpaca_core::memory::scope_context::MemoryScopeContext;
    use openalpaca_core::middleware::prompt::SystemPersona;
    use openalpaca_core::orchestrator::{Orchestrator, skill_catalog, skill_router};
    use openalpaca_core::runner::LoopConfig;
    use openalpaca_core::security::gate::SecurityGate;
    use openalpaca_core::security::policy::{Principal, Scope};
    use openalpaca_core::security::sandbox::SandboxManager;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_core::tools::builtins::{MemoryStoreTool, builtin_tools};
    use openalpaca_core::tools::registry::{BuiltInTool, ToolContext};
    use openalpaca_storage::repository::MemoryRepository;
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

    // C, D and E are the new service (slice 3) and cannot be measured by
    // this build: an explicit `unavailable` report, no model call.
    if matches!(settings.system, System::C | System::D | System::E) {
        report.status = "unavailable";
        report.unavailable_reason = Some(format!(
            "system {} arrives with slice 3 (the new MemoryService); this build has no such system",
            settings.system.as_str()
        ));
        report.run_finished_at = Some(now_rfc3339());
        write_report(&settings.report_path, &report);
        eprintln!(
            "memory_eval: system {} is unavailable in this build — report written to {}",
            settings.system.as_str(),
            settings.report_path.display()
        );
        return;
    }

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
    eprintln!(
        "memory_eval: system {} — router loaded from {} (default model: {})",
        settings.system.as_str(),
        settings.llm_toml.display(),
        router.default_model()
    );

    // ── Embedder (B only) ───────────────────────────────────────────────
    let embedder: Option<Arc<dyn openalpaca_llm::Embedder>> = if settings.system == System::B {
        let llm_config = openalpaca_llm::read_config(&settings.llm_toml).unwrap_or_else(|e| {
            panic!(
                "memory_eval: cannot re-read {}: {e}",
                settings.llm_toml.display()
            )
        });
        let emb_cfg = llm_config
            .embeddings
            .clone()
            .unwrap_or(openalpaca_llm::EmbeddingsConfig {
                enabled: true,
                provider: "local".to_string(),
                model: None,
                dimensions: None,
            });
        let provider_cfg = llm_config
            .providers
            .as_ref()
            .and_then(|p| p.get(&emb_cfg.provider).cloned());
        // G12: the cache lives in the scratch store, and a download into it
        // is a one-time, explicitly authorized step — never a side effect.
        let cache_dir = openalpaca_storage::store::embedding_cache_dir()
            .unwrap_or_else(|e| panic!("memory_eval: cannot create the embedding cache: {e}"));
        let cache_was_empty = std::fs::read_dir(&cache_dir)
            .map(|d| d.count() == 0)
            .unwrap_or(true);
        let info = EmbedderInfo {
            provider: emb_cfg.provider.clone(),
            model: emb_cfg.model.clone(),
            dimensions: emb_cfg.dimensions.unwrap_or(768),
            cache_dir: cache_dir.to_string_lossy().into_owned(),
            cache_was_empty,
            download_authorized: settings.allow_download,
        };
        let built: Result<Arc<dyn openalpaca_llm::Embedder>, String> = if emb_cfg.provider
            == "local"
            && cache_was_empty
            && !settings.allow_download
        {
            Err(format!(
                "the E5 model is not in {} and OPENALPACA_MEMORY_EVAL_ALLOW_DOWNLOAD=1 was not set \
                     (G12: the download must be authorized explicitly)",
                cache_dir.display()
            ))
        } else {
            let cfg = emb_cfg.clone();
            let dir = cache_dir.clone();
            // Loading (or downloading) the model is blocking work.
            tokio::task::spawn_blocking(move || {
                    openalpaca_llm::build_embedder(&cfg, None, provider_cfg.as_ref(), Some(&dir))
                })
                .await
                .expect("embedder builder task")
                .map_err(|e| {
                    format!(
                        "cannot build the {} embedder: {e} (a `local` provider needs `--features live-eval-e5`)",
                        emb_cfg.provider
                    )
                })
        };
        report.embedder = Some(info);
        match built {
            Ok(embedder) => Some(embedder),
            Err(reason) => {
                report.status = "unavailable";
                report.unavailable_reason = Some(reason.clone());
                report.run_finished_at = Some(now_rfc3339());
                write_report(&settings.report_path, &report);
                eprintln!(
                    "memory_eval: baseline B is unavailable — {reason}; report written to {} \
                     (plan §5: the gate is then against A at −10 pp)",
                    settings.report_path.display()
                );
                return;
            }
        }
    } else {
        None
    };

    // ── Scratch DB + a scratch project (so the turn's workspace is never
    //    this checkout, whatever the test process's CWD is) ──────────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let scratch_project = tmp.path().join("project");
    std::fs::create_dir_all(scratch_project.join(".git")).expect("scratch project marker");
    let scratch_project = scratch_project
        .canonicalize()
        .expect("canonical scratch project");
    let db = openalpaca_storage::Database::open(&tmp.path().join("eval.db")).expect("open db");
    let scope_ctx = MemoryScopeContext::for_request(scratch_project.to_str());

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
    let workflow_cap = cfg.orchestrator.routing.max_workflows_per_lane;
    let daemon_config = Arc::new(ArcSwap::from_pointee(cfg));

    // B: the `memory_search` tool, exactly as the daemon registers it
    // (`builtin_tools`), joins the global registry; A and `none` offer no
    // search tool, so the per-turn path is the only recall.
    let memory_search_offered = settings.system == System::B;
    if memory_search_offered {
        for tool in builtin_tools(
            Some(db.clone()),
            embedder.clone(),
            Some(daemon_config.clone()),
            None,
            Some(scratch_project.clone()),
        ) {
            if tool.definition.name == "memory_search" {
                registry.register(tool).expect("register memory_search");
            }
        }
    }

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
        embedder.clone(),
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        daemon_config,
    );

    // ── Seed the gold notes through the store tool path ─────────────────
    let (mut seeded, mut duplicate, mut failed) = (0usize, 0usize, 0usize);
    if settings.system != System::NoMemory {
        let store = MemoryStoreTool::new(Some(db.clone()), embedder.clone());
        let ctx = ToolContext {
            owner_id: Some(OWNER_ID.to_string()),
            source: Some("cli".to_string()),
            ..Default::default()
        };
        for note in &notes {
            let content = flatten_for_store(note);
            match store
                .execute_with_context(&serde_json::json!({ "content": content }), &ctx)
                .await
            {
                Ok(reply) if reply.starts_with("Got it") => seeded += 1,
                Ok(reply) if reply.starts_with("I already have") => {
                    duplicate += 1;
                    eprintln!(
                        "memory_eval: note {} is an exact duplicate — not seeded twice",
                        note.id
                    );
                }
                Ok(other) => {
                    failed += 1;
                    eprintln!(
                        "memory_eval: note {}: unexpected store reply: {other}",
                        note.id
                    );
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("memory_eval: note {}: store failed: {e}", note.id);
                }
            }
        }
        eprintln!(
            "memory_eval: seeded {seeded} of {} notes ({duplicate} duplicate, {failed} failed)",
            notes.len()
        );
    }

    report.conditions = Some(Conditions {
        owner_id: OWNER_ID,
        llm_toml: settings.llm_toml.to_string_lossy().into_owned(),
        llm_toml_sha256,
        reader_default_model: router.default_model(),
        reader_temperature: None,
        reader_max_rounds,
        reader_max_cost_usd: reader_max_cost,
        judge_model_requested: settings.judge_model.clone(),
        judge_temperature: JUDGE_TEMPERATURE,
        judge_max_tokens: JUDGE_MAX_TOKENS,
        judge_prompt_path: judge_prompt_path.to_string_lossy().into_owned(),
        judge_prompt_sha256,
        notes_path: notes_path.to_string_lossy().into_owned(),
        notes_sha256,
        notes_total: notes.len(),
        notes_seeded: seeded,
        notes_duplicate: duplicate,
        notes_failed: failed,
        questions_path: settings.questions_path.to_string_lossy().into_owned(),
        questions_sha256,
        questions_total: all_questions.len(),
        questions_selected: questions.len(),
        limit: settings.limit,
        category_filter: settings.category,
        // The orchestrator builds `MemorySource` whenever it has a database
        // (`orchestrator/mod.rs`, `Orchestrator::new`); nothing switches it
        // off, so B runs with it live too — `per_turn_fts_hits` says how often
        // it found anything.
        per_turn_fts_active: true,
        memory_search_tool_offered: memory_search_offered,
        unattended: true,
        extraction_disabled: true,
        workflow_cap,
        home_store: settings.home_store.to_string_lossy().into_owned(),
        scratch_dir: settings
            .keep_scratch
            .then(|| tmp.path().to_string_lossy().into_owned()),
    });
    write_report(&settings.report_path, &report);

    // ── Ask every question through the front door ───────────────────────
    let repo = MemoryRepository::new(&db);
    let cascade = scope_ctx.cascade_scopes();
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
                        global_id: OWNER_ID.to_string(),
                    },
                    Scope::Global,
                    lane_key,
                )
            })
            .await;
        let latency_ms = t0.elapsed().as_millis() as u64;

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
                }) if agent_id == "orchestrator" => obs.tools.push(tool_name),
                Ok(SystemEvent::LlmCallCompleted {
                    agent_id,
                    model,
                    input_tokens,
                    output_tokens,
                    cost_usd,
                    ..
                }) if agent_id == "orchestrator" => {
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

        // The per-turn path's own reading of this question — the same
        // repository call `MemorySource::resolve` makes (FTS only, limit 10,
        // the turn's cascade) — so a B row can say whether the per-turn path
        // contributed beside the tool.
        let per_turn_fts_hits = repo
            .search_hybrid_cascade(OWNER_ID, &q.question, None, 10, None, &cascade)
            .map(|v| v.len())
            .unwrap_or(0);

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
            "[{}] {:<10} {:<24} {:<5} latency={:<6}ms llm_calls={} tokens={}/{} tools={:?} fts_hits={}{}",
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
         latency p50={}ms p95={}ms; memory_search calls={}; rows with per-turn FTS hits={}; wall {}s",
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
