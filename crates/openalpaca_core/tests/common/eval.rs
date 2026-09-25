//! The memory evaluation's data files and their types (design 09 §7),
//! shared by the live harness (`tests/memory_eval.rs`, `--features
//! live-eval`) and the corpus validation (`tests/memory_eval_corpus.rs`,
//! default CI). Each test crate includes this file with
//! `#[path = "common/eval.rs"] mod eval;` and uses what it needs.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ─── Question categories and languages ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
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
    pub const ALL: [Category; 8] = [
        Category::ExactRecall,
        Category::Paraphrase,
        Category::CrossLanguage,
        Category::ChangedFactsTime,
        Category::ProjectProcedures,
        Category::RejectedFailedActions,
        Category::Abstention,
        Category::SourceConflicts,
    ];

    pub fn as_str(self) -> &'static str {
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
pub enum Language {
    En,
    Zh,
    Mixed,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::En, Language::Zh, Language::Mixed];

    pub fn as_str(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Zh => "zh",
            Language::Mixed => "mixed",
        }
    }
}

// ─── Gold notes and questions ───────────────────────────────────────────────

/// One gold note, in the shape of design §4's frontmatter plus the two
/// eval-only fields `project` and `language`.
#[derive(Debug, Clone, Deserialize)]
pub struct GoldNote {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub scope: String,
    #[serde(default)]
    pub project: Option<String>,
    pub language: Language,
    #[serde(default)]
    pub fact_key: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub body: String,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub id: String,
    pub category: Category,
    pub language: Language,
    pub question: String,
    pub gold_answer: String,
    #[serde(default)]
    pub relies_on: Vec<String>,
    #[serde(default)]
    pub expect_abstain: bool,
}

/// The gold notes some non-abstention question relies on: the notes the
/// learning corpus must state (design 09 §7.1, *Coverage*).
pub fn relied_on(questions: &[Question]) -> BTreeSet<String> {
    questions
        .iter()
        .filter(|q| !q.expect_abstain)
        .flat_map(|q| q.relies_on.iter().cloned())
        .collect()
}

/// A changed fact in the gold set (design 09 §7.1, *15 changed facts*): two
/// notes of one partition sharing a `fact_key`, the older stated first.
/// `linked` when the newer one carries a `supersedes` link to the older (the
/// four `supersedes` pairs); the other four are same-key conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangePair {
    pub fact_key: String,
    pub old: String,
    pub new: String,
    pub linked: bool,
}

/// Every change pair of the gold set: notes grouped by scope, project and
/// `fact_key`, each group's notes in `created_at` order, and each note paired
/// with the one before it. `Err` names a `created_at` that is not RFC 3339.
pub fn change_pairs(notes: &[GoldNote]) -> Result<Vec<ChangePair>, String> {
    let mut groups: BTreeMap<(String, Option<String>, String), Vec<&GoldNote>> = BTreeMap::new();
    for n in notes {
        if let Some(key) = &n.fact_key {
            groups
                .entry((n.scope.clone(), n.project.clone(), key.clone()))
                .or_default()
                .push(n);
        }
    }
    let mut pairs = Vec::new();
    for ((_, _, key), mut group) in groups {
        if group.len() < 2 {
            continue;
        }
        let mut dated = Vec::with_capacity(group.len());
        for n in group.drain(..) {
            let at = parse_instant("created_at", &Some(n.created_at.clone()))
                .map_err(|e| format!("note {}: {e}", n.id))?;
            dated.push((at, n));
        }
        dated.sort_by_key(|(at, _)| *at);
        for w in dated.windows(2) {
            let (old, new) = (w[0].1, w[1].1);
            pairs.push(ChangePair {
                fact_key: key.clone(),
                old: old.id.clone(),
                new: new.id.clone(),
                linked: new.supersedes.contains(&old.id),
            });
        }
    }
    Ok(pairs)
}

// ─── The learning corpus (design 09 §7.1) ───────────────────────────────────

/// `scripts/eval/memory/conversations-learning.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub schema: u32,
    pub sessions: Vec<CorpusSession>,
    /// The sessions L-S feeds after seeding the change pairs' old notes as
    /// the user's own (design 09 §7.2).
    pub suggestion_sessions: Vec<String>,
}

/// The project a corpus session runs in. Both run with the harness's one
/// scratch project as their workspace (design 09 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Project {
    Lantern,
    Moss,
}

impl Project {
    pub fn as_str(self) -> &'static str {
        match self {
            Project::Lantern => "lantern",
            Project::Moss => "moss",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusSession {
    pub id: String,
    /// `null` for a session with no project.
    pub workspace: Option<Project>,
    pub language: Language,
    /// `YYYY-MM-DD`: when the conversation happens in the story. Dated facts
    /// state their dates in the text, since a learning run stamps the wall
    /// clock.
    pub narrative_date: String,
    pub turns: Vec<CorpusTurn>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    /// States a gold fact outright.
    Explicit,
    /// States it in passing, as the premise of something else.
    Indirect,
    /// States the new value of a change pair.
    Change,
    /// States a wrong value and corrects it to the gold one in the same turn.
    Correction,
    /// Repeats an earlier session's fact, same value, other words.
    Restate,
    /// Turns down something the assistant proposed.
    Rejection,
    /// No durable fact: a question, chit-chat or a request.
    Noise,
}

impl TurnMode {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnMode::Explicit => "explicit",
            TurnMode::Indirect => "indirect",
            TurnMode::Change => "change",
            TurnMode::Correction => "correction",
            TurnMode::Restate => "restate",
            TurnMode::Rejection => "rejection",
            TurnMode::Noise => "noise",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusTurn {
    /// What the user types.
    pub user: String,
    /// The gold ids the turn states; empty for noise.
    pub states: Vec<String>,
    pub mode: TurnMode,
}

/// Where a turn sits in the corpus: its session's index and its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TurnAt {
    pub session: usize,
    pub turn: usize,
}

impl Corpus {
    /// Every turn with its position, in corpus order.
    pub fn turns(&self) -> impl Iterator<Item = (TurnAt, &CorpusSession, &CorpusTurn)> {
        self.sessions.iter().enumerate().flat_map(|(si, s)| {
            s.turns.iter().enumerate().map(move |(ti, t)| {
                (
                    TurnAt {
                        session: si,
                        turn: ti,
                    },
                    s,
                    t,
                )
            })
        })
    }

    /// The turns that state a gold id.
    pub fn turns_stating<'a>(
        &'a self,
        id: &'a str,
    ) -> impl Iterator<Item = (TurnAt, &'a CorpusSession, &'a CorpusTurn)> + 'a {
        self.turns()
            .filter(move |(_, _, t)| t.states.iter().any(|s| s == id))
    }
}

// ─── Files ──────────────────────────────────────────────────────────────────

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn eval_dir() -> PathBuf {
    repo_root().join("scripts/eval/memory")
}

pub fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Parse a JSON data file and return it with the SHA-256 of its exact bytes.
pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> (T, String) {
    let bytes = read_bytes(path);
    let parsed = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
    (parsed, sha256_hex(&bytes))
}

pub fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.hyphenated().to_string() == s)
}

pub fn parse_instant(
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
