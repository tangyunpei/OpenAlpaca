//! Platform-specific helpers for tool execution.

/// Build a `tokio::process::Command` that runs the given shell command
/// using the platform's default shell.
///
/// - macOS / Linux: `sh -c "<cmd>"`
/// - Windows: `cmd /c "<cmd>"`
pub fn shell_command(cmd: &str) -> tokio::process::Command {
    if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(cmd);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_command_creates_command() {
        // Verify it doesn't panic — actual execution is OS-dependent
        let _cmd = shell_command("echo hello");
    }
}
