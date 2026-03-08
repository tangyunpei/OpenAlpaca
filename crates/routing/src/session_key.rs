use std::collections::HashMap;

use openalpaca_core::types::{AccountId, AgentId, ChannelId, ChatType, SessionKey};

/// DM scope determines how session keys are partitioned for direct messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DmScope {
    /// All DMs share one session per agent.
    Main,
    /// Each peer gets their own session.
    PerPeer,
    /// Each (channel, peer) pair gets a session.
    PerChannelPeer,
    /// Each (account, channel, peer) tuple gets a session.
    PerAccountChannelPeer,
}

impl DmScope {
    pub fn from_str_loose(s: &str) -> Self {
        match s.to_lowercase().replace('-', "_").as_str() {
            "main" => DmScope::Main,
            "per_peer" | "perpeer" => DmScope::PerPeer,
            "per_channel_peer" | "perchannelpeer" => DmScope::PerChannelPeer,
            "per_account_channel_peer" | "peraccountchannelpeer" => DmScope::PerAccountChannelPeer,
            other => {
                tracing::warn!(
                    value = other,
                    "unknown dm_scope variant, defaulting to per_peer"
                );
                DmScope::PerPeer
            }
        }
    }
}

/// Parameters for building a peer session key.
pub struct PeerSessionParams {
    pub agent_id: AgentId,
    pub channel: ChannelId,
    pub account_id: AccountId,
    pub peer_kind: ChatType,
    pub peer_id: String,
    pub dm_scope: DmScope,
    pub identity_links: Option<HashMap<String, Vec<String>>>,
}

/// Build the main session key: `agent:{agent_id}:{main_key}`.
/// If `main_key` is None, uses "main".
pub fn build_main_session_key(agent_id: &AgentId, main_key: Option<&str>) -> SessionKey {
    let key = main_key.unwrap_or("main");
    SessionKey::new(format!("agent:{}:{}", agent_id.as_str(), key))
}

/// Build a peer session key with DM scope logic.
///
/// - Direct+Main => `agent:{id}:main`
/// - Direct+PerPeer => `agent:{id}:direct:{peer_id}`
/// - Direct+PerChannelPeer => `agent:{id}:{channel}:direct:{peer_id}`
/// - Direct+PerAccountChannelPeer => `agent:{id}:{account}:{channel}:direct:{peer_id}`
/// - Group => `agent:{id}:{channel}:group:{peer_id}`
/// - Channel => `agent:{id}:{channel}:channel:{peer_id}`
pub fn build_peer_session_key(params: &PeerSessionParams) -> SessionKey {
    let agent = params.agent_id.as_str();

    match params.peer_kind {
        ChatType::Direct => {
            let peer = resolve_peer_id(params);
            match params.dm_scope {
                DmScope::Main => build_main_session_key(&params.agent_id, None),
                DmScope::PerPeer => SessionKey::new(format!("agent:{agent}:direct:{peer}")),
                DmScope::PerChannelPeer => SessionKey::new(format!(
                    "agent:{agent}:{}:direct:{peer}",
                    params.channel.as_str()
                )),
                DmScope::PerAccountChannelPeer => SessionKey::new(format!(
                    "agent:{agent}:{}:{}:direct:{peer}",
                    params.account_id.as_str(),
                    params.channel.as_str()
                )),
            }
        }
        ChatType::Group => SessionKey::new(format!(
            "agent:{agent}:{}:group:{}",
            params.channel.as_str(),
            &params.peer_id
        )),
        ChatType::Channel => SessionKey::new(format!(
            "agent:{agent}:{}:channel:{}",
            params.channel.as_str(),
            &params.peer_id
        )),
    }
}

/// Resolve thread session keys. If a thread_id is provided, returns:
/// - thread key: `{base}:thread:{thread_id}`
/// - parent key: Some(base_key)
///
/// Otherwise returns (base_key, None).
pub fn resolve_thread_session_keys(
    base_key: &SessionKey,
    thread_id: Option<&str>,
) -> (SessionKey, Option<SessionKey>) {
    match thread_id {
        Some(tid) if !tid.is_empty() => {
            let thread_key = SessionKey::new(format!("{}:thread:{tid}", base_key.as_str()));
            (thread_key, Some(base_key.clone()))
        }
        _ => (base_key.clone(), None),
    }
}

