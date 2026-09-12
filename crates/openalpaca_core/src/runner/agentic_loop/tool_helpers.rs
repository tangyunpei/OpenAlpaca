/// The default inline bound for a tool result (32 KB), and the fallback for
/// every loop that was given no `[orchestrator.sessions]
/// tool_result_inline_bytes` — a subagent loop, a skill, a test.
pub(crate) const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;

/// Truncate tool result text at the default bound.
#[cfg(test)]
pub(super) fn truncate_tool_result(text: String) -> String {
    truncate_tool_result_to(text, MAX_TOOL_RESULT_SIZE)
}

/// Keep an oversized result's **head and tail**, with a marker naming what
/// went (§5.4: "`Err` results stay inline but switch to head+tail — compiler
/// and test errors sit at the tail").
///
/// A head-only cut on a `cargo test` failure throws away the very lines the
/// model needs; half the budget at each end keeps both the command that ran
/// and the assertion that failed.
pub(super) fn head_tail_tool_result(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let half = limit / 2;
    let mut head_end = half;
    while head_end > 0 && !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len().saturating_sub(half).max(head_end);
    while tail_start < text.len() && !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let dropped = tail_start - head_end;
    format!(
        "{}\n\n[... {} of {} bytes elided; the tail follows ...]\n\n{}",
        &text[..head_end],
        dropped,
        text.len(),
        &text[tail_start..]
    )
}

/// Truncate tool result text if it exceeds `limit` to prevent blowing up the
/// LLM context window. Uses byte-aware truncation at char boundaries.
pub(super) fn truncate_tool_result_to(text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }

    // Find the nearest char boundary at or before the byte limit
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    let slice = &text[..end];

    // Try sentence boundary (last ". " or ".\n" or "! " or "!\n" or "? " or "?\n")
    let sentence_end = slice.rfind(". ")
        .or_else(|| slice.rfind(".\n"))
        .or_else(|| slice.rfind("! "))
        .or_else(|| slice.rfind("!\n"))
        .or_else(|| slice.rfind("? "))
        .or_else(|| slice.rfind("?\n"))
        .map(|pos| pos + 1); // Include the punctuation char

    // Try line boundary
    let line_end = slice.rfind('\n');

    // Try word boundary
    let word_end = slice.rfind(' ');

    // Don't cut more than 25% short — avoid distant sentence boundaries
    // discarding most of the content
    let min_cut = end * 3 / 4;

    // Pick best boundary: sentence (if recent enough) > line > word > char
    let cut = sentence_end
        .filter(|&p| p >= min_cut)
        .or_else(|| line_end.filter(|&p| p >= min_cut))
        .or_else(|| word_end.filter(|&p| p >= min_cut))
        .unwrap_or(end);

    format!(
        "{}\n\n[... truncated: showing first {} of {} bytes]",
        &text[..cut],
        cut,
        text.len()
    )
}

/// Format a tool error message with the standard `[tool_error]` prefix.
/// Centralizes the error format so the LLM always sees a consistent pattern.
pub(super) fn format_tool_error(msg: &str) -> String {
    format!("[tool_error] {}", msg)
}

/// Tool-specific recovery suggestions for common error patterns.
fn tool_recovery_hint(tool_name: &str, error: &str) -> Option<&'static str> {
    if tool_name == "file_read" && (error.contains("not found") || error.contains("No such file")) {
        return Some("Hint: verify the path exists using shell_execute with `ls`.");
    }
    if tool_name == "file_write" && error.contains("Permission denied") {
        return Some("Hint: check file permissions or try a different output path.");
    }
    if tool_name == "web_fetch" && (error.contains("404") || error.contains("not found")) {
        return Some("Hint: use web_search to find the correct URL first.");
    }
    if tool_name == "web_fetch" && error.contains("timeout") {
        return Some("Hint: the URL may be unreachable. Try a different source.");
    }
    if tool_name == "shell_execute" && error.contains("timed out") {
        return Some("Hint: break the command into smaller steps or increase timeout.");
    }
    if tool_name == "shell_execute" && error.contains("not found") {
        return Some("Hint: check if the command is installed or use the full path.");
    }
    if tool_name == "memory_search" && error.contains("no results") {
        return Some("Hint: try broader search terms or check workspace_read for shared context.");
    }
    None
}

/// Format a tool error with an optional recovery hint appended.
pub(super) fn format_tool_error_with_hint(tool_name: &str, msg: &str) -> String {
    let base = format_tool_error(msg);
    match tool_recovery_hint(tool_name, msg) {
        Some(hint) => format!("{}\n{}", base, hint),
        None => base,
    }
}
