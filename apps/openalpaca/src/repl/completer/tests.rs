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
