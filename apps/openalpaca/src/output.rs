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
pub fn print_detail<T: Serialize>(item: &T, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(item).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            // For detail view, just pretty-print the JSON with indentation
            println!(
                "{}",
                serde_json::to_string_pretty(item).unwrap_or_default()
            );
        }
    }
}

/// Colorize a status string based on common patterns.
pub fn status_color(status: &str) -> ColoredString {
    match status.to_lowercase().as_str() {
        "running" | "active" | "ok" | "idle" | "success" | "completed" => status.green(),
        "failed" | "error" | "disabled" | "cancelled" => status.red(),
        "paused" | "waiting" | "queued" | "pending" => status.yellow(),
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
