use std::collections::HashMap;

use openalpaca_config::types_agents::AgentBinding;
use openalpaca_core::types::AgentId;

/// A pre-evaluated binding ready for route matching.
#[derive(Debug, Clone)]
pub struct EvaluatedBinding {
    pub agent_id: AgentId,
    pub channel: Option<String>,
    pub account: Option<String>,
    pub peer_kind: Option<String>,
    pub peer_id: Option<String>,
    pub guild: Option<String>,
    pub team: Option<String>,
    pub roles: Option<Vec<String>>,
}

/// Pre-indexed bindings for O(1) route lookups.
#[derive(Debug, Clone, Default)]
pub struct BindingIndex {
    pub by_peer: HashMap<String, Vec<EvaluatedBinding>>,
    pub by_guild_roles: HashMap<String, Vec<EvaluatedBinding>>,
    pub by_guild: HashMap<String, Vec<EvaluatedBinding>>,
    pub by_team: HashMap<String, Vec<EvaluatedBinding>>,
    pub by_account: Vec<EvaluatedBinding>,
    pub by_channel: Vec<EvaluatedBinding>,
    pub default: Option<EvaluatedBinding>,
}

impl BindingIndex {
    /// Build a binding index from config bindings.
    pub fn build(bindings: &[AgentBinding]) -> Self {
        let mut index = BindingIndex::default();

        for binding in bindings {
            let agent_id = AgentId::new_unchecked(binding.agent.to_lowercase());
            let eval = EvaluatedBinding {
                agent_id,
                channel: binding.channel.clone(),
                account: binding.account.clone(),
                peer_kind: binding.peer.as_ref().and_then(|p| p.kind.clone()),
                peer_id: binding.peer.as_ref().map(|p| p.id.clone()),
                guild: binding.guild.clone(),
                team: binding.team.clone(),
                roles: binding.roles.clone(),
            };

            if eval.peer_id.is_some() {
                let key = build_peer_key(
                    eval.channel.as_deref(),
                    eval.account.as_deref(),
                    eval.peer_id.as_deref().unwrap_or(""),
                );
                index.by_peer.entry(key).or_default().push(eval);
            } else if eval.guild.is_some() && eval.roles.is_some() {
                let key =
                    build_guild_key(eval.channel.as_deref(), eval.guild.as_deref().unwrap_or(""));
                index.by_guild_roles.entry(key).or_default().push(eval);
            } else if eval.guild.is_some() {
                let key =
                    build_guild_key(eval.channel.as_deref(), eval.guild.as_deref().unwrap_or(""));
                index.by_guild.entry(key).or_default().push(eval);
            } else if eval.team.is_some() {
                let key =
                    build_team_key(eval.channel.as_deref(), eval.team.as_deref().unwrap_or(""));
                index.by_team.entry(key).or_default().push(eval);
            } else if eval.account.is_some() {
                index.by_account.push(eval);
            } else if eval.channel.is_some() {
                index.by_channel.push(eval);
            } else {
                // No channel, no account, no peer — this is a default/fallback binding
                index.default = Some(eval);
            }
        }

        index
    }
}

pub fn build_peer_key(channel: Option<&str>, account: Option<&str>, peer_id: &str) -> String {
    format!(
        "{}:{}:{}",
        channel.unwrap_or("*"),
        account.unwrap_or("*"),
        peer_id
    )
}

pub fn build_guild_key(channel: Option<&str>, guild_id: &str) -> String {
    format!("{}:{}", channel.unwrap_or("*"), guild_id)
}

pub fn build_team_key(channel: Option<&str>, team_id: &str) -> String {
    format!("{}:{}", channel.unwrap_or("*"), team_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_config::types_agents::PeerBinding;

    fn make_binding(agent: &str) -> AgentBinding {
        AgentBinding {
            agent: agent.to_string(),
            binding_type: "route".to_string(),
            channel: None,
            account: None,
            peer: None,
            guild: None,
            team: None,
            roles: None,
        }
    }

    #[test]
    fn test_default_binding() {
        let bindings = vec![make_binding("fallback")];
        let index = BindingIndex::build(&bindings);
        assert!(index.default.is_some());
        assert_eq!(index.default.unwrap().agent_id.as_str(), "fallback");
    }

    #[test]
    fn test_channel_binding() {
        let mut b = make_binding("tg-agent");
        b.channel = Some("telegram".into());
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_channel.len(), 1);
        assert_eq!(index.by_channel[0].agent_id.as_str(), "tg-agent");
    }

    #[test]
    fn test_account_binding() {
        let mut b = make_binding("work-agent");
        b.channel = Some("telegram".into());
        b.account = Some("work".into());
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_account.len(), 1);
    }

    #[test]
    fn test_peer_binding() {
        let mut b = make_binding("vip-agent");
        b.channel = Some("telegram".into());
        b.peer = Some(PeerBinding {
            kind: Some("direct".into()),
            id: "user42".into(),
        });
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_peer.len(), 1);
        let key = build_peer_key(Some("telegram"), None, "user42");
        assert!(index.by_peer.contains_key(&key));
    }

    #[test]
    fn test_guild_binding() {
        let mut b = make_binding("guild-agent");
        b.channel = Some("discord".into());
        b.guild = Some("guild123".into());
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_guild.len(), 1);
    }

    #[test]
    fn test_guild_roles_binding() {
        let mut b = make_binding("admin-agent");
        b.channel = Some("discord".into());
        b.guild = Some("guild123".into());
        b.roles = Some(vec!["admin".into()]);
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_guild_roles.len(), 1);
    }

    #[test]
    fn test_team_binding() {
        let mut b = make_binding("team-agent");
        b.channel = Some("slack".into());
        b.team = Some("T12345".into());
        let index = BindingIndex::build(&[b]);
        assert_eq!(index.by_team.len(), 1);
    }

    #[test]
    fn test_mixed_bindings() {
        let bindings = vec![
            {
                let mut b = make_binding("peer-agent");
                b.channel = Some("telegram".into());
                b.peer = Some(PeerBinding {
                    kind: Some("direct".into()),
                    id: "user1".into(),
                });
                b
            },
            {
                let mut b = make_binding("channel-agent");
                b.channel = Some("discord".into());
                b
            },
            make_binding("default-agent"),
        ];
        let index = BindingIndex::build(&bindings);
        assert_eq!(index.by_peer.len(), 1);
        assert_eq!(index.by_channel.len(), 1);
        assert!(index.default.is_some());
    }
}
