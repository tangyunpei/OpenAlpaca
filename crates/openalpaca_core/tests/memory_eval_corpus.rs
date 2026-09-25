//! The learning evaluation's conversation corpus (design 09 §7.1),
//! validated in default CI: no model, no feature, no network.
//!
//! `scripts/eval/memory/conversations-learning.json` is written from the
//! gold notes, never from the held-out questions; these tests hold it to
//! what §7.1 asks of it — every relied-on gold note stated, enough of them
//! only in passing, the 8 gold change pairs stated old-then-new (two `user.*`
//! pairs across a project boundary), 7 corrections, restatements, rejections
//! and noise, the questions' language mix, its size, and **no leakage**: no
//! held-out question shares an 8-word shingle, or a 12-character CJK
//! shingle, with any turn.

#[path = "common/eval.rs"]
mod eval;

use eval::{
    ChangePair, Corpus, CorpusSession, CorpusTurn, GoldNote, Language, Question, TurnAt, TurnMode,
    change_pairs, eval_dir, is_cjk, load_json, relied_on,
};
use openalpaca_storage::memory_index::query::normalize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Design §7.1: 40–60 sessions and 250–350 user turns.
const SESSIONS: std::ops::RangeInclusive<usize> = 40..=60;
const TURNS: std::ops::RangeInclusive<usize> = 250..=350;
/// "At least 20 of them are stated only indirectly."
const MIN_ONLY_INDIRECT: usize = 20;
/// "7 in-conversation corrections."
const MIN_CORRECTIONS: usize = 7;
/// "At least 10 restatements … at least 4 of them about the 8 change pairs'
/// old notes."
const MIN_RESTATEMENTS: usize = 10;
const MIN_OLD_NOTE_RESTATEMENTS: usize = 4;
/// "At least 10 rejections."
const MIN_REJECTIONS: usize = 10;
/// "At least 30 % noise turns."
const MIN_NOISE_SHARE: f64 = 0.30;
/// "At least 2 of the `user.*` pairs state the new value in a project
/// session while the old value was stated in a session with no project."
const MIN_CROSS_PROJECT_USER_PAIRS: usize = 2;
/// Language shares within ±5 percentage points of the questions'.
const LANGUAGE_TOLERANCE: f64 = 0.05;
/// The leakage shingles: 8 words, and 12 CJK characters.
const WORD_SHINGLE: usize = 8;
const CJK_SHINGLE: usize = 12;

struct Fixture {
    corpus: Corpus,
    notes: Vec<GoldNote>,
    heldout: Vec<Question>,
}

fn fixture() -> Fixture {
    let dir = eval_dir();
    let (corpus, _) = load_json::<Corpus>(&dir.join("conversations-learning.json"));
    let (notes, _) = load_json::<Vec<GoldNote>>(&dir.join("notes-gold.json"));
    let (heldout, _) = load_json::<Vec<Question>>(&dir.join("questions-heldout.json"));
    Fixture {
        corpus,
        notes,
        heldout,
    }
}

// ─── Leakage (design 09 §7.1) ───────────────────────────────────────────────

