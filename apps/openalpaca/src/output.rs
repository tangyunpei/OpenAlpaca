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

/// Print a table header row with separator.
pub fn print_table_header(headers: &[(&str, usize)]) {
    let header_line: String = headers
        .iter()
        .map(|(name, width)| format!("{:<width$}", name, width = width))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{}", header_line.dimmed());

    let total_width: usize = headers.iter().map(|(_, w)| w + 1).sum::<usize>();
    println!("{}", "-".repeat(total_width).dimmed());
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
