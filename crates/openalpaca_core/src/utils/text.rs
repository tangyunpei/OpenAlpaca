//! Byte-budget text cuts that cannot panic.
//!
//! A log preview or a refusal message wants a **byte** ceiling — a budget in
//! characters lets one CJK line grow to four times the bytes it was meant to
//! cost. `&s[..n]` is the obvious way to spend that budget and it panics the
//! moment `n` lands inside a multi-byte character, which user- and
//! model-authored text does constantly (a 3-byte CJK character, a 4-byte
//! emoji). [`byte_prefix`] keeps the byte budget and rounds the cut **down**
//! to the nearest character boundary.
//!
//! This is not [`crate::runner::agentic_loop::tool_helpers`]'s tool-result
//! truncation: that one is a different mechanism (sentence/line/word
//! boundaries, head-and-tail, a session-log spill) answering a different
//! question. This is the one-line cut for a preview.
//!
//! [`one_line`] is what a person reads before acting on it — an approval
//! card, a list to pick an id from: one line with nothing hidden in it. The
//! CLI prints note titles through it too.
//!
//! [`escape_hidden`] answers the same danger for a different text: the exact
//! values a person approves or denies — a tool's arguments, the tool's name,
//! the agent asking. Nothing may be dropped from those, because what is
//! dropped from the prompt still runs, so every hidden character — and every
//! code point Unicode says renders as nothing — is shown as `<U+XXXX>` and
//! [`hidden_notice`] says how many there were. The two do not compete:
//! `one_line` is for prose OpenAlpaca composed to be read (a memory
//! suggestion's `preview`, stripped of control and format characters by the
//! daemon before any surface sees it — the default-ignorable letters the
//! escape also shows, a variation selector or a Hangul filler, pass through
//! it), and the escape is for the values beside it that decide what runs.
//! A one-line field — the tool's name, the agent's id — takes
//! [`escape_hidden_line`], which shows a newline, a tab and every other
//! space but the plain one as well: the pretty-printed arguments are laid
//! out in lines, a name is not. A model's own prose printed at a terminal —
//! not an approval surface, but printed where the next prompt will be —
//! takes [`escape_controls`], the same form for the control characters
//! alone, so it cannot leave the terminal in a state the prompt inherits.
//! The CLI's JSON output (`tasks confirmations list --format json`, the
//! payloads of `tasks log`) is the one other rendering of the escape: it
//! writes the [`needs_escape_in_line`] set in JSON's own lossless `\u`
//! form, reading this predicate rather than keeping a copy of the set.
//! The GUI mirrors the escape from one shared table,
//! `apps/openalpaca-gui/src/lib/hidden-chars.json`, which a test here holds
//! equal to [`needs_escape`] and [`needs_escape_in_line`] over every scalar
//! value.

/// Borrow at most `max_bytes` of `s`, ending on a character boundary.
///
/// The result is never longer than `max_bytes`; when the budget lands inside a
/// character, that whole character is dropped. A budget at or past the end of
/// the string returns the whole string, and a budget of 0 returns `""`.
pub(crate) fn byte_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

/// `text` on one line for a person to read and act on: runs of whitespace —
/// newlines and the line and paragraph separators (U+2028, U+2029) among
/// them — as one space, and no [hidden](is_hidden) character. Hidden
/// characters are dropped, not replaced with a visible mark: what is left is
/// the text as stored, in its own order, and a mark would litter the
/// legitimate uses (an emoji's zero-width joiner, a soft hyphen) on the very
/// line that must read clearly.
pub fn one_line(text: &str) -> String {
    text.split_whitespace()
        .map(|word| word.replace(is_hidden, ""))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A character that changes what a reader sees without being seen itself:
/// a control character (Cc) — at a terminal an ESC sequence can rewrite the
/// line — or a Unicode *format* character (Cf): the bidi embeddings,
/// overrides and isolates (U+202A–U+202E, U+2066–U+2069) that reorder the
/// text around them, the zero-width characters (U+200B–U+200F,
/// U+2060–U+2064, U+FEFF), the soft hyphen and the tag characters.
/// Unicode 16.0's `Cf`, held equal to the `regex` crate's own `\p{Cf}` by
/// a test.
pub fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

/// What [`escape_hidden`] or [`escape_hidden_line`] made of a text for an
/// approval surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escaped {
    /// The text, every character of the escape's set in it — [`needs_escape`],
    /// or [`needs_escape_in_line`] for [`escape_hidden_line`] — written as
    /// `<U+XXXX>`.
    pub text: String,
    /// How many characters were escaped in the **whole** input — the part a
    /// byte budget left out included, so a notice never undercounts.
    pub hidden: usize,
    /// How many characters of the input a byte budget left out, 0 when all
    /// of it is shown. A surface that cuts says this number: what is left
    /// out still runs.
    pub left_out: usize,
}