/// A text's words for the 8-word check: NFKC-normalised and case-folded
/// (the memory index's own `normalize`, which also drops Latin diacritics),
/// split on whitespace, punctuation trimmed off each word's ends.
fn words(text: &str) -> Vec<String> {
    normalize(text)
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// A text's CJK characters for the 12-character check, normalised as the
/// words are and with everything else dropped.
fn cjk_chars(text: &str) -> Vec<char> {
    normalize(text).chars().filter(|&c| is_cjk(c)).collect()
}

fn shingles<T: Clone + Ord>(seq: &[T], n: usize) -> BTreeSet<Vec<T>> {
    if seq.len() < n {
        return BTreeSet::new();
    }
    seq.windows(n).map(<[T]>::to_vec).collect()
}

/// One shingle a question shares with the corpus.
#[derive(Debug, PartialEq, Eq)]
enum Leak {
    Words { question: String, shingle: String },
    Cjk { question: String, shingle: String },
}

/// Every shingle any question shares with any turn. Every question gets both
/// checks: an English one has no CJK run to share, a Chinese one no 8-word
/// run, and a mixed one can have either.
fn leaks<'a>(
    questions: impl IntoIterator<Item = (&'a str, &'a str)>,
    turns: impl IntoIterator<Item = &'a str>,
) -> Vec<Leak> {
    let mut turn_words = BTreeSet::new();
    let mut turn_cjk = BTreeSet::new();
    for t in turns {
        turn_words.extend(shingles(&words(t), WORD_SHINGLE));
        turn_cjk.extend(shingles(&cjk_chars(t), CJK_SHINGLE));
    }
    let mut out = Vec::new();
    for (id, q) in questions {
        for s in shingles(&words(q), WORD_SHINGLE) {
            if turn_words.contains(&s) {
                out.push(Leak::Words {
                    question: id.to_string(),
                    shingle: s.join(" "),
                });
            }
        }
        for s in shingles(&cjk_chars(q), CJK_SHINGLE) {
            if turn_cjk.contains(&s) {
                out.push(Leak::Cjk {
                    question: id.to_string(),
                    shingle: s.into_iter().collect(),
                });
            }
        }
    }
    out
}

/// The check itself, on planted text: a Chinese question sharing a
/// 12-character run with a turn — through different punctuation, spacing,
/// Latin words and full-width forms — is caught by the CJK check although no
/// whitespace word matches; 11 shared characters are not; and an English
/// question sharing 8 words, in another case and punctuation, is caught by
/// the word check while 7 are not.
#[test]
fn the_leakage_check_catches_a_planted_shingle() {
    // Twelve Han characters, contiguous in the question and split in the
    // turn by a Latin word, spaces and full-width punctuation.
    let question = "我每周五下午四点做周回顾？";
    let turn = "对，我每周五 (Friday) 下午四点， 做 周回顾。";
    assert_eq!(cjk_chars(question).len(), 12);
    assert_eq!(
        leaks([("q-planted-zh", question)], [turn]),
        vec![Leak::Cjk {
            question: "q-planted-zh".into(),
            shingle: "我每周五下午四点做周回顾".into(),
        }],
        "the word check alone cannot see it: {:?}",
        shingles(&words(question), WORD_SHINGLE)
    );
    // Eleven shared characters: no 12-character shingle in common.
    assert!(leaks([("q-11", "我每周五下午四点做周回吗")], [turn]).is_empty());

    let question = "Which Port Does Lantern Listen On Locally These Days?";
    let turn = "fyi: which port does lantern listen on locally these?";
    assert_eq!(
        leaks([("q-planted-en", question)], [turn]),
        vec![Leak::Words {
            question: "q-planted-en".into(),
            shingle: "which port does lantern listen on locally these".into(),
        }]
    );
    assert_eq!(
        leaks(
            [("q-planted-en", question)],
            ["which port does lantern listen on locally"]
        ),
        vec![],
        "seven words are not a shingle"
    );
    // Full-width Latin folds to ASCII before the words are compared.
    assert_eq!(
        leaks(
            [(
                "q-fw",
                "ｗｈｉｃｈ port does lantern listen on locally these"
            )],
            [turn]
        )
        .len(),
        1
    );
}

#[test]
fn no_heldout_question_leaks_into_the_corpus() {
    let f = fixture();
    let found = leaks(
        f.heldout
            .iter()
            .map(|q| (q.id.as_str(), q.question.as_str())),
        f.corpus.turns().map(|(_, _, t)| t.user.as_str()),
    );
    assert!(
        found.is_empty(),
        "{} shingle(s) shared with held-out questions: {found:#?}",
        found.len()
    );
}

// ─── Shape ──────────────────────────────────────────────────────────────────

