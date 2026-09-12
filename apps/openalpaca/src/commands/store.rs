//! `openalpaca store` — the content store from the command line (plan §4.8).
//!
//! ```text
//! openalpaca store rebase <old> <new> [--dry-run]
//! openalpaca store purge  <project>|--all [--dry-run] [-y]
//! ```
//!
//! A project's path is its identity in four places — its artifacts, its
//! conversations, its runs and its workspace memories — so moving the directory
//! strands all four at once. `rebase` is the one call that moves them together,
//! and the `--dry-run` is the same question asked without the answer being
//! written: how many rows of each kind name the old root, and what would happen
//! to the store directory.
//!
//! Paths are taken as given (absolute, please). The daemon resolves the *old*
//! one the same way a chat turn's workspace path is resolved, because that is
//! how the rows were recorded; the *new* one it takes literally, and refuses a
//! destination that sits inside another project's root rather than quietly
//! re-basing onto that root instead.
//!
//! `purge` is the other direction: a project you are done with, deleted from
//! the store. It prints a plan first — one line per entry of the store, in the
//! retention-class terms the seeded README already uses, saying `delete` or
//! `keep` for each — and prints it *instead of* purging unless you pass `-y`.
//! Conversations, runs and uploads go; produced artifacts, workspace memories
//! and anything OpenAlpaca did not create stay, and the plan says so by name
//! rather than by omission. Unlike `rebase`'s *old* path, `<project>` is never
//! silently walked up to an ancestor root: `purge /repo/src` is refused
//! (`422 WORKSPACE_NOT_A_ROOT`, naming `/repo`) rather than deleting the whole
//! project for a path that named one file of it — a destructive verb takes the
//! path it was given, or it takes nothing.
//!
//! Deliberately not a re-implementation of either transaction: everything below
//! is `GET`/`PATCH /v1/workspaces` and `POST /v1/workspaces/purge`. The daemon
//! owns the DB and the session-log writers, so the CLI never touches the store
//! directly; it owns the refusals too, and the CLI prints them.

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
    /// Delete the history of a project you are done with
    Purge {
        /// The project root to purge
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        project: Option<String>,
        /// Purge every project root on record
        #[arg(long)]
        all: bool,
        /// Print the plan and change nothing — what happens anyway without -y
        #[arg(long)]
        dry_run: bool,
        /// Carry the plan out
        #[arg(short = 'y', long = "yes", conflicts_with = "dry_run")]
        yes: bool,
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
    /// Runs under this root that are `queued` — named separately because the
    /// two writers refuse on different sets: a re-base waits for
    /// `running`/`paused` only, a purge refuses on `queued` as well. `None` is
    /// a daemon that predates the field, which is why the plan prints the line
    /// only when the number is actually reported.
    #[serde(default)]
    queued_tasks: Option<usize>,
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

// ── purge ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct PurgeRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    all: bool,
    dry_run: bool,
}

/// One line of the daemon's plan: a store entry, what it holds under this root,
/// the retention class the README gives it, and `delete` or `keep`.
#[derive(Debug, Deserialize)]
struct PlanEntry {
    entry: String,
    holds: String,
    retention: String,
    action: String,
}

#[derive(Debug, Default, Deserialize)]
struct PurgeCounts {
    #[serde(default)]
    sessions: usize,
    #[serde(default)]
    messages: usize,
    #[serde(default)]
    tool_calls: usize,
    #[serde(default)]
    followups: usize,
    #[serde(default)]
    tasks: usize,
    #[serde(default)]
    spans: usize,
    #[serde(default)]
    run_events: usize,
    #[serde(default)]
    uploads: usize,
}

impl PurgeCounts {
    /// Every member named, zeroes included — the `Counts::describe`
    /// convention: "nothing else went" is the part a reader is checking for.
    fn describe(&self) -> String {
        format!(
            "{} conversations, {} messages, {} tool calls, {} follow-ups, {} runs, {} spans, \
             {} run events, {} uploads",
            self.sessions,
            self.messages,
            self.tool_calls,
            self.followups,
            self.tasks,
            self.spans,
            self.run_events,
            self.uploads
        )
    }
}

#[derive(Debug, Default, Deserialize)]
struct Removed {
    #[serde(default)]
    session_dirs: usize,
    #[serde(default)]
    upload_files: usize,
}

#[derive(Debug, Deserialize)]
struct PurgeProject {
    path: String,
    #[serde(default)]
    entries: Vec<PlanEntry>,
    #[serde(default)]
    counts: PurgeCounts,
    #[serde(default)]
    removed: Option<Removed>,
}