/// A character an approval surface shows as `<U+XXXX>` instead of itself:
/// every [hidden](is_hidden) character, every code point Unicode calls
/// default-ignorable (`Default_Ignorable_Code_Point`: drawn as nothing, so a
/// variation selector inside a path reads as the path without it), and the
/// line and paragraph separators (U+2028, U+2029), which break a line where
/// the text shows none — but not `\n` and `\t`, the two characters the
/// surfaces' own layout (pretty-printed JSON) is written in. `\r` is
/// escaped: at a terminal it returns the cursor and the rest of the line
/// overwrites what was shown.
///
/// The GUI derives its copy from `apps/openalpaca-gui/src/lib/hidden-chars.json`;
/// a test on each side holds its own to that table.
pub fn needs_escape(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        '\u{2028}' | '\u{2029}' => true,
        c => is_hidden(c) || is_default_ignorable(c),
    }
}

/// A code point Unicode says a renderer draws as nothing when it does not
/// support it (`Default_Ignorable_Code_Point`, Unicode 16.0, held equal to
/// the `regex` crate's own table by a test). Most are [hidden](is_hidden)
/// already; what this adds is letters and marks that show as nothing — the
/// variation selectors (U+FE00–U+FE0F, U+E0100–U+E01EF, the Mongolian ones),
/// the combining grapheme joiner (U+034F), the Hangul fillers (U+115F,
/// U+1160, U+3164, U+FFA0), Khmer U+17B4–U+17B5 — and the code points
/// Unicode reserves for future invisible characters (U+2065,
/// U+FFF0–U+FFF8, U+E0000–U+E0FFF), which a renderer newer than this table
/// may already hide.
fn is_default_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{115f}'..='\u{1160}'
            | '\u{17b4}'..='\u{17b5}'
            | '\u{180b}'..='\u{180f}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{3164}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
            | '\u{ffa0}'
            | '\u{fff0}'..='\u{fff8}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// `text` for a person deciding whether it runs: every [`needs_escape`]
/// character written as `<U+XXXX>` — uppercase hex, zero-padded to at least
/// four digits (`<U+202E>`, `<U+009B>`, `<U+E0041>`) — and everything else as
/// it is, right-to-left letters included. Put [`hidden_notice`] of the count
/// under it. On a plain-text surface a value that literally contains the
/// eight characters `<U+202E>` looks the same as an escape; the notice counts
/// only real escapes, so its count is what tells the two apart.
///
/// Escaped, not dropped as [`one_line`] drops them: what an approval prompt
/// shows is what will run, and a value with its hidden characters taken out
/// reads as a different value from the one that runs — `/srv/allowed` for a
/// path with a zero-width space inside it — and hides an override the value
/// carries to wherever it lands. The price, an emoji's zero-width joiner or
/// variation selector shown as `<U+200D>` or `<U+FE0F>`, is right on a
/// surface whose job is to be exact. This is display only: nothing the tool
/// receives, nothing stored and nothing matched against a reply changes.
///
/// This keeps `\n` and `\t` as they are, for text laid out in lines — the
/// pretty-printed arguments, where `serde_json` has already escaped every
/// newline inside a string. A one-line field takes [`escape_hidden_line`].
///
/// What no character set can do, and this does not claim: right-to-left
/// letters are not format characters and stay as written, so the Unicode
/// bidi algorithm still reorders the digits and neutral characters next to
/// them **inside** a value, on every surface: a path made of `/tmp/`, a
/// Hebrew letter and `/1/2` displays as `/tmp/1/2/` and then the letter,
/// and a Hebrew letter followed by ` 100 200` displays as `200 100 ` and
/// then the letter. Isolating the arguments block (`unicode-bidi: isolate`
/// on a web surface) keeps them from reordering the text around the block,
/// not the text within a value; the plain-text surfaces have no isolation
/// at all. Stacked combining marks that draw over neighbouring
/// lines in some renderers are the same kind of residue. Forcing logical
/// order on approval surfaces (an LRO … PDF around the arguments on plain
/// text, `unicode-bidi: bidi-override` in the GUI) is an owner decision,
/// not taken.
///
/// Nor does it show meaning: the escape makes the code points exact, and a
/// visible character that looks like another stays as written — U+0430
/// CYRILLIC SMALL LETTER A for the Latin `a` in a domain, a fullwidth
/// solidus (U+FF0F) or a division slash (U+2215) for `/` in a path, a
/// no-break space inside a shell command, which the shell does not split
/// on — and two tool names a homoglyph apart read alike, in the prompt and
/// in a reply's acknowledgement. Flagging mixed scripts or confusables is an
/// owner decision, not taken.
pub fn escape_hidden(text: &str) -> Escaped {
    escape_hidden_within(text, usize::MAX)
}

