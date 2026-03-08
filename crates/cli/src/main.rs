use std::sync::Arc;

use anyhow::Result;
use clap::{CommandFactory, Parser};

mod channels;
mod commands;

use commands::{Cli, Commands, ConfigArgs, ConfigCommands, GatewayArgs};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Gateway(args)) => {
            handle_gateway(args).await?;
        }
        Some(Commands::Config(config_args)) => {
            handle_config(config_args)?;
        }
        None => {
            Cli::command().print_help()?;
            println!();
        }
    }

    Ok(())
}

async fn handle_gateway(args: GatewayArgs) -> Result<()> {
    let config_path = openalpaca_infra::env::resolve_config_path();
    let state_dir = openalpaca_infra::env::resolve_state_dir();
    let home_dir = openalpaca_infra::home_dir::resolve_home_dir();

    let snapshot = openalpaca_config::load_config(&config_path)?;
    let mut config = snapshot.config;

    // Apply CLI overrides
    if args.port.is_some() || args.bind.is_some() {
        let gw = config.gateway.get_or_insert_with(Default::default);
        if let Some(port) = args.port {
            gw.port = Some(port);
        }
        if let Some(ref bind) = args.bind {
            gw.host = Some(bind.clone());
        }
    }

    // Initialize logging
    let log_config = openalpaca_logging::LogConfig {
        level: config
            .logging
            .as_ref()
            .and_then(|l| l.level.clone())
            .unwrap_or_else(|| "info".into()),
        ..Default::default()
    };
    let _guards = openalpaca_logging::init_logging(&log_config);

    let http_client = openalpaca_infra::http_client::build_http_client();

    let app_ctx = Arc::new(openalpaca_core::context::AppContext {
        config_path,
        state_dir,
        home_dir,
        http_client: http_client.clone(),
        shutdown: tokio_util::sync::CancellationToken::new(),
    });

    // Register built-in channel plugins
    let mut registry = openalpaca_channels::ChannelRegistry::new();
    let handler: Arc<dyn openalpaca_channels::InboundHandler> =
        Arc::new(openalpaca_channels::EchoHandler);
    channels::register_builtin_channels(&mut registry, &config, &http_client, handler);
    tracing::info!("registered {} channel(s)", registry.len());

    // Set up Ctrl+C handler
    let shutdown = app_ctx.shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl+C, shutting down...");
        shutdown.cancel();
    });

    openalpaca_gateway::run_gateway(app_ctx, config).await?;

    Ok(())
}

fn handle_config(args: ConfigArgs) -> Result<()> {
    let config_path = openalpaca_infra::env::resolve_config_path();

    match args.command {
        Some(ConfigCommands::Get { key }) => {
            let snapshot = openalpaca_config::load_config(&config_path)?;

            if !snapshot.exists {
                eprintln!("No config file found at {}", config_path.display());
                eprintln!("Using default configuration.");
            }

            for issue in &snapshot.issues {
                eprintln!("warning: {issue}");
            }

            match key {
                Some(path) => {
                    match openalpaca_config::io::get_value_at_path(&snapshot.config, &path) {
                        Some(value) => {
                            let yaml = serde_yml::to_string(&value)?;
                            print!("{yaml}");
                        }
                        None => {
                            eprintln!("key not found: {path}");
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    let yaml = serde_yml::to_string(&snapshot.config)?;
                    print!("{yaml}");
                }
            }
        }
        Some(ConfigCommands::Set { key, value }) => {
            let snapshot = openalpaca_config::load_config(&config_path)?;
            let mut config_value = serde_yml::to_value(&snapshot.config)?;

            set_value_at_path(&mut config_value, &key, &value)?;

            let config: openalpaca_config::OpenAlpacaConfig = serde_yml::from_value(config_value)?;
            openalpaca_config::save_config(&config_path, &config)?;
            eprintln!("set {key} = {value}");
        }
        Some(ConfigCommands::Validate) => {
            let snapshot = openalpaca_config::load_config(&config_path)?;
            let mut config = snapshot.config;
            openalpaca_config::defaults::apply_defaults(&mut config);
            let issues = openalpaca_config::Validate::validate(&config);

            if issues.is_empty() {
                if snapshot.exists {
                    eprintln!("config OK: {}", config_path.display());
                } else {
                    eprintln!(
                        "no config file found at {}, using defaults (OK)",
                        config_path.display()
                    );
                }
            } else {
                for issue in &issues {
                    eprintln!("{issue}");
                }
                let error_count = issues
                    .iter()
                    .filter(|i| i.severity == openalpaca_config::validation::Severity::Error)
                    .count();
                if error_count > 0 {
                    eprintln!("{error_count} error(s) found");
                    std::process::exit(1);
                } else {
                    eprintln!("config OK (with {} warning(s))", issues.len());
                }
            }
        }
        None => {
            // Default: print full config (same as `config get`)
            let snapshot = openalpaca_config::load_config(&config_path)?;
            let yaml = serde_yml::to_string(&snapshot.config)?;
            print!("{yaml}");
        }
    }

    Ok(())
}

fn set_value_at_path(root: &mut serde_yml::Value, path: &str, value: &str) -> Result<()> {
    let keys: Vec<&str> = path.split('.').collect();
    let mut current = root;

    for (i, key) in keys.iter().enumerate() {
        if i == keys.len() - 1 {
            // Last key — set the value
            let parsed = serde_yml::from_str::<serde_yml::Value>(value)
                .unwrap_or(serde_yml::Value::String(value.to_string()));
            if let serde_yml::Value::Mapping(map) = current {
                map.insert(serde_yml::Value::String(key.to_string()), parsed);
            }
        } else {
            // Navigate or create intermediate mappings
            if let serde_yml::Value::Mapping(map) = current {
                let key_val = serde_yml::Value::String(key.to_string());
                if !map.contains_key(&key_val) {
                    map.insert(
                        key_val.clone(),
                        serde_yml::Value::Mapping(Default::default()),
                    );
                }
                current = map.get_mut(&key_val).unwrap();
            }
        }
    }

    Ok(())
}
