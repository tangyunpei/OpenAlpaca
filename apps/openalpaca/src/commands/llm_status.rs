//! Status, usage, models, credentials, backends, and strategy handlers for `llm` subcommand.

use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

use super::llm::truncate;

// ── Local deserialization structs ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ModelEntry {
    #[serde(alias = "id")]
    model_id: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(default)]
    input_price_per_1m: Option<f64>,
    #[serde(default)]
    output_price_per_1m: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct UsageEntry {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DailyUsageEntry {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    total_input_tokens: Option<i64>,
    #[serde(default)]
    total_output_tokens: Option<i64>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CredentialEntry {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    auto_refresh: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BackendEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ProviderUsageEntry {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    total_tokens: Option<i64>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    health: Option<String>,
}

// ── TableRow impls ──────────────────────────────────────────────

impl TableRow for ModelEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 12),
            ("MODEL", 35),
            ("CONTEXT", 10),
            ("INPUT $/1M", 12),
            ("OUTPUT $/1M", 12),
        ]
    }

    fn table_row(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("-");
        let context = self
            .context_window
            .map(|c| format!("{}K", c / 1000))
            .unwrap_or_else(|| "-".to_string());
        let input_price = self
            .input_price_per_1m
            .map(|p| format!("${:.2}", p))
            .unwrap_or_else(|| "-".to_string());
        let output_price = self
            .output_price_per_1m
            .map(|p| format!("${:.2}", p))
            .unwrap_or_else(|| "-".to_string());

        format!(
            "{:<12} {:<35} {:<10} {:<12} {:<12}",
            provider,
            truncate(&self.model_id, 33),
            context,
            input_price,
            output_price,
        )
    }
}

impl TableRow for UsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("TIME", 20),
            ("AGENT", 15),
            ("MODEL", 25),
            ("TOKENS", 15),
            ("COST", 10),
            ("STATUS", 10),
        ]
    }

    fn table_row(&self) -> String {
        let time = self
            .timestamp
            .as_deref()
            .map(|t| t.chars().take(19).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        let agent = self.agent_id.as_deref().unwrap_or("-");
        let model = self.model.as_deref().unwrap_or("-");
        let tokens = format!(
            "{}/{}",
            self.input_tokens.unwrap_or(0),
            self.output_tokens.unwrap_or(0)
        );
        let cost = format!("${:.4}", self.cost_usd.unwrap_or(0.0));
        let status = self.status.as_deref().unwrap_or("unknown");

        format!(
            "{:<20} {:<15} {:<25} {:<15} {:<10} {:<10}",
            time,
            truncate(agent, 13),
            truncate(model, 23),
            tokens,
            cost,
            status_color(status),
        )
    }
}

impl TableRow for DailyUsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("DATE", 12),
            ("AGENT", 15),
            ("MODEL", 25),
            ("REQUESTS", 10),
            ("TOKENS", 18),
            ("COST", 10),
        ]
    }

    fn table_row(&self) -> String {
        let date = self.date.as_deref().unwrap_or("-");
        let agent = self.agent_id.as_deref().unwrap_or("-");
        let model = self.model.as_deref().unwrap_or("-");
        let requests = self.total_requests.unwrap_or(0);
        let tokens = format!(
            "{}/{}",
            self.total_input_tokens.unwrap_or(0),
            self.total_output_tokens.unwrap_or(0)
        );
        let cost = format!("${:.4}", self.total_cost_usd.unwrap_or(0.0));

        format!(
            "{:<12} {:<15} {:<25} {:<10} {:<18} {:<10}",
            date,
            truncate(agent, 13),
            truncate(model, 23),
            requests,
            tokens,
            cost,
        )
    }
}

impl TableRow for CredentialEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("SOURCE", 14),
            ("PROVIDER", 12),
            ("STATUS", 12),
            ("EXPIRES", 22),
            ("REFRESH", 8),
        ]
    }

    fn table_row(&self) -> String {
        let source = self.source.as_deref().unwrap_or("-");
        let provider = self.provider.as_deref().unwrap_or("-");
        let status = self.status.as_deref().unwrap_or("unknown");
        let expires = self
            .expires_at
            .map(|ts| {
                chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{}", ts))
            })
            .unwrap_or_else(|| "-".to_string());
        let refresh = if self.auto_refresh.unwrap_or(false) {
            "yes"
        } else {
            "no"
        };

        format!(
            "{:<14} {:<12} {:<12} {:<22} {:<8}",
            source,
            provider,
            status_color(status),
            expires,
            refresh,
        )
    }
}

impl TableRow for BackendEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("NAME", 14),
            ("AVAILABLE", 10),
            ("PATH", 35),
            ("ENABLED", 8),
        ]
    }

    fn table_row(&self) -> String {
        let name = self.name.as_deref().unwrap_or("-");
        let available = if self.available.unwrap_or(false) {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        };
        let path = self.path.as_deref().unwrap_or("-");
        let enabled = if self.enabled.unwrap_or(false) {
            "yes"
        } else {
            "no"
        };

        format!(
            "{:<14} {:<10} {:<35} {:<8}",
            name,
            available,
            truncate(path, 33),
            enabled,
        )
    }
}

impl TableRow for ProviderUsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 14),
            ("COST", 12),
            ("TOKENS", 12),
            ("REQUESTS", 10),
            ("HEALTH", 10),
        ]
    }

    fn table_row(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("-");
        let cost = format!("${:.4}", self.total_cost_usd.unwrap_or(0.0));
        let tokens = self.total_tokens.unwrap_or(0);
        let requests = self.total_requests.unwrap_or(0);
        let health = self.health.as_deref().unwrap_or("unknown");

        format!(
            "{:<14} {:<12} {:<12} {:<10} {:<10}",
            provider,
            cost,
            tokens,
            requests,
            status_color(health),
        )
    }
}

