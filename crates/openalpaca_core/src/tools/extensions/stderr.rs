//! A failed stdio MCP server's stderr, kept in its row's `detail` (N-MCP-5).
//!
//! `openalpaca_mcp` keeps the last bytes each spawn wrote to its stderr
//! ([`StderrTail`]), raw. This is the one place they become text a person or a
//! model is shown, and only in the `detail` of a `Failed` row — never in an
//! `McpError`'s text, never in the connection state's reason, never in a log
//! line of their own. What is shown is, in this order:
//!
//! 1. decoded lossily, `\r\n` read as `\n`, once the pieces of a character
//!    a cut left are left out: the continuation bytes a cut head begins
//!    with, and a last character's first bytes without its last;
//! 2. every value the load resolved for the server — its `env` and `env_from`
//!    values, credential-bearing arguments, the daemon's own variables with
//!    secret-like names — masked as [`MASKED`], a value's `\r\n` read as
//!    `\n` as the text's is: every occurrence, looked for at every place,
//!    inside another one too, the masked bytes their union (two
//!    occurrences that overlap read `<masked><masked>`); a value holding a
//!    `\r` not followed by `\n` is looked for in the text as written too,
//!    for where the server writes that `\r` before a `\n` (the newline read
//!    from them is shown: its `\n` is no part of the value);
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
//!    one of its newlines, looked for at every place, inside a whole value
//!    too — all judged in step 2's scan, on the text as written, so a
//!    shorter value inside a part cut at either end, or one that ends
//!    inside a cut start, is no reason to stop looking;
//! 4. the generic shapes of a credential masked: a URL's userinfo, a bearer
//!    token, an `Authorization` or `Cookie` header's value, a `key=value` or
//!    `"key": "value"` whose name looks like a secret, a sensitive query
//!    parameter;
//! 5. cut from the end to [`TAIL_BYTES`] of escaped text, on a whole piece —
//!    never inside a `<masked>` or a `<U+XXXX>` — so the last line is kept;
//! 6. escaped once, here, with [`escape_hidden`] (D19 (a)), so every surface
//!    that shows the detail shows `<U+XXXX>` for a hidden character.
//!
//! What the steps aim at: what is shown is a window on the bytes the
//! server wrote, and no byte of an occurrence of a value in them is shown,
//! whether the occurrence lies inside the window or crosses one of its
//! cuts. That is claimed only as far as an oracle sampled it, with one
//! named exception. The test
//! `the_oracle_s_sample_shows_no_value_byte_outside_class_nl` reads
//! 216,000 generated windows of the fixed seed `0x005E_ED0F_7A11_0001`
//! (9,000 streams, about 5 s in a debug build; its doc lists the classes
//! its generator builds); when it was written, five seeds of 20,000
//! streams (480,000 windows each, about 11 s a seed in a debug build)
//! showed no byte of a value but in class NL: a window the ring cut at
//! its head that holds only newline bytes inside a run of a value's
//! newlines can show one of them (values `y2y\r\n\r\n\r\n1` and `\n1`,
//! window `\n\r\n\r\n`, shown `\n<masked>`), as it did before; it is
//! pinned as shown today. Without the test's NL exemption the official
//! seed is green at 6,000 streams and red at 9,000 and 20,000. A
//! fixed-seed sample, not a proof.
//! Outside it: the generic shapes (step 4), hidden characters, a tail
//! longer than [`TAIL_BYTES`], and which values are in the mask set.
//!
//! A value holding a `\r` not followed by `\n` (as an `env_from` value from
//! a file with CRLF line ends can) is not the lone `\r` a read stopped
//! between a `\r` and its `\n` leaves at a cut, which step 3 reads as the
//! start of a value's newline; both are kept.
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
    let partial_head = tail.truncated;
    let (text, ranges) = read_and_find(tail, secrets);
    let text = masked(&text, ranges);
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

/// Steps 1 to 3: the text as read, and the ranges of it masked.
fn read_and_find(tail: &TailSnapshot, secrets: &[String]) -> (String, Vec<std::ops::Range<usize>>) {
    let written = String::from_utf8_lossy(whole_characters(&tail.bytes, tail.truncated));
    let text = written.replace("\r\n", "\n");
    // The values as the text is read: a value written with `\r\n` line
    // ends is found in the text only with `\n` ones.
    // A value holding a `\r` not followed by `\n` is looked for in the
    // text as written too: where the server writes that `\r` before a
    // `\n`, the text as read holds no `\r` there.
    let written = Written {
        text: &written,
        secrets: &with_a_lone_cr(secrets),
    };
    let ranges = masked_ranges(&text, &as_read(secrets), &written, tail.truncated);
    (text, ranges)
}

/// The text as written and the values to look for in it, the ones holding
/// a lone `\r` ([`with_a_lone_cr`]).
struct Written<'a> {
    text: &'a str,
    secrets: &'a [String],
}

/// The values of `secrets` that hold a `\r` not followed by `\n`, longest
/// first.
fn with_a_lone_cr(secrets: &[String]) -> Vec<String> {
    let mut lone: Vec<String> = secrets
        .iter()
        .filter(|secret| {
            secret
                .match_indices('\r')
                .any(|(at, _)| !secret[at + 1..].starts_with('\n'))
        })
        .cloned()
        .collect();
    lone.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    lone.dedup();
    lone
}

/// `ranges` of the text as `written` as ranges of the text as read
/// (`\r\n` read as `\n`). The `\r` of a `\r\n` is in none: the newline
/// read from it shows only its `\n`, so a range ending at that `\r` stops
/// before the newline, and a range of nothing else is left out.
fn as_read_ranges(
    written: &str,
    ranges: Vec<std::ops::Range<usize>>,
) -> Vec<std::ops::Range<usize>> {
    let bytes = written.as_bytes();
    // Where each character of `written` begins in the text as read; none
    // for the `\r` of a `\r\n`.
    let mut read_at = vec![None; written.len()];
    let mut read = 0;
    for (at, c) in written.char_indices() {
        if c == '\r' && bytes.get(at + 1) == Some(&b'\n') {
            continue;
        }
        read_at[at] = Some(read);
        read += c.len_utf8();
    }
    ranges
        .into_iter()
        .filter_map(|range| {
            let mut shown = written[range.clone()].char_indices().filter_map(|(at, c)| {
                read_at[range.start + at].map(|read| read..read + c.len_utf8())
            });
            let first = shown.next()?;
            let end = shown.next_back().map_or(first.end, |last| last.end);
            Some(first.start..end)
        })
        .collect()
}

