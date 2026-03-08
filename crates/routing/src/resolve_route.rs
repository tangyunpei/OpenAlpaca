use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::CoreError;
use openalpaca_core::types::{AccountId, AgentId, ChannelId, ChatType, SessionKey};

use crate::account_id::normalize_account_id;
use crate::bindings::{
    BindingIndex, EvaluatedBinding, build_guild_key, build_peer_key, build_team_key,
};
use crate::session_key::{
    DmScope, PeerSessionParams, build_main_session_key, build_peer_session_key,
};

/// Peer information for route resolution.
#[derive(Debug, Clone)]
pub struct RoutePeer {
    pub kind: ChatType,
    pub id: String,
}

/// Input for route resolution.
pub struct ResolveRouteInput<'a> {
    pub config: &'a OpenAlpacaConfig,
    pub channel: &'a ChannelId,
    pub account_id: Option<&'a str>,
    pub peer: Option<&'a RoutePeer>,
    pub parent_peer: Option<&'a RoutePeer>,
    pub guild_id: Option<&'a str>,
    pub team_id: Option<&'a str>,
    pub member_role_ids: Option<&'a [String]>,
}

/// Result of route resolution.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: AgentId,
    pub channel: ChannelId,
    pub account_id: AccountId,
    pub session_key: SessionKey,
    pub main_session_key: SessionKey,
    pub matched_by: MatchedBy,
}

/// How the route was matched — which tier won.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchedBy {
    BindingPeer,
    BindingPeerParent,
    BindingGuildRoles,
    BindingGuild,
    BindingTeam,
    BindingAccount,
    BindingChannel,
    Default,
}

/// Resolve which agent handles a message.
///
/// Multi-tier matching (highest priority first):
/// 1. Peer binding — exact peer match
/// 2. Parent peer — parent peer match (for threads)
/// 3. Guild + Roles — Discord guild with specific roles
/// 4. Guild — Discord guild match
/// 5. Team — Slack team/workspace match
/// 6. Account — specific channel account
/// 7. Channel — channel-level binding
/// 8. Default — fallback to first agent or error
pub fn resolve_route(input: &ResolveRouteInput) -> Result<ResolvedRoute, CoreError> {
    let account_id = normalize_account_id(input.account_id);
    let bindings = input.config.bindings.as_deref().unwrap_or(&[]);
    let index = BindingIndex::build(bindings);

    let dm_scope = resolve_dm_scope(input.config);
    let main_key = input
        .config
        .session
        .as_ref()
        .and_then(|s| s.main_key.as_deref());

    // Tier 1: Peer binding
    if let Some(peer) = &input.peer
        && let Some(binding) = find_peer_binding(&index, input.channel, &account_id, &peer.id)
    {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::BindingPeer,
        );
    }

    // Tier 2: Parent peer binding (for thread inheritance)
    if let Some(parent) = &input.parent_peer
        && let Some(binding) = find_peer_binding(&index, input.channel, &account_id, &parent.id)
    {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::BindingPeerParent,
        );
    }

    // Tier 3: Guild + Roles
    if let Some(guild_id) = input.guild_id {
        if let Some(role_ids) = input.member_role_ids
            && let Some(binding) =
                find_guild_roles_binding(&index, input.channel, guild_id, role_ids)
        {
            return build_resolved(
                binding,
                input.channel,
                &account_id,
                input.peer,
                &dm_scope,
                main_key,
                input.config,
                MatchedBy::BindingGuildRoles,
            );
        }

        // Tier 4: Guild only
        if let Some(binding) = find_guild_binding(&index, input.channel, guild_id) {
            return build_resolved(
                binding,
                input.channel,
                &account_id,
                input.peer,
                &dm_scope,
                main_key,
                input.config,
                MatchedBy::BindingGuild,
            );
        }
    }

    // Tier 5: Team
    if let Some(team_id) = input.team_id
        && let Some(binding) = find_team_binding(&index, input.channel, team_id)
    {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::BindingTeam,
        );
    }

    // Tier 6: Account
    if let Some(binding) = find_account_binding(&index, input.channel, &account_id) {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::BindingAccount,
        );
    }

    // Tier 7: Channel
    if let Some(binding) = find_channel_binding(&index, input.channel) {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::BindingChannel,
        );
    }

    // Tier 8: Default binding or first agent
    if let Some(ref binding) = index.default {
        return build_resolved(
            binding,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            main_key,
            input.config,
            MatchedBy::Default,
        );
    }

    // Last resort: use first agent from config
    if let Some(agents) = input.config.agents.as_ref()
        && let Some(list) = agents.list.as_ref()
        && let Some(first) = list.first()
    {
        let agent_id = AgentId(first.id.to_lowercase());
        let main_session_key = build_main_session_key(&agent_id, main_key);
        let session_key = build_session_key(
            &agent_id,
            input.channel,
            &account_id,
            input.peer,
            &dm_scope,
            input.config,
        );
        return Ok(ResolvedRoute {
            agent_id,
            channel: input.channel.clone(),
            account_id,
            session_key,
            main_session_key,
            matched_by: MatchedBy::Default,
        });
    }

    Err(CoreError::NotFound(
        "no agent found: no bindings or agents configured".into(),
    ))
}