#[test]
fn the_corpus_parses_and_every_gold_id_is_real() {
    let f = fixture();
    assert_eq!(f.corpus.schema, 1);
    let gold: BTreeSet<&str> = f.notes.iter().map(|n| n.id.as_str()).collect();
    let mut ids = BTreeSet::new();
    let mut previous_date = String::new();
    for s in &f.corpus.sessions {
        assert!(
            s.id.starts_with("cs-") && s.id[3..].bytes().all(|b| b.is_ascii_digit()),
            "session id {:?} is not cs-<digits>",
            s.id
        );
        assert!(ids.insert(s.id.as_str()), "duplicate session id {}", s.id);
        chrono::NaiveDate::parse_from_str(&s.narrative_date, "%Y-%m-%d")
            .unwrap_or_else(|e| panic!("{}: narrative_date {:?}: {e}", s.id, s.narrative_date));
        assert!(
            s.narrative_date >= previous_date,
            "{}: sessions are in story order ({} after {previous_date})",
            s.id,
            s.narrative_date
        );
        previous_date = s.narrative_date.clone();
        assert!(!s.turns.is_empty(), "{}: a session with no turn", s.id);
        for t in &s.turns {
            assert!(!t.user.trim().is_empty(), "{}: an empty turn", s.id);
            assert!(
                !t.user.trim_start().starts_with('/'),
                "{}: a slash turn is never learned: {:?}",
                s.id,
                t.user
            );
            assert_eq!(
                t.states.is_empty(),
                t.mode == TurnMode::Noise,
                "{}: noise states nothing, and every other turn states a gold note: {:?}",
                s.id,
                t.user
            );
            for id in &t.states {
                assert!(
                    gold.contains(id.as_str()),
                    "{}: states {id}, which is not a gold note",
                    s.id
                );
            }
        }
    }
    let mut seen = BTreeSet::new();
    for id in &f.corpus.suggestion_sessions {
        assert!(
            ids.contains(id.as_str()),
            "suggestion session {id} does not exist"
        );
        assert!(seen.insert(id), "suggestion session {id} is listed twice");
    }
}

#[test]
fn the_session_and_turn_counts_are_in_range() {
    let f = fixture();
    let sessions = f.corpus.sessions.len();
    let turns = f.corpus.turns().count();
    assert!(
        SESSIONS.contains(&sessions),
        "{sessions} sessions, not {SESSIONS:?}"
    );
    assert!(TURNS.contains(&turns), "{turns} user turns, not {TURNS:?}");
}

#[test]
fn language_shares_are_within_five_points_of_the_questions() {
    let f = fixture();
    let share = |count: usize, of: usize| count as f64 / of as f64;
    for lang in Language::ALL {
        let want = share(
            f.heldout.iter().filter(|q| q.language == lang).count(),
            f.heldout.len(),
        );
        let got = share(
            f.corpus
                .sessions
                .iter()
                .filter(|s| s.language == lang)
                .count(),
            f.corpus.sessions.len(),
        );
        assert!(
            (got - want).abs() <= LANGUAGE_TOLERANCE,
            "{}: {:.1} % of sessions against {:.1} % of the questions",
            lang.as_str(),
            got * 100.0,
            want * 100.0
        );
    }
}

/// A project's notes are stated in that project's sessions, so they learn
/// into the project partition (ruling LR1); global notes may come up
/// anywhere.
#[test]
fn project_notes_are_stated_in_their_projects_sessions() {
    let f = fixture();
    let by_id: HashMap<&str, &GoldNote> = f.notes.iter().map(|n| (n.id.as_str(), n)).collect();
    for (_, s, t) in f.corpus.turns() {
        for id in &t.states {
            let note = by_id[id.as_str()];
            if let Some(project) = &note.project {
                assert_eq!(
                    s.workspace.map(|w| w.as_str()),
                    Some(project.as_str()),
                    "{}: {} ({}) is stated outside its project",
                    s.id,
                    note.title,
                    note.fact_key.as_deref().unwrap_or("")
                );
            }
        }
    }
}

