//! OpenAlpaca CLI
//!
//! Command-line interface for interacting with the OpenAlpaca daemon.
//!
//! Commands:
//! - daemon: Manage daemon process (start/stop/status/tail)
//! - config: Manage system configuration
//! - gui: Manage GUI process
//! - connector: Manage platform connectors
//! - tasks: Manage tasks (list, status, create, cancel, resume; pause refused)
//! - agents: Manage agents (list, status, config, create, remove, tidy)
//! - llm: Manage LLM settings, keys, and usage
//! - chat: Chat with the Orchestrator
//! - sessions: List the conversations a lane holds
//! - store: Manage projects (list them, re-point a moved one, delete one's data)
//! - skills: Show the skill catalog and the problems its last scan found (list, info, tidy)

mod chat_stream;
mod client;
mod commands;
#[cfg(test)]
mod contract_bodies;
#[cfg(test)]
mod contract_queries;
#[cfg(test)]
mod contract_reads;
mod manager;
mod output;
mod repl;
mod reuse;
#[cfg(test)]
pub(crate) mod test_util;
mod unattended;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openalpaca")]
#[command(about = "OpenAlpaca CLI - Control your AI agents", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[command(defer = true)]
enum Commands {
    /// Manage daemon process (status, tail, start, stop)
    Daemon(commands::daemon::DaemonArgs),

    /// Manage system configuration (interactive if no subcommands)
    Config(commands::config::ConfigArgs),

    /// Manage GUI process
    Gui(commands::gui::GuiArgs),

    /// Manage platform connectors (Telegram, etc.)
    Connector(commands::connector::ConnectorArgs),

    /// Manage tasks (list, status, create, cancel, resume; pause refused)
    Tasks(commands::tasks::TasksArgs),

    /// Manage agents (list, status, config, create, remove, tidy)
    Agents(commands::agents::AgentsArgs),

    /// Manage LLM settings, keys, and usage
    Llm(commands::llm::LlmArgs),

    /// Chat with the Orchestrator
    Chat(commands::chat::ChatArgs),

    /// List the conversations a lane holds (plan §5.7)
    Sessions(commands::sessions::SessionsArgs),

    /// Manage plugins (list, approve, deny, enable, disable, config)
    Plugin(commands::plugin::PluginArgs),

    /// Manage extensions — MCP servers and plugins (list, info, enable, disable, reload, approve, deny, remove)
    Ext(commands::ext::ExtArgs),

    /// Manage projects (list them, re-point a moved one, delete one's data)
    Store(commands::store::StoreArgs),

    /// Show or edit the persona documents (USER.md) through the daemon
    Persona(commands::persona::PersonaArgs),

    /// Manage memory notes (list, search, timeline, show, add, edit, suggestions, confirm, undo, forget, sync, doctor, adopt)
    Memory(commands::memory::MemoryArgs),

