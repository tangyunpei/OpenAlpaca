//! `openalpaca store` — the projects OpenAlpaca keeps a folder for (design §6).
//!
//! ```text
//! openalpaca store list
//! openalpaca store rebase <old> <new> [--dry-run]
//! openalpaca store purge  <project> [--dry-run] [-y]
//! ```
//!
//! A project's content lives at home, in `~/.openalpaca/projects/<name>/`, and
//! its path is recorded in five columns of the database. `list` shows every
//! project and marks one whose folder is gone or has moved (storage design
//! ST-23). `rebase` re-points a moved project — its rows and its folder's
//! name under `projects/` — and its
//! `--dry-run` reads what is recorded at both paths and names what the daemon
//! would refuse. `purge` deletes everything OpenAlpaca holds for a project
//! (ST-19) — conversations, runs, by-products, uploads, every owner's notes and
//! its folder under `projects/` — and never the project's own folder; a file
//! another conversation still uses is kept, moved to the root store. It prints
//! the plan and deletes nothing unless you pass `-y`. It takes a project
//! exactly: its root, or its folder as `store list` prints it
//! (`projects/<name>`); the daemon refuses a path inside a project, naming
//! the root.
//!
//! Everything here is `GET|PATCH /v1/workspaces` and
//! `POST /v1/workspaces/purge`: the daemon owns the database, the memory and
//! the session writers, and it owns the refusals; this prints them. A relative
//! path is made absolute against this shell's directory before it is sent.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use colored::Colorize;
use openalpaca_storage::store::projects::overlaps_home_store;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;

#[derive(Args)]
pub struct StoreArgs {
    #[command(subcommand)]
    pub command: StoreCommands,
}

#[derive(Subcommand)]
pub enum StoreCommands {
    /// List the projects OpenAlpaca keeps a folder for, and mark a missing one
    List,
    /// Re-point a moved project onto its new path
    Rebase {
        /// The path the project had
        old: String,
        /// The path it has now
        new: String,
        /// Report what would move and what would be refused; change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete everything OpenAlpaca holds for a project (never its own folder)
    Purge {
        /// The project: its root as `store list` prints it, or its folder there
        /// (projects/<name>)
        project: String,
        /// Print the plan and change nothing — what happens anyway without -y
        #[arg(long)]
        dry_run: bool,
        /// Carry the plan out
        #[arg(short = 'y', long = "yes", conflicts_with = "dry_run")]
        yes: bool,
    },
}

/// One project, as the daemon answers it.
#[derive(Debug, Deserialize)]
struct ProjectView {
    id: String,
    root: String,
    name: String,
    #[serde(default)]
    missing: bool,
}

/// The rows the five path columns hold under one root.
#[derive(Debug, Default, Deserialize)]
struct RowCounts {
    #[serde(default)]
    artifacts: usize,
    #[serde(default)]
    sessions: usize,
    #[serde(default)]
    tasks: usize,
    #[serde(default)]
    messages: usize,
    #[serde(default)]
    followups: usize,
}

impl RowCounts {
    fn is_empty(&self) -> bool {
        self.artifacts == 0
            && self.sessions == 0
            && self.tasks == 0
            && self.messages == 0
            && self.followups == 0
    }

    /// Every member named, zeroes included: "nothing else moved" is the part a
    /// reader is checking for.
    fn describe(&self) -> String {
        format!(
            "{} artifacts, {} conversations, {} runs, {} messages, {} follow-ups",
            self.artifacts, self.sessions, self.tasks, self.messages, self.followups
        )
    }
}

/// One path, as `GET /v1/workspaces?path=` describes it.
#[derive(Debug, Deserialize)]
struct WorkspaceView {
    path: String,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    missing: bool,
    #[serde(default)]
    rows: RowCounts,
    #[serde(default)]
    active_tasks: usize,
    /// `queued` runs: they stop a re-point and a delete as running ones do —
    /// their dispatch has already resolved the project by this root (R10).
    #[serde(default)]
    queued_tasks: usize,
}

#[derive(Debug, Deserialize)]
struct RepointResult {
    old_path: String,
    new_path: String,
    moved: RowCounts,
    project: ProjectView,
}

#[derive(Debug, Serialize)]
struct RebaseRequest<'a> {
    old_path: &'a str,
    new_path: &'a str,
}

#[derive(Debug, Serialize)]
struct PurgeRequest<'a> {
    path: &'a str,
    dry_run: bool,
}

