// crates/openalpaca_mcp/src/transport/stdio.rs

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

use crate::error::McpError;

use super::{Transport, TransportConnection, TransportInner};

/// Spawns an MCP server as a subprocess; communicates via stdin/stdout.
pub struct StdioTransport {
    server_name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
}

impl StdioTransport {
    pub fn new(server_name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.insert(key.into(), val.into());
        self
    }

    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Expose fields for testing. Not public API.
    #[cfg(test)]
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    #[cfg(test)]
    pub(crate) fn env(&self) -> &HashMap<String, String> {
        &self.env
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&self) -> Result<TransportConnection, McpError> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }

        tracing::info!(
            server_name = %self.server_name,
            command = %self.command,
            "connecting MCP stdio transport"
        );

        let child = TokioChildProcess::new(cmd).map_err(|e| {
            // TokioChildProcess::new returns an io::Error on spawn failure.
            McpError::Transport(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("failed to spawn MCP server '{}': {e}", self.command),
            ))
        })?;

        Ok(TransportConnection {
            inner: TransportInner::Stdio(child),
        })
    }

    fn kind(&self) -> &'static str {
        "stdio"
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_stores_args_and_env() {
        let t = StdioTransport::new("srv", "npx")
            .with_args(vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()])
            .with_env("FOO", "bar")
            .with_env("BAZ", "qux");
        assert_eq!(t.args(), &["-y", "@modelcontextprotocol/server-filesystem"]);
        assert_eq!(t.env().get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(t.env().get("BAZ").map(String::as_str), Some("qux"));
        assert_eq!(t.kind(), "stdio");
        assert_eq!(t.server_name(), "srv");
    }

    #[tokio::test]
    async fn connect_fails_on_nonexistent_command() {
        let t = StdioTransport::new("doesnotexist", "/definitely/not/a/real/command/xyzzy123");
        // `TransportConnection` wraps an rmcp `TokioChildProcess` which doesn't
        // implement `Debug`, so we can't use `.unwrap_err()` — match manually.
        match t.connect().await {
            Ok(_) => panic!("expected connect() to fail for nonexistent command"),
            Err(err) => assert!(
                matches!(err, McpError::Transport(_)),
                "expected Transport error, got {err:?}"
            ),
        }
    }
}
