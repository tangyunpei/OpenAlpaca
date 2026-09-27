//! Query normalization and the literal query builder (design §5 steps 3–4;
//! the routing is design 09 §6.5.2, LQ17, index schemas 4 and 5).
//!
//! A query is normalized — NFKC, lowercase, Latin diacritics stripped — and
//! split into terms. Each term is one of:
//!
//! - a **word**: a run of alphanumeric characters, three or more of them;
//! - a **literal**: a token with punctuation inside it, an identifier or a
//!   path (`OPENALPACA_HOME_STORE`, `crate::module`, `p50/p95`) — kept whole
//!   as a phrase; its alphanumeric parts of three or more characters are the
//!   **fallback** the search routes as words only when the literal itself
//!   matches no eligible note (`title:tabs` finds a note about tabs;
//!   `OPENALPACA_HOME_STORE` does not drag in every note that says
//!   `openalpaca`);
//! - a **short** term: one or two characters — routed to the bounded
//!   substring pass when it is CJK, or when the query has no other term.
//!
//! Words and literals go to the three FTS tables as follows:
//!
//! - **`fts_word`** (`unicode61`) takes every one, as a phrase split at
//!   script boundaries ([`split_scripts`]) — and as a **prefix query**
//!   (`"prefer"*`) when its last token has three or more characters, unless
//!   the term carries an unsegmented character ([`is_unsegmented`]) or that
//!   token is a stop word ([`is_prefix_term`]). So `prefer` finds `prefers`
//!   and `preference`, `memory_file` finds `memory_files`, and `partner's`,
//!   `用户喜欢` and `and` match exactly. When that query fills its candidate
//!   cap, the terms it sent as prefixes are searched again as whole words,
//!   together in titles, aliases and tags and each alone in bodies
//!   ([`ParsedQuery::word_match_exact_at_cap`]).
//! - **`fts_tri_head`** (`trigram` over titles, aliases and tags) takes every
//!   one of three or more characters, as a substring.
//! - **`fts_tri`** (`trigram` over the bodies of chunks carrying an
//!   unsegmented character) takes only those of three or more characters
//!   that carry such a character.
//!
//! So nothing finds a term **inside** a word of a body written in a
//! space-separated script (`presso` in `espresso`); a title, an alias or a
//! tag still does, and a body in an unsegmented script — CJK, Thai,
//! Devanagari, Tamil and the rest [`is_unsegmented`] names — keeps
//! substring search.
//!
//! The MATCH strings are built from these terms only: every term is a
//! double-quoted FTS5 string with embedded quotes doubled, joined by `OR` —
//! the only syntax added is the index's own `*` after a closing quote and
//! the whole-word queries' column filters — so `AND`, `NOT`, `NEAR`,
//! `column:`, a caller's `*`, parentheses and an unbalanced quote are text,
//! never syntax: the F4 class of defect.
//!
//! English function words are dropped from routing when any other term
//! remains, so "the" does not recall every English note on the automatic path;
//! a query made only of them (`and`) still searches them literally. No
//! stemmer: a prefix query changes no token and no identifier — an exact word
//! is its own prefix.

use std::borrow::Cow;
use std::collections::HashSet;

use unicode_normalization::UnicodeNormalization;

use super::{IndexError, MAX_QUERY_CHARS, Result};

/// The upper bound on routed terms; a longer query keeps its first terms.
pub(crate) const MAX_TERMS: usize = 32;

/// Function words dropped from term routing when another term remains.
/// Never applied to exact title, alias or `fact_key` matching, which compare
/// the whole normalized query.
pub(crate) const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "of", "to", "in", "on", "at", "by", "for", "with",
    "from", "as", "is", "are", "was", "were", "be", "been", "being", "am", "do", "does", "did",
    "have", "has", "had", "i", "me", "my", "we", "our", "you", "your", "he", "she", "it", "its",
    "they", "them", "their", "this", "that", "these", "those", "what", "which", "who", "whom",
    "how", "when", "where", "why", "not", "no", "so", "if", "than", "then", "there", "here", "can",
    "could", "should", "would", "will", "shall", "may", "might", "must", "about", "into", "over",
    "under", "again", "also", "just", "very", "please",
];

/// An identifier or path kept whole, with the parts the search falls back
/// to when the whole matches nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Literal {
    pub text: String,
    /// Its alphanumeric runs of three or more characters, stop words
    /// excluded, in order.
    pub parts: Vec<String>,
}