// ── Handler functions ───────────────────────────────────────────

pub(super) async fn llm_status(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;

    let key_health: serde_json::Value = client.get("/v1/settings/llm/status").await?;
    let orch_config: serde_json::Value = client.get("/v1/orchestrator/config").await?;

    match format {
        OutputFormat::Json => {
            let combined = serde_json::json!({
                "key_health": key_health,
                "orchestrator": orch_config,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&combined).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            println!("{}", "LLM Status".bold());
            println!();
            println!(
                "  {} {}",
                "Model:".dimmed(),
                orch_config["model"].as_str().unwrap_or("-")
            );

            if let Some(fallbacks) = orch_config["fallback_models"].as_array()
                && !fallbacks.is_empty()
            {
                let names: Vec<&str> = fallbacks.iter().filter_map(|m| m.as_str()).collect();
                println!("  {} {}", "Fallbacks:".dimmed(), names.join(", "));
            }

            println!(
                "  {} {}",
                "Active agents:".dimmed(),
                orch_config["active_agents"].as_i64().unwrap_or(0)
            );
            println!(
                "  {} {}",
                "Active tasks:".dimmed(),
                orch_config["active_tasks"].as_i64().unwrap_or(0)
            );
            println!(
                "  {} ${:.4}",
                "Daily cost:".dimmed(),
                orch_config["daily_cost_usd"].as_f64().unwrap_or(0.0)
            );
            println!();

            // Key health summary
            println!("{}", "Key Health:".dimmed());
            if let Some(keys) = key_health.as_array() {
                for k in keys {
                    let provider = k["provider"].as_str().unwrap_or("-");
                    let key_id = k["key_id"].as_str().unwrap_or("-");
                    let ok = k["is_valid"].as_bool().unwrap_or(false);
                    let indicator = if ok { "✓".green() } else { "✗".red() };
                    println!("  {} {} / {}", indicator, provider, key_id);
                }
            } else if let Some(obj) = key_health.as_object() {
                for (provider, data) in obj {
                    let ok = data["is_valid"].as_bool().unwrap_or(false);
                    let indicator = if ok { "✓".green() } else { "✗".red() };
                    println!("  {} {}", indicator, provider);
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn llm_usage(
    agent: Option<String>,
    date: Option<String>,
    key: Option<String>,
    daily: bool,
    format: OutputFormat,
) -> Result<()> {
    let client = DaemonClient::connect()?;

    if daily {
        let mut path = "/v1/llm/usage/daily?limit=30".to_string();
        if let Some(ref a) = agent {
            path.push_str(&format!("&agent_id={}", a));
        }
        if let Some(ref d) = date {
            path.push_str(&format!("&date={}", d));
        }
        let entries: Vec<DailyUsageEntry> = client.get(&path).await?;
        print_list(&entries, format);
    } else {
        let mut path = "/v1/llm/usage?limit=50".to_string();
        if let Some(ref a) = agent {
            path.push_str(&format!("&agent_id={}", a));
        }
        if let Some(ref k) = key {
            path.push_str(&format!("&key_id={}", k));
        }
        let entries: Vec<UsageEntry> = client.get(&path).await?;
        print_list(&entries, format);
    }
    Ok(())
}

pub(super) async fn llm_models(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let models: Vec<ModelEntry> = client.get("/v1/models").await?;
    print_list(&models, format);
    Ok(())
}

pub(super) async fn llm_credentials(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let creds: Vec<CredentialEntry> = client.get("/v1/settings/llm/credentials").await?;

    if creds.is_empty() {
        println!(
            "{}",
            "No discovered credentials. Install Claude Code or Codex CLI to auto-detect.".dimmed()
        );
        return Ok(());
    }

    print_list(&creds, format);
    Ok(())
}

pub(super) async fn llm_backends(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let backends: Vec<BackendEntry> = client.get("/v1/settings/llm/cli-backends").await?;

    if backends.is_empty() {
        println!("{}", "No CLI backends configured.".dimmed());
        return Ok(());
    }

    print_list(&backends, format);
    Ok(())
}

pub(super) async fn llm_provider_usage(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let usage: Vec<ProviderUsageEntry> = client.get("/v1/settings/llm/providers/usage").await?;

    if usage.is_empty() {
        println!("{}", "No provider usage data available.".dimmed());
        return Ok(());
    }

    print_list(&usage, format);
    Ok(())
}

pub(super) async fn llm_strategy(provider: &str, strategy: &str) -> Result<()> {
    let client = DaemonClient::connect()?;

    // Fetch current orchestrator config
    let config: serde_json::Value = client.get("/v1/orchestrator/config").await?;

    // Build update with strategy field
    let mut update = serde_json::Map::new();
    if let Some(model) = config["model"].as_str() {
        update.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    }
    if let Some(fallbacks) = config.get("fallback_models") {
        update.insert("fallback_models".to_string(), fallbacks.clone());
    }
    update.insert(
        "strategy".to_string(),
        serde_json::json!({
            "provider": provider,
            "type": strategy,
        }),
    );

    let _: serde_json::Value = client
        .put(
            "/v1/orchestrator/config",
            &serde_json::Value::Object(update),
        )
        .await?;
    println!(
        "{} Strategy set to '{}' for provider '{}'",
        "✓".green(),
        strategy,
        provider
    );
    Ok(())
}
