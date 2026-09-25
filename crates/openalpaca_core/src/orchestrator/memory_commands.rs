//! Chat memory commands (design 09 §12, owner decision MD-13): `/remember`,
//! `/forget` and `/memories`, answered by the daemon in its deterministic
//! tier with **no model involved**, like `/status` and `/tasks`.
//!
//! **Why this is G2-safe.** G2 forbids a *model* tool call from minting
//! origin `user`. These commands never reach a model: the intent parser takes
//! them out before skills and the main loop, and this module answers with a
//! fixed template. The text a `/remember` saves is exactly what the person
//! typed on an attended turn of the app's or the CLI's chat, so the command
//! is a user-directed surface — the in-chat equivalent of the Memory view —
//! and writes through the one `UserSurface` context a chat turn can build,
//! [`MemoryService::context_for_chat_command`]. Every write is the service's
//! own, so it publishes its `memory_changed` like any other.
//!
//! Every other turn is refused in a fixed sentence and nothing is read or
//! written (design 09 §12.2): a connector's, an unattended one, a scheduled
//! skill, a follow-up, `/v1/command process` ([`NOT_HERE`]), and a turn with
//! no owner or a daemon with no memory service ([`UNAVAILABLE`]). What is
//! printed is bounded — at most ten note lines, each at most
//! [`LINE_CHARS`] characters — and never a note's body: `/memories` is a
//! pointer, not a reader.

use super::Orchestrator;
use super::intent::MemoryVerb;
use crate::memory::service::{
    ChatCommandTurn, ForgetInput, ForgetItemStatus, ID_PREFIX_MIN_HEX, ListInput,
    MemoryAccessContext, MemoryError, MemoryService, SUGGESTIONS_MAX_LIMIT, SearchInput,
    StoreInput,
};
use crate::utils::text::one_line;
use openalpaca_storage::memory_files::{Dedup, EffectiveStatus, NoteId, NoteKind, NoteScope};

#[cfg(test)]
mod tests;

/// The answer on every turn that is not an attended turn of the app's or the
/// CLI's chat (design 09 §12.2).
pub(super) const NOT_HERE: &str = "Memory commands work in the OpenAlpaca app and the CLI.";
/// The answer on a turn with no owner (`Principal::System`, an id that is no
/// `global_user` row) and on a daemon with no memory service (§12.2).
pub(super) const UNAVAILABLE: &str = "Memory is not available here.";

/// A `/remember` title: the first line, cut at 80 characters (§12.1).
const TITLE_CHARS: usize = 80;
/// The longest line any command prints (§12.1).
const LINE_CHARS: usize = 160;
/// How much of what the person typed an answer quotes back.
const ECHO_CHARS: usize = 80;
/// `/forget <text>`: the matches listed (§12.1).
const FORGET_MATCHES: usize = 5;
/// `/forget <prefix>`: the notes an ambiguous prefix lists before "and N more".
const AMBIGUOUS_LISTED: usize = 10;
/// `/memories`: the notes listed (§12.1).
const MEMORIES_LISTED: usize = 10;
/// `/memories <query>`: the hits listed (§12.1).
const MEMORIES_MATCHES: usize = 8;

impl Orchestrator {
    /// Answers one memory command (design 09 §12). The reply is the turn's
    /// whole answer; a refusal is an answer too, never an error.
    pub(super) async fn handle_memory_command(
        &self,
        verb: MemoryVerb,
        args: String,
        turn: ChatCommandTurn,
    ) -> String {
        if !turn.is_attended_chat() {
            return NOT_HERE.to_string();
        }
        let Some(service) = self.memory_service() else {
            return UNAVAILABLE.to_string();
        };
        // The service's work is synchronous file I/O.
        match tokio::task::spawn_blocking(move || run(&service, &turn, verb, &args)).await {
            Ok(reply) => reply,
            Err(e) => {
                tracing::warn!(
                    command = verb.command(),
                    "memory command did not finish: {e}"
                );
                format!(
                    "The /{} command did not finish; see the daemon log.",
                    verb.command()
                )
            }
        }
    }
}