#[derive(Debug, Deserialize)]
struct PurgeResult {
    #[serde(default)]
    applied: bool,
    #[serde(default)]
    projects: Vec<PurgeProject>,
    /// `--all` only: what the home scope holds and never touches — its own
    /// conversations and uploads, and the home store's `state/`, each its own
    /// entry so both get the same "named, not omitted" treatment a project's
    /// plan gives every member.
    #[serde(default)]
    home_scope: Vec<PlanEntry>,
}

pub async fn run(args: StoreArgs) -> Result<()> {
    match args.command {
        StoreCommands::Rebase { old, new, dry_run } => rebase(&old, &new, dry_run).await,
        StoreCommands::Purge {
            project,
            all,
            dry_run,
            yes,
        } => purge(project.as_deref(), all, purge_is_dry(dry_run, yes)).await,
    }
}

/// `--dry-run` is the default until `-y`.
///
/// The two flags conflict in the parser, so this is not resolving a
/// contradiction — it is the statement that the *absence* of both means the
/// safe one.
fn purge_is_dry(dry_run: bool, yes: bool) -> bool {
    dry_run || !yes
}

fn workspace_query(path: &str) -> String {
    format!("/v1/workspaces?path={}", urlencoding::encode(path))
}

async fn rebase(old: &str, new: &str, dry_run: bool) -> Result<()> {
    let client = DaemonClient::connect()?;

    if dry_run {
        let from: WorkspaceView = client.get(&workspace_query(old)).await?;
        let to: WorkspaceView = client.get(&workspace_query(new)).await?;
        print_plan(new, &from, &to);
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

async fn purge(project: Option<&str>, all: bool, dry_run: bool) -> Result<()> {
    let client = DaemonClient::connect()?;
    let result: PurgeResult = client
        .post(
            "/v1/workspaces/purge",
            &PurgeRequest {
                path: project,
                all,
                dry_run,
            },
        )
        .await?;

    if result.projects.is_empty() {
        println!("{}", "No project roots are on record.".yellow());
    }
    for project in &result.projects {
        println!();
        let heading = match result.applied {
            true => format!("{} {}", "Purged".green().bold(), project.path),
            false => format!("{} {}", "Would purge".bold(), project.path),
        };
        println!("{heading}");
        print_entries(&project.entries);
        // Present only on a real run: the daemon fills it from what the
        // transaction actually deleted, not from the plan it was given.
        if let Some(removed) = &project.removed {
            println!("  {} {}", "deleted:".bold(), project.counts.describe());
            println!(
                "  {} {} session log directories, {} upload files",
                "removed:".bold(),
                removed.session_dirs,
                removed.upload_files
            );
        }
    }
    if !result.home_scope.is_empty() {
        println!();
        print_entries(&result.home_scope);
    }
    if !result.applied {
        println!();
        println!(
            "{}",
            "Nothing was deleted. Re-run with -y to carry this out.".dimmed()
        );
    }
    Ok(())
}

/// The plan, one entry per two lines: the verdict and what is there, then the
/// retention class that verdict comes from.
///
/// The class is printed for the keeps as much as for the deletes — "artifacts
/// are never garbage-collected" is the sentence that makes the delete lines
/// trustworthy.
fn print_entries(entries: &[PlanEntry]) {
    print!("{}", render_entries(entries));
}

/// Whether a plan entry deletes something or keeps it — the one fact a
/// verdict cell's colour depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Delete,
    Keep,
}

impl Verdict {
    fn of(entry: &PlanEntry) -> Self {
        match entry.action == "delete" {
            true => Verdict::Delete,
            false => Verdict::Keep,
        }
    }
}

/// The real painter: red for a delete, green for a keep, both bold. `colored`
/// decides at call time whether that becomes escape codes or plain text.
fn paint_verdict(padded: &str, verdict: Verdict) -> String {
    match verdict {
        Verdict::Delete => padded.red().bold().to_string(),
        Verdict::Keep => padded.green().bold().to_string(),
    }
}

/// [`print_entries`]'s body, as a `String` — split out so the column layout
/// can be asserted on directly instead of only by eye.
fn render_entries(entries: &[PlanEntry]) -> String {
    render_entries_with(entries, paint_verdict)
}

/// `render_entries`, with the verdict cell's colouring supplied by the caller
/// instead of reaching for `colored` directly.
///
/// The split exists for one reason: `colored::control`'s override is a
/// process-global `AtomicBool`, and this bin crate's `#[cfg(test)]` modules
/// all share one test binary running on parallel threads — `chat.rs`,
/// `tasks.rs` and `sessions/tests.rs` each pin it to `false` around their own
/// assertions. A test that flipped it to `true` to exercise the coloured path
/// would race every one of them. Injecting the painter lets a test prove
/// "padding happens before painting" with a marker that has nothing to do
/// with that global, so nothing here ever calls `colored::control` at all.
fn render_entries_with(entries: &[PlanEntry], paint: impl Fn(&str, Verdict) -> String) -> String {
    use std::fmt::Write as _;

    let width = entries
        .iter()
        .map(|e| e.entry.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for entry in entries {
        // Padded before it is painted: a width applied after painting counts
        // a wrapper's own characters (escape bytes, a test's marker, whatever
        // it adds) as part of the column, and the column stops lining up.
        let padded = format!("{:>6}", entry.action);
        let verdict = paint(&padded, Verdict::of(entry));
        let _ = writeln!(
            out,
            "  {verdict}  {name:<width$}  {holds}",
            name = entry.entry,
            holds = entry.holds,
        );
        let _ = writeln!(
            out,
            "          {:<width$}  {}",
            "",
            format!("({})", entry.retention).dimmed(),
        );
    }
    out
}

/// The ancestor a destination would be walked up to, when the daemon resolved
/// it to something other than the path that was asked about.
///
/// The `PATCH` takes its destination literally and answers `422`
/// `WORKSPACE_NOT_A_ROOT` for exactly this, so the dry run says so first. The
/// comparison is against the *resolved* answer the `GET` echoed back, which is
/// the only thing here that knows the daemon's own view of the path.
fn destination_ancestor<'a>(requested: &str, resolved: &'a str) -> Option<&'a str> {
    let requested = std::fs::canonicalize(requested.trim())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| requested.trim().trim_end_matches('/').to_string());
    (requested.trim_end_matches('/') != resolved).then_some(resolved)
}

