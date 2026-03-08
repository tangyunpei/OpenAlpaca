use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

static ID_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").unwrap());

fn normalize_id(value: &str) -> Result<String, CoreError> {
    let normalized = value.trim().to_lowercase();
    if !ID_PATTERN.is_match(&normalized) {
        return Err(CoreError::InvalidId(format!(
            "'{normalized}': must match [a-z0-9][a-z0-9_-]{{0,63}}"
        )));
    }
    Ok(normalized)
}

/// Channel identifier — built-in or extension.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelId(String);

impl ChannelId {
    pub fn normalize(value: &str) -> Result<Self, CoreError> {
        Ok(Self(normalize_id(value)?))
    }

    /// Create without validation. Use for deserialization or internal
    /// construction where the value is known-valid.
    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Access the inner string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ChannelId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::normalize(s)
    }
}

/// Chat type for message context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    Direct,
    Group,
    Channel,
}

impl fmt::Display for ChatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChatType::Direct => write!(f, "direct"),
            ChatType::Group => write!(f, "group"),
            ChatType::Channel => write!(f, "channel"),
        }
    }
}

impl FromStr for ChatType {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "direct" => Ok(ChatType::Direct),
            "group" => Ok(ChatType::Group),
            "channel" => Ok(ChatType::Channel),
            _ => Err(CoreError::InvalidId(format!("unknown chat type: '{s}'"))),
        }
    }
}

/// Normalized agent identifier (lowercase, alphanumeric + underscore/hyphen, max 64 chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(String);

impl AgentId {
    pub fn normalize(value: &str) -> Result<Self, CoreError> {
        Ok(Self(normalize_id(value)?))
    }

    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::normalize(s)
    }
}

/// Normalized account identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    pub fn normalize(value: &str) -> Result<Self, CoreError> {
        Ok(Self(normalize_id(value)?))
    }

    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AccountId {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::normalize(s)
    }
}

/// Session key — hierarchical string for agent context isolation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey(String);

impl SessionKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SessionKey {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(CoreError::InvalidId("session key cannot be empty".into()));
        }
        if s.len() > 256 {
            return Err(CoreError::InvalidId(format!(
                "session key too long ({} chars, max 256)",
                s.len()
            )));
        }
        if s.contains('\0') {
            return Err(CoreError::InvalidId(
                "session key contains null byte".into(),
            ));
        }
        if s.contains("..") {
            return Err(CoreError::InvalidId(
                "session key contains '..' (path traversal)".into(),
            ));
        }
        if s.contains('/') || s.contains('\\') {
            return Err(CoreError::InvalidId(
                "session key contains path separator".into(),
            ));
        }
        Ok(Self(s.to_string()))
    }
}

pub const DEFAULT_ACCOUNT_ID: &str = "default";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_id_normalize_valid() {
        let id = AgentId::normalize("MyAgent").unwrap();
        assert_eq!(id.as_str(), "myagent");
    }

    #[test]
    fn test_agent_id_normalize_with_hyphens_and_underscores() {
        let id = AgentId::normalize("my-agent_1").unwrap();
        assert_eq!(id.as_str(), "my-agent_1");
    }

    #[test]
    fn test_agent_id_normalize_rejects_empty() {
        assert!(AgentId::normalize("").is_err());
    }

    #[test]
    fn test_agent_id_normalize_rejects_too_long() {
        let long = "a".repeat(65);
        assert!(AgentId::normalize(&long).is_err());
    }

    #[test]
    fn test_agent_id_max_length_ok() {
        let max = "a".repeat(64);
        assert!(AgentId::normalize(&max).is_ok());
    }

    #[test]
    fn test_agent_id_normalize_rejects_special_chars() {
        assert!(AgentId::normalize("my@agent").is_err());
    }

    #[test]
    fn test_agent_id_normalize_rejects_leading_hyphen() {
        assert!(AgentId::normalize("-agent").is_err());
    }

    #[test]
    fn test_agent_id_normalize_rejects_leading_underscore() {
        assert!(AgentId::normalize("_agent").is_err());
    }

    #[test]
    fn test_account_id_normalize() {
        let id = AccountId::normalize("User123").unwrap();
        assert_eq!(id.as_str(), "user123");
    }

    #[test]
    fn test_channel_id_normalize() {
        let id = ChannelId::normalize("Telegram").unwrap();
        assert_eq!(id.as_str(), "telegram");
    }

    #[test]
    fn test_chat_type_serde_roundtrip() {
        let types = [ChatType::Direct, ChatType::Group, ChatType::Channel];
        for chat_type in &types {
            let json = serde_json::to_string(chat_type).unwrap();
            let deserialized: ChatType = serde_json::from_str(&json).unwrap();
            assert_eq!(chat_type, &deserialized);
        }
    }

    #[test]
    fn test_chat_type_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&ChatType::Direct).unwrap(),
            "\"direct\""
        );
        assert_eq!(
            serde_json::to_string(&ChatType::Group).unwrap(),
            "\"group\""
        );
        assert_eq!(
            serde_json::to_string(&ChatType::Channel).unwrap(),
            "\"channel\""
        );
    }

    #[test]
    fn test_chat_type_from_str() {
        assert_eq!(ChatType::from_str("direct").unwrap(), ChatType::Direct);
        assert_eq!(ChatType::from_str("GROUP").unwrap(), ChatType::Group);
        assert_eq!(ChatType::from_str("Channel").unwrap(), ChatType::Channel);
        assert!(ChatType::from_str("unknown").is_err());
    }

    #[test]
    fn test_session_key_from_str() {
        let key = SessionKey::from_str("agent:channel:account").unwrap();
        assert_eq!(key.as_str(), "agent:channel:account");
    }

    #[test]
    fn test_session_key_rejects_empty() {
        assert!(SessionKey::from_str("").is_err());
    }

    #[test]
    fn test_session_key_rejects_too_long() {
        let long = "a".repeat(257);
        assert!(SessionKey::from_str(&long).is_err());
    }

    #[test]
    fn test_session_key_max_length_ok() {
        let max = "a".repeat(256);
        assert!(SessionKey::from_str(&max).is_ok());
    }

    #[test]
    fn test_session_key_rejects_path_traversal() {
        assert!(SessionKey::from_str("../etc/passwd").is_err());
        assert!(SessionKey::from_str("foo/../bar").is_err());
    }

    #[test]
    fn test_session_key_rejects_path_separators() {
        assert!(SessionKey::from_str("foo/bar").is_err());
        assert!(SessionKey::from_str("foo\\bar").is_err());
    }

    #[test]
    fn test_session_key_rejects_null_byte() {
        assert!(SessionKey::from_str("foo\0bar").is_err());
    }

    #[test]
    fn test_session_key_allows_colons() {
        assert!(SessionKey::from_str("agent:bot:direct:user123").is_ok());
    }

    #[test]
    fn test_agent_id_display() {
        let id = AgentId::normalize("test").unwrap();
        assert_eq!(format!("{id}"), "test");
    }

    #[test]
    fn test_normalize_trims_whitespace() {
        let id = AgentId::normalize("  myagent  ").unwrap();
        assert_eq!(id.as_str(), "myagent");
    }
}
