//! Agents command — list, status, config, pause, resume, set, create, remove, tidy

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use colored::Colorize;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::tidy;
use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color, truncate};

#[derive(Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub command: Option<AgentsCommands>,
}

#[derive(Subcommand, Debug)]
#[command(defer = true)]
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
    },
    /// Remove (archive) an agent
    Remove {
        /// Agent ID
        agent_id: String,
    },
    /// List the unedited copies of shipped templates in config/agents; --apply moves them to .archived
    Tidy {
        /// Move them (without it, nothing moves)
        #[arg(long)]
        apply: bool,
    },
}

// ── Local deserialization structs ────────────────────────────────

/// One stored agent as `GET /v1/agents` and `GET /v1/agents/{id}` send it:
/// the repository's row, whose skills and model are two JSON texts,
/// `skills_json` (a list of capabilities) and `llm_config_json` (an object
/// with a `model`) — *Rev 123*'s 123-6 and *Rev 124*'s 124-1, (b): the CLI
/// reads them, no daemon change. Read through [`AgentItem`] and
/// [`AgentItemFull`], which print and serialize `model` and `skills`.
#[derive(Debug, Deserialize)]
pub(crate) struct AgentRow {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) skills_json: Option<String>,
    #[serde(default)]
    pub(crate) llm_config_json: Option<String>,
}

