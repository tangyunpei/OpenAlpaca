//! CLI backend providers for fallback.
//!
//! Implements `LlmProvider` by spawning local CLI processes (`claude`, `codex`).
//! These are used as a last-resort fallback when API calls fail.

use crate::LlmProvider;
use crate::error::LlmError;
use crate::types::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for CLI backends (from config file).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliBackendsConfig {
    pub claude_code: Option<CliBackendConfig>,
    pub codex: Option<CliBackendConfig>,
}

/// Configuration for a single CLI backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliBackendConfig {
    pub path: Option<String>,
    pub enabled: Option<bool>,
    pub timeout_secs: Option<u64>,
}

/// Status info for a CLI backend (for API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliBackendStatus {
    pub name: String,
    pub available: bool,
    pub path: Option<String>,
    pub enabled: bool,
}

/// Claude Code CLI provider (`claude -p "{prompt}" --output-format json`).
pub struct ClaudeCodeCliProvider {
    binary_path: PathBuf,
    timeout: Duration,
}

impl ClaudeCodeCliProvider {
    pub fn new(binary_path: PathBuf, timeout: Duration) -> Self {
        Self {
            binary_path,
            timeout,
        }
    }

    /// Detect `claude` binary on PATH.
    pub fn detect() -> Option<PathBuf> {
        which::which("claude").ok()
    }

    /// Detect with config override.
    pub fn from_config(config: &CliBackendConfig) -> Option<Self> {
        if config.enabled == Some(false) {
            return None;
        }

        let path = config
            .path
            .as_ref()
            .map(PathBuf::from)
            .or_else(Self::detect)?;

        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(120));
        Some(Self::new(path, timeout))
    }

    pub fn binary_path(&self) -> &PathBuf {
        &self.binary_path
    }
}

#[async_trait]
impl LlmProvider for ClaudeCodeCliProvider {
    fn name(&self) -> &str {
        "claude_cli"
    }

    fn supports_tools(&self) -> bool {
        false
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let prompt = flatten_for_cli(&request.messages);

        let result = tokio::time::timeout(
            self.timeout,
            tokio::process::Command::new(&self.binary_path)
                .args(["-p", &prompt, "--output-format", "json"])
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(LlmError::CliBackend(format!(
                        "claude CLI failed (exit {}): {}",
                        output.status,
                        stderr.trim()
                    )));
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_claude_output(&stdout)
            }
            Ok(Err(e)) => Err(LlmError::CliBackend(format!(
                "Failed to spawn claude CLI: {}",
                e
            ))),
            Err(_) => Err(LlmError::CliBackend(format!(
                "claude CLI timed out after {}s",
                self.timeout.as_secs()
            ))),
        }
    }

    async fn chat_with_key(
        &self,
        _key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        // CLI uses its own auth, ignore key
        self.chat(request).await
    }

    async fn list_models_with_key(&self, _key: &str) -> Result<Vec<String>, LlmError> {
        Ok(vec![])
    }
}

/// Codex CLI provider (`codex exec "{prompt}" --json`).
pub struct CodexCliProvider {
    binary_path: PathBuf,
    timeout: Duration,
}

impl CodexCliProvider {
    pub fn new(binary_path: PathBuf, timeout: Duration) -> Self {
        Self {
            binary_path,
            timeout,
        }
    }

    /// Detect `codex` binary on PATH.
    pub fn detect() -> Option<PathBuf> {
        which::which("codex").ok()
    }

    /// Detect with config override.
    pub fn from_config(config: &CliBackendConfig) -> Option<Self> {
        if config.enabled == Some(false) {
            return None;
        }

        let path = config
            .path
            .as_ref()
            .map(PathBuf::from)
            .or_else(Self::detect)?;

        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(120));
        Some(Self::new(path, timeout))
    }

    pub fn binary_path(&self) -> &PathBuf {
        &self.binary_path
    }
}

#[async_trait]
impl LlmProvider for CodexCliProvider {
    fn name(&self) -> &str {
        "codex_cli"
    }