/// [`escape_hidden`], cut to at most `max_bytes` of **escaped** text. The cut
/// comes after escaping and never lands inside an escape or a character: the
/// first piece that would cross the budget is left out whole, with
/// everything after it. `hidden` still counts the whole of `text`, so the
/// notice says what the part left out holds too, and `left_out` says how
/// many characters of `text` the cut left out, for the surface to print.
pub fn escape_hidden_within(text: &str, max_bytes: usize) -> Escaped {
    escape_where(text, max_bytes, needs_escape)
}

/// A character a **one-line** field shows as `<U+XXXX>`: every
/// [`needs_escape`] character, every whitespace character but the plain
/// space U+0020 — `\n` and `\t`, the no-break spaces (U+00A0, U+202F), the
/// Ogham, math and ideographic spaces (U+1680, U+205F, U+3000) and the
/// typographic ones (U+2000–U+200A) — and U+2800 BRAILLE PATTERN BLANK,
/// which draws as a space and wraps like a letter. A tool's name or an
/// agent's id has no layout of its own — its line is the surface's — and an
/// MCP `<server>__<tool>` or plugin `<plugin>::<tool>` name is any string its
/// third party lists: a newline in it would print forged lines (a benign
/// `Arguments:` block, a `/yes` line) above the real ones, and a run of wide
/// blanks, which a chat client soft-wraps, would start its tail on a visual
/// line of its own. No real name or id carries any of them.
///
/// The plain space stays, runs of it included, and that is residue: a run
/// of plain spaces wraps the same way, so a name can still end in what
/// looks like one extra line of the prompt (about one, in the 256 bytes a
/// name may have), with the real lines directly under it. Showing a run of
/// plain spaces would be a rule about neighbours, not a set; it is an owner
/// decision, not taken.
pub fn needs_escape_in_line(c: char) -> bool {
    needs_escape(c) || (c.is_whitespace() && c != ' ') || c == '\u{2800}'
}

/// [`escape_hidden`] for a one-line field — a tool name, an agent id: every
/// [`needs_escape_in_line`] character written as `<U+XXXX>`, newlines, tabs
/// and wide blanks included (`<U+000A>`, `<U+0009>`, `<U+3000>`), and
/// counted in the same [`hidden_notice`].
pub fn escape_hidden_line(text: &str) -> Escaped {
    escape_where(text, usize::MAX, needs_escape_in_line)
}

/// The one pass behind the escapes: every `shown_escaped` character written
/// as `<U+XXXX>`, cut to `max_bytes` of escaped text on a whole piece,
/// counting the whole input.
fn escape_where(text: &str, max_bytes: usize, shown_escaped: fn(char) -> bool) -> Escaped {
    use std::fmt::Write as _;

    let mut shown = String::with_capacity(text.len().min(max_bytes));
    let mut hidden = 0;
    let mut left_out = 0;
    for c in text.chars() {
        let escape = shown_escaped(c);
        hidden += usize::from(escape);
        if left_out > 0 {
            left_out += 1;
            continue;
        }
        let before = shown.len();
        if escape {
            // Writing into a `String` cannot fail.
            let _ = write!(shown, "<U+{:04X}>", u32::from(c));
        } else {
            shown.push(c);
        }
        if shown.len() > max_bytes {
            shown.truncate(before);
            left_out = 1;
        }
    }
    Escaped {
        text: shown,
        hidden,
        left_out,
    }
}

/// The one line an approval surface prints directly under what it escaped,
/// or `None` when nothing was: the same words on every surface (the GUI's
/// copy is held to them by `hidden-chars.json`'s `notices`).
pub fn hidden_notice(hidden: usize) -> Option<String> {
    match hidden {
        0 => None,
        1 => Some("Note: this request contains 1 hidden character, shown as <U+…>.".to_string()),
        n => Some(format!(
            "Note: this request contains {n} hidden characters, shown as <U+…>."
        )),
    }
}