/// One line of the daemon's plan: an entry, what it holds for this project,
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
    #[serde(default)]
    artifacts: usize,
    #[serde(default)]
    memory_notes: usize,
}

impl PurgeCounts {
    /// Every member named, zeroes included.
    fn describe(&self) -> String {
        format!(
            "{} conversations, {} messages, {} tool calls, {} follow-ups, {} runs, {} spans, \
             {} run events, {} uploads, {} by-products, {} memory notes",
            self.sessions,
            self.messages,
            self.tool_calls,
            self.followups,
            self.tasks,
            self.spans,
            self.run_events,
            self.uploads,
            self.artifacts,
            self.memory_notes
        )
    }
}

#[derive(Debug, Default, Deserialize)]
struct Kept {
    #[serde(default)]
    uploads: usize,
    #[serde(default)]
    artifacts: usize,
}

#[derive(Debug, Default, Deserialize)]
struct Removed {
    #[serde(default)]
    session_dirs: usize,
    #[serde(default)]
    memory_partitions: usize,
    #[serde(default)]
    project_folder: bool,
}

#[derive(Debug, Deserialize)]
struct DeleteResult {
    #[serde(default)]
    applied: bool,
    project: ProjectView,
    #[serde(default)]
    entries: Vec<PlanEntry>,
    #[serde(default)]
    counts: PurgeCounts,
    #[serde(default)]
    kept: Kept,
    /// Present only on a real run: what the daemon actually removed.
    #[serde(default)]
    removed: Option<Removed>,
}

pub async fn run(args: StoreArgs) -> Result<()> {
    let client = DaemonClient::connect()?;
    let text = match args.command {
        StoreCommands::List => list(&client).await?,
        StoreCommands::Rebase { old, new, dry_run } => {
            rebase(&client, &absolute(&old)?, &absolute(&new)?, dry_run).await?
        }
        StoreCommands::Purge {
            project,
            dry_run,
            yes,
        } => {
            let root = purge_target(&client, &project).await?;
            purge(&client, &root, purge_is_dry(dry_run, yes)).await?
        }
    };
    print!("{text}");
    Ok(())
}

/// `--dry-run` is the default until `-y`: the two flags conflict in the
/// parser, and the absence of both means the safe one.
fn purge_is_dry(dry_run: bool, yes: bool) -> bool {
    dry_run || !yes
}

/// A path made absolute against this shell's directory: the daemon refuses a
/// relative one rather than read it against its own (R22).
fn absolute(path: &str) -> Result<String> {
    let trimmed = path.trim();
    let absolute = std::path::absolute(trimmed)
        .with_context(|| format!("cannot make {trimmed:?} an absolute path"))?;
    Ok(absolute.to_string_lossy().into_owned())
}

fn workspace_query(path: &str) -> String {
    format!("/v1/workspaces?path={}", urlencoding::encode(path))
}

// ── list ─────────────────────────────────────────────────────────

async fn list(client: &DaemonClient) -> Result<String> {
    let projects: Vec<ProjectView> = client.get("/v1/workspaces").await?;
    Ok(render_list(&projects))
}

fn render_list(projects: &[ProjectView]) -> String {
    let mut out = String::new();
    if projects.is_empty() {
        let _ = writeln!(
            out,
            "{}",
            "No projects yet. A chat turn in a git repository, or `openalpaca chat --project \
             <dir>`, makes one."
                .yellow()
        );
        return out;
    }
    for project in projects {
        let missing = match project.missing {
            true => format!(
                "  {}",
                "missing — its folder is gone or has moved: re-point it, or delete its data"
                    .yellow()
            ),
            false => String::new(),
        };
        let _ = writeln!(out, "{}{missing}", project.root.bold());
        let _ = writeln!(
            out,
            "  {}",
            format!("projects/{} · id {}", project.name, project.id).dimmed()
        );
    }
    out
}

// ── rebase ───────────────────────────────────────────────────────

