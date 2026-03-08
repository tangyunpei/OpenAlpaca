use std::process::Command;

#[test]
fn test_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_openalpaca"))
        .arg("--version")
        .output()
        .expect("failed to run openalpaca");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("openalpaca"));
}

#[test]
fn test_help_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_openalpaca"))
        .arg("--help")
        .output()
        .expect("failed to run openalpaca");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gateway"));
    assert!(stdout.contains("config"));
}

#[test]
fn test_config_validate_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_openalpaca"))
        .args(["config", "validate"])
        .output()
        .expect("failed to run openalpaca");

    // Should succeed even without a config file (uses defaults)
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OK"));
}
