/// The default inline bound for a tool result (32 KB), and the fallback for
/// every loop that was given no `[orchestrator.sessions]
/// tool_result_inline_bytes` — a subagent loop, a skill, a test.
pub(crate) const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;

/// Truncate tool result text at the default bound.
#[cfg(test)]
pub(super) fn truncate_tool_result(text: String) -> String {
    truncate_tool_result_to(text, MAX_TOOL_RESULT_SIZE)
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

/// The text of a failed tool call as the model reads it: the error, marked
/// `[tool_error]`.
///
/// The mark is what an OpenAI-compatible wire, which has no error field,
/// shows; whether the call failed is the call's own `Err`, carried on the
/// message as `is_error` (N-TOOL-4, Z1-13) — the loop never reads it back
/// off this text. A recovery hint is the failing tool's own words (or the
/// sandbox's, for a command it cut at its time limit), already in `msg`.
pub(super) fn format_tool_error(msg: &str) -> String {
    format!("[tool_error] {}", msg)
}