/// `text` for a terminal when it is prose a model wrote — an answer, its
/// reasoning — rather than a value a person approves: every control
/// character (Cc) but `\n` and `\t` written as `<U+XXXX>`, in
/// [`escape_hidden`]'s form, and everything else as it is.
///
/// At a terminal a control character is an instruction, not text: an ESC or
/// a C1 CSI (U+009B) starts a sequence that can turn line wrap off, conceal
/// what follows, switch to a line-drawing character set or move the cursor,
/// and SO (U+000E) shifts the character set on its own. That state outlives
/// the prose that set it — into an approval prompt printed under it, which
/// its own escape cannot reach: with wrap off, the tail of an argument past
/// the right margin is never drawn, and nothing in it is hidden for a notice
/// to count. Format characters stay: they change how the prose reads, not
/// what the terminal does, a bidi override ends with its line, and the prose
/// is not what runs. Nothing is counted and there is no notice: this is not
/// an approval surface.
pub fn escape_controls(text: &str) -> String {
    escape_where(text, usize::MAX, |c| {
        c.is_control() && c != '\n' && c != '\t'
    })
    .text
}

#[cfg(test)]
mod tests {
    use super::{
        Escaped, byte_prefix, escape_controls, escape_hidden, escape_hidden_line,
        escape_hidden_within, hidden_notice, is_default_ignorable, is_hidden, needs_escape,
        needs_escape_in_line, one_line,
    };

    /// A model's prose reaches a terminal with every control character but
    /// the newline and the tab written as its code point, and nothing else
    /// changed: over every scalar value, exactly Cc less `\n` and `\t`.
    #[test]
    fn escape_controls_shows_every_control_but_newline_and_tab() {
        for c in every_char() {
            let expected = if c.is_control() && c != '\n' && c != '\t' {
                format!("<U+{:04X}>", u32::from(c))
            } else {
                c.to_string()
            };
            assert_eq!(
                escape_controls(&c.to_string()),
                expected,
                "U+{:04X}",
                c as u32
            );
        }
        assert_eq!(
            escape_controls("wrap\u{1b}[?7l off\u{9b}8m\u{e}\r\n\tb \u{202e}\u{200b}c"),
            "wrap<U+001B>[?7l off<U+009B>8m<U+000E><U+000D>\n\tb \u{202e}\u{200b}c"
        );
    }

    /// Every scalar value, for the tests that must hold over all of them.
    fn every_char() -> impl Iterator<Item = char> {
        (0..=0x10ffff_u32).filter_map(char::from_u32)
    }

    /// The escape's set is `is_hidden`'s and the default-ignorable code
    /// points, plus the line and paragraph separators, less the newline and
    /// the tab the surfaces lay JSON out with — and nothing else, so none of
    /// them can drift from the others.
    #[test]
    fn needs_escape_is_hidden_and_default_ignorable_plus_the_separators_less_newline_and_tab() {
        for c in every_char() {
            let expected =
                (is_hidden(c) || is_default_ignorable(c) || c == '\u{2028}' || c == '\u{2029}')
                    && c != '\n'
                    && c != '\t';
            assert_eq!(needs_escape(c), expected, "U+{:04X}", c as u32);
        }
    }

    /// A one-line field's set is the escape's, plus every Unicode space but
    /// the plain one (`White_Space` by the `regex` crate's tables: the
    /// newline and the tab, the no-break and ideographic spaces, …) and the
    /// braille blank, which draws as a space and wraps like a letter.
    #[test]
    fn needs_escape_in_line_is_needs_escape_plus_every_space_but_the_plain_one() {
        let space = regex::Regex::new(r"^\p{White_Space}$").unwrap();
        let mut buf = [0u8; 4];
        for c in every_char() {
            let expected = needs_escape(c)
                || (space.is_match(c.encode_utf8(&mut buf)) && c != ' ')
                || c == '\u{2800}';
            assert_eq!(needs_escape_in_line(c), expected, "U+{:04X}", c as u32);
        }
    }

