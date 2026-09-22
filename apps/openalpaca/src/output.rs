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

/// Print a single item detail in the chosen format.
#[allow(dead_code)]
pub fn print_detail<T: Serialize>(item: &T, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(item).unwrap_or_default());
        }
        OutputFormat::Table => {
            // For detail view, just pretty-print the JSON with indentation
            println!("{}", serde_json::to_string_pretty(item).unwrap_or_default());
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

/// Fit a cell to a number of Unicode characters, including its ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(3)).collect();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncate_cuts_on_character_boundaries() {
        assert_eq!(truncate("short", 21), "short");
        assert_eq!(truncate("abcde", 5), "abcde");
        assert_eq!(truncate("abcdefgh", 5), "ab...");
        assert_eq!(truncate("ünïcödé-server-name", 8), "ünïcö...");
        assert_eq!(truncate("日本語のサーバー", 6), "日本語...");
        assert_eq!(truncate("🙂🙂🙂🙂🙂🙂", 4), "🙂...");
    }
}
