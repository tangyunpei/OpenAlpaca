use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openalpaca", version, about = "OpenAlpaca AI Gateway")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the gateway server
    Gateway(GatewayArgs),
    /// Manage configuration
    Config(ConfigArgs),
}

#[derive(clap::Args)]
pub struct GatewayArgs {
    /// Port to listen on (overrides config)
    #[arg(long)]
    pub port: Option<u16>,
    /// Bind address (overrides config)
    #[arg(long)]
    pub bind: Option<String>,
}

#[derive(clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigCommands>,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Print the current configuration
    Get {
        /// Config key path (dot-separated, e.g. "channels.telegram.enabled")
        key: Option<String>,
    },
    /// Set a config value
    Set {
        /// Config key path
        key: String,
        /// Value to set
        value: String,
    },
    /// Validate the configuration file
    Validate,
}
