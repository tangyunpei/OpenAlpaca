//! Key management subcommand handlers for `llm keys`.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

use super::llm::truncate;

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

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct KeyEntry {
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

pub(super) async fn run_keys(args: KeysArgs) -> Result<()> {
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
