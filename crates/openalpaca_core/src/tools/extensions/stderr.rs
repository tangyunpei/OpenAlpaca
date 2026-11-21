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
//!    secret-like names — replaced by [`MASKED`], longest first;
//! 3. a value cut by the ring's own bound at the head masked too, or the
//!    partial first line dropped; and the start of a value that ends an
//!    unterminated last line (a tail read before the server finished
//!    writing it) masked;
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
    // An unterminated last line may end in the start of a value: the tail
    // was read before its server finished writing it — at the settle's
    // bound, between two reads of the pipe, or ended by a respawn or the
    // reap's bound rather than by the pipe's end.
    let mut text = mask_values(&text, secrets, !text.ends_with('\n'));
    let partial_head = tail.truncated;
    if partial_head && !text.contains('\n') {
        text = mask_head_fragment(&text, secrets);
    }
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

/// Every occurrence of a `secrets` value replaced by [`MASKED`], in one pass
/// over `text`, the longest value first at each place; with `cut_end`, also
/// the longest start of a value that `text` ends with — judged on the text as
/// written, before a shorter value inside it is masked, and only where no
/// whole value begins.
fn mask_values(text: &str, secrets: &[String], cut_end: bool) -> String {
    if secrets.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    'scan: while let Some(c) = rest.chars().next() {
        for secret in secrets {
            if !secret.is_empty() && rest.starts_with(secret.as_str()) {
                out.push_str(MASKED);
                rest = &rest[secret.len()..];
                continue 'scan;
            }
        }
        if cut_end
            && secrets
                .iter()
                .any(|secret| secret.len() > rest.len() && secret.starts_with(rest))
        {
            out.push_str(MASKED);
            break;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// A tail the ring cut at its head may begin with the end of a value: the
/// longest end of any `secrets` value that `text` begins with is masked.
fn mask_head_fragment(text: &str, secrets: &[String]) -> String {
    let longest = secrets
        .iter()
        .flat_map(|secret| {
            secret
                .char_indices()
                .skip(1)
                .map(move |(at, _)| &secret[at..])
        })
        .filter(|end| text.starts_with(end))
        .map(str::len)
        .max();
    match longest {
        Some(len) => format!("{MASKED}{}", &text[len..]),
        None => text.to_string(),
    }
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
