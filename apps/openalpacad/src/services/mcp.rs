//! MCP server bootstrap: loads `config/mcp.toml`, connects to enabled servers
//! with per-server timeouts, registers discovered tools into the tool registry.
//!
//! The entry point is [`register_mcp_servers`], wired into
//! `build_tool_registry` during `initialize_services`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use openalpaca_core::tools::ToolRegistry;
use openalpaca_core::tools::mcp::{
    McpConfig, McpDefaults, McpServerConfig, McpServerStatus, McpServerSummary,
    bridge, config::HttpAuthConfig, config::LoadError,
};
use openalpaca_mcp::{Implementation, McpClient, McpClientConfig, TransportKind};

/// Load config/mcp.toml and connect to all enabled servers. Registers their
/// tools into the provided `ToolRegistry`. Registered tools hold `Arc`s to
/// their `McpClient`, which keeps the client connections alive for the
/// daemon's lifetime.
pub(super) async fn register_mcp_servers(
    config_base_dir: &Path,
    tool_registry: &Arc<ToolRegistry>,
) -> anyhow::Result<()> {
    let config_path = config_base_dir.join("mcp.toml");

    let config = match McpConfig::load(&config_path) {
        Ok(cfg) => cfg,
        Err(LoadError::NotFound(_)) => {
            tracing::info!(
                path = %config_path.display(),
                "no config/mcp.toml — no MCP servers to register"
            );
            return Ok(());
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "failed to load {}: {e}",
                config_path.display()
            ));
        }
    };

    let mut connected = 0usize;
    let mut total = 0usize;

    for (server_name, server_cfg) in config.servers {
        total += 1;
        if !server_cfg.is_enabled() {
            tracing::info!(server_name = %server_name, "MCP server disabled by config");
            continue;
        }

        let summary =
            connect_and_register_one(&server_name, &server_cfg, &config.defaults, tool_registry)
                .await;
        if matches!(summary.status, McpServerStatus::Connected { .. }) {
            connected += 1;
        }
    }

    tracing::info!(connected, total, "MCP server bootstrap complete");

    Ok(())
}

async fn connect_and_register_one(
    server_name: &str,
    server_cfg: &McpServerConfig,
    defaults: &McpDefaults,
    tool_registry: &Arc<ToolRegistry>,
) -> McpServerSummary {
    let transport_kind = server_cfg.transport_kind();
    let mut summary = McpServerSummary {
        server_name: server_name.to_string(),
        transport_kind,
        status: McpServerStatus::Failed {
            reason: "(pending)".into(),
        },
        discovered_tools: Vec::new(),
    };

    tracing::info!(server_name = %server_name, transport_kind = %transport_kind, "MCP server starting");

    let client_config = match build_client_config(server_name, server_cfg, defaults) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(server_name = %server_name, reason = %e, "MCP server config invalid");
            summary.status = McpServerStatus::Failed { reason: e };
            return summary;
        }
    };

    let connect_timeout = Duration::from_secs(
        server_cfg
            .connect_timeout_secs()
            .unwrap_or(defaults.connect_timeout_secs),
    );
    let client = match tokio::time::timeout(connect_timeout, McpClient::connect(client_config)).await {
        Ok(Ok(c)) => Arc::new(c),
        Ok(Err(e)) => {
            tracing::warn!(server_name = %server_name, error = %e, "MCP server connect failed");
            summary.status = McpServerStatus::Failed { reason: format!("connect: {e}") };
            return summary;
        }
        Err(_elapsed) => {
            tracing::warn!(server_name = %server_name, timeout = ?connect_timeout, "MCP server connect timed out");
            summary.status = McpServerStatus::Failed {
                reason: format!("timeout after {connect_timeout:?}"),
            };
            return summary;
        }
    };

    let server_info = client.server_info().cloned();
    let protocol_version = client.protocol_version().cloned();
    let server_version = server_info
        .as_ref()
        .map(|i| i.version.as_str())
        .unwrap_or("unknown");

    let tools = match client.list_tools(None).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(server_name = %server_name, error = %e, "MCP list_tools failed");
            summary.status = McpServerStatus::Failed { reason: format!("list_tools: {e}") };
            return summary;
        }
    };

    let mut registered_names = Vec::new();
    let mut skipped = 0;
    for tool in tools {
        let reg = bridge::rmcp_tool_to_registered(
            server_name,
            server_version,
            tool,
            Arc::clone(&client),
        );
        let name = reg.definition.name.clone();
        match tool_registry.register(reg) {
            Ok(()) => registered_names.push(name),
            Err(e) => {
                tracing::warn!(
                    server_name = %server_name,
                    tool = %name,
                    error = %e,
                    "MCP tool registration failed"
                );
                skipped += 1;
            }
        }
    }

    tracing::info!(
        server_name = %server_name,
        tool_count = registered_names.len(),
        skipped,
        "MCP server registered tools"
    );

    summary.status = McpServerStatus::Connected {
        server_version: server_info
            .map(|i| i.version.clone())
            .unwrap_or_default(),
        protocol_version: protocol_version
            .map(|v| format!("{v:?}"))
            .unwrap_or_default(),
    };
    summary.discovered_tools = registered_names;
    summary
}

fn build_client_config(
    server_name: &str,
    server_cfg: &McpServerConfig,
    defaults: &McpDefaults,
) -> Result<McpClientConfig, String> {
    let request_timeout = Duration::from_secs(
        server_cfg.request_timeout_secs().unwrap_or(defaults.request_timeout_secs),
    );

    let transport = match server_cfg {
        McpServerConfig::Stdio { command, args, env, cwd, .. } => TransportKind::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            cwd: cwd.clone(),
        },
        McpServerConfig::Http { url, auth, extra_headers, .. } => {
            let resolved_auth = resolve_http_auth(server_name, auth.as_ref())?;
            TransportKind::Http {
                url: url.clone(),
                auth: resolved_auth,
                extra_headers: extra_headers.clone(),
            }
        }
    };

    Ok(McpClientConfig {
        server_name: server_name.to_string(),
        transport,
        client_info: Implementation {
            name: "openalpaca-mcp".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            ..Default::default()
        },
        request_timeout,
        max_reconnect_attempts: defaults.max_reconnect_attempts,
        reconnect_backoff_ms: defaults.reconnect_backoff_ms,
    })
}

fn resolve_http_auth(
    server_name: &str,
    auth: Option<&HttpAuthConfig>,
) -> Result<Option<openalpaca_mcp::HttpAuth>, String> {
    use openalpaca_mcp::HttpAuth;
    match auth {
        None => Ok(None),
        Some(HttpAuthConfig::Bearer { bearer }) => Ok(Some(HttpAuth::Bearer(bearer.clone()))),
        Some(HttpAuthConfig::BearerEnv { bearer_env }) => match std::env::var(bearer_env) {
            Ok(val) => Ok(Some(HttpAuth::Bearer(val))),
            Err(_) => Err(format!(
                "missing env var '{bearer_env}' for bearer_env on server '{server_name}'"
            )),
        },
        Some(HttpAuthConfig::ApiKey { api_key_header, api_key_env }) => {
            match std::env::var(api_key_env) {
                Ok(val) => Ok(Some(HttpAuth::ApiKey {
                    header: api_key_header.clone(),
                    value: val,
                })),
                Err(_) => Err(format!(
                    "missing env var '{api_key_env}' for api_key_env on server '{server_name}'"
                )),
            }
        }
    }
}