/// The MATCH strings of the whole-word queries at the word table's cap
/// ([`ParsedQuery::word_match_exact_at_cap`], design 09 §6.5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WholeWordQueries {
    /// Every prefix term as a whole word in titles, aliases and tags: one
    /// ranked query with its own cap.
    pub header: String,
    /// Each prefix term alone as a whole word in bodies: one unranked query
    /// per term, which adds its chunks only when they fit within a cap.
    pub bodies: Vec<String>,
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ParsedQuery {
    /// The whole query normalized, with runs of whitespace collapsed to one
    /// space and the ends trimmed — what exact title and alias matching sees.
    pub normalized: String,
    /// Standalone words of three or more characters — both FTS tables.
    pub words: Vec<String>,
    /// Identifiers and paths — a phrase on both tables.
    pub literals: Vec<Literal>,
    /// One- and two-character terms — the substring pass.
    pub shorts: Vec<String>,
}

impl ParsedQuery {
    /// Every routed term's text, in query order.
    #[cfg(test)]
    pub(crate) fn terms(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.words.iter().map(String::as_str).collect();
        out.extend(self.literals.iter().map(|l| l.text.as_str()));
        out.extend(self.shorts.iter().map(String::as_str));
        out
    }

    /// `words` then the literals' texts: every term a search routes.
    fn routed<'a>(&'a self, words: &'a [String]) -> impl Iterator<Item = &'a str> {
        words
            .iter()
            .map(String::as_str)
            .chain(self.literals.iter().map(|l| l.text.as_str()))
    }

    /// The `unicode61` table's MATCH string over `words` and the literals:
    /// every term a quoted phrase split at script boundaries, a prefix when
    /// [`is_prefix_term`] says so, ORed. `None` when there is nothing to
    /// route.
    pub(crate) fn word_match(&self, words: &[String]) -> Option<String> {
        or_join(self.routed(words).map(|t| {
            let phrase = quote(&split_scripts(t));
            if is_prefix_term(t) {
                format!("{phrase}*")
            } else {
                phrase
            }
        }))
    }

    /// The whole-word queries the search runs when [`Self::word_match`]
    /// fills its candidate cap (design 09 §6.5.2), over the terms that went
    /// out as prefixes: all of them as whole words in titles, aliases and
    /// tags, and each one alone as a whole word in bodies. `None` when no
    /// term was a prefix — the first query was already this one.
    pub(crate) fn word_match_exact_at_cap(&self, words: &[String]) -> Option<WholeWordQueries> {
        let phrases: Vec<String> = self
            .routed(words)
            .filter(|t| is_prefix_term(t))
            .map(|t| quote(&split_scripts(t)))
            .collect();
        if phrases.is_empty() {
            return None;
        }
        Some(WholeWordQueries {
            header: format!("{{title aliases tags}} : ({})", phrases.join(" OR ")),
            bodies: phrases.iter().map(|p| format!("{{body}} : {p}")).collect(),
        })
    }

    /// The header trigram table's MATCH string: every term of three or more
    /// characters (a shorter phrase matches nothing there), as it is.
    pub(crate) fn head_trigram_match(&self, words: &[String]) -> Option<String> {
        or_join(
            self.routed(words)
                .filter(|t| t.chars().count() >= 3)
                .map(quote),
        )
    }

    /// The body trigram table's MATCH string: only the terms of three or
    /// more characters that carry an unsegmented character. A Latin term is
    /// matched in a body only as a word or a word's beginning, on the word
    /// table (design 09 §6.5.2).
    pub(crate) fn body_trigram_match(&self, words: &[String]) -> Option<String> {
        or_join(
            self.routed(words)
                .filter(|t| t.chars().count() >= 3 && t.chars().any(is_unsegmented))
                .map(quote),
        )
    }
}

fn or_join(parts: impl Iterator<Item = String>) -> Option<String> {
    let parts: Vec<String> = parts.collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" OR "))
    }
}