// ─── Coverage ───────────────────────────────────────────────────────────────

#[test]
fn every_relied_on_gold_note_is_stated() {
    let f = fixture();
    let stated: BTreeSet<&str> = f
        .corpus
        .turns()
        .flat_map(|(_, _, t)| t.states.iter().map(String::as_str))
        .collect();
    let missing: Vec<String> = relied_on(&f.heldout)
        .into_iter()
        .filter(|id| !stated.contains(id.as_str()))
        .map(|id| {
            let n = f.notes.iter().find(|n| n.id == id).expect("a gold id");
            format!("{} ({})", n.title, n.fact_key.as_deref().unwrap_or(""))
        })
        .collect();
    assert!(missing.is_empty(), "never stated: {missing:#?}");
}

#[test]
fn enough_notes_are_stated_only_indirectly() {
    let f = fixture();
    let mut modes: BTreeMap<&str, BTreeSet<TurnMode>> = BTreeMap::new();
    for (_, _, t) in f.corpus.turns() {
        for id in &t.states {
            modes.entry(id.as_str()).or_default().insert(t.mode);
        }
    }
    let only_indirect = modes
        .values()
        .filter(|m| m.iter().all(|&mode| mode == TurnMode::Indirect))
        .count();
    assert!(
        only_indirect >= MIN_ONLY_INDIRECT,
        "{only_indirect} notes are stated only indirectly, not {MIN_ONLY_INDIRECT}"
    );
}

// ─── Changes, corrections, restatements, rejections, noise ──────────────────

fn pairs(f: &Fixture) -> Vec<ChangePair> {
    let pairs = change_pairs(&f.notes).expect("the gold notes' created_at parse");
    assert_eq!(pairs.len(), 8, "the gold set's 8 change pairs: {pairs:#?}");
    assert_eq!(
        pairs.iter().filter(|p| p.linked).count(),
        4,
        "4 of them are supersedes pairs"
    );
    pairs
}

fn positions<'a>(
    corpus: &'a Corpus,
    id: &'a str,
) -> Vec<(TurnAt, &'a CorpusSession, &'a CorpusTurn)> {
    corpus.turns_stating(id).collect()
}

/// "The 8 gold pairs are stated old-then-new, in chronological sessions":
/// every turn stating the old note comes before every turn stating the new
/// one, and the new one is stated by a `change` turn. At least two `user.*`
/// pairs change across a project boundary.
#[test]
fn change_pairs_are_stated_old_then_new_and_two_user_pairs_cross_a_project() {
    let f = fixture();
    let mut cross_project = Vec::new();
    for p in pairs(&f) {
        let old = positions(&f.corpus, &p.old);
        let new = positions(&f.corpus, &p.new);
        assert!(
            !old.is_empty(),
            "{}: the old note is never stated",
            p.fact_key
        );
        assert!(
            !new.is_empty(),
            "{}: the new note is never stated",
            p.fact_key
        );
        let last_old = old.iter().map(|(at, _, _)| *at).max().expect("non-empty");
        let first_new = new.iter().map(|(at, _, _)| *at).min().expect("non-empty");
        assert!(
            last_old.session < first_new.session,
            "{}: the old note is stated in {} at or after the new one in {}",
            p.fact_key,
            f.corpus.sessions[last_old.session].id,
            f.corpus.sessions[first_new.session].id
        );
        let changes: Vec<_> = new
            .iter()
            .filter(|(_, _, t)| t.mode == TurnMode::Change)
            .collect();
        assert!(
            !changes.is_empty(),
            "{}: the new value is never stated as a change",
            p.fact_key
        );
        if p.fact_key.starts_with("user.")
            && old.iter().all(|(_, s, _)| s.workspace.is_none())
            && changes.iter().any(|(_, s, _)| s.workspace.is_some())
        {
            cross_project.push(p.fact_key.clone());
        }
    }
    assert!(
        cross_project.len() >= MIN_CROSS_PROJECT_USER_PAIRS,
        "user.* pairs changing across a project boundary: {cross_project:?}"
    );
}