    /// Show the skill catalog and the problems its last scan found (list, info, tidy)
    Skills(commands::skills::SkillsArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon(args) => commands::daemon::run(args).await,
        Commands::Config(args) => commands::config::run(args).await,
        Commands::Gui(args) => commands::gui::run(args).await,
        Commands::Connector(args) => commands::connector::run(args).await,
        Commands::Tasks(args) => commands::tasks::run(args).await,
        Commands::Agents(args) => commands::agents::run(args).await,
        Commands::Llm(args) => commands::llm::run(args).await,
        Commands::Chat(args) => commands::chat::run(args).await,
        Commands::Sessions(args) => commands::sessions::run(args).await,
        Commands::Plugin(args) => commands::plugin::run(args).await,
        Commands::Ext(args) => commands::ext::run(args).await,
        Commands::Store(args) => commands::store::run(args).await,
        Commands::Persona(args) => commands::persona::run(args).await,
        Commands::Memory(args) => commands::memory::run(args).await,
        Commands::Skills(args) => commands::skills::run(args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::Commands;
    use crate::commands;
    use clap::{Command, CommandFactory, Subcommand};

    /// Every `Subcommand` enum of the CLI, by type. A new enum joins this
    /// list; `the_deferred_list_names_every_subcommand_enum` counts them.
    fn every_subcommand_enum() -> Vec<(&'static str, Command)> {
        fn of<E: Subcommand>(name: &'static str) -> (&'static str, Command) {
            (name, E::augment_subcommands(Command::new(name)))
        }
        vec![
            of::<Commands>("Commands"),
            of::<commands::agents::AgentsCommands>("AgentsCommands"),
            of::<commands::config::ConfigAction>("config::ConfigAction"),
            of::<commands::confirmations::ConfirmationCommands>("ConfirmationCommands"),
            of::<commands::connector::ConnectorCommands>("ConnectorCommands"),
            of::<commands::daemon::DaemonAction>("DaemonAction"),
            of::<commands::ext::ExtCommands>("ExtCommands"),
            of::<commands::ext::ExtMcpCommands>("ExtMcpCommands"),
            of::<commands::gui::GuiAction>("GuiAction"),
            of::<commands::llm::LlmCommands>("LlmCommands"),
            of::<commands::llm_keys::KeysCommands>("KeysCommands"),
            of::<commands::memory::MemoryCommands>("MemoryCommands"),
            of::<commands::persona::PersonaCommands>("PersonaCommands"),
            of::<commands::plugin::PluginCommands>("PluginCommands"),
            of::<commands::plugin::ConfigAction>("plugin::ConfigAction"),
            of::<commands::sessions::SessionsCommands>("SessionsCommands"),
            of::<commands::skills::SkillsCommands>("SkillsCommands"),
            of::<commands::store::StoreCommands>("StoreCommands"),
            of::<commands::tasks::TasksCommands>("TasksCommands"),
        ]
    }

    /// M61: `#[command(defer = true)]` on every `Subcommand` enum (clap 5's
    /// announced default), so a variant's arguments and nested subcommands
    /// are built only when clap builds that subcommand — a parse builds the
    /// path it takes, `--help` the page it prints. Unbuilt, every variant is
    /// its name and about alone; built, the tree is whole.
    #[test]
    fn every_subcommand_enum_defers_its_variants() {
        for (name, unbuilt) in every_subcommand_enum() {
            for sub in unbuilt.get_subcommands() {
                assert!(
                    sub.get_arguments().next().is_none() && !sub.has_subcommands(),
                    "{name}::{} builds its arguments before clap asks: \
                     the enum lacks #[command(defer = true)]",
                    sub.get_name()
                );
            }
        }
        let mut built = super::Cli::command();
        built.build();
        let tasks = built.find_subcommand("tasks").expect("tasks is a command");
        let list = tasks.find_subcommand("list").expect("tasks has list");
        assert!(
            list.get_arguments().any(|arg| arg.get_id() == "status"),
            "a built tree carries every variant's arguments"
        );
    }

    /// A deferred variant's arguments are added after the variant's own help
    /// text, so a doc comment on the `Args` struct or the `Subcommand` enum
    /// those arguments bring (or on a flattened struct) would replace it on
    /// the variant's `--help` page. The page keeps the variant's words.
    #[test]
    fn building_a_variant_keeps_its_own_help_text() {
        fn texts(cmd: &Command) -> Vec<(String, Option<String>, Option<String>)> {
            cmd.get_subcommands()
                // the `help` subcommand clap adds when it builds the command
                .filter(|sub| sub.get_name() != "help")
                .map(|sub| {
                    (
                        sub.get_name().to_string(),
                        sub.get_about().map(ToString::to_string),
                        sub.get_long_about().map(ToString::to_string),
                    )
                })
                .collect()
        }
        for (name, unbuilt) in every_subcommand_enum() {
            let mut built = unbuilt.clone();
            built.build();
            assert_eq!(
                texts(&built),
                texts(&unbuilt),
                "{name}: a variant's --help page lost its own about text"
            );
        }
    }

    /// The list above is every `Subcommand` derive in the crate's source,
    /// and each derive is followed by `#[command(defer = true)]` (an enum of
    /// unit variants alone shows no difference to the test above).
    #[test]
    fn the_deferred_list_names_every_subcommand_enum() {
        fn count(dir: &std::path::Path) -> usize {
            let mut total = 0;
            for entry in std::fs::read_dir(dir).expect("read the source dir") {
                let path = entry.expect("a source entry").path();
                if path.is_dir() {
                    total += count(&path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read a source file");
                    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
                    for (at, line) in lines.iter().enumerate() {
                        let derives_subcommand = line.starts_with("#[derive(")
                            && line
                                .split(['(', ')', ',', ' '])
                                .any(|word| word == "Subcommand");
                        if derives_subcommand {
                            assert_eq!(
                                lines.get(at + 1).copied(),
                                Some("#[command(defer = true)]"),
                                "{}:{}: a Subcommand derive without defer",
                                path.display(),
                                at + 1
                            );
                            total += 1;
                        }
                    }
                }
            }
            total
        }
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        assert_eq!(count(&src), every_subcommand_enum().len());
    }

    /// PG-3: the `memory` command's help line names every `MemoryCommands`
    /// variant, in the enum's order, so the line `openalpaca --help` and
    /// docs/api's command table print cannot leave a verb out again.
    #[test]
    fn the_memory_help_line_names_every_memory_verb() {
        let verbs: Vec<String> =
            commands::memory::MemoryCommands::augment_subcommands(Command::new("memory"))
                .get_subcommands()
                .map(|sub| sub.get_name().to_string())
                .collect();
        let cli = super::Cli::command();
        let memory = cli.find_subcommand("memory").expect("memory is a command");
        assert_eq!(
            memory.get_about().map(ToString::to_string),
            Some(format!("Manage memory notes ({})", verbs.join(", ")))
        );
    }
}
