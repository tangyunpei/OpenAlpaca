//! A failed stdio MCP server's stderr, kept in its row's `detail` (N-MCP-5).
//!
//! `openalpaca_mcp` keeps the last bytes each spawn wrote to its stderr
//! ([`StderrTail`]), raw. This is the one place they become text a person or a
//! model is shown, and only in the `detail` of a `Failed` row — never in an
//! `McpError`'s text, never in the connection state's reason, never in a log
//! line of their own. What is shown is, in this order:
//!
//! 1. decoded lossily, `\r\n` read as `\n`;
//! 2. every value the load resolved for the server — its `env` and `env_from`
//!    values, credential-bearing arguments, the daemon's own variables with
//!    secret-like names — replaced by [`MASKED`], longest first, a value's
//!    `\r\n` read as `\n` as the text's is;
//! 3. a tail the ring's own bound cut at the head that lies wholly inside
//!    one value masked whole; else a value cut at the head masked too, or
//!    the partial first line dropped (a cut tail of one line ending with a
//!    newline has none to drop: the value is masked) — and the rest of
//!    a value holding a newline (what follows one of its newlines) masked
//!    whole where the text begins with it, at the text's start or at its
//!    second line's (the cut fell inside a line of the value), so the drop
//!    leaves none of its later lines; and the start of a value that ends
//!    the text (a tail read before the server finished writing it) masked:
//!    inside a line of it, or, for a value holding a newline, just after
//!    one of its newlines — all judged in step 2's one scan, on the text as
//!    written, so a shorter value inside a part cut at the head cannot hide
//!    it (at the end one still can: a shorter value that is a prefix of a
//!    longer one ending the text is masked whole and the longer value's cut
//!    start after it is shown);
//! 4. the generic shapes of a credential masked: a URL's userinfo, a bearer
//!    token, an `Authorization` or `Cookie` header's value, a `key=value` or
//!    `"key": "value"` whose name looks like a secret, a sensitive query
//!    parameter;
//! 5. cut from the end to [`TAIL_BYTES`] of escaped text, on a whole piece —
//!    never inside a `<masked>` or a `<U+XXXX>` — so the last line is kept;
//! 6. escaped once, here, with [`escape_hidden`] (D19 (a)), so every surface
//!    that shows the detail shows `<U+XXXX>` for a hidden character.
//!
//! The detail is then `<base>\nserver stderr (last N bytes, redacted):\n<tail>`,
//! the tail last (D23); a spawn that wrote nothing leaves `base` as it is.
//! [`super::describe::wrap_detail`] still wraps it wherever it enters a tool
//! result or the chat.

use std::sync::LazyLock;

use openalpaca_mcp::{STDERR_SETTLE, SpawnTail, StderrTail, TAIL_BYTES, TailSnapshot};
use regex::Regex;

use crate::utils::text::escape_hidden;

/// What a masked value reads as — the word the MCP fingerprint uses.
pub const MASKED: &str = "<masked>";

/// The heading line between the detail's own text and the tail.
const HEADING: &str = "server stderr (last";

/// Does this name — an `env` key, a flag, a header — name a credential? (R65.)
///
/// Deliberately a name test and deliberately coarse: it is the shape of the
/// key, not the entropy of the value, that says *"this is a secret"*. The
/// daemon refuses to write a declaration whose names pass it (R65, R82), and a
/// stdio server's stderr tail is masked with the values of the daemon's own
/// environment variables whose names pass it (N-MCP-5), since every stdio
/// child inherits that environment.
///
/// The refusal governs only what the **daemon writes**: a hand-authored
/// `mcp.toml` may still carry literal `env` values — the parser reads them
/// unchanged — because that file is the owner's, and the refusal is about the
/// daemon not becoming a secret-writing tool.
pub fn looks_like_a_secret(key: &str) -> bool {
    const MARKERS: [&str; 5] = ["TOKEN", "KEY", "SECRET", "PASSWORD", "CREDENTIAL"];
    let upper = key.to_ascii_uppercase();
    MARKERS.iter().any(|marker| upper.contains(marker))
}

/// `base`, and the newest spawn's stderr tail once that spawn's stderr has
/// ended or [`STDERR_SETTLE`] has passed — the detail of a `Failed` row.
pub async fn detail_with_stderr(base: impl Into<String>, tail: &StderrTail) -> String {
    detail_with_spawn_stderr(base, &tail.current(), tail.secrets()).await
}

/// [`detail_with_stderr`] for one given spawn's tail: an exit's, kept when the
/// exit was reported, so a respawn started since does not lend it its own.
pub async fn detail_with_spawn_stderr(
    base: impl Into<String>,
    spawn: &SpawnTail,
    secrets: &[String],
) -> String {
    let snapshot = spawn.settle(STDERR_SETTLE).await;
    compose_detail(base, &snapshot, secrets)
}

/// The detail from `base` and a tail already read.
pub fn compose_detail(base: impl Into<String>, tail: &TailSnapshot, secrets: &[String]) -> String {
    let base = base.into();
    let shown = shown_tail(tail, secrets);
    if shown.is_empty() {
        return base;
    }
    format!(
        "{base}\n{HEADING} {} bytes, redacted):\n{shown}",
        shown.len()
    )
}

