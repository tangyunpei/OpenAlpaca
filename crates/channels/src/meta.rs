use openalpaca_core::types::{ChannelId, ChatType};

#[derive(Debug, Clone)]
pub struct ChannelMeta {
    pub id: ChannelId,
    pub label: String,
    pub blurb: String,
    pub order: u32,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelCapabilities {
    pub chat_types: Vec<ChatType>,
    pub polls: bool,
    pub reactions: bool,
    pub edit: bool,
    pub threads: bool,
    pub media: bool,
    pub reply: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelAccountSnapshot {
    pub account_id: String,
    pub name: Option<String>,
    pub enabled: bool,
    pub configured: bool,
    pub connected: bool,
    pub running: bool,
    pub last_error: Option<String>,
}