/// `bytes` without the pieces of a character a cut left: with `cut_head`
/// (the ring cut the tail's head), the continuation bytes they begin with
/// (at most three); and at the end, a character's first bytes without its
/// last (a read stopped inside it). Every later rule then sees whole
/// characters, so a value cut inside one is found as cut, never read as a
/// U+FFFD that is in no value.
fn whole_characters(bytes: &[u8], cut_head: bool) -> &[u8] {
    let continuation = |byte: &u8| (0x80..0xC0).contains(byte);
    let mut bytes = bytes;
    if cut_head {
        let left = bytes.iter().take(3).take_while(|b| continuation(b)).count();
        bytes = &bytes[left..];
    }
    // The last character's first byte, at most three bytes back.
    if let Some(back) = bytes.iter().rev().take(4).position(|b| !continuation(b)) {
        let first = bytes.len() - 1 - back;
        let len = match bytes[first] {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xFF => 4,
            _ => 1,
        };
        if first + len > bytes.len() {
            bytes = &bytes[..first];
        }
    }
    bytes
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

/// A rule of [`masked_ranges`]: the ranges it finds in a text with its
/// values.
type Rule<'a> = dyn Fn(&str, &[String]) -> Vec<std::ops::Range<usize>> + 'a;

/// The ranges of `text` masked: every occurrence of a `secrets` value and
/// every start of one that `text` ends with, at every place
/// ([`Places::ranges_from`]); with `cut_head` (the ring cut the tail's
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
///
/// Each rule runs on `text` with `secrets` (as read), and on the text as
/// [`Written`] with its values, the ranges found there mapped to `text`
/// ([`as_read_ranges`]); whether a newline is left outside the masked
/// values is judged on the ranges of both.
fn masked_ranges(
    text: &str,
    secrets: &[String],
    written: &Written<'_>,
    cut_head: bool,
) -> Vec<std::ops::Range<usize>> {
    if secrets.is_empty() {
        return Vec::new();
    }
    // A rule run on the text as read, and on the text as written for the
    // values holding a lone `\r`, its ranges mapped to the text as read.
    let both = |rule: &Rule<'_>| {
        let mut ranges = rule(text, secrets);
        if !written.secrets.is_empty() {
            let found = rule(written.text, written.secrets);
            ranges.extend(as_read_ranges(written.text, found));
        }
        ranges.sort_by_key(|range| (range.start, range.end));
        ranges
    };
    // A tail lying inside a value is found as read for every value: a
    // piece of a value as written is a piece of it as read, a lone `\r` at
    // its end read as the start of a newline.
    if cut_head && lies_inside_a_value(text, secrets) {
        return std::iter::once(0..text.len()).collect();
    }
    let mut ranges = both(&|text, secrets| {
        let mut places = Places::new(text, secrets);
        let mut ranges = places.ranges_from(0);
        if cut_head {
            let second_line = text.find('\n').map(|newline| newline + 1);
            for anchor in std::iter::once(0).chain(second_line) {
                for len in line_remainders(&text[anchor..], secrets) {
                    ranges.push(anchor..anchor + len);
                    ranges.extend(places.ranges_from(anchor + len));
                }
            }
        }
        ranges
    });
    // Judged on the text the drop sees: trailing newlines are trimmed
    // before it, so they are no whole line.
    if cut_head && !newline_outside(text.trim_end_matches('\n'), &ranges) {
        ranges.extend(both(&|text, secrets| {
            let mut places = Places::new(text, secrets);
            let mut ranges = Vec::new();
            for end in head_fragments(text, secrets) {
                ranges.push(0..end);
                ranges.extend(places.ranges_from(end));
            }
            ranges
        }));
        ranges.sort_by_key(|range| (range.start, range.end));
    }
    ranges
}

/// `text` with each of `ranges` (sorted by start) shown as [`MASKED`],
/// ranges that overlap as one.
fn masked(text: &str, ranges: Vec<std::ops::Range<usize>>) -> String {
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

/// The places of `text` a scan visits, each place's longest whole value
/// and whether the rest of `text` is a cut start there, each found once
/// and reused by every scan ([`Places::ranges_from`]) over the same text
/// and values.
struct Places<'a> {
    text: &'a str,
    secrets: &'a [String],
    seen: Vec<Option<(Option<usize>, bool)>>,
}

impl<'a> Places<'a> {
    fn new(text: &'a str, secrets: &'a [String]) -> Self {
        Self {
            text,
            secrets,
            seen: vec![None; text.len() + 1],
        }
    }

    fn at(&mut self, at: usize) -> (Option<usize>, bool) {
        if let Some(found) = self.seen[at] {
            return found;
        }
        let rest = &self.text[at..];
        let whole = self
            .secrets
            .iter()
            .find(|secret| !secret.is_empty() && rest.starts_with(secret.as_str()))
            .map(|secret| at + secret.len());
        let cut = self.secrets.iter().any(|secret| is_cut_start(secret, rest));
        self.seen[at] = Some((whole, cut));
        (whole, cut)
    }