/// The `--dry-run` body: what is there, and what the re-base would do about it.
fn print_plan(requested_new: &str, from: &WorkspaceView, to: &WorkspaceView) {
    print!("{}", render_plan(requested_new, from, to));
}

/// [`print_plan`]'s body, as a `String` — split out for the same reason
/// [`render_entries`] was: the lines a dry run owes its reader are assertable
/// text, not something to check by eye.
///
/// It reports rather than refuses — a dry run that exits non-zero on a
/// condition the real call would also refuse tells the caller nothing the real
/// call would not — except for the one case where there is simply nothing to
/// re-base, which is a mistyped path far more often than it is a no-op.
fn render_plan(requested_new: &str, from: &WorkspaceView, to: &WorkspaceView) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} → {}",
        "Would re-base".bold(),
        from.path.dimmed(),
        to.path
    );
    let _ = writeln!(out, "  {}", from.rows.describe());

    if let Some(ancestor) = destination_ancestor(requested_new, &to.path) {
        let _ = writeln!(
            out,
            "  {}",
            format!(
                "the destination is inside the project rooted at {ancestor}; the re-base would \
                 be refused (a destination must be a project root of its own)"
            )
            .yellow()
        );
    }
    // Advisory: this reads *this* process's home root, which is the daemon's on
    // the machine they share. The daemon is what actually refuses.
    for (label, view) in [("the old path", from), ("the new path", to)] {
        if resolves_to_the_home_store(std::path::Path::new(&view.path)) {
            let _ = writeln!(
                out,
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
        let _ = writeln!(
            out,
            "  {}",
            "nothing is recorded under that root — check the path".yellow()
        );
    }
    if from.active_tasks > 0 {
        let _ = writeln!(
            out,
            "  {}",
            format!(
                "{} run(s) there are still in flight; the re-base would be refused until they finish",
                from.active_tasks
            )
            .yellow()
        );
    }
    // The other half of "in flight", and the reason `GET /v1/workspaces` names
    // both numbers: a queued run is not what a re-base waits for (its row moves
    // with the rest, and it has not resolved a store yet), but it *is* what a
    // purge of the same root refuses on. A reader shown only `active_tasks`
    // could not tell why the purge they tried next was refused with nothing
    // running. Printed only when the daemon actually reports the number.
    if let Some(queued) = from.queued_tasks.filter(|queued| *queued > 0) {
        let _ = writeln!(
            out,
            "  {}",
            format!(
                "{queued} run(s) there are queued; the re-base takes their rows with it, but a \
                 purge of that root would be refused until they finish"
            )
            .yellow()
        );
    }
    if !to.rows.is_empty() {
        let _ = writeln!(
            out,
            "  {}",
            format!(
                "{} already has a store recorded against it ({}); the re-base would be refused",
                to.path,
                to.rows.describe()
            )
            .yellow()
        );
    }
    let _ = match (from.store_present, to.store_present) {
        (true, true) => writeln!(
            out,
            "  {}",
            "both roots hold a store directory; the re-base would be refused".yellow()
        ),
        (true, false) => writeln!(out, "  the store directory would move too"),
        (false, true) => writeln!(out, "  the store directory is already at the new root"),
        (false, false) => writeln!(out, "  neither root holds a store directory"),
    };
    if to.moved {
        let _ = writeln!(
            out,
            "  the store at {} records {}",
            to.path,
            to.recorded_root.as_deref().unwrap_or("nothing").dimmed()
        );
    }
    out
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

    fn try_parse(args: &[&str]) -> Result<StoreCommands, clap::Error> {
        Harness::try_parse_from(std::iter::once("openalpaca").chain(args.iter().copied()))
            .map(|h| h.command)
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
    fn a_destination_the_daemon_resolved_elsewhere_names_the_ancestor() {
        // The daemon answered about `/mono` for a request about `/mono/sub`:
        // that is the 422 the real call would give.
        assert_eq!(
            destination_ancestor("/mono/sub/proj", "/mono"),
            Some("/mono")
        );
        // A destination the daemon took as given is not flagged, trailing
        // separator and all.
        assert_eq!(destination_ancestor("/new/proj", "/new/proj"), None);
        assert_eq!(destination_ancestor("/new/proj/", "/new/proj"), None);
    }

    #[test]
    fn purge_takes_one_project_or_all_and_never_both() {
        assert!(matches!(
            parse(&["purge", "/some/proj"]),
            StoreCommands::Purge { project: Some(p), all: false, .. } if p == "/some/proj"
        ));
        assert!(matches!(
            parse(&["purge", "--all"]),
            StoreCommands::Purge {
                project: None,
                all: true,
                ..
            }
        ));
        // Both is a parse error, and so is neither — the destructive verb
        // never has to guess which project was meant.
        assert!(try_parse(&["purge", "/some/proj", "--all"]).is_err());
        assert!(try_parse(&["purge"]).is_err());
        // `--dry-run` and `-y` are the same question asked twice.
        assert!(try_parse(&["purge", "/some/proj", "--dry-run", "-y"]).is_err());
    }

    #[test]
    fn a_purge_is_a_dry_run_until_minus_y() {
        // Neither flag: the plan, not the deletion.
        assert!(purge_is_dry(false, false));
        assert!(purge_is_dry(true, false));
        assert!(!purge_is_dry(false, true));

        let StoreCommands::Purge { dry_run, yes, .. } = parse(&["purge", "/p"]) else {
            panic!("not a purge");
        };
        assert!(purge_is_dry(dry_run, yes));
        let StoreCommands::Purge { dry_run, yes, .. } = parse(&["purge", "/p", "-y"]) else {
            panic!("not a purge");
        };
        assert!(!purge_is_dry(dry_run, yes));
        let StoreCommands::Purge { dry_run, yes, .. } = parse(&["purge", "--all", "--yes"]) else {
            panic!("not a purge");
        };
        assert!(!purge_is_dry(dry_run, yes));
    }

    #[test]
    fn a_purge_names_every_member_including_the_zeroes() {
        let counts = PurgeCounts {
            sessions: 3,
            messages: 41,
            tool_calls: 12,
            followups: 0,
            tasks: 2,
            spans: 5,
            run_events: 18,
            uploads: 0,
        };
        assert_eq!(
            counts.describe(),
            "3 conversations, 41 messages, 12 tool calls, 0 follow-ups, 2 runs, 5 spans, \
             18 run events, 0 uploads"
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

    /// Every `\x1b[...m` SGR sequence `colored` emits, dropped — so a rendered
    /// plan can be asserted on by its visible columns regardless of whether
    /// the process forces colour on or off.
    fn strip_ansi(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut chars = input.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// A workspace as `GET /v1/workspaces` describes it, with a store at the
    /// root and rows under it — the ordinary source of a re-base.
    fn view(path: &str, overrides: fn(&mut WorkspaceView)) -> WorkspaceView {
        let mut view = WorkspaceView {
            path: path.to_string(),
            store_present: true,
            recorded_root: Some(path.to_string()),
            moved: false,
            rows: Counts {
                artifacts: 12,
                sessions: 2,
                tasks: 3,
                memories: 7,
            },
            active_tasks: 0,
            queued_tasks: Some(0),
        };
        overrides(&mut view);
        view
    }

    /// D7's CLI half. The two numbers refuse different calls — a re-base waits
    /// for `running`/`paused`, a purge refuses on `queued` as well — so a plan
    /// that printed only `active_tasks` left the reader unable to explain the
    /// purge they tried next.
    #[test]
    fn a_dry_run_names_the_queued_runs_a_purge_would_refuse_on() {
        let from = view("/repo/one", |v| v.queued_tasks = Some(2));
        let to = view("/repo/two", |v| {
            v.store_present = false;
            v.rows = Counts::default();
        });

        let rendered = strip_ansi(&render_plan("/repo/two", &from, &to));
        assert!(
            rendered.contains("2 run(s) there are queued"),
            "the queued runs are named: {rendered}"
        );
        assert!(
            rendered.contains("a purge of that root would be refused"),
            "and what they actually refuse is said: {rendered}"
        );
        // A queued run is not what the re-base itself waits for, and the plan
        // must not claim otherwise.
        assert!(
            !rendered.contains("still in flight"),
            "nothing is running: {rendered}"
        );
    }

    /// Zero queued runs is not a line, and a daemon that predates the field
    /// (`None`) is not a line either — never "0 run(s) there are queued".
    #[test]
    fn a_dry_run_stays_silent_about_queued_runs_it_has_no_number_for() {
        for queued in [Some(0), None] {
            let from = view("/repo/one", |_| {});
            let from = WorkspaceView {
                queued_tasks: queued,
                ..from
            };
            let to = view("/repo/two", |v| {
                v.store_present = false;
                v.rows = Counts::default();
            });
            let rendered = strip_ansi(&render_plan("/repo/two", &from, &to));
            assert!(!rendered.contains("queued"), "{queued:?}: {rendered}");
        }
    }

    /// Minor #8, corrected by ruling R74: `print_entries` had no test, and the
    /// pad-before-paint trick it depends on — padding the verdict to width
    /// *before* it is coloured, never after — needs proving without racing
    /// `colored::control`'s process-global override against the four
    /// pre-existing tests in this binary that pin it to `false`
    /// (`chat.rs`, `tasks.rs`, `sessions/tests.rs`). So the painter is
    /// injected: a marker that wraps the cell in `<`/`>` and nothing else,
    /// which proves the same thing a real `ColoredString` would break on — a
    /// width applied after wrapping, rather than before, would make the
    /// "delete" and "keep" rows different widths — without this test ever
    /// calling `set_override` in either direction.
    #[test]
    fn render_entries_pads_the_verdict_before_it_is_painted() {
        let entries = vec![
            PlanEntry {
                entry: "a".to_string(),
                holds: "holds-1".to_string(),
                retention: "ret-1".to_string(),
                action: "delete".to_string(),
            },
            PlanEntry {
                entry: "bb".to_string(),
                holds: "holds-2".to_string(),
                retention: "ret-2".to_string(),
                action: "keep".to_string(),
            },
        ];

        let rendered = render_entries_with(&entries, |padded, verdict| {
            // (a) the cell handed to the painter is already the fixed-width
            // column, for both verdicts — proof that padding precedes
            // painting rather than the other way around.
            assert_eq!(
                padded.chars().count(),
                6,
                "the verdict cell must already be padded to its column width \
                 before a painter ever sees it: {padded:?}"
            );
            let tag = match verdict {
                Verdict::Delete => "D",
                Verdict::Keep => "K",
            };
            format!("<{tag}:{padded}>")
        });

        // (b) every marker is the same width (1 + 1 + 1 + 6 + 1 = 10 chars)
        // regardless of whether the underlying action was "delete" or
        // "keep", so the name column that follows starts at the same offset
        // on every line — the thing padding-before-painting exists to
        // guarantee, asserted here on the unpainted (marker, not colour)
        // width instead of on `colored`'s escape codes. The retention line
        // does not run through the painter at all, so its indent is the
        // same 14 spaces (2 + 6 + 2 + 2 + 2) as ever.
        let expected = concat!(
            "  <D:delete>  a   holds-1\n",
            "              (ret-1)\n",
            "  <K:  keep>  bb  holds-2\n",
            "              (ret-2)\n",
        );
        assert_eq!(strip_ansi(&rendered), expected);
    }
}