/// Steps 1–6 of the module's list.
fn shown_tail(tail: &TailSnapshot, secrets: &[String]) -> String {
    let text = String::from_utf8_lossy(&tail.bytes).replace("\r\n", "\n");
    // The values as the text is read: a value written with `\r\n` line
    // ends is found in the text only with `\n` ones.
    let secrets = as_read(secrets);
    let partial_head = tail.truncated;
    let text = mask_values(&text, &secrets, partial_head);
    let text = mask_shapes(&text);
    let text = text.trim_end_matches('\n');
    let (pieces, cut) = last_pieces(text, TAIL_BYTES);
    let mut shown: String = pieces.concat();
    if (partial_head || cut)
        && let Some(newline) = shown.find('\n')
    {
        // The first line begins where a bound cut it: drop it, rather than
        // show the second half of something.
        shown.drain(..=newline);
    }
    shown
}

/// `secrets` with each `\r\n` read as `\n`, as step 1 reads the text, kept
/// longest first.
fn as_read(secrets: &[String]) -> Vec<String> {
    let mut read: Vec<String> = secrets
        .iter()
        .map(|secret| secret.replace("\r\n", "\n"))
        .filter(|secret| !secret.is_empty())
        .collect();
    read.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    read.dedup();
    read
}

/// Every occurrence of a `secrets` value replaced by [`MASKED`], the longest
/// value first at each place; also the longest start of a value that `text`
/// ends with (see [`value_ranges`]); with `cut_head` (the ring cut the tail's
/// head), the whole text when it lies wholly inside a value
/// ([`lies_inside_a_value`]), else also the rest of a value holding a
/// newline — what follows one of its newlines — that `text` begins with, at
/// its start or just after its first newline, and, with no newline left
/// outside the masked values once trailing newlines are trimmed (no whole
/// line, so no first line for [`shown_tail`] to drop), every end of a value
/// that `text` begins with: each judged on the text as written, before a
/// shorter value inside it is masked.
///
/// The rest of a value is looked for at those two places only. [`shown_tail`]
/// drops the cut head's first line, so what is shown begins at the second
/// place: a cut inside a line of the value leaves the part of that line in
/// the dropped one and the rest of the value at the second place, and a cut
/// just after one of its newlines leaves the rest of it at the start. A
/// later line of the value elsewhere in the text is not the cut's remainder
/// (and a whole value is the scan's).
///
/// Where the head reads more than one way (no end of a value, or one of
/// several, followed by what the scan finds after it), every reading's
/// masked ranges are taken, and ranges that overlap are shown as one
/// [`MASKED`].
fn mask_values(text: &str, secrets: &[String], cut_head: bool) -> String {
    if secrets.is_empty() {
        return text.to_string();
    }
    if cut_head && lies_inside_a_value(text, secrets) {
        return MASKED.to_string();
    }
    let mut ranges = value_ranges(text, secrets, 0);
    if cut_head {
        let second_line = text.find('\n').map(|newline| newline + 1);
        for anchor in std::iter::once(0).chain(second_line) {
            for len in line_remainders(&text[anchor..], secrets) {
                ranges.push(anchor..anchor + len);
                ranges.extend(value_ranges(text, secrets, anchor + len));
            }
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        // Judged on the text the drop sees: trailing newlines are trimmed
        // before it, so they are no whole line.
        if !newline_outside(text.trim_end_matches('\n'), &ranges) {
            for end in head_fragments(text, secrets) {
                ranges.push(0..end);
                ranges.extend(value_ranges(text, secrets, end));
            }
            ranges.sort_by_key(|range| (range.start, range.end));
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut shown_to = 0;
    let mut ranges = ranges.into_iter().peekable();
    while let Some(range) = ranges.next() {
        let mut end = range.end;
        while let Some(next) = ranges.next_if(|next| next.start < end) {
            end = end.max(next.end);
        }
        out.push_str(&text[shown_to..range.start]);
        out.push_str(MASKED);
        shown_to = end;
    }
    out.push_str(&text[shown_to..]);
    out
}

/// Does `text` hold a newline outside `ranges` (sorted by start; they may
/// overlap, and may reach past `text`, a start of a longer text)?
fn newline_outside(text: &str, ranges: &[std::ops::Range<usize>]) -> bool {
    let mut from = 0;
    for range in ranges {
        let start = range.start.min(text.len());
        if start > from && text[from..start].contains('\n') {
            return true;
        }
        from = from.max(range.end).min(text.len());
    }
    text[from..].contains('\n')
}

/// Does `text`, which is not only newlines, lie wholly inside one of
/// `secrets` — a piece of a value with neither of its ends, as a tail the
/// ring cut inside a value and that ends inside it, or at one of its
/// newlines, reads? Judged on the text as read, before a first line is
/// dropped or trailing newlines trimmed: a first line that is not the end
/// of a line of the value, or a newline where the value goes on, is not the
/// value written. A read that stopped between a `\r` and its `\n` leaves a
/// lone `\r`, read as the start of the value's newline ([`is_cut_start`]).
fn lies_inside_a_value(text: &str, secrets: &[String]) -> bool {
    if text.trim_end_matches('\n').is_empty() {
        return false;
    }
    let cut_newline = text.strip_suffix('\r').map(|before| format!("{before}\n"));
    secrets.iter().any(|secret| {
        secret.contains(text)
            || cut_newline
                .as_deref()
                .is_some_and(|read| secret.contains(read))
    })
}

/// The byte ranges of `text` from `from` on that one scan masks: at each
/// place the longest value that begins there; and, where no whole value
/// begins, the rest of `text` when it is a proper start of a value
/// ([`is_cut_start`]). A text whose last line is unterminated may end in
/// any start of a value; one that ends with a newline, only in a start that
/// ends at one of the value's own newlines.
fn value_ranges(text: &str, secrets: &[String], from: usize) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut at = from;
    'scan: while let Some(c) = text[at..].chars().next() {
        let rest = &text[at..];
        for secret in secrets {
            if !secret.is_empty() && rest.starts_with(secret.as_str()) {
                ranges.push(at..at + secret.len());
                at += secret.len();
                continue 'scan;
            }
        }
        if secrets.iter().any(|secret| is_cut_start(secret, rest)) {
            ranges.push(at..text.len());
            break;
        }
        at += c.len_utf8();
    }
    ranges
}

/// Is `rest` (the rest of the text) a proper start of `secret`: the tail
/// read before its server finished writing it — at the settle's bound,
/// between two reads of the pipe, or ended by a respawn or the reap's
/// bound rather than by the pipe's end? A read that stopped between a
/// `\r` and its `\n` leaves a lone `\r` that step 1 did not read as `\n`:
/// it is read as the start of the value's newline.
fn is_cut_start(secret: &str, rest: &str) -> bool {
    if secret.len() > rest.len() && secret.starts_with(rest) {
        return true;
    }
    rest.strip_suffix('\r').is_some_and(|before| {
        secret
            .strip_prefix(before)
            .is_some_and(|after| after.starts_with('\n'))
    })
}

/// The length of every rest of a `secrets` value holding a newline — what
/// follows one of its newlines — that `text` begins with.
fn line_remainders(text: &str, secrets: &[String]) -> Vec<usize> {
    let mut lens: Vec<usize> = secrets
        .iter()
        .flat_map(|secret| {
            secret
                .match_indices('\n')
                .map(move |(at, _)| &secret[at + 1..])
        })
        .filter(|rest| !rest.is_empty() && text.starts_with(rest))
        .map(str::len)
        .collect();
    lens.sort_unstable();
    lens.dedup();
    lens
}

/// The length of every proper end of a `secrets` value that `text` begins
/// with: where a value the ring cut at its head may end.
fn head_fragments(text: &str, secrets: &[String]) -> Vec<usize> {
    let mut ends: Vec<usize> = secrets
        .iter()
        .flat_map(|secret| {
            secret
                .char_indices()
                .skip(1)
                .map(move |(at, _)| &secret[at..])
        })
        .filter(|end| text.starts_with(end))
        .map(str::len)
        .collect();
    ends.sort_unstable();
    ends.dedup();
    ends
}

/// The name a credential travels under: one of [`looks_like_a_secret`]'s five
/// markers, or a header that carries one.
const NAME: &str =
    r"[A-Za-z0-9_.-]*(?:token|key|secret|password|credential|authorization|cookie)[A-Za-z0-9_.-]*";

/// The generic shapes, in the order they run: each replacement keeps the
/// name and the separator and masks the value.
static SHAPES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let shape = |pattern: &str, replacement: &'static str| {
        (
            Regex::new(pattern).expect("a valid redaction pattern"),
            replacement,
        )
    };
    vec![
        // A URL's userinfo: `scheme://user:password@host`.
        shape(r"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/?#@]+@", "${1}<masked>@"),
        // `"name": "value"` (JSON) for a secret-like name.
        shape(
            &format!(r#"(?i)("{NAME}"\s*:\s*")(?:[^"\\\n]|\\.)*(")"#),
            "${1}<masked>${2}",
        ),
        // An `Authorization` or `Cookie` header: its whole value.
        shape(
            r"(?i)\b((?:proxy-)?authorization|set-cookie|cookie)(\s*[:=]\s*)[^\n,;}]*",
            "${1}${2}<masked>",
        ),
        // A bearer token on its own.
        shape(
            r"(?i)\b(bearer)(\s+)[A-Za-z0-9._~+/=-]+",
            "${1}${2}<masked>",
        ),
        // `name=value` for a secret-like name (the quoted `"name": "value"`
        // is the JSON shape above).
        shape(
            &format!(r#"(?i)\b({NAME})(\s*=\s*)(["']?)[^\s"',;&]+"#),
            "${1}${2}${3}<masked>",
        ),
        // A query parameter that carries access.
        shape(
            &format!(
                r#"(?i)([?&](?:{NAME}|[A-Za-z0-9_.-]*(?:auth|sig|signature)[A-Za-z0-9_.-]*)=)[^&\s#"']+"#
            ),
            "${1}<masked>",
        ),
    ]
});