    /// A newline in a tool name is how a third-party name forges whole
    /// prompt lines, and a run of wide blanks how it forges a wrapped one;
    /// in a one-line field both are shown, not laid out.
    #[test]
    fn escape_hidden_line_shows_newline_tab_and_wide_blanks_as_code_points() {
        // (input, shown, hidden)
        let cases: &[(&str, &str, usize)] = &[
            ("", "", 0),
            ("files__write", "files__write", 0),
            (
                "read\nArguments:\n{\n  \"path\": \"/tmp/notes.txt\"\n}",
                "read<U+000A>Arguments:<U+000A>{<U+000A>  \"path\": \"/tmp/notes.txt\"<U+000A>}",
                4,
            ),
            ("a\tb\r\u{202e}", "a<U+0009>b<U+000D><U+202E>", 3),
            ("x\u{fe00}y\u{2028}", "x<U+FE00>y<U+2028>", 2),
            // Wide blanks push the rest of a name onto a line of its own
            // where a chat client wraps: shown, not laid out.
            (
                "shell_exec\u{3000}\u{3000}Arguments: {}",
                "shell_exec<U+3000><U+3000>Arguments: {}",
                2,
            ),
            (
                "a\u{a0}b\u{2800}c\u{2003}d\u{202f}e",
                "a<U+00A0>b<U+2800>c<U+2003>d<U+202F>e",
                4,
            ),
            // The plain space stays, runs of it included (the residue
            // `needs_escape_in_line` records).
            ("files::write two  spaces", "files::write two  spaces", 0),
        ];
        for (input, shown, hidden) in cases {
            assert_eq!(
                escape_hidden_line(input),
                Escaped {
                    text: shown.to_string(),
                    hidden: *hidden,
                    left_out: 0,
                },
                "escape_hidden_line({input:?})"
            );
        }
    }

    /// The hand-written table is Unicode's `Default_Ignorable_Code_Point` by
    /// the `regex` crate's tables — the characters a renderer draws as
    /// nothing, assigned or reserved — so a dependency upgrade that brings a
    /// newer Unicode fails here rather than letting a new one through.
    #[test]
    fn is_default_ignorable_is_exactly_unicodes() {
        let unicode = regex::Regex::new(r"^\p{Default_Ignorable_Code_Point}$").unwrap();
        let mut buf = [0u8; 4];
        for c in every_char() {
            assert_eq!(
                is_default_ignorable(c),
                unicode.is_match(c.encode_utf8(&mut buf)),
                "U+{:04X}",
                c as u32
            );
        }
    }

    #[test]
    fn escape_hidden_shows_every_hidden_character_as_its_code_point() {
        // (input, shown, hidden)
        let cases: &[(&str, &str, usize)] = &[
            ("", "", 0),
            ("ls -la /tmp", "ls -la /tmp", 0),
            // A bidi override is shown, neither obeyed nor dropped.
            (
                "Invoice \u{202e}gpj.exe\u{202c}",
                "Invoice <U+202E>gpj.exe<U+202C>",
                2,
            ),
            // A zero-width space inside a path is a different path.
            ("/srv/allow\u{200b}ed", "/srv/allow<U+200B>ed", 1),
            // C1: U+009B is a terminal's one-byte CSI.
            ("a\u{9b}31mb", "a<U+009B>31mb", 1),
            // A carriage return can overwrite a line at a terminal.
            ("done\r", "done<U+000D>", 1),
            // Astral: a tag character, five hex digits.
            ("tag\u{e0041}", "tag<U+E0041>", 1),
            ("\u{1d173}", "<U+1D173>", 1),
            // At least four digits, zero-padded.
            ("\u{0}\u{1b}[2J\u{7f}", "<U+0000><U+001B>[2J<U+007F>", 3),
            // Line and paragraph separators are escaped, not laid out.
            ("one\u{2028}two\u{2029}", "one<U+2028>two<U+2029>", 2),
            // Isolates, the BOM and the soft hyphen.
            (
                "\u{feff}Edi\u{ad}tor \u{2066}x\u{2069}",
                "<U+FEFF>Edi<U+00AD>tor <U+2066>x<U+2069>",
                4,
            ),
            // Newline and tab are the layout, and stay.
            ("{\n\t\"a\": 1\n}", "{\n\t\"a\": 1\n}", 0),
            // Visible text stays as written: CJK, right-to-left letters (not
            // format characters), emoji — an emoji's joiner is the price.
            ("日本語 שלום 👩\u{200d}💻", "日本語 שלום 👩<U+200D>💻", 1),
            // Default-ignorable but neither Cc nor Cf: a variation selector
            // inside a path is a different path that reads the same.
            ("/srv/allow\u{fe00}ed", "/srv/allow<U+FE00>ed", 1),
            // A Hangul filler as an identifier: code that reads as one thing.
            (
                "const { timeout,\u{3164} } = q; exec(\u{3164});",
                "const { timeout,<U+3164> } = q; exec(<U+3164>);",
                2,
            ),
            // The combining grapheme joiner and a Hangul choseong filler.
            ("a\u{115f}b\u{34f}c", "a<U+115F>b<U+034F>c", 2),
            // An astral variation selector; reserved (unassigned) ignorables,
            // which a newer renderer may already draw as nothing.
            (
                "vs\u{e0100} t\u{e0002} \u{fff0}x\u{2065}",
                "vs<U+E0100> t<U+E0002> <U+FFF0>x<U+2065>",
                4,
            ),
            // An emoji's variation selector is the same price as its joiner.
            ("\u{2764}\u{fe0f}", "\u{2764}<U+FE0F>", 1),
        ];
        for (input, shown, hidden) in cases {
            assert_eq!(
                escape_hidden(input),
                Escaped {
                    text: shown.to_string(),
                    hidden: *hidden,
                    left_out: 0,
                },
                "escape_hidden({input:?})"
            );
        }
    }