fn find_peer_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
    account_id: &AccountId,
    peer_id: &str,
) -> Option<&'a EvaluatedBinding> {
    // Try exact match: channel:account:peer
    let key = build_peer_key(Some(&channel.0), Some(&account_id.0), peer_id);
    if let Some(bindings) = index.by_peer.get(&key)
        && let Some(b) = bindings.first()
    {
        return Some(b);
    }
    // Try channel:*:peer
    let key = build_peer_key(Some(&channel.0), None, peer_id);
    if let Some(bindings) = index.by_peer.get(&key)
        && let Some(b) = bindings.first()
    {
        return Some(b);
    }
    // Try *:*:peer (any channel)
    let key = build_peer_key(None, None, peer_id);
    if let Some(bindings) = index.by_peer.get(&key) {
        return bindings.first();
    }
    None
}

fn find_guild_roles_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
    guild_id: &str,
    role_ids: &[String],
) -> Option<&'a EvaluatedBinding> {
    // Check channel-specific guild+roles
    let key = build_guild_key(Some(&channel.0), guild_id);
    if let Some(bindings) = index.by_guild_roles.get(&key) {
        for binding in bindings {
            if let Some(ref required_roles) = binding.roles
                && required_roles
                    .iter()
                    .any(|r| role_ids.iter().any(|mr| mr == r))
            {
                return Some(binding);
            }
        }
    }
    // Check wildcard channel guild+roles
    let key = build_guild_key(None, guild_id);
    if let Some(bindings) = index.by_guild_roles.get(&key) {
        for binding in bindings {
            if let Some(ref required_roles) = binding.roles
                && required_roles
                    .iter()
                    .any(|r| role_ids.iter().any(|mr| mr == r))
            {
                return Some(binding);
            }
        }
    }
    None
}

fn find_guild_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
    guild_id: &str,
) -> Option<&'a EvaluatedBinding> {
    let key = build_guild_key(Some(&channel.0), guild_id);
    if let Some(bindings) = index.by_guild.get(&key) {
        return bindings.first();
    }
    let key = build_guild_key(None, guild_id);
    if let Some(bindings) = index.by_guild.get(&key) {
        return bindings.first();
    }
    None
}

fn find_team_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
    team_id: &str,
) -> Option<&'a EvaluatedBinding> {
    let key = build_team_key(Some(&channel.0), team_id);
    if let Some(bindings) = index.by_team.get(&key) {
        return bindings.first();
    }
    let key = build_team_key(None, team_id);
    if let Some(bindings) = index.by_team.get(&key) {
        return bindings.first();
    }
    None
}

fn find_account_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
    account_id: &AccountId,
) -> Option<&'a EvaluatedBinding> {
    index
        .by_account
        .iter()
        .find(|b| {
            b.channel.as_deref() == Some(&channel.0) && b.account.as_deref() == Some(&account_id.0)
        })
        .or_else(|| {
            // Match any channel with this account
            index
                .by_account
                .iter()
                .find(|b| b.channel.is_none() && b.account.as_deref() == Some(&account_id.0))
        })
}

fn find_channel_binding<'a>(
    index: &'a BindingIndex,
    channel: &ChannelId,
) -> Option<&'a EvaluatedBinding> {
    index
        .by_channel
        .iter()
        .find(|b| b.channel.as_deref() == Some(&channel.0))
}

