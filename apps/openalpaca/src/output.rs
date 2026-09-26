//! Output formatting utilities for CLI commands
//!
//! Provides table and JSON output modes, with colored status indicators.

use colored::{ColoredString, Colorize};
use serde::Serialize;

/// Output format selector for CLI commands.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

/// Trait for types that can be rendered as a table row.
pub trait TableRow {
    /// Column headers and their widths.
    fn headers() -> Vec<(&'static str, usize)>;
    /// Render one row as a formatted string.
    fn table_row(&self) -> String;
}

/// Print a list of items in the chosen format.
pub fn print_list<T: Serialize + TableRow>(items: &[T], format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(items).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            if items.is_empty() {
                println!("{}", "No items found.".dimmed());
                return;
            }
            print_table_header(&T::headers());
            for item in items {
                println!("{}", item.table_row());
            }
        }
    }
}

/// A dollar figure, four decimals, with negative zero erased (L12).
///
/// `openalpaca llm status` printed `Daily cost: $-0.0000`. Nothing was refunded:
/// IEEE `-0.0` — which a sum of priced-at-zero calls can land on — formats with
/// its sign, and an owner reading a minus sign against their spend is being
/// told something untrue. `-0.0 == 0.0` is what erases it; every other figure,
/// including a genuinely negative one, prints exactly as it came.
pub fn format_usd(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("${value:.4}")
}

/// Colorize a status string based on common patterns.
pub fn status_color(status: &str) -> ColoredString {
    match status.to_lowercase().as_str() {
        "running" | "active" | "ok" | "idle" | "success" | "completed" => status.green(),
        "failed" | "error" | "disabled" | "cancelled" => status.red(),
        // `interrupted` (§5.6b) is terminal but is not a failure — the daemon
        // went away — so it is not painted red.
        "paused" | "waiting" | "queued" | "pending" | "interrupted" => status.yellow(),
        _ => status.dimmed(),
    }
}

/// What an approval prompt writes first on a terminal: a reset of five
/// modes that change how the prompt reads, none of which moves the cursor —
/// shift-in (SI), G0 as ASCII (`ESC ( B`), SGR 0, replace mode (`ESC [ 4 l`)
/// and auto-wrap on (`ESC [ ? 7 h`).
///
/// The prompt's own escape shows every hidden character in what it prints,
/// but it cannot undo what was printed before it: auto-wrap turned off cuts
/// an argument at the right margin with nothing left for a notice to count,
/// a line-drawing character set garbles the letters (an SGR reset does not
/// undo it), and a conceal or background colour left set hides the headline
/// — `colored` resets only after the text it paints, and under `NO_COLOR`
/// writes nothing at all. Nothing into a pipe.
///
/// Not undone: a scroll region, the cursor's position or text drawn
/// elsewhere on the screen (resetting a scroll region moves the cursor), and
/// palette colours changed with OSC 4, 10 or 11 (resetting them, OSC 104,
/// 110 and 111, would also throw away a theme the person set at run time —
/// base16-shell, say — at every prompt). None of it can come from what
/// `openalpaca chat` prints of the model's or a tool's text: the answer, its
/// reasoning, a delegated run's result, a resumed conversation's turns and a
/// failed turn's message all reach a terminal through `escape_controls`
/// (`chat_stream::model_text`). What is left is other output in that
/// terminal: other programs', and CLI listings that print stored or
/// third-party text as it comes — `tasks log` and `tasks status` print a
/// run's title, summary or waiting tool that way.
pub const TERMINAL_RESET: &str = "\u{f}\u{1b}(B\u{1b}[0m\u{1b}[4l\u{1b}[?7h";

/// [`TERMINAL_RESET`] on a terminal, nothing on a pipe.
pub fn terminal_reset(tty: bool) -> &'static str {
    if tty { TERMINAL_RESET } else { "" }
}

/// `json` with each hidden character written as a JSON `\uXXXX` escape.
///
/// JSON a person reads at a terminal but a program may parse: the one-line
/// set (`openalpaca_core::utils::text::needs_escape_in_line`) in JSON's own
/// lossless form rather than `<U+XXXX>`, so the document still reads back as
/// exactly the daemon's values — `tasks confirmations list --format json`,
/// and the payloads of `tasks log`. No notice: it would not be JSON.
///
/// Sound on `serde_json`'s output and only there: it writes JSON's structure
/// in ASCII laid out with `\n` and spaces, and escapes every C0 control
/// inside a string itself, so every other character of the one-line set
/// (`needs_escape_in_line`) it leaves raw is inside a string, where the
/// escape is the same character to any reader.
pub fn json_with_hidden_escaped(json: &str) -> String {
    use openalpaca_core::utils::text::needs_escape_in_line;
    use std::fmt::Write as _;

    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        if c != '\n' && needs_escape_in_line(c) {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                // Writing into a `String` cannot fail.
                let _ = write!(out, "\\u{unit:04x}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Print a table header row with separator.
pub fn print_table_header(headers: &[(&str, usize)]) {
    let (header_line, rule) = table_header_lines(headers);
    println!("{}", header_line.dimmed());
    println!("{}", rule.dimmed());
}

/// A table's header row and the rule under it, unpainted: what
/// [`print_table_header`] prints, for a command that builds its output as
/// text so a test can read it.
pub fn table_header_lines(headers: &[(&str, usize)]) -> (String, String) {
    let header_line: String = headers
        .iter()
        .map(|(name, width)| format!("{:<width$}", name, width = width))
        .collect::<Vec<_>>()
        .join(" ");
    let total_width: usize = headers.iter().map(|(_, w)| w + 1).sum::<usize>();
    (header_line, "-".repeat(total_width))
}

/// Fit a cell to `max` **characters**, ellipsis included.
///
/// A char budget, not a byte one, because the padding around every call site
/// is `{:<width$}` — `std`'s fill counts characters, so a byte budget would
/// leave the columns ragged the moment a cell held anything but ASCII.
///
/// Char-safe by construction: byte-slicing `&s[..max - 3]` panics whenever the
/// cut lands inside a multibyte character, and every string that reaches here
/// can hold one — a task title (the dispatcher caps titles at 50 *characters*,
/// which is up to 150 bytes), an agent or model id, a plugin directory name, a
/// daemon-generated `reason`. `ext list`, `tasks list` and `llm status` are
/// the surfaces an operator reaches for when something is already wrong, so
/// they must not be the thing that panics. ASCII-identical to the byte slice
/// this replaced.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let head: String = s.chars().take(keep).collect();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use super::truncate;

    /// A cell whose cut lands inside a multibyte character used to panic —
    /// these surfaces are what an operator reaches for when something has
    /// already gone wrong, and an id, a title and a daemon-generated `reason`
    /// can all hold one.
    #[test]
    fn truncate_cuts_on_character_boundaries() {
        assert_eq!(truncate("short", 21), "short");
        // Exactly the width: untouched.
        assert_eq!(truncate("abcde", 5), "abcde");
        // Over the width: 3 chars of ellipsis, `max - 3` chars kept.
        assert_eq!(truncate("abcdefgh", 5), "ab...");
        // Multibyte, cut mid-character under the old byte slice.
        assert_eq!(truncate("ünïcödé-server-name", 8), "ünïcö...");
        assert_eq!(truncate("日本語のサーバー", 6), "日本語...");
        // Every emoji is 4 bytes; the old code panicked on all of these.
        assert_eq!(truncate("🙂🙂🙂🙂🙂🙂", 4), "🙂...");
    }
}
