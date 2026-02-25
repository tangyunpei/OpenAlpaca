use super::*;

#[test]
fn test_parse_help() {
    assert!(matches!(SlashCommand::parse("/help"), SlashCommand::Help));
}

#[test]
fn test_parse_status() {
    assert!(matches!(
        SlashCommand::parse("/status"),
        SlashCommand::Status
    ));
}

#[test]
fn test_parse_model() {
    assert!(matches!(SlashCommand::parse("/model"), SlashCommand::Model));
}

#[test]
fn test_parse_models() {
    assert!(matches!(
        SlashCommand::parse("/models"),
        SlashCommand::Models
    ));
}

#[test]
fn test_parse_agents() {
    assert!(matches!(
        SlashCommand::parse("/agents"),
        SlashCommand::Agents
    ));
}

#[test]
fn test_parse_tasks_default() {
    match SlashCommand::parse("/tasks") {
        SlashCommand::Tasks(n) => assert_eq!(n, 5),
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_parse_tasks_with_arg() {
    match SlashCommand::parse("/tasks 10") {
        SlashCommand::Tasks(n) => assert_eq!(n, 10),
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_parse_tasks_invalid_arg() {
    match SlashCommand::parse("/tasks abc") {
        SlashCommand::Tasks(n) => assert_eq!(n, 5),
        _ => panic!("expected Tasks"),
    }
}

#[test]
fn test_parse_keys() {
    assert!(matches!(SlashCommand::parse("/keys"), SlashCommand::Keys));
}

#[test]
fn test_parse_usage() {
    assert!(matches!(SlashCommand::parse("/usage"), SlashCommand::Usage));
}

#[test]
fn test_parse_clear() {
    assert!(matches!(SlashCommand::parse("/clear"), SlashCommand::Clear));
}

#[test]
fn test_parse_verbose() {
    assert!(matches!(
        SlashCommand::parse("/verbose"),
        SlashCommand::Verbose
    ));
}

#[test]
fn test_parse_unknown() {
    match SlashCommand::parse("/foo") {
        SlashCommand::Unknown(c) => assert_eq!(c, "/foo"),
        _ => panic!("expected Unknown"),
    }
}