fn resolve_dm_scope(config: &OpenAlpacaConfig) -> DmScope {
    config
        .session
        .as_ref()
        .and_then(|s| s.dm_scope.as_deref())
        .map(DmScope::from_str_loose)
        .unwrap_or(DmScope::PerPeer)
}

fn build_session_key(
    agent_id: &AgentId,
    channel: &ChannelId,
    account_id: &AccountId,
    peer: Option<&RoutePeer>,
    dm_scope: &DmScope,
    config: &OpenAlpacaConfig,
) -> SessionKey {
    match peer {
        Some(p) => {
            let identity_links = config
                .session
                .as_ref()
                .and_then(|s| s.identity_links.clone());
            build_peer_session_key(&PeerSessionParams {
                agent_id: agent_id.clone(),
                channel: channel.clone(),
                account_id: account_id.clone(),
                peer_kind: p.kind,
                peer_id: p.id.clone(),
                dm_scope: dm_scope.clone(),
                identity_links,
            })
        }
        None => {
            let main_key = config.session.as_ref().and_then(|s| s.main_key.as_deref());
            build_main_session_key(agent_id, main_key)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_resolved(
    binding: &EvaluatedBinding,
    channel: &ChannelId,
    account_id: &AccountId,
    peer: Option<&RoutePeer>,
    dm_scope: &DmScope,
    main_key: Option<&str>,
    config: &OpenAlpacaConfig,
    matched_by: MatchedBy,
) -> Result<ResolvedRoute, CoreError> {
    let agent_id = binding.agent_id.clone();
    let main_session_key = build_main_session_key(&agent_id, main_key);
    let session_key = build_session_key(&agent_id, channel, account_id, peer, dm_scope, config);

    Ok(ResolvedRoute {
        agent_id,
        channel: channel.clone(),
        account_id: account_id.clone(),
        session_key,
        main_session_key,
        matched_by,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_config::types_agents::{AgentBinding, AgentConfig, AgentsConfig, PeerBinding};

    fn channel(s: &str) -> ChannelId {
        ChannelId(s.to_string())
    }

    fn make_config(bindings: Vec<AgentBinding>, agents: Vec<AgentConfig>) -> OpenAlpacaConfig {
        OpenAlpacaConfig {
            bindings: if bindings.is_empty() {
                None
            } else {
                Some(bindings)
            },
            agents: Some(AgentsConfig {
                list: if agents.is_empty() {
                    None
                } else {
                    Some(agents)
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

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

    fn make_agent(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_default_fallback_to_first_agent() {
        let config = make_config(vec![], vec![make_agent("assistant")]);
        let ch = channel("telegram");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "assistant");
        assert_eq!(result.matched_by, MatchedBy::Default);
    }

    #[test]
    fn test_no_agents_returns_error() {
        let config = make_config(vec![], vec![]);
        let ch = channel("telegram");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        assert!(resolve_route(&input).is_err());
    }

    #[test]
    fn test_channel_binding_matches() {
        let mut binding = make_binding("tg-agent");
        binding.channel = Some("telegram".into());
        let config = make_config(vec![binding], vec![make_agent("tg-agent")]);
        let ch = channel("telegram");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "tg-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingChannel);
    }

    #[test]
    fn test_peer_binding_beats_channel() {
        let mut channel_binding = make_binding("channel-agent");
        channel_binding.channel = Some("telegram".into());

        let mut peer_binding = make_binding("vip-agent");
        peer_binding.channel = Some("telegram".into());
        peer_binding.peer = Some(PeerBinding {
            kind: Some("direct".into()),
            id: "user42".into(),
        });

        let config = make_config(
            vec![channel_binding, peer_binding],
            vec![make_agent("channel-agent"), make_agent("vip-agent")],
        );
        let ch = channel("telegram");
        let peer = RoutePeer {
            kind: ChatType::Direct,
            id: "user42".into(),
        };
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: Some(&peer),
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "vip-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingPeer);
    }

    #[test]
    fn test_parent_peer_inheritance() {
        let mut peer_binding = make_binding("thread-agent");
        peer_binding.channel = Some("discord".into());
        peer_binding.peer = Some(PeerBinding {
            kind: Some("group".into()),
            id: "parent-msg-123".into(),
        });

        let config = make_config(vec![peer_binding], vec![make_agent("thread-agent")]);
        let ch = channel("discord");
        let peer = RoutePeer {
            kind: ChatType::Group,
            id: "child-msg-456".into(),
        };
        let parent = RoutePeer {
            kind: ChatType::Group,
            id: "parent-msg-123".into(),
        };
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: Some(&peer),
            parent_peer: Some(&parent),
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "thread-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingPeerParent);
    }

    #[test]
    fn test_guild_roles_binding() {
        let mut binding = make_binding("admin-agent");
        binding.channel = Some("discord".into());
        binding.guild = Some("guild1".into());
        binding.roles = Some(vec!["admin".into()]);

        let config = make_config(vec![binding], vec![make_agent("admin-agent")]);
        let ch = channel("discord");
        let roles = vec!["admin".to_string(), "member".to_string()];
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: Some("guild1"),
            team_id: None,
            member_role_ids: Some(&roles),
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "admin-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingGuildRoles);
    }

    #[test]
    fn test_guild_binding_without_roles() {
        let mut binding = make_binding("guild-agent");
        binding.channel = Some("discord".into());
        binding.guild = Some("guild1".into());

        let config = make_config(vec![binding], vec![make_agent("guild-agent")]);
        let ch = channel("discord");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: Some("guild1"),
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "guild-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingGuild);
    }

    #[test]
    fn test_team_binding() {
        let mut binding = make_binding("team-agent");
        binding.channel = Some("slack".into());
        binding.team = Some("T12345".into());

        let config = make_config(vec![binding], vec![make_agent("team-agent")]);
        let ch = channel("slack");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: Some("T12345"),
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "team-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingTeam);
    }

    #[test]
    fn test_account_binding() {
        let mut binding = make_binding("work-agent");
        binding.channel = Some("telegram".into());
        binding.account = Some("work".into());

        let config = make_config(vec![binding], vec![make_agent("work-agent")]);
        let ch = channel("telegram");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: Some("work"),
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "work-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingAccount);
    }

    #[test]
    fn test_default_binding() {
        let binding = make_binding("default-agent");
        let config = make_config(vec![binding], vec![make_agent("default-agent")]);
        let ch = channel("telegram");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "default-agent");
        assert_eq!(result.matched_by, MatchedBy::Default);
    }

    #[test]
    fn test_session_key_uses_dm_scope() {
        let mut binding = make_binding("bot");
        binding.channel = Some("telegram".into());

        let mut config = make_config(vec![binding], vec![make_agent("bot")]);
        config.session = Some(openalpaca_config::types_base::SessionConfig {
            dm_scope: Some("per-channel-peer".into()),
            ..Default::default()
        });

        let ch = channel("telegram");
        let peer = RoutePeer {
            kind: ChatType::Direct,
            id: "user123".into(),
        };
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: Some(&peer),
            parent_peer: None,
            guild_id: None,
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.session_key.0, "agent:bot:telegram:direct:user123");
    }

    #[test]
    fn test_priority_order() {
        // Set up: guild binding + channel binding + default
        // With guild_id provided, guild should win over channel and default
        let mut guild_binding = make_binding("guild-agent");
        guild_binding.channel = Some("discord".into());
        guild_binding.guild = Some("g1".into());

        let mut channel_binding = make_binding("channel-agent");
        channel_binding.channel = Some("discord".into());

        let default_binding = make_binding("default-agent");

        let config = make_config(
            vec![guild_binding, channel_binding, default_binding],
            vec![
                make_agent("guild-agent"),
                make_agent("channel-agent"),
                make_agent("default-agent"),
            ],
        );
        let ch = channel("discord");
        let input = ResolveRouteInput {
            config: &config,
            channel: &ch,
            account_id: None,
            peer: None,
            parent_peer: None,
            guild_id: Some("g1"),
            team_id: None,
            member_role_ids: None,
        };
        let result = resolve_route(&input).unwrap();
        assert_eq!(result.agent_id.0, "guild-agent");
        assert_eq!(result.matched_by, MatchedBy::BindingGuild);
    }
}
