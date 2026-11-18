use super::*;

/// TEMPLATE-TEMP-OPTIONAL (item 24 of the owner's answer of 2026-11-14):
/// the creation screen no longer asks for a temperature, which nothing
/// sends to a model, so the body it posts carries none — at any depth, the
/// cost constraint or not.
#[test]
fn the_screens_create_body_carries_no_temperature() {
    let skills = vec!["research".to_string()];
    for max_cost in [0.0, 2.5] {
        let body = create_agent_body(
            "Reader",
            "Reads things",
            "some-model",
            "You read.",
            &skills,
            max_cost,
        );
        assert!(!body.to_string().contains("temperature"), "{body}");
        assert_eq!(body["config"]["name"], "Reader");
        assert_eq!(body["config"]["persona"], "You read.");
    }
}

/// 121-2: the credential-file check looks under the home it is given — in
/// production `paths::user_home`'s, `std::env::home_dir` when absolute — and
/// with no usable home looks for nothing, never a relative
/// `.claude/.credentials.json` the process's directory resolves. A temporary
/// home: no test reaches the machine's credential files.
#[test]
fn the_credential_file_check_reads_the_home_it_is_given() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    assert!(!credential_file_present(Some(home), "claude_code"));
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::write(home.join(".codex/auth.json"), "{}").unwrap();
    assert!(credential_file_present(Some(home), "claude_code"));
    assert!(credential_file_present(Some(home), "codex"));
    assert!(!credential_file_present(Some(home), "other"));
    assert!(!credential_file_present(None, "claude_code"));
    assert_eq!(
        openalpaca_app_support::paths::usable_home(Some("relative-home".into())),
        None
    );
}
