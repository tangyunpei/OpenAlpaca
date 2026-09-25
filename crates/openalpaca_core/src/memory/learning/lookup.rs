//! The known-notes lookup's prompt side (design 09 §2.3): the queries a
//! chunk's rows yield, and the `<known_notes>` / `<known_keys>` text a
//! [`LearningView`] renders to within the chunk's byte budget.
//!
//! The lookup itself — the index reads, the `note_source` join, the file
//! verification — is the service's, on the `Extraction` surface only
//! ([`crate::memory::service::MemoryService::learning_view`]). Nothing here
//! reads a file or a database; it is deterministic text work.

use super::window::Chunk;
use crate::memory::service::{KnownNote, LearningView};
use openalpaca_storage::memory_files::Origin;
use openalpaca_storage::memory_index::MAX_QUERY_CHARS;

/// The most query windows one row yields: the first three and the last
/// (design 09 §2.3 step 3).
pub(crate) const MAX_WINDOWS_PER_ROW: usize = 4;
/// Each listed note's knowledge excerpt, in characters.
pub(crate) const EXCERPT_CHARS: usize = 160;
/// The most `known_keys` text one call carries, out of `known_bytes`.
pub(crate) const MAX_KEYS_BYTES: usize = 2 * 1024;

/// What the prompt lists, and what each `n<k>` names.
#[derive(Debug, Clone, Default)]
pub(crate) struct Lookup {
    /// The listed notes: `n<k>` is `listed[k - 1]`. The extractor's own
    /// record of the lookup — a model can only name a note by its ref.
    pub listed: Vec<KnownNote>,
    /// This window's learned notes, active and candidate: the re-run guard's
    /// input (design 09 §1.4(b)). Never shown as such.
    pub from_this_window: Vec<KnownNote>,
    /// The `<known_notes>` element's body: empty, or a newline and one line
    /// per listed note.
    pub notes_text: String,
    /// The `<known_keys>` element's body: the keys, comma-separated.
    pub keys_text: String,
}

/// The queries a chunk's rows yield (design 09 §2.3 step 3): each of the
/// user's own rows, split at whitespace into windows of at most 512
/// characters — a text with no whitespace, as Chinese has none, is cut at
/// 512 — and at most four windows per row, the first three and the last, so
/// a fact stated late in a long row is still looked up.
pub(crate) fn queries_for(chunk: &Chunk) -> Vec<String> {
    chunk
        .rows
        .iter()
        .filter(|r| r.kind.is_users_text())
        .flat_map(|r| query_windows(&r.text))
        .collect()
}

/// One row's query windows.
pub(crate) fn query_windows(text: &str) -> Vec<String> {
    let mut windows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0usize;
    for word in text.split_whitespace() {
        let mut word = word;
        loop {
            let chars = word.chars().count();
            let room = if current.is_empty() {
                MAX_QUERY_CHARS
            } else {
                MAX_QUERY_CHARS.saturating_sub(current_chars + 1)
            };
            if chars <= room {
                if !current.is_empty() {
                    current.push(' ');
                    current_chars += 1;
                }
                current.push_str(word);
                current_chars += chars;
                break;
            }
            if !current.is_empty() {
                windows.push(std::mem::take(&mut current));
                current_chars = 0;
                continue;
            }
            // One run longer than a whole window: cut it at 512 characters.
            let cut = word
                .char_indices()
                .nth(MAX_QUERY_CHARS)
                .map_or(word.len(), |(i, _)| i);
            windows.push(word[..cut].to_string());
            word = &word[cut..];
            if word.is_empty() {
                break;
            }
        }
    }
    if !current.is_empty() {
        windows.push(current);
    }
    if windows.len() > MAX_WINDOWS_PER_ROW {
        let last = windows.pop().expect("more than four windows");
        windows.truncate(MAX_WINDOWS_PER_ROW - 1);
        windows.push(last);
    }
    windows
}

/// How the prompt names a note's origin (design 09 §2.3 step 5): the label
/// tells the model that replacing a note the user saved becomes a
/// suggestion. The decision itself is the service's, never the model's.
/// `extracted_other` is never listed: it is always a candidate.
pub(crate) fn origin_label(origin: Origin) -> Option<&'static str> {
    match origin {
        Origin::User => Some("saved-by-you"),
        Origin::ExtractedUser => Some("learned"),
        Origin::External => Some("edited-by-hand"),
        Origin::Imported => Some("imported"),
        Origin::ExtractedOther => None,
    }
}

/// A note's knowledge on one line, cut to [`EXCERPT_CHARS`] characters.
fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= EXCERPT_CHARS {
        return flat;
    }
    let mut out: String = flat.chars().take(EXCERPT_CHARS - 1).collect();
    out.push('…');
    out
}

/// One `known_notes` line (design 09 §2.1):
/// `n1 key=user.editor origin=learned kind=preference scope=global "Editor preference" — The user edits with vim.`
pub(crate) fn note_line(n: usize, note: &KnownNote) -> Option<String> {
    let origin = origin_label(note.origin)?;
    let mut line = format!(
        "n{n} key={} origin={origin} kind={} scope={} \"{}\" — {}",
        note.fact_key.as_deref().unwrap_or("-"),
        note.kind.as_str(),
        note.scope.as_str(),
        note.title,
        excerpt(&note.knowledge),
    );
    if note.from_this_window {
        line.push_str(" [from this conversation]");
    }
    line.push('\n');
    Some(line)
}

