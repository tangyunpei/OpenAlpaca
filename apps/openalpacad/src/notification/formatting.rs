//! Notification message formatting (pure functions, testable).

/// Build completion notification message.
pub(super) fn format_completion_message(
    title: &str,
    summary: Option<&str>,
    outcome_kind: Option<&str>,
    artifact_count: Option<i32>,
    outcome_summary: Option<&str>,
) -> String {
    let display_summary = outcome_summary.or(summary).unwrap_or("Done");

    let outcome_line = match outcome_kind {
        Some("text_only") => "\nNo files were produced.".to_string(),
        Some("artifact_only") => {
            let count = artifact_count.unwrap_or(0);
            format!(
                "\n{} file{} produced.",
                count,
                if count != 1 { "s" } else { "" }
            )
        }
        Some("mixed") => {
            let count = artifact_count.unwrap_or(0);
            format!(
                "\n{} file{} produced (with text summary).",
                count,
                if count != 1 { "s" } else { "" }
            )
        }
        _ => String::new(),
    };

    format!(
        "Task completed: {}\n\n{}{}",
        title, display_summary, outcome_line
    )
}

/// Build failure notification message.
///
/// When `artifact_count` indicates artifacts from earlier steps, we note
/// they may still be available instead of claiming "no files".
pub(super) fn format_failure_message(
    title: &str,
    error: &str,
    outcome_kind: Option<&str>,
    artifact_count: Option<i32>,
) -> String {
    let file_note = match (outcome_kind, artifact_count) {
        (Some("failed"), Some(n)) if n > 0 => {
            format!(
                "\n{} file{} from earlier steps may still be available.",
                n,
                if n != 1 { "s" } else { "" }
            )
        }
        (Some("failed"), _) => "\nNo files were produced.".to_string(),
        _ => String::new(),
    };
    format!("Task failed: {}\n\nError: {}{}", title, error, file_note)
}