/// The model `llm_config_json` names; `None` when it names none or is not
/// an object's JSON.
pub(crate) fn row_model(llm_config_json: Option<&str>) -> Option<String> {
    let config: serde_json::Value = serde_json::from_str(llm_config_json?).ok()?;
    config["model"]
        .as_str()
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

/// The capability names `skills_json` lists, each an object's `name` (as
/// the store writes them) or a bare string; `None` when it lists none or is
/// not a list's JSON.
pub(crate) fn row_skills(skills_json: Option<&str>) -> Option<Vec<String>> {
    let list: Vec<serde_json::Value> = serde_json::from_str(skills_json?).ok()?;
    let names: Vec<String> = list
        .iter()
        .filter_map(|item| item.as_str().or_else(|| item["name"].as_str()))
        .map(str::to_string)
        .collect();
    (!names.is_empty()).then_some(names)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(from = "AgentRow")]
pub(crate) struct AgentItem {
    id: String,
    name: String,
    status: String,
    model: Option<String>,
    skills: Option<Vec<String>>,
}

impl From<AgentRow> for AgentItem {
    fn from(row: AgentRow) -> Self {
        Self {
            model: row_model(row.llm_config_json.as_deref()),
            skills: row_skills(row.skills_json.as_deref()),
            id: row.id,
            name: row.name,
            status: row.status,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentDetail {
    agent: AgentItemFull,
    #[serde(default)]
    metrics: Option<AgentMetricsInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(from = "AgentRow")]
pub(crate) struct AgentItemFull {
    id: String,
    name: String,
    status: String,
    model: Option<String>,
    description: Option<String>,
    skills: Option<Vec<String>>,
}

impl From<AgentRow> for AgentItemFull {
    fn from(row: AgentRow) -> Self {
        Self {
            model: row_model(row.llm_config_json.as_deref()),
            skills: row_skills(row.skills_json.as_deref()),
            id: row.id,
            name: row.name,
            status: row.status,
            description: row.description,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentMetricsInfo {
    #[serde(default)]
    tasks_completed: Option<i64>,
    #[serde(default)]
    tasks_failed: Option<i64>,
    #[serde(default)]
    success_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AgentConfigResponse {
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

// ── Command runner ───────────────────────────────────────────────

pub async fn run(args: AgentsArgs) -> Result<()> {
    let Some(command) = args.command else {
        // No subcommand → interactive create
        return create_interactive().await;
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
        } => {
            if let Some(ref path) = from_file {
                create_from_file(path).await
            } else if interactive {
                create_interactive().await
            } else {
                // Default: interactive mode
                create_interactive().await
            }
        }
        AgentsCommands::Remove { agent_id } => remove_agent(&agent_id).await,
        AgentsCommands::Tidy { apply } => {
            let (config_dir, daemon) = tidy::target().await?;
            for line in tidy_lines(&config_dir, daemon.as_ref(), apply, Utc::now()).await? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// What `agents tidy` prints for `config_dir`'s `agents/`, having moved
/// what it moved (subagent design §2.12; [`tidy`] says how).
pub(crate) async fn tidy_lines(
    config_dir: &Path,
    daemon: Option<&DaemonClient>,
    apply: bool,
    now: DateTime<Utc>,
) -> Result<Vec<String>> {
    tidy::tidy_lines(tidy::Kind::Agents, config_dir, daemon, apply, now).await
}

/// `GET /v1/agents`'s path: fifty rows, and `status` as typed, encoded
/// (*Rev 147*'s item 6, (b)). `agent list`, the REPL's `/agents` and the
/// config screen's list ask it; `contract_queries` holds it to the route's query
/// sample (CONTRACT-SAMPLES-3).
pub(crate) fn list_path(status: Option<&str>) -> String {
    let mut path = "/v1/agents?limit=50".to_string();
    if let Some(s) = status {
        path.push_str(&format!("&status={}", urlencoding::encode(s)));
    }
    path
}

async fn list_agents(status: Option<String>, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let path = list_path(status.as_deref());
    let agents: Vec<AgentItem> = client.get(&path).await?;
    print_list(&agents, format);
    Ok(())
}

async fn agent_status(agent_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let detail: AgentDetail = client.get(&format!("/v1/agents/{}", agent_id)).await?;

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&detail).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            for line in status_lines(&detail) {
                println!("{line}");
            }
        }
    }
    Ok(())
}

/// What `agent status` prints for one agent as a table, line by line.
fn status_lines(detail: &AgentDetail) -> Vec<String> {
    let mut out = Vec::new();
    let a = &detail.agent;
    out.push(format!("{} {}", "Agent:".dimmed(), a.id));
    out.push(format!("{} {}", "Name:".dimmed(), a.name));
    if let Some(ref desc) = a.description {
        out.push(format!("{} {}", "Description:".dimmed(), desc));
    }
    out.push(format!(
        "{} {}",
        "Status:".dimmed(),
        status_color(&a.status)
    ));
    out.push(format!(
        "{} {}",
        "Model:".dimmed(),
        a.model.as_deref().unwrap_or("-")
    ));
    if let Some(ref skills) = a.skills {
        out.push(format!("{} {}", "Skills:".dimmed(), skills.join(", ")));
    }

    if let Some(ref m) = detail.metrics {
        out.push(String::new());
        out.push(format!("{}", "Metrics:".dimmed()));
        if let Some(tc) = m.tasks_completed {
            out.push(format!("  {} {}", "Tasks completed:".dimmed(), tc));
        }
        if let Some(tf) = m.tasks_failed {
            out.push(format!("  {} {}", "Tasks failed:".dimmed(), tf));
        }
        if let Some(sr) = m.success_rate {
            out.push(format!("  {} {:.1}%", "Success rate:".dimmed(), sr * 100.0));
        }
    }
    out
}

async fn agent_config(agent_id: &str, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect().await?;
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
    let client = DaemonClient::connect().await?;
    let body = action_body(action);
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
    let client = DaemonClient::connect().await?;
    agent_set_with(&client, agent_id, key_path, value).await
}

async fn agent_set_with(
    client: &DaemonClient,
    agent_id: &str,
    key_path: &str,
    value: &str,
) -> Result<()> {
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
    let body = config_body(config, version);

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
        // Only a stale version is the retry case. Any other refusal — a
        // run's own instance while it runs, `409 AGENT_RUNNING`, among them
        // — is the daemon's sentence, returned so it is printed and the
        // exit is non-zero.
        Err(e) if e.to_string().starts_with("CONFIG_CONFLICT: ") => {
            println!(
                "{} Config modified by another client, please retry",
                "✗".red()
            );
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

async fn create_from_file(path: &str) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let client = DaemonClient::connect().await?;

    let body = from_toml_body(&content);
    let result: serde_json::Value = client.post("/v1/agents/from-toml", &body).await?;

    let agent_id = result["agent_id"].as_str().unwrap_or("unknown");
    println!("{} Agent created: {}", "✓".green(), agent_id);
    Ok(())
}

async fn create_interactive() -> Result<()> {
    let theme = ColorfulTheme::default();

    // 1. Agent name
    let name: String = Input::with_theme(&theme)
        .with_prompt("Agent name")
        .interact_text()?;

    // 1a. The id, derived from the name and the person's to change
    let id = ask_agent_id(&theme, &name)?;

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
                .with_prompt("Model (e.g., claude-sonnet-5-5)")
                .interact_text()?;
            m
        }
    };

    // 4. Skills
    let skills_input: String = Input::with_theme(&theme)
        .with_prompt(CAPABILITIES_PROMPT)
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

    let body = create_agent_body(
        &id,
        &name,
        &description,
        &model,
        &persona,
        &skills,
        max_cost,
    );
    let client = DaemonClient::connect().await?;
    let result: serde_json::Value = client.post("/v1/agents", &body).await?;

    let agent_id = result["agent_id"].as_str().unwrap_or("unknown");
    println!("{} Agent created: {}", "✓".green(), agent_id);
    Ok(())
}

/// `POST /v1/agents/{id}/action`'s body (`pause`, `resume`), from `agent
/// pause|resume` and the `openalpaca config` screen.
pub(crate) fn action_body(action: &str) -> serde_json::Value {
    serde_json::json!({ "action": action })
}

/// `PUT /v1/agents/{id}/config`'s body: the edited config and the version
/// it was read at, from `agent set` and the `openalpaca config` screen.
pub(crate) fn config_body(config: serde_json::Value, version: u64) -> serde_json::Value {
    serde_json::json!({
        "config": config,
        "config_version": version,
    })
}

/// `POST /v1/agents/from-toml`'s body, from `agent create --from-file`.
pub(crate) fn from_toml_body(toml_content: &str) -> serde_json::Value {
    serde_json::json!({ "toml_content": toml_content })
}

/// The prompt both interactive creations (`agents create` and the
/// `openalpaca config` screen) ask the agent's capabilities with: the answer
/// becomes `capabilities.assigned`, the names the sandbox admits tools by,
/// so its examples are capabilities a shipped template assigns (a test
/// holds them to the bundle). Nothing checks the answer.
pub(crate) const CAPABILITIES_PROMPT: &str =
    "Capabilities (comma-separated, e.g., web_access,memory_read)";

/// The prompt both interactive creations ask the agent's id with, after
/// its name (*Rev 142*'s item 11, (a)).
pub(crate) const AGENT_ID_PROMPT: &str = "Agent id";

/// The id both interactive creations give the agent: [`AGENT_ID_PROMPT`]
/// asked through `ask` with [`agent_id_for`]'s id as its default. An answer
/// is taken only in the form [`agent_id_for`] gives (`agent_id_for(typed)
/// == typed`), the form the CLI sent before the id could be typed (*Rev
/// 147*'s item 4, the CLI side): otherwise `say` is given
/// [`agent_id_refusal`] and the id is asked again, its form of the answer
/// the default. `POST /v1/agents` itself checks no id's form and joins it
/// into a file path; only this CLI's prompts hold it.
pub(crate) fn agent_id_with(
    name: &str,
    mut ask: impl FnMut(&str, String) -> Result<String>,
    mut say: impl FnMut(&str),
) -> Result<String> {
    let mut offered = agent_id_for(name);
    loop {
        let typed = ask(AGENT_ID_PROMPT, offered)?;
        match agent_id_refusal(&typed) {
            None => return Ok(typed),
            Some(refusal) => {
                say(&refusal);
                offered = agent_id_for(&typed);
            }
        }
    }
}

/// Why `typed` is not an agent id this CLI sends, in [`agent_id_for`]'s
/// own terms, or `None` when it is one (`agent_id_for(typed) == typed`).
pub(crate) fn agent_id_refusal(typed: &str) -> Option<String> {
    let form = agent_id_for(typed);
    (form != typed).then(|| {
        format!(
            "'{typed}' is not an agent id: an id holds only lower-case ASCII letters and \
             digits, each run of other characters one `_`, none at either end; \
             `{form}` is that form of it"
        )
    })
}

/// [`agent_id_with`] at the terminal: Enter keeps the offered id, and an
/// answer in another form is refused on its own line and asked again.
pub(crate) fn ask_agent_id(theme: &ColorfulTheme, name: &str) -> Result<String> {
    agent_id_with(
        name,
        |prompt, offered| {
            Ok(Input::with_theme(theme)
                .with_prompt(prompt)
                .default(offered)
                .interact_text()?)
        },
        |refusal| eprintln!("{} {refusal}", "✗".red()),
    )
}

/// The id an interactive creation offers for the agent it names: the name
/// in lower case, each run of characters other than ASCII letters and
/// digits one `_`, none at either end — `"Code Reviewer"` is
/// `code_reviewer`. A name with no such character at all is `agent`.
pub(crate) fn agent_id_for(name: &str) -> String {
    let mut id = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('_') {
            id.push('_');
        }
    }
    let id = id.trim_end_matches('_').to_string();
    if id.is_empty() {
        "agent".to_string()
    } else {
        id
    }
}

/// The `POST /v1/agents` body both interactive creations send — `agent
/// create`'s prompts and the `openalpaca config` screen's
/// (AGENT-CREATE-CLI; *Rev 123*'s 123-4, (b)): `{config}`, the agent's
/// config file as the route's `CreateAgentRequest` reads it, built from the
/// type itself (`openalpaca_core::agent::config::AgentConfigFile`). The id
/// is the one the person kept or typed, in [`agent_id_for`]'s form
/// ([`agent_id_with`]). The skills are the capabilities assigned (the type
/// has no `skills`), the persona the preset's, the cost the one constraint
/// when above zero. No temperature: the screens do not ask for one, since
/// no agent turn sends a temperature to the model.
pub(crate) fn create_agent_body(
    id: &str,
    name: &str,
    description: &str,
    model: &str,
    persona: &str,
    skills: &[String],
    max_cost: f64,
) -> serde_json::Value {
    use openalpaca_core::agent::config::{
        AgentCapabilitiesConfig, AgentConfigFile, AgentConstraintsConfig, AgentLlmConfigFile,
        AgentMeta, AgentPresetConfig,
    };
    let constraints = (max_cost > 0.0).then_some(AgentConstraintsConfig {
        max_tool_calls: None,
        timeout_seconds: None,
        max_cost_per_task: Some(max_cost),
        max_rounds: None,
        require_confirmation_for: None,
        allowed_capabilities: None,
        denied_capabilities: None,
        allowed_models: None,
        denied_models: None,
        auto_approve: None,
        denied_sections: None,
        max_context_tokens: None,
    });
    let config = AgentConfigFile {
        agent: AgentMeta {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            icon: None,
        },
        capabilities: AgentCapabilitiesConfig {
            assigned: skills.to_vec(),
            denied: None,
        },
        preset: AgentPresetConfig {
            persona: persona.to_string(),
            temperature: None,
            verbosity: None,
        },
        constraints,
        llm: Some(AgentLlmConfigFile {
            model: Some(model.to_string()),
            fallback_models: None,
            overrides: None,
        }),
    };
    serde_json::json!({ "config": config })
}

async fn get_model_list() -> Result<Vec<String>> {
    let client = DaemonClient::connect().await?;
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

    let client = DaemonClient::connect().await?;
    let result: serde_json::Value = client
        .delete_req(&format!("/v1/agents/{}", agent_id))
        .await?;

    let status = result["status"].as_str().unwrap_or("archived");
    println!("{} Agent {} -> {}", "✓".green(), agent_id, status);
    Ok(())
}

#[cfg(test)]
mod tests;