    /// Pretty-printed JSON keeps its structural newlines and indentation;
    /// only what `serde_json` passes through raw inside a string — it
    /// escapes C0 controls itself, not Cf, C1 or the separators — is shown
    /// escaped.
    #[test]
    fn pretty_json_keeps_its_layout_and_shows_what_serde_passes_through() {
        let args = serde_json::json!({
            "cmd": "cat /srv/\u{202e}txt.sh",
            "note": "a\u{200b}b\u{9b}\u{2028}\r",
        });
        let pretty = serde_json::to_string_pretty(&args).unwrap();
        let escaped = escape_hidden(&pretty);
        assert_eq!(
            escaped.text,
            "{\n  \"cmd\": \"cat /srv/<U+202E>txt.sh\",\n  \
             \"note\": \"a<U+200B>b<U+009B><U+2028>\\r\"\n}"
        );
        assert_eq!(escaped.hidden, 4);
    }

    /// The cut comes after escaping and never lands inside an escape or a
    /// character: at every budget the text shown is a whole-piece prefix of
    /// the uncut text, within the budget, and the count is the whole input's.
    #[test]
    fn escape_hidden_within_never_cuts_inside_an_escape_or_a_character() {
        let input = "ab\u{202e}c日\u{e0041}🙂\u{200b}";
        let whole = escape_hidden(input);
        assert_eq!(whole.text, "ab<U+202E>c日<U+E0041>🙂<U+200B>");
        // Where a cut may end: before or after each whole piece.
        let pieces = [
            "a",
            "b",
            "<U+202E>",
            "c",
            "日",
            "<U+E0041>",
            "🙂",
            "<U+200B>",
        ];
        let mut ends = vec![0];
        for piece in pieces {
            ends.push(ends.last().unwrap() + piece.len());
        }
        for budget in 0..=whole.text.len() + 2 {
            let cut = escape_hidden_within(input, budget);
            let shown = ends.iter().rposition(|&end| end <= budget).unwrap();
            assert_eq!(cut.text, whole.text[..ends[shown]], "budget {budget}");
            assert!(cut.text.len() <= budget, "budget {budget}");
            assert_eq!(
                cut.left_out,
                pieces.len() - shown,
                "budget {budget}: every input character after the cut"
            );
            assert_eq!(
                cut.hidden, 3,
                "budget {budget}: the count is the whole input's"
            );
        }
    }

    /// A cut stops at the first piece that does not fit — a later, shorter
    /// piece never slips in after a gap.
    #[test]
    fn escape_hidden_within_is_a_prefix_not_a_packing() {
        let cut = escape_hidden_within("ab\u{200b}cd", 4);
        assert_eq!(
            cut,
            Escaped {
                text: "ab".to_string(),
                hidden: 1,
                left_out: 3,
            }
        );
    }

    #[test]
    fn hidden_notice_is_one_line_only_when_something_was_escaped() {
        assert_eq!(hidden_notice(0), None);
        assert_eq!(
            hidden_notice(1).as_deref(),
            Some("Note: this request contains 1 hidden character, shown as <U+…>.")
        );
        assert_eq!(
            hidden_notice(3).as_deref(),
            Some("Note: this request contains 3 hidden characters, shown as <U+…>.")
        );
    }