async fn rebase(client: &DaemonClient, old: &str, new: &str, dry_run: bool) -> Result<String> {
    if dry_run {
        let from: WorkspaceView = client.get(&workspace_query(old)).await?;
        let to: WorkspaceView = client.get(&workspace_query(new)).await?;
        return Ok(render_plan(&from, &to));
    }
    let result: RepointResult = client
        .patch(
            "/v1/workspaces",
            &RebaseRequest {
                old_path: old,
                new_path: new,
            },
        )
        .await?;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} {} → {}",
        "Re-pointed".green().bold(),
        result.old_path.dimmed(),
        result.new_path
    );
    let _ = writeln!(out, "  {}", result.moved.describe());
    let _ = writeln!(out, "  its folder is now projects/{}", result.project.name);
    // R22: an app window keeps the root it was given. One that holds the
    // project's id follows on its own; one an earlier build set does not.
    let _ = writeln!(
        out,
        "  if an app window still names {} in Settings → Connection, choose {} there with Use",
        result.old_path, result.project.root
    );
    Ok(out)
}

fn flag(out: &mut String, text: String) {
    let _ = writeln!(out, "  {}", text.yellow());
}

/// The `--dry-run` body: what is recorded at the old path and what the
/// re-point would refuse. It reports rather than refuses. The home-store and
/// not-a-directory lines read this machine's filesystem — the daemon's, on the
/// machine they share; the daemon is what decides.
fn render_plan(from: &WorkspaceView, to: &WorkspaceView) -> String {
    let mut out = String::new();
    let source = from.root.as_deref().unwrap_or(&from.path);
    let _ = writeln!(
        out,
        "{} {} → {}",
        "Would re-point".bold(),
        source.dimmed(),
        to.path
    );
    let _ = writeln!(out, "  {}", from.rows.describe());
    if from.id.is_none() {
        flag(
            &mut out,
            format!("{source} is not a project OpenAlpaca knows; the re-point would be refused"),
        );
    }
    if overlaps_home_store(Path::new(&to.path)) {
        flag(
            &mut out,
            format!(
                "{} is, contains or lies inside the OpenAlpaca home store; the re-point would \
                 be refused",
                to.path
            ),
        );
    } else if !Path::new(&to.path).is_dir() {
        flag(
            &mut out,
            format!(
                "{} is not a directory; the re-point would be refused — create it first",
                to.path
            ),
        );
    }
    if to.root.as_deref() == Some(to.path.as_str()) && (to.id.is_some() || !to.rows.is_empty()) {
        flag(
            &mut out,
            format!(
                "{} is already a project of its own, or OpenAlpaca's records already name it; \
                 the re-point would be refused — delete that project's data first, or re-point \
                 onto another path",
                to.path
            ),
        );
    }
    if from.active_tasks > 0 {
        flag(
            &mut out,
            format!(
                "{} run(s) there are still in flight; the re-point would be refused until they \
                 finish",
                from.active_tasks
            ),
        );
    }
    if from.queued_tasks > 0 {
        flag(
            &mut out,
            format!(
                "{} run(s) there are queued; the re-point would be refused until they finish, \
                 as would deleting that project's data",
                from.queued_tasks
            ),
        );
    }
    if from.missing {
        let _ = writeln!(
            out,
            "  its folder is gone or has moved; re-pointing is what re-attaches its data"
        );
    }
    if let Some(name) = &from.name {
        let _ = writeln!(
            out,
            "  its folder projects/{name} would be renamed for the new path"
        );
    }
    out
}

// ── purge ────────────────────────────────────────────────────────

/// What `purge` sends (the lead's R-S8): a delete takes a project exactly.
/// `projects/<name>`, a folder as `store list` prints it, is looked up in
/// the daemon's list and sent as that project's root; anything else is a
/// path, made absolute against this shell's directory and sent as written —
/// the daemon refuses one inside a project, naming the project's root
/// (`422 WORKSPACE_NOT_A_ROOT`), and one in no project (`404`). A folder of
/// this shell's directory that is itself called `projects` is
/// `./projects/…`.
async fn purge_target(client: &DaemonClient, project: &str) -> Result<String> {
    let trimmed = project.trim();
    let Some(name) = trimmed.strip_prefix("projects/") else {
        return absolute(trimmed);
    };
    let name = name.trim_end_matches('/');
    let projects: Vec<ProjectView> = client.get("/v1/workspaces").await?;
    projects
        .into_iter()
        .find(|listed| listed.name == name)
        .map(|listed| listed.root)
        .with_context(|| {
            format!(
                "no project's folder is projects/{name}: `openalpaca store list` names each \
                 project's root and folder"
            )
        })
}

async fn purge(client: &DaemonClient, project: &str, dry_run: bool) -> Result<String> {
    let result: DeleteResult = client
        .post(
            "/v1/workspaces/purge",
            &PurgeRequest {
                path: project,
                dry_run,
            },
        )
        .await?;
    Ok(render_delete(&result))
}