/// One command on one turn.
fn run(service: &MemoryService, turn: &ChatCommandTurn, verb: MemoryVerb, args: &str) -> String {
    let ctx = match service.context_for_chat_command(turn) {
        Ok(ctx) => ctx,
        // Not an attended app or CLI chat turn: the caller answered first.
        Err(MemoryError::InvalidInput(_)) => return NOT_HERE.to_string(),
        Err(_) => return UNAVAILABLE.to_string(),
    };
    match verb {
        MemoryVerb::Remember => remember(service, &ctx, args),
        MemoryVerb::Forget => forget(service, &ctx, args),
        MemoryVerb::Memories => memories(service, &ctx, args),
    }
}

// ── /remember ────────────────────────────────────────────────────────────

/// `/remember [--kind <k>] [--global | --project] <text>`, parsed.
#[derive(Debug, PartialEq)]
struct RememberArgs<'a> {
    kind: NoteKind,
    /// `None`: by kind, against the turn's project (design §3 rule 2).
    scope: Option<NoteScope>,
    text: &'a str,
}

/// The options come first and end at the first word that is not one, or at
/// `--`; the rest is the text, as typed. An unknown `--option` is refused
/// rather than saved as text: `--` saves a text that starts with dashes.
fn parse_remember(args: &str) -> Result<RememberArgs<'_>, MemoryError> {
    let mut kind: Option<NoteKind> = None;
    let mut scope: Option<NoteScope> = None;
    let mut rest = args.trim_start();
    while rest.starts_with("--") {
        let (token, after) = split_word(rest);
        rest = after;
        match token {
            "--" => break,
            "--global" | "--project" => {
                if scope.is_some() {
                    return Err(MemoryError::invalid("choose one of --global and --project"));
                }
                scope = Some(if token == "--global" {
                    NoteScope::Global
                } else {
                    NoteScope::Project
                });
            }
            "--kind" => {
                let (value, after) = split_word(rest);
                rest = after;
                set_kind(&mut kind, value)?;
            }
            _ if token.starts_with("--kind=") => set_kind(&mut kind, &token["--kind=".len()..])?,
            _ => {
                return Err(MemoryError::invalid(format!(
                    "unknown option `{}`: /remember takes --kind <kind>, --global or --project before the text (put -- before a text that starts with --)",
                    cut_chars(&one_line(token), ECHO_CHARS)
                )));
            }
        }
    }
    let text = rest.trim();
    if text.is_empty() {
        return Err(MemoryError::invalid(
            "write what to remember after /remember, for example `/remember I prefer tabs`",
        ));
    }
    Ok(RememberArgs {
        kind: kind.unwrap_or(NoteKind::Fact),
        scope,
        text,
    })
}

/// The first whitespace-separated word of `text` and what follows it, the
/// whitespace in between dropped.
fn split_word(text: &str) -> (&str, &str) {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let (word, rest) = text.split_at(end);
    (word, rest.trim_start())
}

