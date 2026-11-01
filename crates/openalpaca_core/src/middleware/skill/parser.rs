use super::SkillParseError;

/// Extract the raw YAML string between `---` delimiters.
/// Returns (yaml_str, body_lines) on success.
///
/// One leading byte-order mark is dropped first: U+FEFF is not White_Space,
/// so `trim_start` keeps it, and a file saved with one would read as having
/// no frontmatter. The closing `---` is a line of its own (trailing spaces and
/// a CR allowed), so a `----` or `---x` line is part of the frontmatter, not
/// its end.
pub(super) fn extract_frontmatter_str(input: &str) -> Result<(&str, Vec<String>), SkillParseError> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let trimmed = input.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillParseError::MissingFrontmatter);
    }

    // Find the opening ---
    let after_first = &trimmed[3..];
    // Skip the rest of the opening line (should be just newline)
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    // Find the closing ---: the first line, after the opening one, that is
    // `---` and nothing else.
    let mut line_start = 0;
    loop {
        let rest = &after_first[line_start..];
        let (line, next_line) = match rest.find('\n') {
            Some(end) => (&rest[..end], Some(line_start + end + 1)),
            None => (rest, None),
        };
        if line.trim_end() == "---" {
            // The YAML ends before the newline that ends the line above.
            let yaml_str = &after_first[..line_start.saturating_sub(1)];
            let remainder = next_line.map_or("", |next| &after_first[next..]);
            let body_lines: Vec<String> = remainder.lines().map(|l| l.to_string()).collect();
            return Ok((yaml_str, body_lines));
        }
        match next_line {
            Some(next) => line_start = next,
            None => return Err(SkillParseError::UnterminatedFrontmatter),
        }
    }
}
