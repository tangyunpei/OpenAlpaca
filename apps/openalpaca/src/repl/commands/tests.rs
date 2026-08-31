use super::*;

#[test]
fn test_parse_help() {
    assert!(matches!(SlashCommand::parse("/help"), SlashCommand::Help));
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
fn test_local_commands_are_not_forwarded() {
    for cmd in [
        "/help", "/model", "/models", "/agents", "/keys", "/usage", "/clear", "/verbose",
    ] {
        assert!(
            !SlashCommand::parse(cmd).is_forward(),
            "{cmd} should stay local"
        );
    }
}

#[test]
fn test_known_daemon_commands_forward() {
    for cmd in [
        "/status",
        "/status abc123",
        "/tasks",
        "/steer focus on the tests",
        "/cancel",
        "/cancel abc123",
        "/pause",
        "/resume abc123",
    ] {
        assert!(
            SlashCommand::parse(cmd).is_forward(),
            "{cmd} should forward to the daemon"
        );
    }
}

#[test]
fn test_unknown_slash_forwards() {
    // Unknown slashes may be skill commands only the daemon knows.
    assert!(SlashCommand::parse("/unknownskill").is_forward());
    assert!(SlashCommand::parse("/review src/main.rs").is_forward());
}

#[test]
fn test_parse_local_command_with_args_stays_local() {
    // Arguments do not change the routing of a client-side command.
    assert!(matches!(
        SlashCommand::parse("/model extra"),
        SlashCommand::Model
    ));
}