    fn supports_tools(&self) -> bool {
        false
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let prompt = flatten_for_cli(&request.messages);

        let result = tokio::time::timeout(
            self.timeout,
            tokio::process::Command::new(&self.binary_path)
                .args(["exec", &prompt, "--json"])
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(LlmError::CliBackend(format!(
                        "codex CLI failed (exit {}): {}",
                        output.status,
                        stderr.trim()
                    )));
                }

                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_codex_output(&stdout)
            }
            Ok(Err(e)) => Err(LlmError::CliBackend(format!(
                "Failed to spawn codex CLI: {}",
                e
            ))),
            Err(_) => Err(LlmError::CliBackend(format!(
                "codex CLI timed out after {}s",
                self.timeout.as_secs()
            ))),
        }
    }

    async fn chat_with_key(
        &self,
        _key: &str,
        request: ChatRequest,
    ) -> Result<ChatResponse, LlmError> {
        self.chat(request).await
    }

    async fn list_models_with_key(&self, _key: &str) -> Result<Vec<String>, LlmError> {
        Ok(vec![])
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Flatten messages into a single prompt string for CLI backends.
fn flatten_for_cli(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            Role::System => parts.push(format!("[System] {}", msg.content)),
            Role::User => parts.push(msg.content.clone()),
            Role::Assistant => parts.push(format!("[Assistant] {}", msg.content)),
            Role::Tool => parts.push(format!("[Tool] {}", msg.content)),
        }
    }
    parts.join("\n")
}

/// Parse Claude CLI JSON output.
/// Format: `{"result": "...", "is_error": false, ...}`
fn parse_claude_output(stdout: &str) -> Result<ChatResponse, LlmError> {
    #[derive(Deserialize)]
    struct ClaudeOutput {
        result: Option<String>,
        is_error: Option<bool>,
    }

    match serde_json::from_str::<ClaudeOutput>(stdout.trim()) {
        Ok(parsed) => {
            if parsed.is_error == Some(true) {
                return Err(LlmError::CliBackend(
                    parsed
                        .result
                        .unwrap_or_else(|| "Unknown CLI error".to_string()),
                ));
            }

            let content = parsed.result.unwrap_or_default();
            Ok(ChatResponse {
                content,
                tool_calls: vec![],
                model: "claude_cli".to_string(),
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
        Err(_) => {
            // Fallback: treat raw stdout as plain text
            Ok(ChatResponse {
                content: stdout.trim().to_string(),
                tool_calls: vec![],
                model: "claude_cli".to_string(),
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
    }
}

/// Parse Codex CLI JSON output.
/// Format: `{"response": "...", ...}`
fn parse_codex_output(stdout: &str) -> Result<ChatResponse, LlmError> {
    #[derive(Deserialize)]
    struct CodexOutput {
        response: Option<String>,
    }

    match serde_json::from_str::<CodexOutput>(stdout.trim()) {
        Ok(parsed) => {
            let content = parsed.response.unwrap_or_default();
            Ok(ChatResponse {
                content,
                tool_calls: vec![],
                model: "codex_cli".to_string(),
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
        Err(_) => {
            // Fallback: treat raw stdout as plain text
            Ok(ChatResponse {
                content: stdout.trim().to_string(),
                tool_calls: vec![],
                model: "codex_cli".to_string(),
                usage: Usage::default(),
                finish_reason: FinishReason::Stop,
                thinking: None,
                parts: None,
            })
        }
    }
}

/// Detect available CLI backends based on config.
pub fn detect_cli_backends(config: &CliBackendsConfig) -> Vec<CliBackendStatus> {
    let mut statuses = Vec::new();

    // Claude Code
    let claude_config = config.claude_code.as_ref();
    let claude_enabled = claude_config.and_then(|c| c.enabled).unwrap_or(true);
    let claude_path = claude_config
        .and_then(|c| c.path.as_ref().map(PathBuf::from))
        .or_else(ClaudeCodeCliProvider::detect);

    statuses.push(CliBackendStatus {
        name: "claude_code".to_string(),
        available: claude_path.is_some(),
        path: claude_path.map(|p| p.to_string_lossy().to_string()),
        enabled: claude_enabled,
    });

    // Codex
    let codex_config = config.codex.as_ref();
    let codex_enabled = codex_config.and_then(|c| c.enabled).unwrap_or(true);
    let codex_path = codex_config
        .and_then(|c| c.path.as_ref().map(PathBuf::from))
        .or_else(CodexCliProvider::detect);

    statuses.push(CliBackendStatus {
        name: "codex".to_string(),
        available: codex_path.is_some(),
        path: codex_path.map(|p| p.to_string_lossy().to_string()),
        enabled: codex_enabled,
    });

    statuses
}

#[cfg(test)]
mod tests;
