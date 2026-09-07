//! `openalpaca store` — the content store from the command line (plan §4.8).
//!
//! ```text
//! openalpaca store rebase <old> <new> [--dry-run]
//! ```
//!
//! A project's path is its identity in four places — its artifacts, its
//! conversations, its runs and its workspace memories — so moving the directory
//! strands all four at once. `rebase` is the one call that moves them together,
//! and the `--dry-run` is the same question asked without the answer being
//! written: how many rows of each kind name the old root, and what would happen
//! to the store directory.
//!
//! Paths are taken as given (absolute, please) and resolved by the daemon the
//! same way a chat turn's workspace path is, because the daemon is the one that
//! recorded them.
//!
//! Deliberately not a re-implementation of the transaction: everything below is
//! `GET`/`PATCH /v1/workspaces`. The daemon owns the refusals, and the CLI
//! prints them.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use openalpaca_core::memory::scope_context::resolves_to_the_home_store;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;

#[derive(Args)]
pub struct StoreArgs {
    #[command(subcommand)]
    pub command: StoreCommands,
}

#[derive(Subcommand)]
pub enum StoreCommands {
    /// Re-base a moved project onto its new path
    Rebase {
        /// The path the project used to have
        old: String,
        /// The path it has now
        new: String,
        /// Report what would move and change nothing
        #[arg(long)]
        dry_run: bool,
    },
}

/// One workspace as `GET /v1/workspaces` describes it.
#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceView {
    path: String,
    #[serde(default)]
    store_present: bool,
    #[serde(default)]
    recorded_root: Option<String>,
    #[serde(default)]
    moved: bool,
    rows: Counts,
    #[serde(default)]
    active_tasks: usize,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Counts {
    #[serde(default)]
    artifacts: usize,
    #[serde(default)]
    sessions: usize,
    #[serde(default)]
    tasks: usize,
    #[serde(default)]
    memories: usize,
}

impl Counts {
    fn is_empty(&self) -> bool {
        self.artifacts == 0 && self.sessions == 0 && self.tasks == 0 && self.memories == 0
    }

    /// `12 artifacts, 2 conversations, 3 runs, 7 memories` — every member
    /// named, including the zeroes, because "nothing else moved" is the part a
    /// reader is checking for.
    fn describe(&self) -> String {
        format!(
            "{} artifacts, {} conversations, {} runs, {} memories",
            self.artifacts, self.sessions, self.tasks, self.memories
        )
    }
}

#[derive(Debug, Deserialize)]
struct RebaseResult {
    old_path: String,
    new_path: String,
    moved: Counts,
    #[serde(default)]
    store_moved: bool,
}

#[derive(Debug, Serialize)]
struct RebaseRequest<'a> {
    old_path: &'a str,
    new_path: &'a str,
}

pub async fn run(args: StoreArgs) -> Result<()> {
    match args.command {
        StoreCommands::Rebase { old, new, dry_run } => rebase(&old, &new, dry_run).await,
    }
}

fn workspace_query(path: &str) -> String {
    format!("/v1/workspaces?path={}", urlencoding::encode(path))
}

async fn rebase(old: &str, new: &str, dry_run: bool) -> Result<()> {
    let client = DaemonClient::connect()?;

    if dry_run {
        let from: WorkspaceView = client.get(&workspace_query(old)).await?;
        let to: WorkspaceView = client.get(&workspace_query(new)).await?;
        print_plan(&from, &to);
        return Ok(());
    }

    let result: RebaseResult = client
        .patch(
            "/v1/workspaces",
            &RebaseRequest {
                old_path: old,
                new_path: new,
            },
        )
        .await?;

    println!(
        "{} {} → {}",
        "Re-based".green().bold(),
        result.old_path.dimmed(),
        result.new_path
    );
    println!("  {}", result.moved.describe());
    if result.store_moved {
        println!("  the store directory moved with them");
    }
    Ok(())
}

/// The `--dry-run` body: what is there, and what the re-base would do about it.
///
/// It reports rather than refuses — a dry run that exits non-zero on a
/// condition the real call would also refuse tells the caller nothing the real
/// call would not — except for the one case where there is simply nothing to
/// re-base, which is a mistyped path far more often than it is a no-op.
fn print_plan(from: &WorkspaceView, to: &WorkspaceView) {
    println!(
        "{} {} → {}",
        "Would re-base".bold(),
        from.path.dimmed(),
        to.path
    );
    println!("  {}", from.rows.describe());

    // Advisory: this reads *this* process's home root, which is the daemon's on
    // the machine they share. The daemon is what actually refuses.
    for (label, view) in [("the old path", from), ("the new path", to)] {
        if resolves_to_the_home_store(std::path::Path::new(&view.path)) {
            println!(
                "  {}",
                format!(
                    "{label} resolves to {}, which is the home store rather than a project; \
                     the re-base would be refused",
                    view.path
                )
                .yellow()
            );
        }
    }

    if from.rows.is_empty() {
        println!(
            "  {}",
            "nothing is recorded under that root — check the path".yellow()
        );
    }
    if from.active_tasks > 0 {
        println!(
            "  {}",
            format!(
                "{} run(s) there are still in flight; the re-base would be refused until they finish",
                from.active_tasks
            )
            .yellow()
        );
    }
    if !to.rows.is_empty() {
        println!(
            "  {}",
            format!(
                "{} already has a store recorded against it ({}); the re-base would be refused",
                to.path,
                to.rows.describe()
            )
            .yellow()
        );
    }
    match (from.store_present, to.store_present) {
        (true, true) => println!(
            "  {}",
            "both roots hold a store directory; the re-base would be refused".yellow()
        ),
        (true, false) => println!("  the store directory would move too"),
        (false, true) => println!("  the store directory is already at the new root"),
        (false, false) => println!("  neither root holds a store directory"),
    }
    if to.moved {
        println!(
            "  the store at {} records {}",
            to.path,
            to.recorded_root.as_deref().unwrap_or("nothing").dimmed()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        command: StoreCommands,
    }

    fn parse(args: &[&str]) -> StoreCommands {
        Harness::parse_from(std::iter::once("openalpaca").chain(args.iter().copied())).command
    }

    #[test]
    fn rebase_takes_two_paths_and_an_optional_dry_run() {
        assert!(matches!(
            parse(&["rebase", "/old/p", "/new/p"]),
            StoreCommands::Rebase { old, new, dry_run: false }
                if old == "/old/p" && new == "/new/p"
        ));
        assert!(matches!(
            parse(&["rebase", "/old/p", "/new/p", "--dry-run"]),
            StoreCommands::Rebase { dry_run: true, .. }
        ));
    }

    #[test]
    fn the_path_is_url_encoded_into_the_query() {
        assert_eq!(
            workspace_query("/Users/me/my project"),
            "/v1/workspaces?path=%2FUsers%2Fme%2Fmy%20project"
        );
    }

    #[test]
    fn every_member_is_named_including_the_zeroes() {
        let counts = Counts {
            artifacts: 12,
            sessions: 2,
            tasks: 0,
            memories: 7,
        };
        assert_eq!(
            counts.describe(),
            "12 artifacts, 2 conversations, 0 runs, 7 memories"
        );
        assert!(!counts.is_empty());
        assert!(Counts::default().is_empty());
    }
}