    /// The byte ranges of the text from `from` on that one scan masks,
    /// tried at every place, inside a value already found too: the longest
    /// value that begins there; and the rest of the text when it is a proper
    /// start of a value ([`is_cut_start`]). A text whose last line is
    /// unterminated may end in any start of a value; one that ends with a
    /// newline, only in a start that ends at one of the value's own
    /// newlines.
    ///
    /// What a place holds depends only on the text, the values and the
    /// place, so it is found once ([`Places::at`]), and every scan over the
    /// same text and values (from 0, after each rest of a line, after each
    /// end of a value) reuses it: the ranges a scan that looked again would
    /// find, without its cost.
    ///
    /// The masked bytes are the union of what is found. A range found starts
    /// no earlier than the end of those found before it, so masks that only
    /// touch stay two: two occurrences that overlap read `<masked><masked>`.
    fn ranges_from(&mut self, from: usize) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let mut found_to = from;
        let mut at = from;
        while let Some(c) = self.text[at..].chars().next() {
            let (whole, cut) = self.at(at);
            if let Some(end) = whole
                && end > found_to
            {
                ranges.push(at.max(found_to)..end);
                found_to = end;
            }
            if cut {
                if self.text.len() > found_to {
                    ranges.push(at.max(found_to)..self.text.len());
                }
                break;
            }
            at += c.len_utf8();
        }
        ranges
    }
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

    /// The value pairs the oracle's first run found shown in part (none of
    /// them needs a long tail): F, a cut inside a character of a value; G,
    /// a whole shorter value that is a prefix of a longer one hiding the
    /// longer one's cut start; the overlap, a whole value ending inside a
    /// cut value's start; WW, two whole occurrences that overlap, in a tail
    /// with no cut. Two masks that only touch stay two (the masked bytes
    /// are the union).
    #[test]
    fn values_are_masked_where_occurrences_meet_or_a_character_is_cut() {
        let cut = |text: &str, byte: &[u8]| [text.as_bytes(), byte].concat();
        let cases: [(&[&str], Vec<u8>, bool, &str); 14] = [
            // F: the ring cut inside the value's "\u{e9}" (C3 A9): the A9
            // is left at the head.
            (
                &["x\u{e9}yz"],
                [&[0xA9][..], b"yz rest"].concat(),
                true,
                "<masked> rest",
            ),
            // F: the read stopped inside it: the C3 is left at the end.
            (
                &["x\u{e9}yz"],
                cut("using x", &[0xC3]),
                false,
                "using <masked>",
            ),
            // F: a 4-byte character cut after its third byte, at the end.
            (
                &["k\u{1F600}9"],
                cut("a k", &"\u{1F600}".as_bytes()[..3]),
                false,
                "a <masked>",
            ),
            // G: "1" whole, then the rest of "1a2b3c4d5e"'s cut start.
            (
                &["1a2b3c4d5e", "1"],
                b"token is 1a2b3".to_vec(),
                false,
                "token is <masked><masked>",
            ),
            // The overlap: a whole value ending inside a cut value's start.
            (
                &["is 1", "1a2b3c"],
                b"token is 1a2b".to_vec(),
                false,
                "token <masked><masked>",
            ),
            (
                &["yz1", "1a2b3c"],
                b"say xyz1a2b".to_vec(),
                false,
                "say x<masked><masked>",
            ),
            // WW: two whole occurrences that overlap, of one value or of
            // two, in a tail with no cut.
            (&["33"], b"333b".to_vec(), false, "<masked><masked>b"),
            (&["x2", "2y"], b"x2y".to_vec(), false, "<masked><masked>"),
            (&["yb", "b1"], b"zyb1".to_vec(), false, "z<masked><masked>"),
            // Regression guards (green before): occurrences that only touch
            // are two masks, and an uncut tail with no overlap is as it was.
            (&["yb", "b1"], b"ybb1".to_vec(), false, "<masked><masked>"),
            (&["33"], b"33 b 33".to_vec(), false, "<masked> b <masked>"),
            // The cost of F's trim: a stray first byte of a character that
            // ends a tail is left out, where it read as U+FFFD; in a cut
            // tail that leaves one line, which is shown, where the first
            // line was dropped and the U+FFFD shown.
            (&[], cut("done", &[0xC3]), false, "done"),
            (&[], cut("abc\n", &[0xC3]), true, "abc"),
            // The cost of G at a cut head: the whole shorter value and the
            // longer one's cut start leave no newline outside the masks,
            // so the partial first line's `a` is shown where the drop took
            // it (and showed the `x` of the cut value).
            (
                &["1\nx2", "1"],
                b"a1\nx".to_vec(),
                true,
                "a<masked><masked>",
            ),
        ];
        let failures: Vec<String> = cases
            .iter()
            .filter_map(|(values, raw, truncated, shown)| {
                let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
                let detail = compose_detail("x", &snapshot(raw, *truncated), &values);
                let tail = tail_of(&detail);
                (tail != *shown)
                    .then(|| format!("{values:?} {raw:?}: shown {tail:?}, expected {shown:?}"))
            })
            .collect();
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// A value holding a `\r` not followed by `\n` (one read from a file
    /// with CRLF line ends, its `\n` trimmed) is found as the server wrote
    /// it: written before a `\n`, its `\r` is part of a `\r\n` that the
    /// text as read holds as a `\n`, so it is looked for in the text as
    /// written too. The newline after it is shown: its `\n` is no part of
    /// the value.
    #[test]
    fn a_value_holding_a_lone_cr_is_found_as_the_server_wrote_it() {
        let mut failures = mismatches(
            &["32\r".to_string()],
            &[
                ("z cz  32\r32\r\n", false, "z cz  <masked><masked>"),
                (
                    "start\nusing 32\r\nnext",
                    false,
                    "start\nusing <masked>\nnext",
                ),
                // Regression guards (green before): a lone `\r` in the text
                // is found as read, and a `\n` with no `\r` before it is no
                // part of the value.
                ("x 32\rz\n", false, "x <masked>z"),
                ("x 32\n", false, "x 32"),
            ],
        );
        failures.extend(mismatches(
            &["y 2a\r".to_string()],
            &[
                ("xy 2a\r\n", false, "x<masked>"),
                // The ring cut the head inside it, and the drop has no line
                // to take.
                ("a\r\n", true, "<masked>"),
            ],
        ));
        // Regression guard (green before): a value of a `\r` alone shows no
        // byte of itself in a `\r\n`, and masks nothing there.
        failures.extend(mismatches(
            &["\r".to_string()],
            &[("a\r\nb", false, "a\nb")],
        ));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// KNOWN LEAK, outside what the oracle holds (class NL): a window the
    /// ring cut at its head that holds only newline bytes inside a run of a
    /// value's newlines shows one of them today. The "only newlines"
    /// exclusion of [`lies_inside_a_value`] (df4cdbd8) lets it through, a
    /// second value beginning with a newline masks the last one as a cut
    /// start, and the first-line drop takes only the first. Shown before
    /// this change too. Pinned as shown today, with the oracle's predicate
    /// for it, true on these windows and false on their near misses.
    #[test]
    fn a_head_cut_window_of_a_value_s_newlines_shows_one_of_them_today() {
        // Each case: the values, a stream holding the window, the window.
        let cases: [(&[&str], &[u8], std::ops::Range<usize>); 3] = [
            (
                &["\n3a 2\nxy31", "cxa\r\n\r\n\r\n2"],
                b"cxa\r\n\r\n\r\n2",
                4..9,
            ),
            (&["11\n\n\ny3 ", "\ny3  \nyc"], b"11\n\n\ny3 ", 2..5),
            (&["y2y\r\n\r\n\r\n1", "\n1"], b"y2y\r\n\r\n\r\n1", 4..9),
        ];
        let mut failures = Vec::new();
        for (values, stream, window) in cases {
            let values: Vec<String> = values.iter().map(|v| v.to_string()).collect();
            let occurrences = oracle_occurrences(stream, &values);
            let detail = compose_detail("x", &snapshot(&stream[window.clone()], true), &values);
            if tail_of(&detail) != "\n<masked>" {
                failures.push(format!("{values:?} {window:?}: shown {detail:?}"));
            }
            let (head, end) = (window.start, window.end);
            // Uncut, and one byte that is not a newline taken in.
            let misses = [(0, end), (head, end + 1)];
            if !oracle_nl_window(stream, &occurrences, head, end)
                || misses
                    .iter()
                    .any(|(h, e)| oracle_nl_window(stream, &occurrences, *h, *e))
            {
                failures.push(format!("{values:?} {window:?}: the predicate"));
            }
            if oracle_nl_window(stream, &[], head, end) {
                failures.push(format!("{values:?} {window:?}: no occurrence"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// The costs of the union and of the written reading, pinned (more of
    /// kinds already pinned, larger now): a value's start ending an uncut
    /// tail is masked where a shorter whole value hid it, as at 0cd631ab;
    /// masks that only touch are shown apart, so a run of a value that
    /// overlaps itself reads one mask per place after its first; and in a
    /// cut tail, a value holding a newline and a lone `\r` whose mask now
    /// takes the head's first newline leaves no first line to drop, so the
    /// ring's partial first line (ordinary text) is shown.
    #[test]
    fn the_union_s_and_the_written_reading_s_costs_are_pinned() {
        let mut failures = mismatches(
            &["1a2b3c4d5e".to_string(), "1".to_string()],
            &[("token is 1a2b3", false, "token is <masked><masked>")],
        );
        failures.extend(mismatches(
            &["aaaaa".to_string()],
            &[
                (&"a".repeat(12), false, &MASKED.repeat(8)),
                ("aaaaa x", false, "<masked> x"),
            ],
        ));
        failures.extend(mismatches(
            &["k\nm\r".to_string()],
            &[
                ("zz k\nm\r\n", true, "zz <masked>"),
                // With a line after it there is a first line to drop.
                ("zz k\nm\r\nab\n", true, "ab"),
            ],
        ));
        // Regression guard (green before): what a place holds is found
        // once and reused by every later scan, never forgotten, so
        // ranges that overlap are still shown as one mask.
        failures.extend(mismatches(
            &["2", "ya\r\n\r\n", "\n\r\n"].map(String::from),
            &[("\n\r\n\n\r", true, "<masked>")],
        ));
        assert!(failures.is_empty(), "{failures:#?}");
    }

    // The oracle: the module doc's invariant, checked on generated streams.

    /// A fixed-seed xorshift64* generator: the same cases on every run, and
    /// no dependency.
    struct Prng(u64);

    impl Prng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).expect("a small bound");
            usize::try_from(self.next() % n).expect("below a usize bound")
        }

        fn chance(&mut self, percent: usize) -> bool {
            self.below(100) < percent
        }
    }

    const ORACLE_SEED: u64 = 0x005E_ED0F_7A11_0001;
    const ORACLE_STREAMS: usize = 9_000;
    const ORACLE_CUTS_PER_STREAM: usize = 24;

    /// What streams and values are built from: ASCII, 2-, 3- and 4-byte
    /// characters, LF and CRLF line ends. None spells a generic shape's
    /// trigger (no `=`, `:`, `"`, `/`, `?`, `&`, `@`, no "bearer") or is a
    /// hidden character, so masking by value is all an oracle case meets
    /// but the escape of a lone `\r` a cut leaves.
    const ORACLE_UNITS: [&str; 15] = [
        "a",
        "b",
        "c",
        "x",
        "y",
        "z",
        "1",
        "2",
        "3",
        " ",
        "\n",
        "\r\n",
        "\u{e9}",
        "\u{4e2d}",
        "\u{1F600}",
    ];
    const ORACLE_PLAIN: usize = 10;

    fn oracle_units(rng: &mut Prng, min: usize, max: usize, newlines: bool) -> String {
        let count = min + rng.below(max - min + 1);
        let pool = if newlines {
            ORACLE_UNITS.len()
        } else {
            ORACLE_PLAIN
        };
        let mut out = String::new();
        for _ in 0..count {
            let unit = match rng.below(10) {
                0..=5 => ORACLE_UNITS[rng.below(pool.min(ORACLE_PLAIN))],
                _ => ORACLE_UNITS[rng.below(pool)],
            };
            out.push_str(unit);
        }
        out
    }

    /// A proper piece of `value` on character boundaries: its `start..end`
    /// in characters, `None` when it has too few.
    fn oracle_piece(value: &str, start: usize, end: usize) -> Option<String> {
        let chars: Vec<char> = value.chars().collect();
        (start < end && end <= chars.len() && end - start < chars.len())
            .then(|| chars[start..end].iter().collect())
    }

    /// Does `value` hold a `\r` not followed by `\n`?
    fn holds_a_lone_cr(value: &str) -> bool {
        value
            .match_indices('\r')
            .any(|(at, _)| !value[at + 1..].starts_with('\n'))
    }

    /// The case classes the generator is built to hold, each counted.
    #[derive(Default)]
    struct OracleCoverage {
        prefix_pairs: usize,
        overlapping_pairs: usize,
        value_inside_another: usize,
        one_character_values: usize,
        long_values: usize,
        multi_line_values: usize,
        crlf_values: usize,
        trailing_newline_values: usize,
        multi_byte_values: usize,
        lone_cr_values: usize,
        overlapping_occurrences: usize,
        windows: usize,
        uncut_windows: usize,
        head_cut_in_a_character: usize,
        end_cut_in_a_character: usize,
        windows_inside_an_occurrence: usize,
        windows_of_one_line: usize,
        windows_ending_in_a_newline: usize,
        adjacent_occurrences: usize,
    }

    fn oracle_value(rng: &mut Prng, values: &[String], cover: &mut OracleCoverage) -> String {
        let chars = |v: &String| v.chars().count();
        let pick = |rng: &mut Prng| values[rng.below(values.len())].clone();
        loop {
            let mut value = match rng.below(9) {
                0 if !values.is_empty() => {
                    let v = pick(rng);
                    let n = chars(&v);
                    let Some(prefix) = oracle_piece(&v, 0, 1 + rng.below(n.max(2) - 1)) else {
                        continue;
                    };
                    cover.prefix_pairs += 1;
                    prefix
                }
                1 if !values.is_empty() => {
                    let v = pick(rng);
                    let n = chars(&v);
                    let Some(end) = oracle_piece(&v, 1 + rng.below(n.max(2) - 1), n) else {
                        continue;
                    };
                    cover.overlapping_pairs += 1;
                    end + &oracle_units(rng, 1, 4, true)
                }
                2 if !values.is_empty() => {
                    let v = pick(rng);
                    let n = chars(&v);
                    let Some(start) = oracle_piece(&v, 0, 1 + rng.below(n.max(2) - 1)) else {
                        continue;
                    };
                    cover.overlapping_pairs += 1;
                    oracle_units(rng, 1, 4, true) + &start
                }
                3 if !values.is_empty() => {
                    let v = pick(rng);
                    let n = chars(&v);
                    let start = rng.below(n.max(1));
                    let end = start + 1 + rng.below((n - start).max(1));
                    let Some(inside) = oracle_piece(&v, start, end) else {
                        continue;
                    };
                    cover.value_inside_another += 1;
                    inside
                }
                4 => {
                    cover.one_character_values += 1;
                    ORACLE_UNITS[rng.below(ORACLE_UNITS.len())].to_string()
                }
                5 => {
                    cover.long_values += 1;
                    oracle_units(rng, 30, 60, true)
                }
                6 => {
                    let newline = if rng.chance(50) { "\n" } else { "\r\n" };
                    let lines = 2 + rng.below(3);
                    let mut v = String::new();
                    for i in 0..lines {
                        if i > 0 {
                            v.push_str(newline);
                        }
                        if !rng.chance(20) {
                            v.push_str(&oracle_units(rng, 1, 4, false));
                        }
                    }
                    if rng.chance(30) {
                        v.push_str(newline);
                    }
                    v
                }
                _ => {
                    let newlines = rng.chance(30);
                    oracle_units(rng, 1, 8, newlines)
                }
            };
            if value.is_empty() {
                continue;
            }
            if rng.chance(4) {
                // A value ending in a lone `\r`, or holding one, as one
                // read from a file with CRLF line ends can.
                value.push('\r');
                if rng.chance(50) {
                    value.push_str(&oracle_units(rng, 1, 3, false));
                }
            }
            if holds_a_lone_cr(&value) {
                cover.lone_cr_values += 1;
            }
            if value.contains('\n') {
                cover.multi_line_values += 1;
            }
            if value.contains("\r\n") {
                cover.crlf_values += 1;
            }
            if value.ends_with('\n') {
                cover.trailing_newline_values += 1;
            }
            if !value.is_ascii() {
                cover.multi_byte_values += 1;
            }
            return value;
        }
    }

    /// Every occurrence of a value in `haystack`, found by brute force at
    /// every position, overlapping ones included, as `origin` ranges of the
    /// stream's bytes.
    fn oracle_matches(
        haystack: &[u8],
        origin: &[std::ops::Range<usize>],
        values: &[String],
    ) -> Vec<std::ops::Range<usize>> {
        let mut found = Vec::new();
        for value in values {
            let needle = value.as_bytes();
            if needle.is_empty() || needle.len() > haystack.len() {
                continue;
            }
            for at in 0..=haystack.len() - needle.len() {
                if &haystack[at..at + needle.len()] == needle {
                    found.push(origin[at].start..origin[at + needle.len() - 1].end);
                }
            }
        }
        found
    }

    /// Every occurrence of a value in `stream`: as written, and as read
    /// (`\r\n` as `\n` in the stream and the value, the occurrence then
    /// covering both bytes of each `\r\n` it holds), so the oracle is at
    /// least as strict as either reading.
    fn oracle_occurrences(stream: &[u8], values: &[String]) -> Vec<std::ops::Range<usize>> {
        let written: Vec<std::ops::Range<usize>> = (0..stream.len()).map(|i| i..i + 1).collect();
        let mut found = oracle_matches(stream, &written, values);
        let mut read = Vec::new();
        let mut origin = Vec::new();
        let mut i = 0;
        while i < stream.len() {
            if stream[i] == b'\r' && stream.get(i + 1) == Some(&b'\n') {
                read.push(b'\n');
                origin.push(i..i + 2);
                i += 2;
            } else {
                read.push(stream[i]);
                origin.push(i..i + 1);
                i += 1;
            }
        }
        found.extend(oracle_matches(&read, &origin, &as_read(values)));
        found
    }

    /// Each byte of a stream of `len` bytes: inside an occurrence or not.
    fn oracle_inside(len: usize, occurrences: &[std::ops::Range<usize>]) -> Vec<bool> {
        let mut inside = vec![false; len];
        for occurrence in occurrences {
            inside[occurrence.clone()].fill(true);
        }
        inside
    }

    /// `bytes` decoded as `String::from_utf8_lossy` decodes them, then
    /// `\r\n` read as `\n`: each character with the bytes of `bytes` it
    /// shows. The oracle's rule for a newline read from `\r\n`: it shows
    /// its `\n` byte, and the `\r` is shown only where a `\r` is (a lone
    /// one, escaped); a U+FFFD shows every byte it replaces.
    fn oracle_decode(bytes: &[u8]) -> Vec<(char, std::ops::Range<usize>)> {
        let mut decoded = Vec::new();
        let mut at = 0;
        for chunk in bytes.utf8_chunks() {
            for (offset, c) in chunk.valid().char_indices() {
                decoded.push((c, at + offset..at + offset + c.len_utf8()));
            }
            at += chunk.valid().len();
            if !chunk.invalid().is_empty() {
                decoded.push(('\u{FFFD}', at..at + chunk.invalid().len()));
                at += chunk.invalid().len();
            }
        }
        let mut read: Vec<(char, std::ops::Range<usize>)> = Vec::new();
        for (c, range) in decoded {
            if c == '\n'
                && let Some(last) = read.last_mut()
                && last.0 == '\r'
            {
                *last = ('\n', range);
                continue;
            }
            read.push((c, range));
        }
        read
    }

    /// The masked text as [`masked`] writes it (`plain`) and as escaped
    /// (`written`), each byte of `written` with the character of the read
    /// text it shows (none for a mask).
    #[derive(Default)]
    struct Traced {
        plain: String,
        written: String,
        sources: Vec<Option<usize>>,
    }

    impl Traced {
        fn text(&mut self, text: &str, part: std::ops::Range<usize>) {
            for (index, (at, c)) in text.char_indices().enumerate() {
                if part.contains(&at) {
                    let mut buf = [0u8; 4];
                    let escaped = escape_hidden(c.encode_utf8(&mut buf)).text;
                    self.plain.push(c);
                    self.written.push_str(&escaped);
                    self.sources
                        .extend(std::iter::repeat_n(Some(index), escaped.len()));
                }
            }
        }

        fn mask(&mut self) {
            self.plain.push_str(MASKED);
            self.written.push_str(MASKED);
            self.sources.extend(std::iter::repeat_n(None, MASKED.len()));
        }
    }

    /// One window's leak: the window's bytes, its head cut, what is shown,
    /// and the window's bytes inside an occurrence that are shown.
    #[derive(Debug)]
    struct OracleLeak {
        values: Vec<String>,
        window: Vec<u8>,
        truncated: bool,
        shown: String,
        leaked: Vec<usize>,
    }

    impl OracleLeak {
        fn describe(&self) -> String {
            format!(
                "values {:?}, window {:?} ({:?}), head cut {}, shown {:?}, \
                 window bytes shown inside an occurrence {:?}",
                self.values,
                self.window,
                String::from_utf8_lossy(&self.window),
                self.truncated,
                self.shown,
                self.leaked
            )
        }
    }

    /// The bytes of `stream[head..end]` that [`shown_tail`] (the path
    /// [`compose_detail`] takes) shows, as offsets into the window, with
    /// what it shows. The shown text is traced back to the window through
    /// the steps' seams: [`read_and_find`] (its text must be a lossy decode
    /// of a part of the window that leaves out at most three bytes at each
    /// end) and [`masked`], then the escape, the
    /// trailing-newline trim and the cuts at the head, which leave a suffix
    /// of that text (both asserted).
    fn oracle_shown(
        stream: &[u8],
        values: &[String],
        head: usize,
        end: usize,
    ) -> (Vec<usize>, String) {
        let window = &stream[head..end];
        let tail = TailSnapshot {
            bytes: window.to_vec(),
            truncated: head > 0,
            ended: true,
        };
        let (text, ranges) = read_and_find(&tail, values);
        let n = window.len();
        let (from, decoded) = (0..=n.min(3))
            .flat_map(|from| (n.saturating_sub(3).max(from)..=n).map(move |to| (from, to)))
            .find_map(|(from, to)| {
                let decoded = oracle_decode(&window[from..to]);
                (decoded.iter().map(|(c, _)| *c).collect::<String>() == text)
                    .then_some((from, decoded))
            })
            .unwrap_or_else(|| panic!("step 1 read {text:?} from no part of {window:?}"));
        let mut traced = Traced::default();
        let mut shown_to = 0;
        let mut ranges_iter = ranges.clone().into_iter().peekable();
        while let Some(range) = ranges_iter.next() {
            let mut range_end = range.end;
            while let Some(next) = ranges_iter.next_if(|next| next.start < range_end) {
                range_end = range_end.max(next.end);
            }
            traced.text(&text, shown_to..range.start);
            traced.mask();
            shown_to = range_end;
        }
        traced.text(&text, shown_to..text.len());
        let Traced {
            plain,
            written,
            sources,
        } = traced;
        assert_eq!(
            plain,
            masked(&text, ranges),
            "the oracle's trace writes what `masked` writes"
        );
        let trimmed = written.trim_end_matches('\n').len();
        let shown = shown_tail(&tail, values);
        assert!(
            written[..trimmed].ends_with(&shown),
            "what is shown is a suffix of the masked text: {shown:?} of {written:?}"
        );
        let mut bytes: Vec<usize> = sources[trimmed - shown.len()..trimmed]
            .iter()
            .flatten()
            .flat_map(|index| decoded[*index].1.clone())
            .map(|byte| from + byte)
            .collect();
        bytes.sort_unstable();
        bytes.dedup();
        (bytes, shown)
    }

    /// The window `head..end` of `stream` checked against the invariant:
    /// a leak when a byte it shows lies inside an occurrence.
    fn oracle_check(
        stream: &[u8],
        inside: &[bool],
        values: &[String],
        head: usize,
        end: usize,
    ) -> Option<OracleLeak> {
        let (bytes, shown) = oracle_shown(stream, values, head, end);
        let leaked: Vec<usize> = bytes
            .into_iter()
            .filter(|byte| inside[head + byte])
            .collect();
        (!leaked.is_empty()).then(|| OracleLeak {
            values: values.to_vec(),
            window: stream[head..end].to_vec(),
            truncated: head > 0,
            shown,
            leaked,
        })
    }

    fn oracle_case(rng: &mut Prng, cover: &mut OracleCoverage) -> (Vec<String>, Vec<u8>) {
        let count = 1 + rng.below(4);
        let mut values: Vec<String> = Vec::new();
        while values.len() < count {
            let value = oracle_value(rng, &values, cover);
            if !values.contains(&value) {
                values.push(value);
            }
        }
        let mut stream = String::new();
        for _ in 0..1 + rng.below(5) {
            stream.push_str(&oracle_units(rng, 0, 6, true));
            let value = values[rng.below(values.len())].clone();
            stream.push_str(&value);
            if value.ends_with('\r') && rng.chance(50) {
                // Written at a line end: its `\r` then a `\n`.
                stream.push('\n');
            }
            if rng.chance(15) {
                stream.push_str(&value);
                cover.adjacent_occurrences += 1;
            } else if rng.chance(15) {
                stream.push_str(&values[rng.below(values.len())]);
                cover.adjacent_occurrences += 1;
            }
        }
        stream.push_str(&oracle_units(rng, 0, 6, true));
        (values, stream.into_bytes())
    }

    /// The windows a stream is read through: the whole stream (no cut),
    /// and cuts at arbitrary byte offsets, half of them near an
    /// occurrence's edge (so inside a character, a value or a `\r\n`).
    fn oracle_cuts(rng: &mut Prng, inside: &[bool]) -> Vec<(usize, usize)> {
        let n = inside.len();
        let edges: Vec<usize> = (0..=n)
            .filter(|&i| (i > 0 && inside[i - 1]) != (i < n && inside[i]))
            .collect();
        let mut cuts = vec![(0, n)];
        let at = |rng: &mut Prng| {
            if !edges.is_empty() && rng.chance(50) {
                (edges[rng.below(edges.len())] + rng.below(7))
                    .saturating_sub(3)
                    .min(n)
            } else {
                rng.below(n + 1)
            }
        };
        while cuts.len() < ORACLE_CUTS_PER_STREAM {
            let (a, b) = (at(rng), at(rng));
            let (head, end) = (a.min(b), a.max(b));
            cuts.push(match rng.below(10) {
                0 => (0, end),
                1 => (head, n),
                _ => (head, end),
            });
        }
        cuts
    }

    fn oracle_count(
        cover: &mut OracleCoverage,
        stream: &[u8],
        inside: &[bool],
        head: usize,
        end: usize,
    ) {
        let continuation = |byte: u8| (0x80..0xC0).contains(&byte);
        let window = &stream[head..end];
        cover.windows += 1;
        if (head, end) == (0, stream.len()) {
            cover.uncut_windows += 1;
        }
        if head > 0 && head < stream.len() && continuation(stream[head]) {
            cover.head_cut_in_a_character += 1;
        }
        if end < stream.len() && end > 0 && continuation(stream[end]) {
            cover.end_cut_in_a_character += 1;
        }
        if head < end && inside[head..end].iter().all(|b| *b) {
            cover.windows_inside_an_occurrence += 1;
        }
        let trimmed = window.strip_suffix(b"\n").unwrap_or(window);
        if !window.is_empty() && !trimmed.contains(&b'\n') {
            cover.windows_of_one_line += 1;
        }
        if window.ends_with(b"\n") {
            cover.windows_ending_in_a_newline += 1;
        }
    }

    /// A leak made smaller while it still leaks: values left out, the
    /// stream's bytes outside the window left out, the window narrowed.
    fn oracle_minimise(
        mut values: Vec<String>,
        mut stream: Vec<u8>,
        mut head: usize,
        mut end: usize,
    ) -> OracleLeak {
        let leaks = |values: &[String], stream: &[u8], head: usize, end: usize| {
            let inside = oracle_inside(stream.len(), &oracle_occurrences(stream, values));
            oracle_check(stream, &inside, values, head, end)
        };
        let mut last = leaks(&values, &stream, head, end).expect("a leak to minimise");
        loop {
            let mut smaller = false;
            for i in 0..values.len() {
                if values.len() > 1 {
                    let mut fewer = values.clone();
                    fewer.remove(i);
                    if let Some(leak) = leaks(&fewer, &stream, head, end) {
                        (values, last, smaller) = (fewer, leak, true);
                        break;
                    }
                }
            }
            if smaller {
                continue;
            }
            let mut tries: Vec<(Vec<u8>, usize, usize)> = Vec::new();
            if head > 1 {
                tries.push((stream[1..].to_vec(), head - 1, end - 1));
            }
            if end < stream.len() {
                tries.push((stream[..stream.len() - 1].to_vec(), head, end));
            }
            if head + 1 < end {
                if head > 0 {
                    tries.push((stream.clone(), head + 1, end));
                }
                tries.push((stream.clone(), head, end - 1));
            }
            for (s, h, e) in tries {
                if let Some(leak) = leaks(&values, &s, h, e) {
                    (stream, head, end, last, smaller) = (s, h, e, leak, true);
                    break;
                }
            }
            if !smaller {
                return last;
            }
        }
    }

    /// Class NL, the one named exception: a window the ring cut at its
    /// head that holds only newline bytes (`\n`, `\r`) lying inside one
    /// occurrence of a value (so inside a run of that value's newlines).
    /// Judged from the stream and the brute-force occurrences alone, no
    /// function of the module's.
    fn oracle_nl_window(
        stream: &[u8],
        occurrences: &[std::ops::Range<usize>],
        head: usize,
        end: usize,
    ) -> bool {
        head > 0
            && head < end
            && stream[head..end].iter().all(|b| matches!(b, b'\n' | b'\r'))
            && occurrences
                .iter()
                .any(|occurrence| occurrence.start <= head && end <= occurrence.end)
    }

    /// One oracle run: the classes met, the windows class NL's predicate
    /// matched, the leaks it exempted (each showing only `\n` bytes of
    /// such a window), and the windows that leak outside it, the first
    /// `limit` of them minimised.
    #[derive(Default)]
    struct OracleRun {
        cover: OracleCoverage,
        nl_windows: usize,
        nl_exempted: usize,
        leaking: usize,
        leaks: Vec<OracleLeak>,
    }

    /// Every case of `streams` streams from `seed`, its windows checked.
    fn oracle_run(seed: u64, streams: usize, limit: usize) -> OracleRun {
        let mut rng = Prng(seed);
        let mut run = OracleRun::default();
        for _ in 0..streams {
            let (values, stream) = oracle_case(&mut rng, &mut run.cover);
            let written: Vec<std::ops::Range<usize>> =
                (0..stream.len()).map(|i| i..i + 1).collect();
            let whole = oracle_matches(&stream, &written, &values);
            if whole
                .iter()
                .any(|a| whole.iter().any(|b| a.start < b.start && b.start < a.end))
            {
                run.cover.overlapping_occurrences += 1;
            }
            let occurrences = oracle_occurrences(&stream, &values);
            let inside = oracle_inside(stream.len(), &occurrences);
            for (head, end) in oracle_cuts(&mut rng, &inside) {
                oracle_count(&mut run.cover, &stream, &inside, head, end);
                let nl_window = oracle_nl_window(&stream, &occurrences, head, end);
                run.nl_windows += usize::from(nl_window);
                let Some(leak) = oracle_check(&stream, &inside, &values, head, end) else {
                    continue;
                };
                if nl_window && leak.leaked.iter().all(|byte| stream[head + byte] == b'\n') {
                    run.nl_exempted += 1;
                    continue;
                }
                run.leaking += 1;
                if run.leaks.len() < limit {
                    run.leaks
                        .push(oracle_minimise(values.clone(), stream.clone(), head, end));
                }
            }
        }
        run
    }

    /// The oracle, a permanent fixed-seed sample ([`ORACLE_SEED`]):
    /// [`ORACLE_STREAMS`] streams, each read through
    /// [`ORACLE_CUTS_PER_STREAM`] windows (216,000), every class it is
    /// built to hold met at least 200 times; about 4.8 s in a debug build.
    /// Its rule: a byte of the window is shown if it appears in the output
    /// (a newline read from `\r\n` shows its `\n`; a `\r` is shown only
    /// where a `\r` is, escaped; a U+FFFD shows every byte it replaces).
    /// No byte of the window that lies inside an occurrence of a value in
    /// the stream — found by brute force at every position, as written and
    /// as read — is shown, the occurrence inside the window or crossing a
    /// cut, but in class NL ([`oracle_nl_window`], pinned as shown today by
    /// `a_head_cut_window_of_a_value_s_newlines_shows_one_of_them_today`),
    /// whose windows are counted and exempted. Without that exemption this
    /// seed is green at 6,000 streams and red at 9,000 and at 20,000 (one
    /// window each). A sample, not a proof.
    ///
    /// What it covers: values from 1 to about 60 characters, one to four
    /// to a server, built as prefixes of each other, overlapping (one's
    /// end another's start), inside each other, of one character, longer
    /// than a window, of several lines (with blank lines, a trailing
    /// newline, LF or CRLF line ends), with 2-, 3- and 4-byte characters,
    /// and holding a `\r` not followed by `\n` (ending in one, or with one
    /// inside); streams of those values, adjacent and repeated (a value
    /// ending in a `\r` written at a line end, then a `\n`), between
    /// ASCII, multi-byte characters and LF and CRLF line ends; windows cut
    /// at any byte at either end (inside a character, a value or a
    /// `\r\n`), uncut, of one line, ending in newlines, lying inside an
    /// occurrence.
    ///
    /// What it leaves out: the generic shapes and hidden characters
    /// (no unit spells one); a tail longer than [`TAIL_BYTES`] (step 5's
    /// cut); which values are in the mask set.
    #[test]
    fn the_oracle_s_sample_shows_no_value_byte_outside_class_nl() {
        let OracleRun {
            cover,
            nl_windows,
            nl_exempted,
            leaking,
            leaks,
        } = oracle_run(ORACLE_SEED, ORACLE_STREAMS, 1);
        for (class, count) in [
            ("prefix pairs", cover.prefix_pairs),
            ("overlapping pairs", cover.overlapping_pairs),
            ("a value inside another", cover.value_inside_another),
            ("one-character values", cover.one_character_values),
            ("values longer than a window", cover.long_values),
            ("multi-line values", cover.multi_line_values),
            ("CRLF values", cover.crlf_values),
            ("values ending in a newline", cover.trailing_newline_values),
            ("multi-byte values", cover.multi_byte_values),
            ("values holding a lone CR", cover.lone_cr_values),
            ("overlapping occurrences", cover.overlapping_occurrences),
            ("uncut windows", cover.uncut_windows),
            (
                "head cuts inside a character",
                cover.head_cut_in_a_character,
            ),
            ("end cuts inside a character", cover.end_cut_in_a_character),
            (
                "windows inside an occurrence",
                cover.windows_inside_an_occurrence,
            ),
            ("windows of one line", cover.windows_of_one_line),
            (
                "windows ending in a newline",
                cover.windows_ending_in_a_newline,
            ),
            ("adjacent occurrences", cover.adjacent_occurrences),
        ] {
            assert!(count >= 200, "the generator met {class} only {count} times");
        }
        assert_eq!(cover.windows, ORACLE_STREAMS * ORACLE_CUTS_PER_STREAM);
        // The seed is fixed, so the exemption's reach is pinned exactly:
        // one window of the 821 its predicate matches.
        assert_eq!(
            (nl_exempted, nl_windows),
            (1, 821),
            "class NL's exempted windows of those its predicate matched"
        );
        assert!(
            leaks.is_empty(),
            "{leaking} windows show a byte of a masked value outside class NL \
             ({nl_exempted} exempted of the {nl_windows} its predicate matched); \
             one, minimised: {}",
            leaks[0].describe()
        );
    }
}
