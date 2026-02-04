//! Identity models for OpenAlpaca
//!
//! Defines identity-related entities: GlobalUser, ExternalIdentity, ConversationMap, LinkToken

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// GlobalUser: The canonical identity in the system
/// Think of this as the "Passport" - each user has exactly one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalUser {
    pub id: String, // UUID
    pub display_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// ExternalIdentity: A user's identity from an external provider (Telegram, iMessage, etc.)
/// Multiple external identities can map to one global_user via /link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub id: i64,
    pub provider: String,               // 'telegram', 'imessage', 'wechat', etc.
    pub provider_user_id: String,       // The ID from the provider
    pub global_user_id: Option<String>, // NULL if not linked yet
    pub display_name: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub linked_at: Option<DateTime<Utc>>,
}

/// ConversationMap: Maps a provider's conversation/chat to a global_user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMap {
    pub id: i64,
    pub provider: String,
    pub provider_conversation_id: String,
    pub global_user_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// LinkToken: Temporary tokens for /link command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkToken {
    pub id: i64,
    pub token: String,
    pub global_user_id: String,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