fn render_delete(result: &DeleteResult) -> String {
    let mut out = String::new();
    let heading = match result.applied {
        true => format!(
            "{} {}",
            "Deleted the data of".green().bold(),
            result.project.root
        ),
        false => format!(
            "{} {}",
            "Would delete the data of".bold(),
            result.project.root
        ),
    };
    let _ = writeln!(out, "{heading}");
    let _ = writeln!(
        out,
        "  {}",
        format!(
            "projects/{} · id {}",
            result.project.name, result.project.id
        )
        .dimmed()
    );
    out.push_str(&render_entries(&result.entries));
    // Present only on a real run: filled from what the daemon actually removed.
    if let Some(removed) = &result.removed {
        let _ = writeln!(out, "  {} {}", "deleted:".bold(), result.counts.describe());
        let folder = match removed.project_folder {
            true => format!("the folder projects/{}", result.project.name),
            // R-F2: the delete did not finish (a partition that could not
            // be dropped, or a folder that could not be removed or was
            // already gone); a second purge finishes it.
            false => format!(
                "not the folder projects/{} — run `openalpaca store purge {} -y` again to finish \
                 (the daemon log says why)",
                result.project.name, result.project.root
            ),
        };
        let _ = writeln!(
            out,
            "  {} {} session log directories, {} memory partitions, {folder}",
            "removed:".bold(),
            removed.session_dirs,
            removed.memory_partitions
        );
    }
    if result.kept.uploads + result.kept.artifacts > 0 {
        let _ = writeln!(
            out,
            "  {} {} uploads and {} by-products another conversation still uses, moved to the \
             root store",
            "kept:".bold(),
            result.kept.uploads,
            result.kept.artifacts
        );
    }
    if !result.applied {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{}",
            "Nothing was deleted. Re-run with -y to carry this out.".dimmed()
        );
    }
    out
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

