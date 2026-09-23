//! Query normalization and the literal query builder (design §5 steps 3–4).
//!
//! A query is normalized — NFKC, lowercase, Latin diacritics stripped — and
//! split into terms. Each term is one of:
//!
//! - a **word**: a run of alphanumeric characters, three or more of them —
//!   routed to both FTS tables;
//! - a **literal**: a token with punctuation inside it, an identifier or a
//!   path (`OPENALPACA_HOME_STORE`, `crate::module`, `p50/p95`) — kept whole
//!   as a phrase on both tables; its alphanumeric parts of three or more
//!   characters are the **fallback** the search routes as words only when
//!   the literal itself matches no eligible note (`title:tabs` finds a note
//!   about tabs; `OPENALPACA_HOME_STORE` does not drag in every note that
//!   says `openalpaca`);
//! - a **short** term: one or two characters — routed to the bounded
//!   substring pass when it is CJK, or when the query has no other term.
//!
//! The MATCH strings are built from these terms only: every term is a
//! double-quoted FTS5 string with embedded quotes doubled, joined by `OR`, so
//! `AND`, `NOT`, `NEAR`, `column:`, `*`, parentheses and an unbalanced quote
//! are text, never syntax — the F4 class of defect.
//!
//! English function words are dropped from routing when any other term
//! remains, so "the" does not recall every English note on the automatic path;
//! a query made only of them (`and`) still searches them literally. No
//! stemmer: `prefer` finds `prefers` through the trigram table, not through a
//! rule that changes what an identifier or a CJK run means.

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

    /// The `unicode61` table's MATCH string over `words` and the literals:
    /// every term a quoted phrase, ORed. `None` when there is nothing to
    /// route.
    pub(crate) fn word_match(&self, words: &[String]) -> Option<String> {
        let parts: Vec<String> = words
            .iter()
            .map(String::as_str)
            .chain(self.literals.iter().map(|l| l.text.as_str()))
            .map(quote)
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" OR "))
        }
    }

    /// The trigram table's MATCH string: the same terms, restricted to those
    /// of three or more characters (a shorter phrase matches nothing there).
    pub(crate) fn trigram_match(&self, words: &[String]) -> Option<String> {
        let parts: Vec<String> = words
            .iter()
            .map(String::as_str)
            .chain(self.literals.iter().map(|l| l.text.as_str()))
            .filter(|t| t.chars().count() >= 3)
            .map(quote)
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" OR "))
        }
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
