//! Stateless ReplHelper — tab completion + inline hints for rustyline v17

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

const COMMANDS: &[&str] = &[
    "/agents", "/clear", "/help", "/keys", "/model", "/models", "/status", "/tasks", "/usage",
    "/verbose",
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
mod tests {
    use super::*;

    fn complete(input: &str) -> Vec<String> {
        let helper = ReplHelper;
        let (_, pairs) = helper
            .complete(
                input,
                input.len(),
                &Context::new(&rustyline::history::DefaultHistory::new()),
            )
            .unwrap();
        pairs.into_iter().map(|p| p.replacement).collect()
    }

    fn hint(input: &str) -> Option<String> {
        let helper = ReplHelper;
        helper.hint(
            input,
            input.len(),
            &Context::new(&rustyline::history::DefaultHistory::new()),
        )
    }

    #[test]
    fn test_complete_mod() {
        let results = complete("/mod");
        assert!(results.contains(&"/model".to_string()));
        assert!(results.contains(&"/models".to_string()));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_complete_status() {
        let results = complete("/status");
        assert_eq!(results, vec!["/status".to_string()]);
    }

    #[test]
    fn test_complete_non_slash() {
        let results = complete("hello");
        assert!(results.is_empty());
    }

    #[test]
    fn test_complete_all() {
        let results = complete("/");
        assert_eq!(results.len(), COMMANDS.len());
    }

    #[test]
    fn test_hint_partial() {
        assert_eq!(hint("/sta"), Some("tus".to_string()));
    }

    #[test]
    fn test_hint_exact() {
        assert_eq!(hint("/status"), None);
    }

    #[test]
    fn test_hint_non_slash() {
        assert_eq!(hint("hello"), None);
    }
}
