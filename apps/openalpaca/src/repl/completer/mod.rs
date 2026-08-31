//! Stateless ReplHelper — tab completion + inline hints for rustyline v17

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

/// Client-side commands plus the known daemon-forwarded commands
/// (/cancel, /pause, /resume, /status, /steer, /tasks). Unknown slashes are
/// forwarded to the daemon as-is, so skill commands work without completion.
const COMMANDS: &[&str] = &[
    "/agents", "/cancel", "/clear", "/help", "/keys", "/model", "/models", "/pause", "/resume",
    "/status", "/steer", "/tasks", "/usage", "/verbose",
];

pub struct ReplHelper;

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        _pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        if !line.starts_with('/') {
            return Ok((0, vec![]));
        }
        let matches: Vec<Pair> = COMMANDS
            .iter()
            .filter(|c| c.starts_with(line))
            .map(|c| Pair {
                display: c.to_string(),
                replacement: c.to_string(),
            })
            .collect();
        Ok((0, matches))
    }
}

impl Hinter for ReplHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        if pos < line.len() || !line.starts_with('/') {
            return None;
        }
        COMMANDS
            .iter()
            .find(|c| c.starts_with(line) && **c != line)
            .map(|c| c[line.len()..].to_string())
    }
}

impl Highlighter for ReplHelper {}
impl Validator for ReplHelper {}
impl Helper for ReplHelper {}

#[cfg(test)]
mod tests;
