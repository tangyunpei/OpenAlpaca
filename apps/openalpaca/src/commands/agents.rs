//! Agents command — list, status, config, pause, resume, set, create, remove

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use colored::Colorize;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color};

#[derive(Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub command: Option<AgentsCommands>,
}

#[derive(Subcommand)]
pub enum AgentsCommands {
    /// List agents
    List {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show detailed agent status
    Status {
        /// Agent ID
        agent_id: String,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show agent configuration
    Config {
        /// Agent ID
        agent_id: String,
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Pause an agent
    Pause {
        /// Agent ID
        agent_id: String,
    },
    /// Resume a paused agent
    Resume {
        /// Agent ID
        agent_id: String,
    },
    /// Set a config value (dotted path)
    Set {
        /// Agent ID
        agent_id: String,
        /// Config key path (e.g., llm.model)
        key_path: String,
        /// New value
        value: String,
    },
    /// Create a new agent
    Create {
        /// Create from TOML file
        #[arg(long)]
        from_file: Option<String>,
        /// Interactive creation mode
        #[arg(long)]
        interactive: bool,
        /// Create from chat description (planned)
        #[arg(long)]
        from_chat: Option<String>,
    },
    /// Remove (archive) an agent
    Remove {
        /// Agent ID
        agent_id: String,
    },
}

// ── Local deserialization structs ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
struct AgentItem {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skills: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentDetail {
    agent: AgentItemFull,
    #[serde(default)]
    metrics: Option<AgentMetricsInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentItemFull {
    id: String,
    name: String,
    status: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    skills: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentMetricsInfo {
    #[serde(default)]
    tasks_completed: Option<i64>,
    #[serde(default)]
    tasks_failed: Option<i64>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    success_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentConfigResponse {
    config: serde_json::Value,
    config_version: u64,
}

impl TableRow for AgentItem {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("ID", 20),
            ("NAME", 22),
            ("STATUS", 10),
            ("MODEL", 20),
            ("SKILLS", 20),
        ]
    }

    fn table_row(&self) -> String {
        let model = self.model.as_deref().unwrap_or("-");
        let skills = self
            .skills
            .as_ref()
            .map(|s| s.join(", "))
            .unwrap_or_else(|| "-".to_string());

        format!(
            "{:<20} {:<22} {:<10} {:<20} {:<20}",
            truncate(&self.id, 18),
            truncate(&self.name, 20),
            status_color(&self.status),
            truncate(model, 18),
            truncate(&skills, 18),
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

pub async fn run(args: AgentsArgs) -> Result<()> {
    let command = match args.command {
        Some(cmd) => cmd,
        None => {
            // No subcommand → interactive create
            return create_interactive().await;
        }
    };

    match command {
        AgentsCommands::List { status, format } => list_agents(status, format).await,
        AgentsCommands::Status { agent_id, format } => agent_status(&agent_id, format).await,
        AgentsCommands::Config { agent_id, format } => agent_config(&agent_id, format).await,
        AgentsCommands::Pause { agent_id } => agent_action(&agent_id, "pause").await,
        AgentsCommands::Resume { agent_id } => agent_action(&agent_id, "resume").await,
        AgentsCommands::Set {
            agent_id,
            key_path,
            value,
        } => agent_set(&agent_id, &key_path, &value).await,
        AgentsCommands::Create {
            from_file,
            interactive,
            from_chat,
        } => {
            if let Some(ref desc) = from_chat {
                create_from_chat(desc).await
            } else if let Some(ref path) = from_file {
                create_from_file(path).await
            } else if interactive {
                create_interactive().await
            } else {
                // Default: interactive mode
                create_interactive().await
            }
        }
        AgentsCommands::Remove { agent_id } => remove_agent(&agent_id).await,
    }
}

async fn list_agents(status: Option<String>, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let mut path = "/v1/agents?limit=50".to_string();
    if let Some(ref s) = status {
        path.push_str(&format!("&status={}", s));
    }
    let agents: Vec<AgentItem> = client.get(&path).await?;
    print_list(&agents, format);
    Ok(())
}

async fn agent_status(agent_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let detail: AgentDetail = client.get(&format!("/v1/agents/{}", agent_id)).await?;

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&detail).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            let a = &detail.agent;
            println!("{} {}", "Agent:".dimmed(), a.id);
            println!("{} {}", "Name:".dimmed(), a.name);
            if let Some(ref desc) = a.description {
                println!("{} {}", "Description:".dimmed(), desc);
            }
            println!("{} {}", "Status:".dimmed(), status_color(&a.status));
            println!(
                "{} {}",
                "Model:".dimmed(),
                a.model.as_deref().unwrap_or("-")
            );
            if let Some(ref skills) = a.skills {
                println!("{} {}", "Skills:".dimmed(), skills.join(", "));
            }

            if let Some(ref m) = detail.metrics {
                println!();
                println!("{}", "Metrics:".dimmed());
                if let Some(tc) = m.tasks_completed {
                    println!("  {} {}", "Tasks completed:".dimmed(), tc);
                }
                if let Some(tf) = m.tasks_failed {
                    println!("  {} {}", "Tasks failed:".dimmed(), tf);
                }
                if let Some(sr) = m.success_rate {
                    println!("  {} {:.1}%", "Success rate:".dimmed(), sr * 100.0);
                }
                if let Some(cost) = m.total_cost_usd {
                    println!("  {} ${:.4}", "Total cost:".dimmed(), cost);
                }
            }
        }
    }
    Ok(())
}

async fn agent_config(agent_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let config: AgentConfigResponse = client
        .get(&format!("/v1/agents/{}/config", agent_id))
        .await?;

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            println!("{} {}", "Config version:".dimmed(), config.config_version);
            println!();
            println!(
                "{}",
                serde_json::to_string_pretty(&config.config).unwrap_or_default()
            );
        }
    }
    Ok(())
}

async fn agent_action(agent_id: &str, action: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "action": action });
    let result: serde_json::Value = client
        .post(&format!("/v1/agents/{}/action", agent_id), &body)
        .await?;

    let status = result["status"].as_str().unwrap_or("unknown");
    println!(
        "{} Agent {} -> {}",
        "✓".green(),
        agent_id,
        status_color(status)
    );
    Ok(())
}

async fn agent_set(agent_id: &str, key_path: &str, value: &str) -> Result<()> {
    let client = DaemonClient::connect()?;

    // 1. Get current config
    let config_resp: AgentConfigResponse = client
        .get(&format!("/v1/agents/{}/config", agent_id))
        .await?;

    let mut config = config_resp.config;
    let version = config_resp.config_version;

    // 2. Navigate dotted path and set value
    let parts: Vec<&str> = key_path.split('.').collect();
    if parts.is_empty() {
        bail!("Invalid key path");
    }

    // Parse value: try number, then bool, then string
    let json_value = if let Ok(n) = value.parse::<i64>() {
        serde_json::Value::Number(n.into())
    } else if let Ok(n) = value.parse::<f64>() {
        serde_json::json!(n)
    } else if value == "true" {
        serde_json::Value::Bool(true)
    } else if value == "false" {
        serde_json::Value::Bool(false)
    } else {
        serde_json::Value::String(value.to_string())
    };

    // Navigate to parent and set leaf
    let mut current = &mut config;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Set the value
            if let Some(obj) = current.as_object_mut() {
                obj.insert(part.to_string(), json_value.clone());
            } else {
                bail!("Path '{}' is not an object", parts[..i].join("."));
            }
        } else {
            // Navigate deeper
            if current.get(*part).is_none() {
                bail!("Path '{}' does not exist in config", parts[..=i].join("."));
            }
            current = current.get_mut(*part).unwrap();
        }
    }

    // 3. PUT updated config with optimistic lock
    let body = serde_json::json!({
        "config": config,
        "config_version": version,
    });

    match client
        .put::<_, serde_json::Value>(&format!("/v1/agents/{}/config", agent_id), &body)
        .await
    {
        Ok(result) => {
            let new_version = result["config_version"].as_u64().unwrap_or(version + 1);
            println!(
                "{} Set {}.{} = {} (version {})",
                "✓".green(),
                agent_id,
                key_path,
                value,
                new_version
            );
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("409") || msg.contains("CONFLICT") || msg.contains("conflict") {
                println!(
                    "{} Config modified by another client, please retry",
                    "✗".red()
                );
            } else {
                return Err(e);
            }
        }
    }
    Ok(())
}