fn set_kind(kind: &mut Option<NoteKind>, value: &str) -> Result<(), MemoryError> {
    if kind.is_some() {
        return Err(MemoryError::invalid("--kind is given twice"));
    }
    let kinds = NoteKind::ALL
        .iter()
        .map(|k| k.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if value.is_empty() {
        return Err(MemoryError::invalid(format!(
            "--kind needs a kind after it: one of {kinds}"
        )));
    }
    *kind = Some(NoteKind::parse(&value.to_ascii_lowercase()).ok_or_else(|| {
        MemoryError::invalid(format!(
            "`{}` is not a kind: use one of {kinds}",
            cut_chars(&one_line(value), ECHO_CHARS)
        ))
    })?);
    Ok(())
}

/// The first line, cut at [`TITLE_CHARS`] characters on a character
/// boundary (§12.1).
fn title_of(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    first
        .chars()
        .take(TITLE_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn remember(service: &MemoryService, ctx: &MemoryAccessContext, args: &str) -> String {
    let parsed = match parse_remember(args) {
        Ok(parsed) => parsed,
        Err(e) => return refused("Nothing was saved", &e),
    };
    let title = title_of(parsed.text);
    let input = StoreInput {
        title: title.clone(),
        body: parsed.text.to_string(),
        kind: Some(parsed.kind),
        scope: parsed.scope,
        ..Default::default()
    };
    match service.store(ctx, input) {
        Ok(out) => {
            let shown = one_line(&title);
            let (id, scope) = (id8(out.id), out.scope);
            match out.dedup {
                Dedup::Created => format!(
                    "Saved as a note you wrote: “{shown}” ({id}, {scope}). It is used from your next message."
                ),
                Dedup::Existing => {
                    format!("Already saved: “{shown}” ({id}, {scope}). Nothing new was written.")
                }
            }
        }
        Err(e) => refused("Nothing was saved", &e),
    }
}

// ── /forget ──────────────────────────────────────────────────────────────

/// A single word of hex digits (`0-9`, `a-f`, `A-F`) and hyphens, starting
/// with a hex digit, with at least [`ID_PREFIX_MIN_HEX`] of them: an id or a
/// prefix of one. Anything else is text to search for. A date such as
/// `2026-09-25`, or a number of eight digits or more, is id-shaped too, and
/// is looked up as an id prefix rather than searched for.
fn is_id_shaped(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
        && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && text.chars().filter(char::is_ascii_hexdigit).count() >= ID_PREFIX_MIN_HEX
}

fn forget(service: &MemoryService, ctx: &MemoryAccessContext, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        return "Send /forget <id> to forget a note, or /forget <words> to find it first. Nothing was forgotten."
            .to_string();
    }
    if is_id_shaped(args) {
        forget_by_id(service, ctx, args)
    } else {
        forget_search(service, ctx, args)
    }
}

/// `/forget <id-or-prefix>` (§12.1): a unique prefix forgets that note at
/// the revision just read; an ambiguous one lists the notes it matches and
/// forgets nothing.
fn forget_by_id(service: &MemoryService, ctx: &MemoryAccessContext, token: &str) -> String {
    let echo = cut_chars(token, ECHO_CHARS);
    let found = match service.find_by_id_prefix(ctx, token) {
        Ok(found) => found,
        Err(e) => return refused("Nothing was forgotten", &e),
    };
    let none = format!("No note of yours has an id starting with `{echo}`. Nothing was forgotten.");
    match found.as_slice() {
        [] => none,
        [note] => {
            let title = cut_chars(&one_line(&note.title), ECHO_CHARS);
            let input = ForgetInput {
                targets: vec![(note.id, note.revision.clone())],
                reason: Some("/forget in chat".to_string()),
            };
            let out = match service.forget(ctx, input) {
                Ok(out) => out,
                Err(e) => return refused("Nothing was forgotten", &e),
            };
            match out.results.first().map(|r| &r.status) {
                Some(ForgetItemStatus::Forgotten) => {
                    format!("Forgot “{title}” ({}). {}", id8(note.id), out.retained.note)
                }
                Some(ForgetItemStatus::RevisionConflict { .. }) => format!(
                    "“{title}” ({id}) changed while it was being forgotten, so nothing was forgotten. Send /forget {id} again.",
                    id = id8(note.id)
                ),
                _ => none,
            }
        }
        many => {
            let mut lines = vec![format!(
                "`{echo}` matches {} notes, so nothing was forgotten. Send more of the id:",
                many.len()
            )];
            lines.extend(many.iter().take(AMBIGUOUS_LISTED).map(|note| {
                note_line(
                    &note.id.to_string(),
                    &note.title,
                    note.kind,
                    note.scope,
                    Some(note.effective_status),
                    "",
                )
            }));
            if many.len() > AMBIGUOUS_LISTED {
                lines.push(format!("…and {} more.", many.len() - AMBIGUOUS_LISTED));
            }
            lines.join("\n")
        }
    }
}

/// `/forget <text>` (§12.1): up to five active notes that match, and
/// nothing forgotten — a natural-language forget first selects (design §6,
/// *Forget contract*).
fn forget_search(service: &MemoryService, ctx: &MemoryAccessContext, text: &str) -> String {
    let input = SearchInput {
        query: text.to_string(),
        limit: Some(FORGET_MATCHES),
        ..Default::default()
    };
    match service.search(ctx, input) {
        Ok(out) if out.results.is_empty() => nothing_matched(text),
        Ok(out) => {
            let mut lines: Vec<String> = out
                .results
                .iter()
                .map(|r| note_line(&id8(r.id), &r.title, r.kind, r.scope, None, ""))
                .collect();
            lines.push("Send /forget <id> to forget one.".to_string());
            lines.join("\n")
        }
        Err(e) => refused("Nothing was forgotten", &e),
    }
}

// ── /memories ────────────────────────────────────────────────────────────

fn memories(service: &MemoryService, ctx: &MemoryAccessContext, args: &str) -> String {
    let query = args.trim();
    if query.is_empty() {
        memories_recent(service, ctx)
    } else {
        memories_search(service, ctx, query)
    }
}

/// `/memories` (§12.1): the ten most recently updated active notes, and how
/// many suggestions wait in the Memory view (design 09 §3.5).
fn memories_recent(service: &MemoryService, ctx: &MemoryAccessContext) -> String {
    let input = ListInput {
        limit: Some(MEMORIES_LISTED),
        ..Default::default()
    };
    let page = match service.list(ctx, input) {
        Ok(page) => page,
        Err(e) => return refused("Your notes could not be listed", &e),
    };
    let mut lines: Vec<String> = page
        .items
        .iter()
        .map(|i| note_line(&id8(i.id), &i.title, i.kind, i.scope, None, ""))
        .collect();
    if lines.is_empty() {
        lines.push("Nothing remembered yet. Send /remember <text> to save a note.".to_string());
    } else if page.next_cursor.is_some() {
        lines.push("More notes are kept: send /memories <words> to search them.".to_string());
    }
    // A pointer only: a failure to count is not worth failing the listing.
    let pending = match service.suggestions(ctx, Some(SUGGESTIONS_MAX_LIMIT)) {
        Ok(listed) => listed.len(),
        Err(e) => {
            tracing::debug!("memory: /memories could not count the suggestions: {e}");
            0
        }
    };
    match pending {
        0 => {}
        1 => lines.push("1 suggestion is waiting in the Memory view.".to_string()),
        n if n >= SUGGESTIONS_MAX_LIMIT => lines.push(format!(
            "At least {n} suggestions are waiting in the Memory view."
        )),
        n => lines.push(format!("{n} suggestions are waiting in the Memory view.")),
    }
    lines.join("\n")
}

/// `/memories <query>` (§12.1): up to eight active notes that match, each
/// with its match reason.
fn memories_search(service: &MemoryService, ctx: &MemoryAccessContext, query: &str) -> String {
    let input = SearchInput {
        query: query.to_string(),
        limit: Some(MEMORIES_MATCHES),
        ..Default::default()
    };
    match service.search(ctx, input) {
        Ok(out) if out.results.is_empty() => nothing_matched(query),
        Ok(out) => out
            .results
            .iter()
            .map(|r| {
                let reasons: Vec<&str> = r.matches.iter().map(|m| m.as_str()).collect();
                let suffix = if reasons.is_empty() {
                    String::new()
                } else {
                    format!("  (matched: {})", reasons.join(", "))
                };
                note_line(&id8(r.id), &r.title, r.kind, r.scope, None, &suffix)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => refused("Nothing was searched", &e),
    }
}

// ── Rendering ────────────────────────────────────────────────────────────

/// The first 8 characters of an id — a unique prefix in any real store.
fn id8(id: NoteId) -> String {
    id.to_string().chars().take(8).collect()
}

/// One note as one line — `<id>  <title>  [<kind>/<scope>]<suffix>`, the
/// status added when the note is not in force — at most [`LINE_CHARS`]
/// characters: the title gives way, ending in `…`.
fn note_line(
    id: &str,
    title: &str,
    kind: NoteKind,
    scope: NoteScope,
    status: Option<EffectiveStatus>,
    suffix: &str,
) -> String {
    let tag = match status {
        Some(status) if status != EffectiveStatus::Active => {
            format!("[{kind}/{scope}/{}]", status.as_str())
        }
        _ => format!("[{kind}/{scope}]"),
    };
    let fixed = id.chars().count() + tag.chars().count() + suffix.chars().count() + 4;
    let title = cut_chars(&one_line(title), LINE_CHARS.saturating_sub(fixed));
    format!("{id}  {title}  {tag}{suffix}")
}

fn nothing_matched(text: &str) -> String {
    format!(
        "Nothing matched “{}”.",
        cut_chars(&one_line(text), ECHO_CHARS)
    )
}

/// A refusal in the service's own words (§12.1: "the refusal's own
/// sentence"), after what did not happen.
fn refused(what: &str, e: &MemoryError) -> String {
    let reason = e.to_string();
    format!("{what}: {}.", reason.trim_end_matches('.'))
}

/// `text` cut to at most `max` characters, ending in `…` when it was longer.
fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}
