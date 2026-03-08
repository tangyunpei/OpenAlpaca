use std::collections::HashMap;

use openalpaca_core::types::{AccountId, ChannelId};

use crate::plugin::ChannelPlugin;

/// Registry key: (ChannelId, AccountId) to support multi-account channels.
pub type ChannelKey = (ChannelId, AccountId);

/// Registry of all active channel plugins, keyed by (channel_id, account_id).
pub struct ChannelRegistry {
    channels: HashMap<ChannelKey, Box<dyn ChannelPlugin>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }

    /// Register a channel plugin for a specific account.
    pub fn register(&mut self, account_id: AccountId, plugin: Box<dyn ChannelPlugin>) {
        let key = (plugin.id().clone(), account_id);
        self.channels.insert(key, plugin);
    }

    /// Look up a specific channel + account.
    pub fn get(
        &self,
        channel_id: &ChannelId,
        account_id: &AccountId,
    ) -> Option<&dyn ChannelPlugin> {
        let key = (channel_id.clone(), account_id.clone());
        self.channels.get(&key).map(|b| b.as_ref())
    }

    /// Get all plugins for a given channel ID (all accounts).
    pub fn get_by_channel(&self, channel_id: &ChannelId) -> Vec<&dyn ChannelPlugin> {
        self.channels
            .iter()
            .filter(|((cid, _), _)| cid == channel_id)
            .map(|(_, plugin)| plugin.as_ref())
            .collect()
    }

    /// List all registered plugins.
    pub fn list(&self) -> Vec<&dyn ChannelPlugin> {
        self.channels.values().map(|p| p.as_ref()).collect()
    }

    /// Iterate all registered entries with their keys.
    pub fn entries(&self) -> Vec<(&ChannelId, &AccountId, &dyn ChannelPlugin)> {
        self.channels
            .iter()
            .map(|((cid, aid), plugin)| (cid, aid, plugin.as_ref()))
            .collect()
    }

    /// Number of registered channel-account pairs.
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::echo::EchoChannel;

    #[test]
    fn test_register_and_get() {
        let mut registry = ChannelRegistry::new();
        let echo = EchoChannel::new();
        let channel_id = echo.id().clone();
        let account_id = AccountId("default".into());

        registry.register(account_id.clone(), Box::new(echo));

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        let plugin = registry.get(&channel_id, &account_id).unwrap();
        assert_eq!(plugin.id().0, "echo");
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = ChannelRegistry::new();
        let channel_id = ChannelId("telegram".into());
        let account_id = AccountId("default".into());
        assert!(registry.get(&channel_id, &account_id).is_none());
    }

    #[test]
    fn test_get_by_channel() {
        let mut registry = ChannelRegistry::new();
        registry.register(AccountId("a1".into()), Box::new(EchoChannel::new()));
        registry.register(AccountId("a2".into()), Box::new(EchoChannel::new()));

        let channel_id = ChannelId("echo".into());
        let plugins = registry.get_by_channel(&channel_id);
        assert_eq!(plugins.len(), 2);
    }

    #[test]
    fn test_list_all() {
        let mut registry = ChannelRegistry::new();
        registry.register(AccountId("default".into()), Box::new(EchoChannel::new()));
        assert_eq!(registry.list().len(), 1);
    }
}