/// The keys, comma-separated, as many as fit `budget` bytes, in the order
/// given (the most recently updated first).
pub(crate) fn render_keys(keys: &[String], budget: usize) -> String {
    let mut out = String::new();
    for key in keys {
        let add = if out.is_empty() {
            key.len()
        } else {
            key.len() + 2
        };
        if out.len() + add > budget {
            break;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(key);
    }
    out
}

/// Renders a view within `known_bytes` (design 09 §1.6, §2.3): the keys take
/// at most 2 KiB of it, and the notes, in the view's order, as many of the
/// rest as fit — a line too long for what is left is skipped, not cut.
/// `n<k>` numbers the lines actually listed.
pub(crate) fn render(view: LearningView, known_bytes: usize) -> Lookup {
    let keys_text = render_keys(&view.keys, MAX_KEYS_BYTES.min(known_bytes));
    let mut left = known_bytes.saturating_sub(keys_text.len());
    let mut listed: Vec<KnownNote> = Vec::new();
    let mut lines = String::new();
    for note in view.notes {
        let Some(line) = note_line(listed.len() + 1, &note) else {
            continue;
        };
        // The element's leading newline is paid by its first line.
        let cost = line.len() + usize::from(lines.is_empty());
        if cost > left {
            continue;
        }
        left -= cost;
        lines.push_str(&line);
        listed.push(note);
    }
    let notes_text = if lines.is_empty() {
        String::new()
    } else {
        format!("\n{lines}")
    };
    Lookup {
        listed,
        from_this_window: view.from_this_window,
        notes_text,
        keys_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::memory_files::{EffectiveStatus, NoteId, NoteKind, NoteScope};

    fn note(title: &str, knowledge: &str, origin: Origin) -> KnownNote {
        KnownNote {
            id: NoteId::new(),
            revision: "0".repeat(64),
            partition_key: "home-x".into(),
            scope: NoteScope::Global,
            title: title.into(),
            kind: NoteKind::Preference,
            origin,
            effective_status: EffectiveStatus::Active,
            fact_key: None,
            knowledge: knowledge.into(),
            supersedes: Vec::new(),
            from_this_window: false,
        }
    }

    /// Design 09 §2.3 step 3: a row over 512 characters is searched in
    /// windows of at most 512 — the first three and the last — so a fact
    /// stated at its very end is still looked up; a run with no whitespace
    /// is cut, never passed whole to a query builder that refuses it.
    #[test]
    fn a_long_row_is_searched_in_512_char_windows() {
        let filler = "lorem ipsum dolor sit amet ".repeat(120);
        let text = format!("{filler} my laptop is a thinkpad");
        assert!(text.chars().count() > 4 * 512);
        let windows = query_windows(&text);
        assert_eq!(windows.len(), 4, "{:?}", windows.len());
        assert!(windows.iter().all(|w| w.chars().count() <= 512));
        assert!(windows[3].ends_with("thinkpad"), "the last window is kept");
        assert!(windows[0].starts_with("lorem"));

        let chinese = "我的笔记本电脑是联想".repeat(60);
        let windows = query_windows(&chinese);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].chars().count(), 512);
        assert_eq!(windows[1].chars().count(), 600 - 512);

        assert_eq!(query_windows("short row"), vec!["short row".to_string()]);
        assert!(query_windows("   ").is_empty());
    }

    /// Design 09 §2.1, §2.3: the prompt lists keys within 2 KiB and notes
    /// within what is left of `known_bytes`, numbered as listed, with the
    /// origin labels and a 160-character excerpt; a candidate-only origin
    /// is never listed.
    #[test]
    fn the_prompt_lists_known_keys_and_notes_within_budget() {
        let mut editor = note(
            "Editor preference",
            "The user edits with vim.",
            Origin::ExtractedUser,
        );
        editor.fact_key = Some("user.editor".into());
        editor.from_this_window = true;
        let saved = note("Work laptop", &"a ThinkPad X1 ".repeat(40), Origin::User);
        let inferred = note("Probably Rust", "inferred", Origin::ExtractedOther);
        let keys: Vec<String> = (0..400).map(|i| format!("user.key{i:03}")).collect();
        let view = LearningView {
            notes: vec![editor.clone(), inferred, saved.clone()],
            from_this_window: vec![editor.clone()],
            keys,
        };

        let lookup = render(view.clone(), 3086);
        assert!(lookup.keys_text.len() <= MAX_KEYS_BYTES);
        assert!(lookup.keys_text.starts_with("user.key000, user.key001"));
        assert!(lookup.notes_text.len() + lookup.keys_text.len() <= 3086);
        assert_eq!(
            lookup.listed.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![editor.id, saved.id]
        );
        let lines: Vec<&str> = lookup.notes_text.lines().skip(1).collect();
        assert_eq!(
            lines[0],
            "n1 key=user.editor origin=learned kind=preference scope=global \"Editor preference\" — The user edits with vim. [from this conversation]"
        );
        assert!(lines[1].starts_with("n2 key=- origin=saved-by-you kind=preference"));
        let excerpt = lines[1].split(" — ").nth(1).unwrap();
        assert_eq!(excerpt.chars().count(), EXCERPT_CHARS);
        assert!(excerpt.ends_with('…'));

        // A tight budget lists what fits and nothing past it.
        let tight = render(view.clone(), 2048 + 150);
        assert_eq!(tight.listed.len(), 1, "{:?}", tight.notes_text);
        assert!(tight.notes_text.len() + tight.keys_text.len() <= 2048 + 150);
        let none = render(
            LearningView {
                notes: Vec::new(),
                from_this_window: Vec::new(),
                keys: Vec::new(),
            },
            3086,
        );
        assert_eq!(
            (none.notes_text.as_str(), none.keys_text.as_str()),
            ("", "")
        );
    }
}