async fn create_from_file(path: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let client = DaemonClient::connect()?;

    let body = serde_json::json!({ "toml_content": content });
    let result: serde_json::Value = client.post("/v1/agents/from-toml", &body).await?;

    let agent_id = result["agent_id"].as_str().unwrap_or("unknown");
    println!("{} Agent created: {}", "✓".green(), agent_id);
    Ok(())
}

async fn create_from_chat(description: &str) -> Result<()> {
    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "description": description });

    match client.post_raw("/v1/agents/from-chat", &body).await {
        Ok(_) => {
            println!("{} Agent created from chat description", "✓".green());
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("501") || msg.contains("Not Implemented") {
                println!(
                    "{} Agent creation from chat description is planned for Phase 6.0+",
                    "→".yellow()
                );
            } else {
                return Err(e);
            }
        }
    }
    Ok(())
}

async fn create_interactive() -> Result<()> {
    let theme = ColorfulTheme::default();

    // 1. Agent name
    let name: String = Input::with_theme(&theme)
        .with_prompt("Agent name")
        .interact_text()?;

    // 2. Description
    let description: String = Input::with_theme(&theme)
        .with_prompt("Description")
        .allow_empty(true)
        .interact_text()?;

    // 3. Model — try to fetch from daemon, fall back to free text
    let model = match get_model_list().await {
        Ok(models) if !models.is_empty() => {
            let display: Vec<String> = models.to_vec();
            let idx = Select::with_theme(&theme)
                .with_prompt("Model")
                .items(&display)
                .default(0)
                .interact()?;
            models[idx].clone()
        }
        _ => {
            let m: String = Input::with_theme(&theme)
                .with_prompt("Model (e.g., claude-sonnet-4-5-20250929)")
                .interact_text()?;
            m
        }
    };

    // 4. Skills
    let skills_input: String = Input::with_theme(&theme)
        .with_prompt("Skills (comma-separated, e.g., research,code_review)")
        .allow_empty(true)
        .interact_text()?;
    let skills: Vec<String> = skills_input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // 5. Max cost per task
    let max_cost_input: String = Input::with_theme(&theme)
        .with_prompt("Max cost per task (USD, 0 for unlimited)")
        .default("0".to_string())
        .interact_text()?;
    let max_cost: f64 = max_cost_input.parse().unwrap_or(0.0);

    // 6. Persona
    let persona: String = Input::with_theme(&theme)
        .with_prompt("Persona/system prompt")
        .allow_empty(true)
        .interact_text()?;

    // Build the config
    let mut config = serde_json::json!({
        "name": name,
        "description": description,
        "llm": {
            "model": model,
        },
        "skills": skills,
    });

    if max_cost > 0.0 {
        config["constraints"] = serde_json::json!({
            "max_cost_per_task": max_cost,
        });
    }
    if !persona.is_empty() {
        config["persona"] = serde_json::Value::String(persona);
    }

    let client = DaemonClient::connect()?;
    let body = serde_json::json!({ "config": config });
    let result: serde_json::Value = client.post("/v1/agents", &body).await?;

    let agent_id = result["agent_id"].as_str().unwrap_or("unknown");
    println!("{} Agent created: {}", "✓".green(), agent_id);
    Ok(())
}

async fn get_model_list() -> Result<Vec<String>> {
    let client = DaemonClient::connect()?;
    let models: Vec<serde_json::Value> = client.get("/v1/models").await?;
    let names: Vec<String> = models
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .collect();
    Ok(names)
}

async fn remove_agent(agent_id: &str) -> Result<()> {
    let confirm = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!(
            "Remove agent '{}'? This will archive it.",
            agent_id
        ))
        .default(false)
        .interact()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    let client = DaemonClient::connect()?;
    let result: serde_json::Value = client
        .delete_req(&format!("/v1/agents/{}", agent_id))
        .await?;

    let status = result["status"].as_str().unwrap_or("archived");
    println!("{} Agent {} -> {}", "✓".green(), agent_id, status);
    Ok(())
}
