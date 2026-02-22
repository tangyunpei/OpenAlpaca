//! LLM command — status, keys, usage, models, strategy

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct LlmArgs {
    #[command(subcommand)]
    pub command: LlmCommands,
}

#[derive(Subcommand)]
pub enum LlmCommands {
    /// Show LLM system status overview
    Status {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Manage API keys
    Keys(KeysArgs),
    /// Show LLM usage statistics
    Usage {
        /// Filter by agent ID
        #[arg(long)]
        agent: Option<String>,
        /// Filter by date (YYYY-MM-DD)
        #[arg(long)]
        date: Option<String>,
        /// Filter by key ID
        #[arg(long)]
        key: Option<String>,
        /// Show daily aggregates instead of individual calls
        #[arg(long)]
        daily: bool,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// List available models
    Models {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Set routing strategy for a provider
    Strategy {
        /// Provider name (anthropic, openai, ollama)
        #[arg(long)]
        provider: String,
        /// Strategy name (e.g., round_robin, priority, cost_optimized)
        strategy: String,
    },
    /// Show discovered OAuth credentials (Claude Code, Codex)
    Credentials {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show CLI backend status (claude, codex CLI tools)
    Backends {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show per-provider usage summaries
    ProviderUsage {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
}

#[derive(Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: KeysCommands,
}

#[derive(Subcommand)]
pub enum KeysCommands {
    /// List all API keys
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Add a new API key
    Add {
        /// Provider (anthropic, openai, ollama)
        #[arg(long)]
        provider: Option<String>,
        /// Secret key value
        #[arg(long)]
        secret: Option<String>,
        /// Key priority
        #[arg(long, default_value = "primary")]
        priority: String,
        /// Source of the key
        #[arg(long)]
        source: Option<String>,
        /// Notes
        #[arg(long)]
        notes: Option<String>,
    },
    /// Remove an API key
    Remove {
        /// Provider name
        provider: String,
        /// Key ID
        key_id: String,
    },
    /// Validate an API key
    Validate {
        /// Provider name
        #[arg(long)]
        provider: String,
        /// Secret key to validate
        #[arg(long)]
        secret: String,
    },
    /// Set a key as primary for its provider
    SetPrimary {
        /// Provider name
        provider: String,
        /// Key ID to make primary
        key_id: String,
    },
    /// Reorder keys
    Reorder {
        /// Key IDs in desired order
        key_ids: Vec<String>,
    },
}

// ── Local deserialization structs ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct KeyEntry {
    provider: String,
    key_id: String,
    #[serde(default)]
    masked_secret: Option<String>,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelEntry {
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
struct UsageEntry {
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
struct DailyUsageEntry {
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

impl TableRow for KeyEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 12),
            ("KEY_ID", 15),
            ("SECRET", 20),
            ("PRIORITY", 10),
            ("SOURCE", 15),
            ("STATUS", 10),
        ]
    }

    fn table_row(&self) -> String {
        let priority_marker = if self.is_primary {
            "Primary"
        } else {
            "Fallback"
        };
        let masked = self.masked_secret.as_deref().unwrap_or("***");
        let source = self.source.as_deref().unwrap_or("-");
        let status = self.status.as_deref().unwrap_or("unknown");

        format!(
            "{:<12} {:<15} {:<20} {:<10} {:<15} {:<10}",
            self.provider,
            truncate(&self.key_id, 13),
            masked,
            priority_marker,
            source,
            status_color(status),
        )
    }
}

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

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

// ── Command runner ───────────────────────────────────────────────

pub async fn run(args: LlmArgs) -> Result<()> {
    match args.command {
        LlmCommands::Status { format } => llm_status(format).await,
        LlmCommands::Keys(keys_args) => run_keys(keys_args).await,
        LlmCommands::Usage {
            agent,
            date,
            key,
            daily,
            format,
        } => llm_usage(agent, date, key, daily, format).await,
        LlmCommands::Models { format } => llm_models(format).await,
        LlmCommands::Strategy { provider, strategy } => llm_strategy(&provider, &strategy).await,
        LlmCommands::Credentials { format } => llm_credentials(format).await,
        LlmCommands::Backends { format } => llm_backends(format).await,
        LlmCommands::ProviderUsage { format } => llm_provider_usage(format).await,
    }
}

async fn llm_status(format: OutputFormat) -> Result<()> {
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
                // Could be a single summary object
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

async fn run_keys(args: KeysArgs) -> Result<()> {
    match args.command {
        KeysCommands::List { format } => keys_list(format).await,
        KeysCommands::Add {
            provider,
            secret,
            priority,
            source,
            notes,
        } => keys_add(provider, secret, priority, source, notes).await,
        KeysCommands::Remove { provider, key_id } => keys_remove(&provider, &key_id).await,
        KeysCommands::Validate { provider, secret } => keys_validate(&provider, &secret).await,
        KeysCommands::SetPrimary { provider, key_id } => keys_set_primary(&provider, &key_id).await,
        KeysCommands::Reorder { key_ids } => keys_reorder(key_ids).await,
    }
}

async fn keys_list(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let config: serde_json::Value = client.get("/v1/settings/llm").await?;

    // Flatten providers.*.keys into a flat list
    let mut keys = Vec::new();
    if let Some(providers) = config["providers"].as_object() {
        for (provider_name, provider_data) in providers {
            let primary_key_id = provider_data["primary_key_id"].as_str().unwrap_or("");
            if let Some(provider_keys) = provider_data["keys"].as_array() {
                for key in provider_keys {
                    let key_id = key["key_id"].as_str().unwrap_or("").to_string();
                    keys.push(KeyEntry {
                        provider: provider_name.clone(),
                        key_id: key_id.clone(),
                        masked_secret: key["masked_secret"].as_str().map(|s| s.to_string()),
                        is_primary: key_id == primary_key_id,
                        source: key["source"].as_str().map(|s| s.to_string()),
                        status: key["status"].as_str().map(|s| s.to_string()),
                    });
                }
            }
        }
    }

    print_list(&keys, format);
    Ok(())
}

async fn keys_add(
    provider: Option<String>,
    secret: Option<String>,
    priority: String,
    source: Option<String>,
    notes: Option<String>,
) -> Result<()> {
    let theme = ColorfulTheme::default();

    // Interactive fallback for missing provider
    let provider = match provider {
        Some(p) => p,
        None => {
            let providers = vec!["anthropic", "openai", "ollama"];
            let idx = Select::with_theme(&theme)
                .with_prompt("Provider")
                .items(&providers)
                .default(0)
                .interact()?;
            providers[idx].to_string()
        }
    };

    // Interactive fallback for missing secret
    let secret = match secret {
        Some(s) => s,
        None => {
            let s: String = dialoguer::Password::with_theme(&theme)
                .with_prompt("API key secret")
                .interact()?;
            s
        }
    };

    // Auto-validate the key
    print!("{}", "Validating key... ".dimmed());
    let client = DaemonClient::connect()?;
    let validate_body = serde_json::json!({
        "provider": provider,
        "secret": secret,
    });
    match client
        .post::<_, serde_json::Value>("/v1/settings/llm/validate", &validate_body)
        .await
    {
        Ok(result) => {
            let valid = result["valid"].as_bool().unwrap_or(false);
            if valid {
                println!("{}", "valid".green());
            } else {
                println!("{}", "invalid".red());
                let msg = result["message"]
                    .as_str()
                    .unwrap_or("Key validation failed");
                println!("  {}", msg);
            }
        }
        Err(e) => {
            println!("{} ({})", "could not validate".yellow(), e);
        }
    }

    // Interactive priority if not specified via flag
    let priority = if priority == "primary" || priority == "fallback" {
        priority
    } else {
        let options = vec!["primary", "fallback"];
        let idx = Select::with_theme(&theme)
            .with_prompt("Priority")
            .items(&options)
            .default(0)
            .interact()?;
        options[idx].to_string()
    };

    // Interactive source
    let source = match source {
        Some(s) => s,
        None => {
            let sources = vec![
                "API Console",
                "Claude Code",
                "Codex",
                "Environment",
                "Other",
            ];
            let idx = Select::with_theme(&theme)
                .with_prompt("Source")
                .items(&sources)
                .default(0)
                .interact()?;
            sources[idx].to_string()
        }
    };

    // Notes
    let notes = match notes {
        Some(n) => n,
        None => {
            let n: String = Input::with_theme(&theme)
                .with_prompt("Notes (optional)")
                .allow_empty(true)
                .interact_text()?;
            n
        }
    };

    // Submit
    let body = serde_json::json!({
        "provider": provider,
        "key": {
            "key_id": format!("{}_{}", provider, chrono::Utc::now().timestamp()),
            "secret": secret,
            "source": source,
            "notes": notes,
            "priority": priority,
        }
    });

    let _: serde_json::Value = client.put("/v1/settings/llm", &body).await?;
    println!("{} Key added for {}", "✓".green(), provider);
    Ok(())
}

async fn keys_remove(provider: &str, key_id: &str) -> Result<()> {
    let confirm = dialoguer::Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Remove key '{}/{}' ?", provider, key_id))
        .default(false)
        .interact()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    let client = DaemonClient::connect()?;
    let _: serde_json::Value = client
        .delete_req(&format!("/v1/settings/llm/keys/{}/{}", provider, key_id))
        .await?;
    println!("{} Key removed: {}/{}", "✓".green(), provider, key_id);
    Ok(())
}

async fn keys_validate(provider: &str, secret: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({
        "provider": provider,
        "secret": secret,
    });

    let result: serde_json::Value = client.post("/v1/settings/llm/validate", &body).await?;

    let valid = result["valid"].as_bool().unwrap_or(false);
    if valid {
        println!("{} Key is {}", "✓".green(), "valid".green());
    } else {
        println!("{} Key is {}", "✗".red(), "invalid".red());
    }

    // Print additional details if available
    if let Some(tier) = result["tier"].as_str() {
        println!("  {} {}", "Tier:".dimmed(), tier);
    }
    if let Some(rate_limit) = result["rate_limit"].as_str() {
        println!("  {} {}", "Rate limit:".dimmed(), rate_limit);
    }
    if let Some(models) = result["available_models"].as_array()
        && !models.is_empty()
    {
        let names: Vec<&str> = models.iter().filter_map(|m| m.as_str()).collect();
        println!("  {} {}", "Models:".dimmed(), names.join(", "));
    }

    Ok(())
}

async fn keys_set_primary(provider: &str, key_id: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({
        "provider": provider,
        "primary_key_id": key_id,
    });
    let _: serde_json::Value = client.put("/v1/settings/llm/keys/reorder", &body).await?;
    println!("{} Set {} as primary for {}", "✓".green(), key_id, provider);
    Ok(())
}

async fn keys_reorder(key_ids: Vec<String>) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({
        "key_ids": key_ids,
    });
    let _: serde_json::Value = client.put("/v1/settings/llm/keys/reorder", &body).await?;
    println!("{} Keys reordered", "✓".green());
    Ok(())
}

async fn llm_usage(
    agent: Option<String>,
    _date: Option<String>,
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

async fn llm_models(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let models: Vec<ModelEntry> = client.get("/v1/models").await?;
    print_list(&models, format);
    Ok(())
}

// ── Deserialization structs for new commands ────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct CredentialEntry {
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
struct BackendEntry {
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
struct ProviderUsageEntry {
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

async fn llm_credentials(format: OutputFormat) -> Result<()> {
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

async fn llm_backends(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let backends: Vec<BackendEntry> = client.get("/v1/settings/llm/cli-backends").await?;

    if backends.is_empty() {
        println!("{}", "No CLI backends configured.".dimmed());
        return Ok(());
    }

    print_list(&backends, format);
    Ok(())
}

async fn llm_provider_usage(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let usage: Vec<ProviderUsageEntry> = client.get("/v1/settings/llm/providers/usage").await?;

    if usage.is_empty() {
        println!("{}", "No provider usage data available.".dimmed());
        return Ok(());
    }

    print_list(&usage, format);
    Ok(())
}

async fn llm_strategy(provider: &str, strategy: &str) -> Result<()> {
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
