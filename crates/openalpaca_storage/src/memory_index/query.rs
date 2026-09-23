//! Query normalization and the literal query builder (design §5 steps 3–4).
//!
//! A query is normalized — NFKC, lowercase, Latin diacritics stripped — and
//! split into terms. Each term is one of:
//!
//! - a **word**: a run of alphanumeric characters, three or more of them —
//!   routed to both FTS tables;
//! - a **literal**: a token with punctuation inside it, an identifier or a
//!   path (`OPENALPACA_HOME_STORE`, `crate::module`, `p50/p95`) — kept whole
//!   as a phrase on both tables, and its alphanumeric parts of three or more
//!   characters also become words;
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
    "a", "an", "the", "and", "or", "but", "of", "to", "in", "on", "at", "by",
    "for", "with", "from", "as", "is", "are", "was", "were", "be", "been",
    "being", "am", "do", "does", "did", "have", "has", "had", "i", "me", "my",
    "we", "our", "you", "your", "he", "she", "it", "its", "they", "them",
    "their", "this", "that", "these", "those", "what", "which", "who", "whom",
    "how", "when", "where", "why", "not", "no", "so", "if", "than", "then",
    "there", "here", "can", "could", "should", "would", "will", "shall", "may",
    "might", "must", "about", "into", "over", "under", "again", "also", "just",
    "very", "please",
];

/// One routed term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Term {
    /// Three or more alphanumeric characters — both FTS tables.
    Word(String),
    /// An identifier or path kept whole — a phrase on both tables.
    Literal(String),
    /// One or two characters — the substring pass.
    Short(String),
}

impl Term {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Word(s) | Self::Literal(s) | Self::Short(s) => s,
        }
    }
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedQuery {
    /// The whole query normalized, with runs of whitespace collapsed to one
    /// space and the ends trimmed — what exact title and alias matching sees.
    pub normalized: String,
    /// The routed terms, deduplicated, in query order.
    pub terms: Vec<Term>,
}

impl ParsedQuery {
    pub(crate) fn words(&self) -> impl Iterator<Item = &str> {
        self.terms.iter().filter_map(|t| match t {
            Term::Word(s) => Some(s.as_str()),
            _ => None,
        })
    }

    pub(crate) fn literals(&self) -> impl Iterator<Item = &str> {
        self.terms.iter().filter_map(|t| match t {
            Term::Literal(s) => Some(s.as_str()),
            _ => None,
        })
    }

    pub(crate) fn shorts(&self) -> impl Iterator<Item = &str> {
        self.terms.iter().filter_map(|t| match t {
            Term::Short(s) => Some(s.as_str()),
            _ => None,
        })
    }

    /// The `unicode61` table's MATCH string: every word and literal as a
    /// quoted phrase, ORed. `None` when there is nothing to route.
    pub(crate) fn word_match(&self) -> Option<String> {
        let parts: Vec<String> = self
            .words()
            .chain(self.literals())
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
    pub(crate) fn trigram_match(&self) -> Option<String> {
        let parts: Vec<String> = self
            .words()
            .chain(self.literals())
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
    let lowered: String = text
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect();
    lowered
        .nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .nfc()
        .collect()
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

    let mut raw: Vec<Term> = Vec::new();
    for token in normalized_full.split_whitespace() {
        let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().all(char::is_alphanumeric) {
            raw.push(classify(trimmed));
        } else {
            raw.push(Term::Literal(trimmed.to_string()));
            for part in tokens(trimmed) {
                if part.chars().count() >= 3 {
                    raw.push(Term::Word(part.to_string()));
                }
            }
        }
    }

    let has_long = raw
        .iter()
        .any(|t| matches!(t, Term::Word(_) | Term::Literal(_)));
    let has_non_stop = raw.iter().any(|t| match t {
        Term::Word(w) => !STOP_WORDS.contains(&w.as_str()),
        Term::Literal(_) => true,
        Term::Short(_) => false,
    });

    let mut seen: HashSet<String> = HashSet::new();
    let mut terms: Vec<Term> = Vec::new();
    for term in raw {
        let keep = match &term {
            Term::Word(w) => !(has_non_stop && STOP_WORDS.contains(&w.as_str())),
            Term::Literal(_) => true,
            Term::Short(s) => !has_long || s.chars().all(is_cjk),
        };
        if !keep {
            continue;
        }
        let key = format!("{}:{}", term_tag(&term), term.text());
        if seen.insert(key) {
            terms.push(term);
        }
        if terms.len() >= MAX_TERMS {
            break;
        }
    }
    Ok(ParsedQuery { normalized, terms })
}

fn classify(token: &str) -> Term {
    if token.chars().count() >= 3 {
        Term::Word(token.to_string())
    } else {
        Term::Short(token.to_string())
    }
}

fn term_tag(term: &Term) -> &'static str {
    match term {
        Term::Word(_) => "w",
        Term::Literal(_) => "l",
        Term::Short(_) => "s",
    }
}
