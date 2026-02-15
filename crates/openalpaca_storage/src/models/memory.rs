//! Memory v2 model: owner-scoped, typed, deduped memory entries.

use serde::{Deserialize, Serialize};

/// A memory entry in the v2 schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryV2 {
    pub id: i64,
    pub owner_id: String,
    pub kind: MemoryKind,
    pub scope: MemoryScope,
    pub scope_id: String,
    pub source: MemorySource,
    pub content: String,
    pub content_hash: String,
    pub importance: f64,
    pub confidence: f64,
    pub created_at: String,
    pub metadata: Option<serde_json::Value>,
    // --- Lifecycle fields (migration 018) ---
    /// When this memory was last modified (e.g. via supersession).
    pub updated_at: Option<String>,
    /// If this memory superseded another, the old memory's id.
    pub supersedes_id: Option<i64>,
    /// Last time this memory was retrieved (for importance decay).
    pub last_accessed_at: Option<String>,
}

/// The type of memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Preference,
    KbChunk,
}

impl MemoryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::KbChunk => "kb_chunk",
        }
    }
}

impl std::str::FromStr for MemoryKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fact" => Ok(Self::Fact),
            "preference" => Ok(Self::Preference),
            "kb_chunk" => Ok(Self::KbChunk),
            _ => anyhow::bail!("Invalid memory kind: {}", s),
        }
    }
}

/// The scope hierarchy of a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Global,
    Workspace,
    Lane,
}

impl MemoryScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Workspace => "workspace",
            Self::Lane => "lane",
        }
    }
}

impl std::str::FromStr for MemoryScope {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "global" => Ok(Self::Global),
            "workspace" => Ok(Self::Workspace),
            "lane" => Ok(Self::Lane),
            _ => anyhow::bail!("Invalid memory scope: {}", s),
        }
    }
}

/// The source of a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Conversation,
    Tool,
    Ingestion,
}

impl MemorySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Tool => "tool",
            Self::Ingestion => "ingestion",
        }
    }
}

impl std::str::FromStr for MemorySource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "conversation" => Ok(Self::Conversation),
            "tool" => Ok(Self::Tool),
            "ingestion" => Ok(Self::Ingestion),
            _ => anyhow::bail!("Invalid memory source: {}", s),
        }
    }
}