/// An FTS5 string literal: double-quoted, embedded quotes doubled.
fn quote(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// NFKC, lowercase, then the Latin combining diacritics (U+0300–U+036F)
/// removed. Fullwidth forms fold to ASCII (`ｑｕｉｃｋ` → `quick`), the
/// ideographic space to a space, `café` to `cafe`; CJK is untouched. Applied
/// to indexed text and to queries alike, so the FTS tokenizers see the same
/// bytes on both sides.
pub fn normalize(text: &str) -> String {
    let lowered: String = text.nfkc().flat_map(char::to_lowercase).collect();
    lowered
        .nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .nfc()
        .collect()
}

/// [`normalize`] with runs of whitespace collapsed to one space and the ends
/// trimmed — the form exact title and alias matching compares.
pub fn normalize_exact(text: &str) -> String {
    normalize(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The alphanumeric runs of a normalized string — the token rule `unicode61`
/// applies by default (letters and numbers are token characters, everything
/// else separates).
pub(crate) fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
}

/// A character of a script `unicode61` cannot split into words (design 09
/// §6.5.1). Two kinds:
///
/// - **written without spaces between words** — CJK (as [`is_cjk`]), Thai,
///   Lao, Myanmar, Khmer, Tibetan, Yi, Javanese, Balinese, Buginese and the
///   Tai scripts: `unicode61` reads a run of them as one token, so a word
///   inside the run is found only as a substring;
/// - **words built with vowel signs `unicode61` breaks at** — Devanagari and
///   the other Brahmic scripts, Thaana, the Philippine scripts, Limbu,
///   Lepcha, Cham, Meetei Mayek, Miao: its tables count a combining mark as
///   no token character, so `दोस्तों` is indexed as the consonants `द स त`,
///   fragments no query or ranker reads as the word (repair round 1 of
///   LQ17).
///
/// A chunk carrying one keeps its body in the trigram table, and a term
/// carrying one is matched exactly on the word table, never as a prefix.
/// The vowel points of Hebrew, Arabic, Syriac and Samaritan are left out:
/// their ordinary text is unpointed, and a test lists every mark
/// `unicode61` breaks a word at that this does not cover.
///
/// **The one predicate.** The upsert, the query builder, the ranker and the
/// file scan all call it, and it decides what `fts_tri` and the script split
/// hold: a change to its ranges changes the index's contents and must bump
/// [`super::SCHEMA_VERSION`].
pub(crate) fn is_unsegmented(c: char) -> bool {
    is_cjk(c)
        || matches!(
            c as u32,
            // Written without spaces between words.
            0x0E00..=0x0E7F       // Thai
            | 0x0E80..=0x0EFF     // Lao
            | 0x0F00..=0x0FFF     // Tibetan
            | 0x1000..=0x109F     // Myanmar
            | 0x1780..=0x17FF     // Khmer
            | 0x1950..=0x197F     // Tai Le
            | 0x1980..=0x19DF     // New Tai Lue
            | 0x19E0..=0x19FF     // Khmer symbols
            | 0x1A00..=0x1A1F     // Buginese
            | 0x1A20..=0x1AAF     // Tai Tham
            | 0x1B00..=0x1B7F     // Balinese
            | 0xA000..=0xA4CF     // Yi syllables and radicals
            | 0xA980..=0xA9DF     // Javanese
            | 0xA9E0..=0xA9FF     // Myanmar extended-B
            | 0xAA60..=0xAA7F     // Myanmar extended-A
            | 0xAA80..=0xAADF     // Tai Viet
            // Words built with vowel signs `unicode61` breaks at.
            | 0x0780..=0x07BF     // Thaana
            | 0x0900..=0x0DFF     // Devanagari, Bengali … Malayalam, Sinhala
            | 0x1700..=0x177F     // Tagalog, Hanunoo, Buhid, Tagbanwa
            | 0x1900..=0x194F     // Limbu
            | 0x1B80..=0x1BFF     // Sundanese, Batak
            | 0x1C00..=0x1C4F     // Lepcha
            | 0x1CD0..=0x1CFF     // Vedic extensions
            | 0xA800..=0xA82F     // Syloti Nagri
            | 0xA880..=0xA8FF     // Saurashtra, Devanagari extended
            | 0xA900..=0xA95F     // Kayah Li, Rejang
            | 0xAA00..=0xAA5F     // Cham
            | 0xAAE0..=0xAAFF     // Meetei Mayek extensions
            | 0xABC0..=0xABFF     // Meetei Mayek
            | 0x10A00..=0x10A5F   // Kharoshthi
            | 0x11000..=0x11FFF   // the Brahmic scripts of plane 1
            | 0x16F00..=0x16F9F   // Miao
        )
}

/// `a` then `b`, adjacent alphanumeric characters, sit on a script boundary:
/// exactly one of them is unsegmented.
fn script_boundary(a: char, b: char) -> bool {
    a.is_alphanumeric() && b.is_alphanumeric() && is_unsegmented(a) != is_unsegmented(b)
}

/// `text` with a space at every script boundary — the text the word table
/// indexes and the phrases it is queried with, so `和commit` holds the word
/// `commit` and `2026年7月` reads `2026 年 7 月` (design 09 §6.5.1). A run of
/// unsegmented characters stays one token. Borrowed when there is no
/// boundary.
pub(crate) fn split_scripts(text: &str) -> Cow<'_, str> {
    let mut chars = text.chars();
    let Some(mut prev) = chars.next() else {
        return Cow::Borrowed(text);
    };
    if !chars.any(|c| {
        let boundary = script_boundary(prev, c);
        prev = c;
        boundary
    }) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut last: Option<char> = None;
    for c in text.chars() {
        if last.is_some_and(|p| script_boundary(p, c)) {
            out.push(' ');
        }
        out.push(c);
        last = Some(c);
    }
    Cow::Owned(out)
}

/// The tokens of [`split_scripts`]`(text)`, as slices of `text`: the
/// alphanumeric runs, each cut again at its script boundaries — what the
/// ranker and the file scan compare terms with, so they read the word
/// table's tokens.
pub(crate) fn tokens_split(text: &str) -> impl Iterator<Item = &str> {
    tokens(text).flat_map(|token| {
        let mut pieces: Vec<&str> = Vec::new();
        let mut start = 0usize;
        let mut prev: Option<char> = None;
        for (i, c) in token.char_indices() {
            if prev.is_some_and(|p| script_boundary(p, c)) {
                pieces.push(&token[start..i]);
                start = i;
            }
            prev = Some(c);
        }
        pieces.push(&token[start..]);
        pieces
    })
}

/// The shortest last token a prefix query is sent for — the threshold of
/// the rest of routing (design 09 §6.5.2: from 4 the proxy was worse).
pub(crate) const PREFIX_MIN_CHARS: usize = 3;

/// Whether a word or a literal goes to the word table as a prefix query
/// (design 09 §6.5.2): its last token has at least [`PREFIX_MIN_CHARS`]
/// characters and is not a stop word, and the term carries no unsegmented
/// character — such a term matches a whole run there, and anywhere inside a
/// run through the trigram tables, so its reason does not depend on where
/// in the run it stands.
pub(crate) fn is_prefix_term(term: &str) -> bool {
    !term.chars().any(is_unsegmented)
        && tokens(term)
            .last()
            .is_some_and(|t| t.chars().count() >= PREFIX_MIN_CHARS && !STOP_WORDS.contains(&t))
}

pub(crate) fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x2E80..=0x2FDF   // CJK radicals
        | 0x3040..=0x30FF // Hiragana, Katakana
        | 0x3100..=0x312F // Bopomofo
        | 0x3130..=0x318F // Hangul compatibility jamo
        | 0x31F0..=0x31FF // Katakana extensions
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility
        | 0x1100..=0x11FF // Hangul jamo
        | 0x20000..=0x323AF // CJK extensions B–H
    )
}