    /// The GUI's table, read as committed.
    fn gui_table() -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/openalpaca-gui/src/lib/hidden-chars.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
    }

    /// The GUI derives its set from `hidden-chars.json`; this holds that
    /// table to [`needs_escape`] over every scalar value, so a change on
    /// either side fails here (and the GUI's own test holds its module to
    /// the same table).
    #[test]
    fn needs_escape_is_the_gui_table() {
        let table = gui_table();
        let ranges: Vec<(u32, u32)> = table["ranges"]
            .as_array()
            .expect("ranges is an array")
            .iter()
            .map(|range| {
                let bound = |i: usize| {
                    let hex = range[i].as_str().expect("a range bound is a hex string");
                    u32::from_str_radix(hex, 16).expect("a range bound is hex")
                };
                (bound(0), bound(1))
            })
            .collect();
        for pair in ranges.windows(2) {
            assert!(
                pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0,
                "ranges must be sorted and disjoint: {pair:X?}"
            );
        }
        for c in every_char() {
            let code = c as u32;
            let listed = ranges.iter().any(|&(from, to)| (from..=to).contains(&code));
            assert_eq!(needs_escape(c), listed, "U+{code:04X}");
        }
    }

    /// The GUI's one-line set is its `ranges` plus `line_adds` (ranges of
    /// the same shape, sorted, disjoint and outside `ranges`), held to
    /// [`needs_escape_in_line`] over every scalar value.
    #[test]
    fn needs_escape_in_line_is_the_gui_table() {
        let table = gui_table();
        let ranges_of = |key: &str| -> Vec<(u32, u32)> {
            let hex = |value: &serde_json::Value| {
                let hex = value.as_str().expect("a range bound is a hex string");
                u32::from_str_radix(hex, 16).expect("a range bound is hex")
            };
            table[key]
                .as_array()
                .unwrap_or_else(|| panic!("{key} is an array"))
                .iter()
                .map(|range| (hex(&range[0]), hex(&range[1])))
                .collect()
        };
        let ranges = ranges_of("ranges");
        let adds = ranges_of("line_adds");
        for pair in adds.windows(2) {
            assert!(
                pair[0].0 <= pair[0].1 && pair[0].1 < pair[1].0,
                "line_adds must be sorted and disjoint: {pair:X?}"
            );
        }
        let within = |set: &[(u32, u32)], code: u32| {
            set.iter().any(|&(from, to)| (from..=to).contains(&code))
        };
        for c in every_char() {
            let code = c as u32;
            assert!(
                !(within(&ranges, code) && within(&adds, code)),
                "U+{code:04X} is in both ranges and line_adds"
            );
            let listed = within(&ranges, code) || within(&adds, code);
            assert_eq!(needs_escape_in_line(c), listed, "U+{code:04X}");
        }
    }

    /// The GUI's one-line worked cases are what Rust shows.
    #[test]
    fn the_gui_tables_line_cases_are_what_rust_shows() {
        let table = gui_table();
        let cases = table["line_cases"]
            .as_array()
            .expect("line_cases is an array");
        assert!(!cases.is_empty());
        for case in cases {
            let input = case["input"].as_str().expect("input");
            assert_eq!(
                escape_hidden_line(input),
                Escaped {
                    text: case["shown"].as_str().expect("shown").to_string(),
                    hidden: case["hidden"].as_u64().expect("hidden") as usize,
                    left_out: 0,
                },
                "escape_hidden_line({input:?})"
            );
        }
    }

    /// The GUI's worked cases and notices are what Rust shows.
    #[test]
    fn the_gui_tables_cases_and_notices_are_what_rust_shows() {
        let table = gui_table();
        let cases = table["cases"].as_array().expect("cases is an array");
        assert!(!cases.is_empty());
        for case in cases {
            let input = case["input"].as_str().expect("input");
            assert_eq!(
                escape_hidden(input),
                Escaped {
                    text: case["shown"].as_str().expect("shown").to_string(),
                    hidden: case["hidden"].as_u64().expect("hidden") as usize,
                    left_out: 0,
                },
                "escape_hidden({input:?})"
            );
        }
        let notices = table["notices"].as_array().expect("notices is an array");
        assert!(!notices.is_empty());
        for notice in notices {
            let hidden = notice["hidden"].as_u64().expect("hidden") as usize;
            assert_eq!(
                hidden_notice(hidden).as_deref(),
                notice["text"].as_str(),
                "hidden_notice({hidden})"
            );
        }
    }

    /// The hand-written table is Unicode's: every scalar value is hidden
    /// exactly when it is `Cc` or `Cf` by the `regex` crate's tables, so a
    /// dependency upgrade that brings a newer Unicode fails here rather than
    /// letting a new format character through.
    #[test]
    fn is_hidden_is_exactly_cc_and_cf() {
        let unicode = regex::Regex::new(r"^[\p{Cc}\p{Cf}]$").unwrap();
        let mut buf = [0u8; 4];
        for c in (0..=0x10ffff_u32).filter_map(char::from_u32) {
            assert_eq!(
                is_hidden(c),
                unicode.is_match(c.encode_utf8(&mut buf)),
                "U+{:04X}",
                c as u32
            );
        }
    }

    #[test]
    fn one_line_flattens_whitespace_and_drops_hidden_characters() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("plain text", "plain text"),
            ("  two\n\tlines \r\n", "two lines"),
            // Line and paragraph separators are whitespace.
            ("one\u{2028}two\u{2029}three", "one two three"),
            // Controls: ESC sequences lose their ESC, BEL and NUL go.
            ("a\u{1b}[2Jb\u{7}\u{0}", "a[2Jb"),
            // A bidi override is dropped, not obeyed: the stored order shows.
            ("Invoice \u{202e}gpj.exe\u{202c}", "Invoice gpj.exe"),
            // Isolates and embeddings.
            ("\u{2066}x\u{2069} \u{202a}y\u{202c}", "x y"),
            // Zero-width characters, the BOM and the soft hyphen.
            ("Edi\u{200b}tor\u{ad}\u{feff}", "Editor"),
            // A word made only of hidden characters leaves no double space.
            ("a \u{200b}\u{200d} b", "a b"),
            // What is visible stays, emoji and CJK included.
            ("日本語 🙂", "日本語 🙂"),
        ];
        for (input, expected) in cases {
            assert_eq!(one_line(input), *expected, "one_line({input:?})");
        }
    }

    /// Every one of these but the ASCII rows panicked under `&s[..n]`.
    #[test]
    fn byte_prefix_rounds_down_to_a_character_boundary() {
        // (input, budget, expected)
        let cases: &[(&str, usize, &str)] = &[
            // Empty string: nothing to cut, any budget.
            ("", 0, ""),
            ("", 80, ""),
            // Zero budget: nothing kept, even when the first char is wide.
            ("hello", 0, ""),
            ("日本語", 0, ""),
            // Budget at or past the end: the whole string.
            ("hello", 5, "hello"),
            ("hello", 500, "hello"),
            ("日本語", 9, "日本語"),
            ("日本語", 500, "日本語"),
            // ASCII under budget: an exact byte cut, unchanged behaviour.
            ("hello world", 5, "hello"),
            // 2-byte characters: budget 2 is exactly on the boundary, budget
            // 1 and 3 land inside one and drop it.
            ("üü", 1, ""),
            ("üü", 2, "ü"),
            ("üü", 3, "ü"),
            ("üü", 4, "üü"),
            // Mixed widths: "ünïcödé" is ü(2) n(1) ï(2) …, so byte 3 IS a
            // boundary and byte 4 is not.
            ("ünïcödé", 3, "ün"),
            ("ünïcödé", 4, "ün"),
            ("ünïcödé", 5, "ünï"),
            // 3-byte CJK: budget 4 and 5 both land inside the second char.
            ("日本語", 3, "日"),
            ("日本語", 4, "日"),
            ("日本語", 5, "日"),
            ("日本語", 6, "日本"),
            // 4-byte emoji: only a multiple of 4 keeps a whole one.
            ("🙂🙂", 4, "🙂"),
            ("🙂🙂", 5, "🙂"),
            ("🙂🙂", 7, "🙂"),
            ("🙂🙂", 8, "🙂🙂"),
            // Mixed: the budget must not cut the emoji in half.
            ("ok🙂", 3, "ok"),
            ("ok🙂", 6, "ok🙂"),
        ];

        for (input, budget, expected) in cases {
            assert_eq!(
                byte_prefix(input, *budget),
                *expected,
                "byte_prefix({input:?}, {budget})"
            );
            assert!(
                byte_prefix(input, *budget).len() <= *budget,
                "byte_prefix({input:?}, {budget}) overspent its byte budget"
            );
        }
    }
}