fn mask_shapes(text: &str) -> String {
    let mut text = text.to_string();
    for (pattern, replacement) in SHAPES.iter() {
        if let std::borrow::Cow::Owned(replaced) = pattern.replace_all(&text, *replacement) {
            text = replaced;
        }
    }
    text
}

/// `text` as escaped pieces — each `<masked>` one piece, each other character
/// one, escaped by [`escape_hidden`] — the last ones that fit in `max_bytes`,
/// and whether any were left out.
fn last_pieces(text: &str, max_bytes: usize) -> (Vec<String>, bool) {
    let mut pieces: Vec<String> = Vec::new();
    for (i, segment) in text.split(MASKED).enumerate() {
        if i > 0 {
            pieces.push(MASKED.to_string());
        }
        let mut buf = [0u8; 4];
        pieces.extend(
            segment
                .chars()
                .map(|c| escape_hidden(c.encode_utf8(&mut buf)).text),
        );
    }
    let mut total = 0;
    let mut start = pieces.len();
    while start > 0 && total + pieces[start - 1].len() <= max_bytes {
        start -= 1;
        total += pieces[start].len();
    }
    let cut = start > 0;
    (pieces.split_off(start), cut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(bytes: &[u8], truncated: bool) -> TailSnapshot {
        TailSnapshot {
            bytes: bytes.to_vec(),
            truncated,
            ended: true,
        }
    }

    fn tail_of(detail: &str) -> &str {
        detail
            .split_once(&format!("\n{HEADING} "))
            .and_then(|(_, rest)| rest.split_once("redacted):\n"))
            .map(|(_, tail)| tail)
            .unwrap_or_else(|| panic!("no tail in {detail:?}"))
    }

    #[test]
    fn a_spawn_that_wrote_nothing_leaves_the_detail_as_it_was() {
        let detail = compose_detail("connect: closed", &snapshot(b"", false), &[]);
        assert_eq!(detail, "connect: closed");
        let detail = compose_detail("connect: closed", &snapshot(b"\n\n", false), &[]);
        assert_eq!(detail, "connect: closed", "nothing but newlines is nothing");
    }

    #[test]
    fn the_detail_is_the_base_then_a_heading_then_the_tail_last() {
        let detail = compose_detail("connect: closed", &snapshot(b"one\r\ntwo\n", false), &[]);
        assert_eq!(
            detail,
            "connect: closed\nserver stderr (last 7 bytes, redacted):\none\ntwo"
        );
    }

    #[test]
    fn the_load_s_values_are_masked_longest_first() {
        let secrets = vec!["abcdef".to_string(), "abc".to_string()];
        let detail = compose_detail(
            "x",
            &snapshot(b"v=abcdef and abc and ab\n", false),
            &secrets,
        );
        assert_eq!(tail_of(&detail), "v=<masked> and <masked> and ab");
    }

    #[test]
    fn the_generic_shapes_are_masked() {
        let cases = [
            (
                "fetch https://user:pa55@host/x failed",
                "fetch https://<masked>@host/x failed",
            ),
            (
                "Authorization: Basic dXNlcjpwYXNz",
                "Authorization: <masked>",
            ),
            (
                "sent bearer abc.def-123 upstream",
                "sent bearer <masked> upstream",
            ),
            ("API_KEY=sk-live-1 retry", "API_KEY=<masked> retry"),
            ("password=hunter2", "password=<masked>"),
            (
                r#"{"client_secret": "s3\"cr", "ok": 1}"#,
                r#"{"client_secret": "<masked>", "ok": 1}"#,
            ),
            (
                "GET /cb?code=1&sig=abcd&x=2",
                "GET /cb?code=1&sig=<masked>&x=2",
            ),
            (
                "GET /cb?access_token=abcd&x=2",
                "GET /cb?access_token=<masked>&x=2",
            ),
            (
                "Cookie: session=1; theme=dark",
                "Cookie: <masked>; theme=dark",
            ),
            // An unquoted `name: value` is not a shape (the hint names
            // `key=value` and `"key":"value"`), so neither is masked here
            // (an owner item, N-MCP-5).
            ("password: hunter2", "password: hunter2"),
            ("KeyError: 'OPENAI_API_KEY'", "KeyError: 'OPENAI_API_KEY'"),
        ];
        for (raw, shown) in cases {
            let detail = compose_detail("x", &snapshot(raw.as_bytes(), false), &[]);
            assert_eq!(tail_of(&detail), shown, "{raw}");
        }
    }

    #[test]
    fn hidden_characters_are_escaped_once() {
        let detail = compose_detail(
            "x",
            &snapshot("a\u{202E}b\u{200B}c\u{1b}[31m\n".as_bytes(), false),
            &[],
        );
        assert_eq!(tail_of(&detail), "a<U+202E>b<U+200B>c<U+001B>[31m");
        assert!(!detail.contains('\u{202E}') && !detail.contains('\u{200B}'));
    }

    #[test]
    fn the_tail_fits_the_bound_and_ends_with_the_last_line() {
        let mut raw = Vec::new();
        for i in 0..400 {
            raw.extend(format!("line {i:03} of the log\n").as_bytes());
        }
        let detail = compose_detail("x", &snapshot(&raw, true), &[]);
        let tail = tail_of(&detail);
        assert!(tail.len() <= TAIL_BYTES, "{}", tail.len());
        assert!(tail.ends_with("line 399 of the log"), "{tail}");
        assert!(
            tail.starts_with("line "),
            "a whole first line: {}",
            &tail[..30]
        );
        assert!(detail.contains(&format!("(last {} bytes, redacted)", tail.len())));
    }

    /// Escapes and masks grow the text past the bound: the cut is taken on
    /// escaped text, on whole pieces, never inside an escape or a mask.
    #[test]
    fn a_cut_never_lands_inside_an_escape_or_a_mask() {
        let secrets = vec!["k1".to_string()];
        let unit = "k1\u{200B}";
        let raw = unit.repeat(4096 / unit.len() + 1);
        let detail = compose_detail("x", &snapshot(raw.as_bytes(), true), &secrets);
        let tail = tail_of(&detail);
        assert!(tail.len() <= TAIL_BYTES);
        assert!(
            tail.starts_with("<masked>") || tail.starts_with("<U+200B>"),
            "{}",
            &tail[..20]
        );
        assert!(tail.ends_with("<U+200B>"));
        assert!(!tail.contains("k1"));
    }

    /// A value the ring cut at its head shows none of its end.
    #[test]
    fn a_value_straddling_the_ring_s_cut_is_masked() {
        let secrets = vec!["tok-0123456789".to_string()];
        let detail = compose_detail("x", &snapshot(b"456789 then more", true), &secrets);
        assert_eq!(tail_of(&detail), "<masked> then more");
        // With a newline the partial first line is dropped instead.
        let detail = compose_detail(
            "x",
            &snapshot(b"456789 partial\nwhole line", true),
            &secrets,
        );
        assert_eq!(tail_of(&detail), "whole line");
    }

    /// A tail read before its server finished writing a value — at the
    /// settle's bound, between two reads, or ended by a respawn — shows none
    /// of its start; a terminated last line is shown as written.
    #[test]
    fn a_value_cut_at_the_tail_s_end_is_masked() {
        let secrets = vec!["tok-0123456789".to_string()];
        for ended in [false, true] {
            let cut = TailSnapshot {
                bytes: b"starting\nusing tok-01234".to_vec(),
                truncated: false,
                ended,
            };
            let detail = compose_detail("x", &cut, &secrets);
            assert_eq!(
                tail_of(&detail),
                "starting\nusing <masked>",
                "ended: {ended}"
            );
        }
        let whole = TailSnapshot {
            bytes: b"a word: tok\n".to_vec(),
            truncated: false,
            ended: false,
        };
        let detail = compose_detail("x", &whole, &secrets);
        assert_eq!(tail_of(&detail), "a word: tok");
        // A shorter value inside the cut start does not hide it from the
        // check: the start is judged on the text as the server wrote it.
        let with_a_short_value = vec!["tok-0123456789".to_string(), "1".to_string()];
        let detail = compose_detail(
            "x",
            &snapshot(b"using tok-01234", false),
            &with_a_short_value,
        );
        assert_eq!(tail_of(&detail), "using <masked>");
        // A whole value that ends the text is masked whole, never read as
        // the start of itself.
        let repetitive = vec!["abcabc".to_string()];
        for (raw, shown) in [
            ("x abcabc", "x <masked>"),
            ("x abcab", "x <masked>"),
            ("x abcabcab", "x <masked><masked>"),
        ] {
            let detail = compose_detail("x", &snapshot(raw.as_bytes(), false), &repetitive);
            assert_eq!(tail_of(&detail), shown, "{raw}");
        }
    }

    /// The end of a value the ring cut at the tail's head is judged on the
    /// text as the server wrote it: a shorter value inside it does not hide
    /// it from the check, and where the head reads two ways — the end of a
    /// value, or a shorter end followed by a whole value — neither shows.
    #[test]
    fn a_value_cut_at_the_tail_s_head_is_judged_on_the_text_as_written() {
        let with_a_short_value = vec!["tok-0123456789".to_string(), "8".to_string()];
        for (raw, shown) in [
            ("56789 then more", "<masked> then more"),
            ("56789 then tok-0123456789", "<masked> then <masked>"),
        ] {
            let detail = compose_detail("x", &snapshot(raw.as_bytes(), true), &with_a_short_value);
            assert_eq!(tail_of(&detail), shown, "{raw}");
        }
        let repetitive = vec!["abcabc".to_string()];
        for (raw, shown) in [
            ("bcabc x", "<masked> x"),
            ("abcabc x", "<masked> x"),
            ("cabcabc x", "<masked> x"),
            ("bcabcabc x", "<masked> x"),
            ("c x abcabc", "<masked> x <masked>"),
        ] {
            let detail = compose_detail("x", &snapshot(raw.as_bytes(), true), &repetitive);
            assert_eq!(tail_of(&detail), shown, "{raw}");
        }
        // Every reading counts, not only the longest end and the plain scan:
        // "x" (the end of "qx") then the whole "yzw" is the one that masks
        // the "w" — "xy" (the end of "qqxy") and the scan's "xyz" stop
        // short of it.
        let overlapping = ["qqxy", "qx", "yzw", "xyz"].map(String::from).to_vec();
        let detail = compose_detail("x", &snapshot(b"xyzw!", true), &overlapping);
        assert_eq!(tail_of(&detail), "<masked>!");
        // A newline inside a value (a PEM key, a JSON credential) is not a
        // whole line: the head is still checked.
        let multi_line = vec!["tok-0123456789".to_string(), "l1\nl2".to_string()];
        let detail = compose_detail("x", &snapshot(b"456789 and l1\nl2 end", true), &multi_line);
        assert_eq!(tail_of(&detail), "<masked> and <masked> end");
        // Without the ring's cut the head is read as it is.
        let detail = compose_detail(
            "x",
            &snapshot(b"56789 then more", false),
            &with_a_short_value,
        );
        assert_eq!(tail_of(&detail), "567<masked>9 then more");
    }

    /// Every case's tail, the ones not shown as expected listed together.
    fn mismatches(secrets: &[String], cases: &[(&str, bool, &str)]) -> Vec<String> {
        cases
            .iter()
            .filter_map(|(raw, truncated, shown)| {
                let detail = compose_detail("x", &snapshot(raw.as_bytes(), *truncated), secrets);
                let tail = tail_of(&detail);
                (tail != *shown).then(|| format!("{raw:?}: shown {tail:?}, expected {shown:?}"))
            })
            .collect()
    }

    /// A value holding a newline (a PEM key, a JSON credential from
    /// `env_from`) that the ring cut at its head shows none of its later
    /// lines: the rest of it the text begins with — at the start, or at the
    /// start of the line after the first when the cut fell inside a line —
    /// is masked whole, so dropping the partial first line leaves nothing of
    /// it.
    #[test]
    fn a_multi_line_value_cut_at_the_tail_s_head_shows_none_of_its_lines() {
        let probe = vec!["l1\nl2\nSECRETPART".to_string()];
        let failures = mismatches(
            &probe,
            &[("l2\nSECRETPART rest\nlast line", true, "last line")],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let three = vec!["aa1\nbb2\ncc3".to_string()];
        let failures = mismatches(
            &three,
            &[
                // Cut after its first newline: the text begins with a whole
                // later line of it.
                ("bb2\ncc3 rest\nlast", true, "last"),
                // Cut after its second newline (green before: the rest is
                // one line, and the first line is dropped).
                ("cc3 rest\nlast", true, "last"),
                // Cut just before a newline of it.
                ("\nbb2\ncc3 rest\nlast", true, "<masked> rest\nlast"),
                // Cut inside its first or its second line: the partial line
                // is dropped and what follows begins with the rest of it.
                ("a1\nbb2\ncc3 rest\nlast", true, "<masked> rest\nlast"),
                ("b2\ncc3 rest\nlast", true, "<masked> rest\nlast"),
                // A later line of it written elsewhere as ordinary text is
                // not the cut remainder: only the anchored one is masked.
                (
                    "bb2\ncc3 rest\nbb2 is a word here\nlast",
                    true,
                    "bb2 is a word here\nlast",
                ),
                (
                    "a1\nbb2\ncc3 rest\nsay cc3 and bb2\nlast",
                    true,
                    "<masked> rest\nsay cc3 and bb2\nlast",
                ),
                // The two places are the text's start and its second line's,
                // as written: a line further on that begins with a later
                // line of it is ordinary text.
                (
                    "bb2\ncc3 rest\ncc3 is a word\nlast",
                    true,
                    "cc3 is a word\nlast",
                ),
                // No line after the rest of it: nothing to drop.
                ("bb2\ncc3 rest", true, "<masked> rest"),
                ("b2\ncc3 rest", true, "<masked> rest"),
                // Regression guard (green before): with no cut at the head
                // the text is read as it is, byte for byte.
                ("bb2\ncc3 rest\nlast", false, "bb2\ncc3 rest\nlast"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // CRLF line ends, in the value or in what the server writes around
        // it: the value is compared as the text is read (`\r\n` as `\n`).
        let crlf = vec!["aa1\r\nbb2\r\ncc3".to_string()];
        let failures = mismatches(
            &crlf,
            &[
                ("bb2\r\ncc3 rest\r\nlast", true, "last"),
                ("a1\r\nbb2\r\ncc3 rest\r\nlast", true, "<masked> rest\nlast"),
                // The ring cut between a `\r` and its `\n`.
                ("\nbb2\r\ncc3 rest\r\nlast", true, "<masked> rest\nlast"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let failures = mismatches(&three, &[("bb2\ncc3 rest\r\nlast", true, "last")]);
        assert!(failures.is_empty(), "{failures:#?}");
        // The cost: a second line that begins with a later line of a value
        // is masked there, however short that line is ("}" of a JSON
        // value) — the over-masking the mask set's owner item names.
        let json = vec!["{\n\"k\": 1\n}".to_string()];
        let failures = mismatches(&json, &[("zz\n} done\nlast", true, "<masked> done\nlast")]);
        assert!(failures.is_empty(), "{failures:#?}");
        // The same cost for a value whose last line is empty: its rest
        // "\n" masks a blank second line.
        let blank_end = vec!["abc\n\n".to_string()];
        let failures = mismatches(
            &blank_end,
            &[("zz\n\nworld\nend\n", true, "<masked>world\nend")],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // Regression guard (green before): a single-line value is treated
        // as it was.
        let single = vec!["tok-0123456789".to_string()];
        let failures = mismatches(
            &single,
            &[
                (
                    "6789 partial\nwhole line tok-0123456789\nlast\n",
                    true,
                    "whole line <masked>\nlast",
                ),
                ("tok-0123456789 x\nlast\n", false, "<masked> x\nlast"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A value holding a newline that the tail ends inside — a tail read
    /// before its server finished writing it — shows none of its earlier
    /// lines, the tail ending inside a line of it or just after one of its
    /// newlines.
    #[test]
    fn a_multi_line_value_cut_at_the_tail_s_end_shows_none_of_its_lines() {
        let three = vec!["aa1\nbb2\ncc3".to_string()];
        let failures = mismatches(
            &three,
            &[
                // Inside a line (green before: an unterminated last line).
                ("start\naa1\nbb", false, "start\n<masked>"),
                ("start\naa", false, "start\n<masked>"),
                // Just after one of its newlines.
                ("start\naa1\nbb2\n", false, "start\n<masked>"),
                ("start\naa1\n", false, "start\n<masked>"),
                // Only the occurrence the text ends with is the cut start.
                (
                    "aa1 is a word\nstart\naa1\n",
                    false,
                    "aa1 is a word\nstart\n<masked>",
                ),
                // Regression guard (green before): the whole value.
                ("start\naa1\nbb2\ncc3\n", false, "start\n<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let crlf = vec!["aa1\r\nbb2\r\ncc3".to_string()];
        let failures = mismatches(
            &crlf,
            &[
                ("start\r\naa1\r\nbb2\r\n", false, "start\n<masked>"),
                ("start\r\naa1\r\nbb", false, "start\n<masked>"),
                // The read stopped between a `\r` and its `\n` (regression
                // guard, green before: the value read raw begins "aa1\r").
                ("start\r\naa1\r", false, "start\n<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // The read stopped between a `\r` and its `\n` of a value with
        // `\n` line ends that the server wrote with `\r\n`.
        let failures = mismatches(&three, &[("start\r\naa1\r", false, "start\n<masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
        // The cost: a tail ending with a line that is a first line of a
        // value loses it ("{" of a JSON value), as a one-character start
        // ending an unterminated line is masked today.
        let json = vec!["{\n\"k\": 1\n}".to_string()];
        let failures = mismatches(&json, &[("config = {\n", false, "config = <masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
        // The same cost for a value whose first line is empty: its start
        // "\n" ends every tail that ends with a newline.
        let blank_start = vec!["\nXYZ".to_string()];
        let failures = mismatches(
            &blank_start,
            &[("hello\nworld\n", false, "hello\nworld<masked>")],
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A value with CRLF line ends is found whole: the text's `\r\n` is read
    /// as `\n` (step 1), and so is the value's.
    #[test]
    fn a_value_with_crlf_line_ends_is_masked_whole() {
        let crlf = vec!["aa1\r\nbb2\r\ncc3".to_string()];
        let failures = mismatches(&crlf, &[("x aa1\r\nbb2\r\ncc3 y\n", false, "x <masked> y")]);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The rest of a multi-line value is found in the shapes such a value
    /// takes: one that ends with its newline (a PEM file read whole), one
    /// with a blank line inside, one with a shorter value inside the rest
    /// (judged on the text as written), and a head the ring cut inside a
    /// character (the first line begins with U+FFFD).
    #[test]
    fn the_rest_of_a_multi_line_value_is_found_in_every_shape() {
        let pem = vec!["aa1\nbb2\ncc3\n".to_string()];
        let failures = mismatches(
            &pem,
            &[
                ("bb2\ncc3\nlast\nend", true, "end"),
                // The rest "cc3\n" is masked with its own newline, so the
                // mask joins the line after it.
                ("b2\ncc3\nlast\nend", true, "<masked>last\nend"),
                ("start\naa1\nbb2\n", false, "start\n<masked>"),
                // Regression guard (green before): the whole value.
                ("start\naa1\nbb2\ncc3\n", false, "start\n<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let blank = vec!["aa1\n\nbb2".to_string()];
        let failures = mismatches(
            &blank,
            &[
                ("\nbb2 rest\nlast", true, "last"),
                ("1\n\nbb2 rest\nlast", true, "<masked> rest\nlast"),
                ("start\naa1\n\n", false, "start\n<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let with_a_short_value = vec!["aa1\nbb2\ncc3".to_string(), "2".to_string()];
        let failures = mismatches(
            &with_a_short_value,
            &[
                ("bb2\ncc3 rest\nlast", true, "last"),
                ("b2\ncc3 rest\nlast", true, "<masked> rest\nlast"),
                ("start\naa1\nbb2\n", false, "start\n<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let wide = vec!["a\u{e9}1\nbb2\ncc3".to_string()];
        let mut raw = vec![0xA9];
        raw.extend_from_slice(b"1\nbb2\ncc3 rest\nlast");
        let detail = compose_detail("x", &snapshot(&raw, true), &wide);
        assert_eq!(tail_of(&detail), "<masked> rest\nlast");
    }

    /// A cut tail of one line that ends with a newline shows none of the end
    /// of a value it begins with: there is no first line for the drop to
    /// take, so the end is masked as in a tail with no newline at all — the
    /// newline is judged on the text the drop sees, trailing newlines
    /// trimmed.
    #[test]
    fn a_cut_tail_of_one_line_ending_with_a_newline_shows_no_value_s_end() {
        let single = vec!["tok-0123456789".to_string()];
        let failures = mismatches(
            &single,
            &[
                ("456789 then more\n", true, "<masked> then more"),
                ("456789 then more\r\n", true, "<masked> then more"),
                ("456789 then more\n\n", true, "<masked> then more"),
                // The cost, as for a cut tail with no newline: a one-line
                // cut tail that begins with a one-character end of a value
                // is masked there.
                ("9 is a digit\n", true, "<masked> is a digit"),
                // Regression guard (green before): with no cut at the head
                // the text is read as it is.
                ("456789 then more\n", false, "456789 then more"),
                // Regression guard (green before): the end check is left
                // alone — a single-line value is not read as cut when the
                // text ends with a newline, head cut or not.
                ("a word: tok\n", true, "a word: tok"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        let three = vec!["aa1\nbb2\ncc3".to_string()];
        let failures = mismatches(
            &three,
            &[
                ("c3 rest\n", true, "<masked> rest"),
                ("c3 rest\r\n\n", true, "<masked> rest"),
                // Regression guard (green before): a masked range that takes
                // in the trailing newline.
                ("x aa1\n", true, "x <masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // Regression guard (green before): a value whose first line is empty
        // ends the text in its start "\n" (the mask set's cost), head cut.
        let blank_start = vec!["\nXYZ".to_string()];
        let failures = mismatches(&blank_start, &[("world\n", true, "world<masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A cut tail lying wholly inside one value — the ring cut its head
    /// inside the value and the tail ends inside it too, or at one of its
    /// own newlines — is masked whole: neither of its ends is an end or a
    /// start of the value, so no other rule finds it. It is judged on the
    /// text as read, before the first line is dropped and before trailing
    /// newlines are trimmed.
    #[test]
    fn a_cut_tail_lying_wholly_inside_a_value_shows_none_of_it() {
        let three = vec!["aa1\nbb2\ncc3".to_string()];
        let failures = mismatches(
            &three,
            &[
                // Cut inside a line, ended inside a line.
                ("bb2\ncc", true, "<masked>"),
                ("a1\nbb2\ncc", true, "<masked>"),
                // A whole middle line of it.
                ("bb2", true, "<masked>"),
                // One line ending with one of its newlines (with the cut
                // tail of one line ending in a newline).
                ("b2\n", true, "<masked>"),
                // The value read with `\n`, the text written with `\r\n`.
                ("bb2\r\ncc", true, "<masked>"),
                // Regression guard (green before): with no cut at the head
                // the text is read as it is.
                ("bb2\ncc", false, "bb2\ncc"),
                // Regression guard (green before): a whole line not in the
                // value is never masked by this rule.
                ("bb2\nzz9 x\ncc", true, "zz9 x\ncc"),
                // Judged before the drop (regression guard, green before):
                // a first line that is not the end of a line of the value
                // is not the value written, so what follows it is ordinary
                // text.
                ("zz\nbb2\ncc", true, "bb2\ncc"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // Ending at one of the value's own newlines: shown before.
        let four = vec!["aa1\nbb2\ncc3\ndd4".to_string()];
        let failures = mismatches(
            &four,
            &[
                ("bb2\ncc3\n", true, "<masked>"),
                ("b2\ncc3\n", true, "<masked>"),
                ("\ncc3\n", true, "<masked>"),
                ("b2\nc", true, "<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // A single-line value longer than the tail on both sides.
        let single = vec!["abcdefgh".to_string()];
        let failures = mismatches(
            &single,
            &[
                ("cde", true, "<masked>"),
                // Regression guard (green before): no cut at the head.
                ("cde", false, "cde"),
                // Judged before the trim (regression guard, green before):
                // a newline where the value goes on is not the value
                // written.
                ("cde\n", true, "cde"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // A value with CRLF line ends, read as the text is.
        let crlf = vec!["aa1\r\nbb2\r\ncc3\r\ndd4".to_string()];
        let failures = mismatches(
            &crlf,
            &[
                ("bb2\r\ncc", true, "<masked>"),
                ("bb2\ncc", true, "<masked>"),
            ],
        );
        assert!(failures.is_empty(), "{failures:#?}");
        // Two values, the tail inside the longer only; and a shorter value
        // inside the tail, which does not hide it (judged before the
        // values are masked).
        for values in [
            ["abcdefgh", "aa1\nbb2\ncc3\ndd4"],
            ["aa1\nbb2\ncc3\ndd4", "2"],
        ] {
            let values = values.map(String::from).to_vec();
            let failures = mismatches(&values, &[("b2\ncc", true, "<masked>")]);
            assert!(failures.is_empty(), "{values:?}: {failures:#?}");
        }
        // A cut text of newlines only is still nothing to show (regression
        // guard, green before), even inside a value with a blank line.
        let blank = vec!["aa1\n\nbb2".to_string()];
        let detail = compose_detail("x", &snapshot(b"\n", true), &blank);
        assert_eq!(detail, "x");
        // The cost: an ordinary cut tail that is a piece of a value is
        // masked whole (in practice a cut tail is the ring's whole 4 KB).
        let json = vec!["{\n\"k\": 1\n}".to_string()];
        let failures = mismatches(&json, &[(": 1", true, "<masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
        // Regression guards (green before): a cut tail that is a whole
        // value, or an end of one, was masked already.
        let failures = mismatches(&json, &[("}", true, "<masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
        let one = vec!["1".to_string()];
        let failures = mismatches(&one, &[("1", true, "<masked>")]);
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A cut tail lying wholly inside a value whose read stopped between a
    /// `\r` and its `\n`: the lone `\r` is read as the start of the value's
    /// newline, as at the end of any text (step 1's reading at a cut).
    #[test]
    fn a_cut_tail_inside_a_value_read_stopped_after_a_cr_shows_none_of_it() {
        for value in ["aa1\nbb2\ncc3\ndd4", "aa1\r\nbb2\r\ncc3\r\ndd4"] {
            let values = vec![value.to_string()];
            let failures = mismatches(&values, &[("bb2\r\ncc3\r", true, "<masked>")]);
            assert!(failures.is_empty(), "{value:?}: {failures:#?}");
        }
    }

    /// A value straddling the output's cut: masking comes first, so the cut
    /// sees only the mask, whole or not at all.
    #[test]
    fn a_value_straddling_the_output_s_cut_is_masked_whole() {
        let secret = "s".repeat(40);
        let mut raw = secret.clone();
        raw.push_str(&"y".repeat(TAIL_BYTES - 20));
        let detail = compose_detail("x", &snapshot(raw.as_bytes(), false), &[secret]);
        let tail = tail_of(&detail);
        assert!(!tail.contains("ss"), "no part of the value");
        assert!(tail.starts_with("<masked>"), "it fits once masked");
    }

    #[test]
    fn the_secret_name_test_is_the_five_markers() {
        for name in [
            "GITHUB_TOKEN",
            "api_key",
            "MySecret",
            "DB_PASSWORD",
            "Credentials",
        ] {
            assert!(looks_like_a_secret(name), "{name}");
        }
        for name in ["PATH", "HOME", "LANG", "DEBUG"] {
            assert!(!looks_like_a_secret(name), "{name}");
        }
    }
}