/// Resolve peer ID, checking identity_links for cross-platform mapping.
fn resolve_peer_id(params: &PeerSessionParams) -> &str {
    if let Some(ref links) = params.identity_links {
        for ids in links.values() {
            if ids.iter().any(|id| id == &params.peer_id)
                && let Some(canonical) = ids.first()
            {
                return canonical.as_str();
            }
        }
    }
    &params.peer_id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(s: &str) -> AgentId {
        AgentId::new_unchecked(s)
    }
    fn channel(s: &str) -> ChannelId {
        ChannelId::new_unchecked(s)
    }
    fn account(s: &str) -> AccountId {
        AccountId::new_unchecked(s)
    }

    #[test]
    fn test_build_main_session_key_default() {
        let key = build_main_session_key(&agent("assistant"), None);
        assert_eq!(key.as_str(), "agent:assistant:main");
    }

    #[test]
    fn test_build_main_session_key_custom() {
        let key = build_main_session_key(&agent("assistant"), Some("workspace"));
        assert_eq!(key.as_str(), "agent:assistant:workspace");
    }

    #[test]
    fn test_direct_main_scope() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("telegram"),
            account_id: account("default"),
            peer_kind: ChatType::Direct,
            peer_id: "user123".into(),
            dm_scope: DmScope::Main,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:main");
    }

    #[test]
    fn test_direct_per_peer_scope() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("telegram"),
            account_id: account("default"),
            peer_kind: ChatType::Direct,
            peer_id: "user123".into(),
            dm_scope: DmScope::PerPeer,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:direct:user123");
    }

    #[test]
    fn test_direct_per_channel_peer_scope() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("telegram"),
            account_id: account("default"),
            peer_kind: ChatType::Direct,
            peer_id: "user123".into(),
            dm_scope: DmScope::PerChannelPeer,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:telegram:direct:user123");
    }

    #[test]
    fn test_direct_per_account_channel_peer_scope() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("telegram"),
            account_id: account("work"),
            peer_kind: ChatType::Direct,
            peer_id: "user123".into(),
            dm_scope: DmScope::PerAccountChannelPeer,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:work:telegram:direct:user123");
    }

    #[test]
    fn test_group_session_key() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("discord"),
            account_id: account("default"),
            peer_kind: ChatType::Group,
            peer_id: "guild-general".into(),
            dm_scope: DmScope::PerPeer,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:discord:group:guild-general");
    }

    #[test]
    fn test_channel_session_key() {
        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("slack"),
            account_id: account("default"),
            peer_kind: ChatType::Channel,
            peer_id: "C12345".into(),
            dm_scope: DmScope::PerPeer,
            identity_links: None,
        };
        let key = build_peer_session_key(&params);
        assert_eq!(key.as_str(), "agent:bot:slack:channel:C12345");
    }

    #[test]
    fn test_thread_session_keys_with_thread() {
        let base = SessionKey::new("agent:bot:direct:user123");
        let (thread_key, parent) = resolve_thread_session_keys(&base, Some("thread-456"));
        assert_eq!(
            thread_key.as_str(),
            "agent:bot:direct:user123:thread:thread-456"
        );
        assert_eq!(parent.unwrap().as_str(), "agent:bot:direct:user123");
    }

    #[test]
    fn test_thread_session_keys_without_thread() {
        let base = SessionKey::new("agent:bot:direct:user123");
        let (key, parent) = resolve_thread_session_keys(&base, None);
        assert_eq!(key.as_str(), "agent:bot:direct:user123");
        assert!(parent.is_none());
    }

    #[test]
    fn test_dm_scope_from_str_loose() {
        assert_eq!(DmScope::from_str_loose("main"), DmScope::Main);
        assert_eq!(DmScope::from_str_loose("per-peer"), DmScope::PerPeer);
        assert_eq!(DmScope::from_str_loose("per_peer"), DmScope::PerPeer);
        assert_eq!(
            DmScope::from_str_loose("per-channel-peer"),
            DmScope::PerChannelPeer
        );
        assert_eq!(
            DmScope::from_str_loose("per-account-channel-peer"),
            DmScope::PerAccountChannelPeer
        );
        assert_eq!(DmScope::from_str_loose("unknown"), DmScope::PerPeer);
    }

    #[test]
    fn test_identity_links_resolve() {
        let mut links = HashMap::new();
        links.insert(
            "alice".to_string(),
            vec!["alice-tg".to_string(), "alice-dc".to_string()],
        );

        let params = PeerSessionParams {
            agent_id: agent("bot"),
            channel: channel("discord"),
            account_id: account("default"),
            peer_kind: ChatType::Direct,
            peer_id: "alice-dc".into(),
            dm_scope: DmScope::PerPeer,
            identity_links: Some(links),
        };
        let key = build_peer_session_key(&params);
        // Should use canonical ID "alice-tg" from identity links
        assert_eq!(key.as_str(), "agent:bot:direct:alice-tg");
    }
}