/// A `change` turn states the new note of a change pair, never anything
/// else — the metric reads every one as a pair's second half.
#[test]
fn every_change_turn_states_a_pairs_new_note() {
    let f = fixture();
    let new: BTreeSet<String> = pairs(&f).into_iter().map(|p| p.new).collect();
    for (_, s, t) in f.corpus.turns() {
        if t.mode == TurnMode::Change {
            assert!(
                t.states.iter().all(|id| new.contains(id)),
                "{}: a change turn states something that is no pair's new note: {:?}",
                s.id,
                t.user
            );
        }
    }
}

#[test]
fn corrections_restatements_rejections_and_noise_are_there() {
    let f = fixture();
    let count = |mode: TurnMode| f.corpus.turns().filter(|(_, _, t)| t.mode == mode).count();
    assert!(
        count(TurnMode::Correction) >= MIN_CORRECTIONS,
        "{} corrections",
        count(TurnMode::Correction)
    );
    assert!(
        count(TurnMode::Rejection) >= MIN_REJECTIONS,
        "{} rejections",
        count(TurnMode::Rejection)
    );
    let turns = f.corpus.turns().count();
    let noise = count(TurnMode::Noise) as f64 / turns as f64;
    assert!(
        noise >= MIN_NOISE_SHARE,
        "{:.1} % noise turns",
        noise * 100.0
    );

    // A restatement repeats a fact an earlier session stated.
    let old: BTreeSet<String> = pairs(&f).into_iter().map(|p| p.old).collect();
    let mut restatements = 0;
    let mut about_old = 0;
    for (at, s, t) in f.corpus.turns() {
        if t.mode != TurnMode::Restate {
            continue;
        }
        restatements += 1;
        for id in &t.states {
            let earlier = f
                .corpus
                .turns_stating(id)
                .any(|(before, _, bt)| before.session < at.session && bt.mode != TurnMode::Restate);
            assert!(
                earlier,
                "{}: restates {id}, which no earlier session stated: {:?}",
                s.id, t.user
            );
        }
        if t.states.iter().any(|id| old.contains(id)) {
            about_old += 1;
        }
    }
    assert!(
        restatements >= MIN_RESTATEMENTS,
        "{restatements} restatements"
    );
    assert!(
        about_old >= MIN_OLD_NOTE_RESTATEMENTS,
        "{about_old} restatements of the change pairs' old notes"
    );
}

/// L-S (design 09 §7.2) seeds the pairs' old notes and feeds these sessions:
/// they carry every change turn and every restatement of an old note.
#[test]
fn suggestion_sessions_carry_the_changes_and_old_note_restatements() {
    let f = fixture();
    let listed: BTreeSet<&str> = f
        .corpus
        .suggestion_sessions
        .iter()
        .map(String::as_str)
        .collect();
    let order: Vec<&str> = f
        .corpus
        .sessions
        .iter()
        .map(|s| s.id.as_str())
        .filter(|id| listed.contains(id))
        .collect();
    assert_eq!(
        order,
        f.corpus
            .suggestion_sessions
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "suggestion sessions are listed in story order"
    );
    let old: BTreeSet<String> = pairs(&f).into_iter().map(|p| p.old).collect();
    let mut old_restatements = 0;
    for (_, s, t) in f.corpus.turns() {
        let about_old = t.mode == TurnMode::Restate && t.states.iter().any(|id| old.contains(id));
        if t.mode == TurnMode::Change || about_old {
            assert!(
                listed.contains(s.id.as_str()),
                "{} holds a {} turn but is not a suggestion session",
                s.id,
                t.mode.as_str()
            );
        }
        old_restatements += usize::from(about_old);
    }
    assert!(old_restatements >= MIN_OLD_NOTE_RESTATEMENTS);
}