/// Parses and validates a query (design §7a: 1–512 characters).
pub(crate) fn parse_query(query: &str) -> Result<ParsedQuery> {
    if query.trim().is_empty() {
        return Err(IndexError::InvalidInput(
            "query must not be empty".to_string(),
        ));
    }
    let chars = query.chars().count();
    if chars > MAX_QUERY_CHARS {
        return Err(IndexError::InvalidInput(format!(
            "query is {chars} characters; the limit is {MAX_QUERY_CHARS}"
        )));
    }
    let normalized_full = normalize(query);
    let normalized = normalized_full
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    let mut words: Vec<String> = Vec::new();
    let mut literals: Vec<Literal> = Vec::new();
    let mut shorts: Vec<String> = Vec::new();
    for token in normalized_full.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().all(char::is_alphanumeric) {
            if trimmed.chars().count() >= 3 {
                words.push(trimmed.to_string());
            } else {
                shorts.push(trimmed.to_string());
            }
        } else {
            let parts: Vec<String> = tokens(trimmed)
                .filter(|p| p.chars().count() >= 3 && !STOP_WORDS.contains(p))
                .map(str::to_string)
                .collect();
            literals.push(Literal {
                text: trimmed.to_string(),
                parts: dedupe(parts),
            });
        }
    }

    let has_long = !words.is_empty() || !literals.is_empty();
    let has_non_stop =
        !literals.is_empty() || words.iter().any(|w| !STOP_WORDS.contains(&w.as_str()));
    if has_non_stop {
        words.retain(|w| !STOP_WORDS.contains(&w.as_str()));
    }
    shorts.retain(|s| !has_long || s.chars().all(is_cjk));

    let mut parsed = ParsedQuery {
        normalized,
        words: dedupe(words),
        literals: Vec::new(),
        shorts: dedupe(shorts),
    };
    let mut seen: HashSet<String> = HashSet::new();
    for literal in literals {
        if seen.insert(literal.text.clone()) {
            parsed.literals.push(literal);
        }
    }
    // Bound the work a long query can ask for: the first terms win.
    let mut budget = MAX_TERMS;
    parsed.words.truncate(budget);
    budget -= parsed.words.len();
    parsed.literals.truncate(budget);
    budget -= parsed.literals.len();
    parsed.shorts.truncate(budget);
    Ok(parsed)
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}
