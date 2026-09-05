// crates/openalpaca_core/src/tools/mcp/config.rs

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level MCP configuration parsed from `config/mcp.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub defaults: McpDefaults,
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpDefaults {
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default = "default_max_reconnect")]
    pub max_reconnect_attempts: u32,
    #[serde(default = "default_reconnect_backoff")]
    pub reconnect_backoff_ms: u64,
}

impl Default for McpDefaults {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout(),
            request_timeout_secs: default_request_timeout(),
            max_reconnect_attempts: default_max_reconnect(),
            reconnect_backoff_ms: default_reconnect_backoff(),
        }
    }
}

fn default_connect_timeout() -> u64 { 30 }
fn default_request_timeout() -> u64 { 30 }
fn default_max_reconnect() -> u32 { 3 }
fn default_reconnect_backoff() -> u64 { 100 }
fn default_enabled() -> bool { true }

/// One `[servers.<name>]` block.
///
/// `Serialize` exists for exactly one reader: the `config_fingerprint`
/// preimage of extension design §3.3 E2 (`super::fingerprint`), which hashes a
/// canonical rendering of the *parsed* block so that a comment, a blank line or
/// a key-order edit changes nothing while a value edit does. The writer does
/// not use it — that is a surgical `toml_edit` assignment (§2.1).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum McpServerConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        connect_timeout_secs: Option<u64>,
        #[serde(default)]
        request_timeout_secs: Option<u64>,
        #[serde(default = "default_enabled")]
        enabled: bool,
    },
    Http {
        url: url::Url,
        #[serde(default)]
        auth: Option<HttpAuthConfig>,
        #[serde(default)]
        extra_headers: HashMap<String, String>,
        #[serde(default)]
        connect_timeout_secs: Option<u64>,
        #[serde(default)]
        request_timeout_secs: Option<u64>,
        #[serde(default = "default_enabled")]
        enabled: bool,
    },
}

impl McpServerConfig {
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Stdio { enabled, .. } | Self::Http { enabled, .. } => *enabled,
        }
    }

    pub fn transport_kind(&self) -> &'static str {
        match self {
            Self::Stdio { .. } => "stdio",
            Self::Http { .. } => "streamable-http",
        }
    }

    pub fn connect_timeout_secs(&self) -> Option<u64> {
        match self {
            Self::Stdio { connect_timeout_secs, .. }
            | Self::Http { connect_timeout_secs, .. } => *connect_timeout_secs,
        }
    }

    pub fn request_timeout_secs(&self) -> Option<u64> {
        match self {
            Self::Stdio { request_timeout_secs, .. }
            | Self::Http { request_timeout_secs, .. } => *request_timeout_secs,
        }
    }
}

/// `Serialize` for the same single reader as [`McpServerConfig`] — the
/// fingerprint preimage, where the auth *kind* is structure and the literal
/// `bearer` value is masked before it is hashed.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum HttpAuthConfig {
    BearerEnv { bearer_env: String },
    Bearer { bearer: String },
    ApiKey {
        api_key_header: String,
        api_key_env: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("config file not found at {0}")]
    NotFound(PathBuf),
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid server name '{0}': must match ^[a-zA-Z][a-zA-Z0-9_-]{{0,30}}$")]
    InvalidServerName(String),
}

impl McpConfig {
    /// Load and validate MCP config from a TOML file.
    pub fn load(path: &Path) -> Result<Self, LoadError> {
        if !path.exists() {
            return Err(LoadError::NotFound(path.to_path_buf()));
        }
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    /// Parse and validate MCP config from TOML text.
    ///
    /// Split out of [`Self::load`] so the atomic writer can re-parse its own
    /// rendered output with **the reader's own parser** before the rename
    /// (extension design §2.1): a `toml_edit` index-assignment can synthesize a
    /// structurally valid table this type rejects — `enabled` assigned into a
    /// `[servers.<n>]` block that no longer exists produces a table with no
    /// `transport` tag — and that must abort the write, not land.
    pub fn parse(text: &str) -> Result<Self, LoadError> {
        let config: McpConfig = toml::from_str(text)?;
        for name in config.servers.keys() {
            if !is_valid_server_name(name) {
                return Err(LoadError::InvalidServerName(name.clone()));
            }
        }
        Ok(config)
    }
}

fn is_valid_server_name(s: &str) -> bool {
    if s.len() > 31 { return false; }
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_alphabetic() { return false; }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn write_temp_toml(contents: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{contents}").unwrap();
        f
    }

    #[test]
    fn load_missing_returns_not_found() {
        let p = Path::new("/definitely/does/not/exist/mcp.toml");
        let err = McpConfig::load(p).unwrap_err();
        assert!(matches!(err, LoadError::NotFound(_)), "got {err:?}");
    }

    #[test]
    fn load_malformed_returns_parse_err() {
        let tmp = write_temp_toml("not a valid = = toml [[[");
        let err = McpConfig::load(tmp.path()).unwrap_err();
        assert!(matches!(err, LoadError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn load_empty_returns_ok_default() {
        let tmp = write_temp_toml("");
        let cfg = McpConfig::load(tmp.path()).unwrap();
        assert!(cfg.servers.is_empty());
        assert_eq!(cfg.defaults.connect_timeout_secs, 30);
        assert_eq!(cfg.defaults.max_reconnect_attempts, 3);
    }

    #[test]
    fn load_parses_stdio_server() {
        let toml = r#"
            [servers.fs]
            transport = "stdio"
            command = "npx"
            args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        "#;
        let tmp = write_temp_toml(toml);
        let cfg = McpConfig::load(tmp.path()).unwrap();
        let server = cfg.servers.get("fs").expect("fs server");
        assert_eq!(server.transport_kind(), "stdio");
        assert!(server.is_enabled());
        match server {
            McpServerConfig::Stdio { command, args, .. } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 3);
            }
            _ => panic!("expected stdio variant"),
        }
    }

    #[test]
    fn load_parses_http_server_with_bearer_env() {
        let toml = r#"
            [servers.brave]
            transport = "http"
            url = "https://example.com/mcp"
            auth = { bearer_env = "BRAVE_TOKEN" }
        "#;
        let tmp = write_temp_toml(toml);
        let cfg = McpConfig::load(tmp.path()).unwrap();
        let server = cfg.servers.get("brave").expect("brave server");
        match server {
            McpServerConfig::Http { url, auth, .. } => {
                assert_eq!(url.as_str(), "https://example.com/mcp");
                match auth {
                    Some(HttpAuthConfig::BearerEnv { bearer_env }) => {
                        assert_eq!(bearer_env, "BRAVE_TOKEN")
                    }
                    other => panic!("expected BearerEnv, got {other:?}"),
                }
            }
            _ => panic!("expected http variant"),
        }
    }

    #[test]
    fn load_parses_http_server_with_api_key() {
        let toml = r#"
            [servers.gh]
            transport = "http"
            url = "https://example.com/mcp"
            auth = { api_key_header = "X-API-Key", api_key_env = "GH_TOKEN" }
        "#;
        let tmp = write_temp_toml(toml);
        let cfg = McpConfig::load(tmp.path()).unwrap();
        let server = cfg.servers.get("gh").unwrap();
        match server {
            McpServerConfig::Http { auth: Some(HttpAuthConfig::ApiKey { api_key_header, api_key_env }), .. } => {
                assert_eq!(api_key_header, "X-API-Key");
                assert_eq!(api_key_env, "GH_TOKEN");
            }
            _ => panic!("expected ApiKey auth"),
        }
    }

    #[test]
    fn load_rejects_invalid_server_name() {
        let toml = r#"
            [servers."has spaces"]
            transport = "stdio"
            command = "x"
        "#;
        let tmp = write_temp_toml(toml);
        let err = McpConfig::load(tmp.path()).unwrap_err();
        assert!(matches!(err, LoadError::InvalidServerName(_)), "got {err:?}");
    }

    #[test]
    fn server_name_valid_cases() {
        assert!(is_valid_server_name("fs"));
        assert!(is_valid_server_name("my-server"));
        assert!(is_valid_server_name("Srv_1"));
        assert!(is_valid_server_name("a"));
        assert!(is_valid_server_name("a0"));
    }

    #[test]
    fn server_name_invalid_cases() {
        assert!(!is_valid_server_name(""));
        assert!(!is_valid_server_name("1abc"));
        assert!(!is_valid_server_name("-abc"));
        assert!(!is_valid_server_name("_abc"));
        assert!(!is_valid_server_name("has spaces"));
        assert!(!is_valid_server_name("dotted.name"));
        assert!(!is_valid_server_name(&"a".repeat(32)));
    }

    #[test]
    fn enabled_default_true() {
        let toml = r#"
            [servers.x]
            transport = "stdio"
            command = "c"
        "#;
        let tmp = write_temp_toml(toml);
        let cfg = McpConfig::load(tmp.path()).unwrap();
        assert!(cfg.servers.get("x").unwrap().is_enabled());
    }

    #[test]
    fn enabled_false_honored() {
        let toml = r#"
            [servers.x]
            transport = "stdio"
            command = "c"
            enabled = false
        "#;
        let tmp = write_temp_toml(toml);
        let cfg = McpConfig::load(tmp.path()).unwrap();
        assert!(!cfg.servers.get("x").unwrap().is_enabled());
    }
}
