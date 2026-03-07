//! LLM command — status, keys, usage, models, strategy

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::output::OutputFormat;

use super::llm_keys::{self, KeysArgs};
use super::llm_status;

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

pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max.saturating_sub(3)])
    } else {
        s.to_string()
    }
}

pub async fn run(args: LlmArgs) -> Result<()> {
    match args.command {
        LlmCommands::Status { format } => llm_status::llm_status(format).await,
        LlmCommands::Keys(keys_args) => llm_keys::run_keys(keys_args).await,
        LlmCommands::Usage {
            agent,
            date,
            key,
            daily,
            format,
        } => llm_status::llm_usage(agent, date, key, daily, format).await,
        LlmCommands::Models { format } => llm_status::llm_models(format).await,
        LlmCommands::Strategy { provider, strategy } => {
            llm_status::llm_strategy(&provider, &strategy).await
        }
        LlmCommands::Credentials { format } => llm_status::llm_credentials(format).await,
        LlmCommands::Backends { format } => llm_status::llm_backends(format).await,
        LlmCommands::ProviderUsage { format } => llm_status::llm_provider_usage(format).await,
    }
}
