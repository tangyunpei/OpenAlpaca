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
        assert_eq!(body["config"]["agent"]["name"], "Reader");
        assert_eq!(body["config"]["preset"]["persona"], "You read.");
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

/// 121-2's wiring: `check_credential_file` itself reads the home
/// `paths::user_home` reports. A child of this binary runs the check under
/// an absolute `HOME`, where the planted files are found, and under a
/// relative one naming, from the child's directory, a folder that holds
/// them too, where none is looked for: a hand read of `HOME` would find
/// them. Everything is in a temporary folder, so no test reaches the
/// machine's credential files.
#[test]
fn the_credential_check_reads_the_home_user_home_reports() {
    use crate::test_util::{HOME_CHILD_ENV, run_with_home};
    if std::env::var_os(HOME_CHILD_ENV).is_some() {
        let home = std::env::var_os("HOME").expect("the child's HOME");
        let usable = std::path::Path::new(&home).is_absolute();
        assert_eq!(check_credential_file("claude_code"), usable, "{home:?}");
        assert_eq!(check_credential_file("codex"), usable, "{home:?}");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for home in ["abs-home", "rel-home"] {
        let home = root.join(home);
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/auth.json"), "{}").unwrap();
    }
    let test = concat!(
        module_path!(),
        "::the_credential_check_reads_the_home_user_home_reports"
    );
    run_with_home(test, &root.join("abs-home"), root, &[]);
    run_with_home(test, std::path::Path::new("rel-home"), root, &[]);
}