/// The plan's body, one entry per two lines: the verdict and what is there,
/// then the retention class that verdict comes from — printed for the keeps as
/// much as for the deletes, because "artifacts are never garbage-collected" is
/// the sentence that makes the delete lines trustworthy. A `String`, so the
/// column layout can be asserted on directly instead of only by eye.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{EnvSandbox, json_answer, serve};
    use clap::Parser;
    use serde_json::json;

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

    fn client(addr: std::net::SocketAddr) -> DaemonClient {
        DaemonClient::for_tests(&format!("http://{addr}"), "token")
    }

    fn ok(body: serde_json::Value) -> String {
        json_answer("200 OK", "e", &body.to_string())
    }

    const APP_ID: &str = "11111111-1111-4111-8111-111111111111";
    const OLD_ID: &str = "22222222-2222-4222-8222-222222222222";

    fn zeros() -> serde_json::Value {
        json!({"artifacts": 0, "sessions": 0, "tasks": 0, "messages": 0, "followups": 0})
    }

    #[test]
    fn list_takes_no_arguments() {
        assert!(matches!(parse(&["list"]), StoreCommands::List));
        assert!(try_parse(&["list", "/x"]).is_err());
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
    fn purge_takes_exactly_one_project() {
        assert!(matches!(
            parse(&["purge", "/some/proj"]),
            StoreCommands::Purge { project, .. } if project == "/some/proj"
        ));
        assert!(matches!(
            parse(&["purge", "projects/-some-proj"]),
            StoreCommands::Purge { project, .. } if project == "projects/-some-proj"
        ));
        assert!(
            try_parse(&["purge"]).is_err(),
            "a destructive verb never guesses the project"
        );
        assert!(
            try_parse(&["purge", "--all"]).is_err(),
            "--all is gone (ST-19)"
        );
        assert!(try_parse(&["purge", "/some/proj", "--dry-run", "-y"]).is_err());
    }

    #[test]
    fn a_purge_is_a_dry_run_until_minus_y() {
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
    }

    #[test]
    fn the_path_is_url_encoded_into_the_query() {
        assert_eq!(
            workspace_query("/Users/me/my project"),
            "/v1/workspaces?path=%2FUsers%2Fme%2Fmy%20project"
        );
    }

    #[test]
    fn a_relative_path_is_made_absolute_against_this_shells_directory() {
        assert_eq!(absolute("/already/absolute").unwrap(), "/already/absolute");
        let expected = std::env::current_dir()
            .unwrap()
            .join("here")
            .to_string_lossy()
            .into_owned();
        assert_eq!(absolute("here").unwrap(), expected);
        assert!(absolute("  ").is_err());
    }

    #[test]
    fn every_member_is_named_including_the_zeroes() {
        let counts = RowCounts {
            artifacts: 12,
            sessions: 2,
            tasks: 0,
            messages: 7,
            followups: 0,
        };
        assert_eq!(
            counts.describe(),
            "12 artifacts, 2 conversations, 0 runs, 7 messages, 0 follow-ups"
        );
        assert!(!counts.is_empty());
        assert!(RowCounts::default().is_empty());
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
            artifacts: 4,
            memory_notes: 9,
        };
        assert_eq!(
            counts.describe(),
            "3 conversations, 41 messages, 12 tool calls, 0 follow-ups, 2 runs, 5 spans, \
             18 run events, 0 uploads, 4 by-products, 9 memory notes"
        );
    }

    #[tokio::test]
    async fn list_prints_every_project_and_marks_a_missing_one() {
        let (addr, server) = serve(vec![ok(json!([
            {"id": APP_ID, "root": "/work/app", "name": "-work-app", "missing": false},
            {"id": OLD_ID, "root": "/work/old", "name": "-work-old", "missing": true},
        ]))]);

        let text = strip_ansi(&list(&client(addr)).await.unwrap());

        let seen = server.join().unwrap();
        assert!(
            seen[0].starts_with("GET /v1/workspaces HTTP/1.1"),
            "{}",
            seen[0]
        );
        assert_eq!(
            text,
            format!(
                "/work/app\n  projects/-work-app · id {APP_ID}\n\
                 /work/old  missing — its folder is gone or has moved: re-point it, or delete its \
                 data\n  projects/-work-old · id {OLD_ID}\n"
            )
        );
    }

    #[tokio::test]
    async fn list_with_no_projects_says_how_one_is_made() {
        let (addr, server) = serve(vec![ok(json!([]))]);
        let text = strip_ansi(&list(&client(addr)).await.unwrap());
        server.join().unwrap();
        assert!(text.contains("openalpaca chat --project <dir>"), "{text}");
    }

    fn delete_answer(applied: bool) -> serde_json::Value {
        json!({
            "dry_run": !applied,
            "applied": applied,
            "project": {"id": APP_ID, "root": "/work/app", "name": "-work-app", "missing": false},
            "entries": [{
                "entry": "sessions/",
                "holds": "2 conversations",
                "retention": "size-capped, optional age sweep",
                "action": "delete"
            }],
            "counts": {
                "sessions": 2, "messages": 5, "tool_calls": 0, "followups": 0, "tasks": 1,
                "spans": 0, "run_events": 0, "uploads": 1, "artifacts": 3, "memory_notes": 4
            },
            "kept": {"uploads": 1, "artifacts": 0},
            "removed": if applied {
                json!({"session_dirs": 2, "memory_partitions": 1, "project_folder": true})
            } else {
                serde_json::Value::Null
            },
        })
    }

    #[tokio::test]
    async fn purge_sends_one_project_and_prints_the_plan_by_default() {
        let (addr, server) = serve(vec![ok(delete_answer(false))]);

        let text = strip_ansi(
            &purge(&client(addr), "/work/app", purge_is_dry(false, false))
                .await
                .unwrap(),
        );

        let seen = server.join().unwrap();
        assert!(
            seen[0].starts_with("POST /v1/workspaces/purge HTTP/1.1"),
            "{}",
            seen[0]
        );
        assert!(
            seen[0].ends_with(r#"{"path":"/work/app","dry_run":true}"#),
            "{}",
            seen[0]
        );
        assert!(
            text.starts_with("Would delete the data of /work/app\n"),
            "{text}"
        );
        assert!(
            text.contains("delete  sessions/  2 conversations"),
            "{text}"
        );
        assert!(
            text.contains("kept: 1 uploads and 0 by-products another conversation still uses"),
            "{text}"
        );
        assert!(
            text.ends_with("Nothing was deleted. Re-run with -y to carry this out.\n"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_real_purge_prints_what_went_and_what_was_removed() {
        let (addr, server) = serve(vec![ok(delete_answer(true))]);

        let text = strip_ansi(&purge(&client(addr), "/work/app", false).await.unwrap());

        let seen = server.join().unwrap();
        assert!(
            seen[0].ends_with(r#"{"path":"/work/app","dry_run":false}"#),
            "{}",
            seen[0]
        );
        assert!(
            text.starts_with("Deleted the data of /work/app\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "deleted: 2 conversations, 5 messages, 0 tool calls, 0 follow-ups, 1 runs, 0 spans, \
                 0 run events, 1 uploads, 3 by-products, 4 memory notes"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "removed: 2 session log directories, 1 memory partitions, the folder projects/-work-app"
            ),
            "{text}"
        );
        assert!(!text.contains("Nothing was deleted"), "{text}");
    }

    /// R-F2 (S5b M1): `project_folder: false` is a delete that did not
    /// finish — a partition that could not be dropped, or a folder already
    /// gone — and a second purge is what finishes it.
    #[tokio::test]
    async fn a_purge_that_kept_the_folder_says_a_second_purge_finishes_it() {
        let mut answer = delete_answer(true);
        answer["removed"] =
            json!({"session_dirs": 2, "memory_partitions": 0, "project_folder": false});
        let (addr, server) = serve(vec![ok(answer)]);

        let text = strip_ansi(&purge(&client(addr), "/work/app", false).await.unwrap());

        server.join().unwrap();
        assert!(
            text.contains(
                "removed: 2 session log directories, 0 memory partitions, not the folder \
                 projects/-work-app — run `openalpaca store purge /work/app -y` again to finish \
                 (the daemon log says why)"
            ),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_rebase_dry_run_reads_both_paths_and_names_what_would_be_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let target = tmp.path().canonicalize().unwrap().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let target = target.to_string_lossy().into_owned();
        let from = json!({
            "path": "/work/app", "root": "/work/app", "id": APP_ID, "name": "-work-app",
            "missing": true,
            "rows": {"artifacts": 2, "sessions": 1, "tasks": 3, "messages": 4, "followups": 0},
            "active_tasks": 0, "queued_tasks": 2,
        });
        let to = json!({
            "path": target, "root": target, "id": OLD_ID, "name": "-target", "missing": false,
            "rows": zeros(), "active_tasks": 0, "queued_tasks": 0,
        });
        let (addr, server) = serve(vec![ok(from), ok(to)]);

        let text = strip_ansi(
            &rebase(&client(addr), "/work/app", &target, true)
                .await
                .unwrap(),
        );

        let seen = server.join().unwrap();
        assert!(seen[0].starts_with(&format!("GET {} HTTP/1.1", workspace_query("/work/app"))));
        assert!(seen[1].starts_with(&format!("GET {} HTTP/1.1", workspace_query(&target))));
        assert!(
            text.contains("2 artifacts, 1 conversations, 3 runs, 4 messages, 0 follow-ups"),
            "{text}"
        );
        assert!(text.contains("is already a project of its own"), "{text}");
        assert!(
            text.contains("delete that project's data first, or re-point onto another path"),
            "{text}"
        );
        // R10: a queued run refuses the re-point as a running one does.
        assert!(
            text.contains(
                "2 run(s) there are queued; the re-point would be refused until they finish"
            ),
            "{text}"
        );
        assert!(!text.contains("takes their rows with it"), "{text}");
        assert!(
            text.contains(
                "its folder is gone or has moved; re-pointing is what re-attaches its data"
            ),
            "{text}"
        );
        assert!(
            text.contains("its folder projects/-work-app would be renamed"),
            "{text}"
        );
        assert!(!text.contains("not a directory"), "{text}");
        assert!(!text.contains("not a project OpenAlpaca knows"), "{text}");
    }

    #[tokio::test]
    async fn a_rebase_dry_run_from_an_unknown_path_onto_a_missing_one_says_both() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let nowhere = tmp
            .path()
            .join("nowhere")
            .join("new")
            .to_string_lossy()
            .into_owned();
        let from = json!({
            "path": "/work/loose", "root": null, "id": null, "name": null, "missing": false,
            "rows": zeros(), "active_tasks": 0, "queued_tasks": 0,
        });
        let to = json!({
            "path": nowhere, "root": null, "id": null, "name": null, "missing": false,
            "rows": zeros(), "active_tasks": 0, "queued_tasks": 0,
        });
        let (addr, server) = serve(vec![ok(from), ok(to)]);

        let text = strip_ansi(
            &rebase(&client(addr), "/work/loose", &nowhere, true)
                .await
                .unwrap(),
        );

        server.join().unwrap();
        assert!(
            text.contains("/work/loose is not a project OpenAlpaca knows"),
            "{text}"
        );
        assert!(
            text.contains(&format!("{nowhere} is not a directory")),
            "{text}"
        );
        assert!(!text.contains("would be renamed"), "{text}");
    }

    #[tokio::test]
    async fn a_rebase_prints_the_project_under_its_new_folder() {
        let (addr, server) = serve(vec![ok(json!({
            "old_path": "/work/app",
            "new_path": "/work/new",
            "moved": {"artifacts": 2, "sessions": 1, "tasks": 0, "messages": 3, "followups": 0},
            "project": {"id": APP_ID, "root": "/work/new", "name": "-work-new", "missing": false},
        }))]);

        let text = strip_ansi(
            &rebase(&client(addr), "/work/app", "/work/new", false)
                .await
                .unwrap(),
        );

        let seen = server.join().unwrap();
        assert!(
            seen[0].starts_with("PATCH /v1/workspaces HTTP/1.1"),
            "{}",
            seen[0]
        );
        assert!(
            seen[0].ends_with(r#"{"old_path":"/work/app","new_path":"/work/new"}"#),
            "{}",
            seen[0]
        );
        assert_eq!(
            text,
            "Re-pointed /work/app → /work/new\n  \
             2 artifacts, 1 conversations, 0 runs, 3 messages, 0 follow-ups\n  \
             its folder is now projects/-work-new\n  \
             if an app window still names /work/app in Settings → Connection, choose \
             /work/new there with Use\n"
        );
    }

    /// The lead's R-S8: a delete takes a project exactly — its root as
    /// written, or its folder as `store list` prints it, `projects/<name>`,
    /// which is looked up in the daemon's list and sent as that root.
    #[tokio::test]
    async fn a_project_is_named_by_its_root_or_by_its_folder_as_store_list_prints_it() {
        let (addr, server) = serve(vec![ok(json!([
            {"id": APP_ID, "root": "/work/app", "name": "-work-app", "missing": false},
            {"id": OLD_ID, "root": "/work/old", "name": "-work-old", "missing": true},
        ]))]);

        let root = purge_target(&client(addr), "projects/-work-old")
            .await
            .unwrap();

        let seen = server.join().unwrap();
        assert!(
            seen[0].starts_with("GET /v1/workspaces HTTP/1.1"),
            "{}",
            seen[0]
        );
        assert_eq!(root, "/work/old");
        // A root is sent as written; nothing is listed for it.
        assert_eq!(
            purge_target(&client(addr), "/work/app").await.unwrap(),
            "/work/app"
        );
    }

    #[tokio::test]
    async fn a_folder_no_project_has_is_an_error_that_names_store_list() {
        let (addr, server) = serve(vec![ok(json!([
            {"id": APP_ID, "root": "/work/app", "name": "-work-app", "missing": false},
        ]))]);

        let error = purge_target(&client(addr), "projects/-work-gone")
            .await
            .expect_err("no listed project has that folder");

        server.join().unwrap();
        let text = format!("{error:#}");
        assert!(text.contains("projects/-work-gone"), "{text}");
        assert!(text.contains("openalpaca store list"), "{text}");
    }

    /// A path inside a project is sent as written, and the daemon's refusal
    /// — naming the root — is what the command answers.
    #[tokio::test]
    async fn a_path_inside_a_project_prints_the_daemons_refusal_naming_the_root() {
        let refusal = json!({"error": {
            "code": "WORKSPACE_NOT_A_ROOT",
            "message": "/work/app/src is inside the project rooted at /work/app; deleting a \
                        project's data takes its root itself. Delete the data of /work/app if \
                        that is what you meant",
        }});
        let (addr, server) = serve(vec![json_answer(
            "422 Unprocessable Entity",
            "e",
            &refusal.to_string(),
        )]);

        let error = purge(&client(addr), "/work/app/src", true)
            .await
            .expect_err("the daemon refuses a path inside a project");

        let seen = server.join().unwrap();
        assert!(
            seen[0].ends_with(r#"{"path":"/work/app/src","dry_run":true}"#),
            "{}",
            seen[0]
        );
        let text = format!("{error:#}");
        assert!(text.starts_with("WORKSPACE_NOT_A_ROOT: "), "{text}");
        assert!(text.contains("Delete the data of /work/app"), "{text}");
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
