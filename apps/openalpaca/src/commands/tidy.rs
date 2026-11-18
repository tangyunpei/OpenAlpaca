//! `openalpaca agents tidy` and `openalpaca skills tidy` — the set-aside
//! copies, moved only when the person runs it (subagent design §2.12, §7
//! O-2 (b), `tasks/subagent-brief-design.md`).
//!
//! An earlier build wrote its agent templates and skills into
//! `config/agents/` and `config/skills/`. This build carries them in the
//! binary, and a copy there byte-identical to a version OpenAlpaca shipped
//! is set aside: not read, not touched, listed as not used, and the built-in
//! one runs — for a skill, a plugin's of the same name, where one is
//! installed, until B-05 (§2.3 P-3). No start, hot reload or route ever
//! moves it (U1). These verbs
//! do, when asked: without `--apply` they list each copy, its version
//! (`current` or `earlier`), the Installation Manual's edit it carries, if
//! any, and where it would go, and move nothing; with `--apply` they move
//! each to `.archived/` beside it — a template to
//! `.archived/<file stem>.<UTC timestamp>.md`, a skill's folder to
//! `.archived/<key>.<UTC timestamp>/`, the precedent of the template
//! routes' archive — and end with what that means for an earlier build.
//! Nothing the rule does not set aside is moved: an override, a file of the
//! person's own, a file that switches a template off, a copy of something
//! no longer built in, a broken file.
//!
//! The CLI classifies by itself, with core's rule against this build's
//! list (D-2), so no daemon need run. When one runs, it is asked first:
//! a daemon of an older build may read as the person's a copy of a version
//! its list lacks, so only the copies its own listing names as unused
//! copies are moved, and the others are named as left.
//!
//! With the config directory inside a git work tree `--apply` moves nothing
//! (the owner's item 22 of 2026-11-12, C5a's development checkout as (b)):
//! where there is a copy to move it is refused, naming the work tree,
//! before anything moves or the daemon is asked, and the listing says so in
//! its last line; where there is none, the "nothing to tidy" line comes
//! first, with `--apply` as without (TIDY-1). A development checkout's
//! `config/`, named by `OPENALPACA_CONFIG_DIR`, is the source of the
//! built-in files, so every file there is an unedited copy and a move would
//! take the repository's own files out of place. An `agents/` or `skills/`
//! a link places inside a work tree, the config directory outside one, is
//! refused the same way, naming that folder as a link into it (the owner's
//! item 6 of 2026-11-14), and so is one that is a work tree itself, a
//! folder of the config directory's own holding a `.git` entry (TIDY-LINKED,
//! "check it too"): a move there is a move inside the work tree.
//!
//! One copy's own folder is checked too, and only for a `.git` entry
//! (TIDY-COPY-GIT, the advisor's "leave that one copy with a line"): a
//! skill's folder holding one is left where it is, the listing saying it
//! would be left and `--apply` saying it was, each naming it and why, while
//! every other copy is tidied as before and the run is not refused. Few
//! folders reach it: core's rule counts every file under a folder, so a
//! `.git` holding any file (a work tree's `HEAD`, a linked worktree's
//! `.git` file) makes the folder the person's, never listed; only a `.git`
//! holding no file but a `.DS_Store` leaves it a copy. A template is a
//! file, with no folder of its own. A copy that is itself a link (a move
//! renames the link, not what it names) is tidied as before.
//! [`git_work_tree`] says how a work tree is found.

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use openalpaca_core::bundled::ShippedVersion;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::client::DaemonClient;

/// Which of the two directories a tidy reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Agents,
    Skills,
}

impl Kind {
    /// The directory under the config directory.
    fn dir(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Skills => "skills",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Agents => "templates",
            Self::Skills => "skills",
        }
    }

    /// The route whose rows carry the daemon's `not_used` (R-1, R-2).
    fn route(self) -> &'static str {
        match self {
            Self::Agents => "/v1/agent-templates",
            Self::Skills => "/v1/skills",
        }
    }

    /// What runs instead of a set-aside copy: the built-in one; for a skill
    /// a plugin's of the same name goes over both until B-05 (§2.3 P-3),
    /// while a template's key is the built-in one's (§7 O-4 (a), C3).
    fn in_their_place(self) -> &'static str {
        match self {
            Self::Agents => "the built-in ones run in their place",
            Self::Skills => {
                "the built-in ones, or a plugin's skill of the same name, run in their place"
            }
        }
    }

    /// §1.4's warning, said once anything has moved.
    fn warning(self) -> &'static str {
        match self {
            Self::Agents => {
                "An earlier build started after this finds agents/ without these files and \
                 does not seed them again; to go back, move agents/ aside and let that build \
                 seed it (Installation Manual, Keep the builds apart)."
            }
            Self::Skills => {
                "An earlier build started after this finds skills/ without these folders and \
                 does not seed them again; to go back, move skills/ aside and let that build \
                 seed it (Installation Manual, Keep the builds apart)."
            }
        }
    }

    /// Every copy the daemon's rule sets aside in `dir`, in file-name
    /// order, with what `archive` would call it.
    fn copies(self, dir: &Path, stamp: &str) -> std::io::Result<Vec<Copy>> {
        Ok(match self {
            Self::Agents => openalpaca_core::agent::file_layer::set_aside_copies(dir)?
                .into_iter()
                .map(|(path, copy)| {
                    let stem = path
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    Copy {
                        archived_as: format!("{stem}.{stamp}.md"),
                        path,
                        version: copy.version,
                        edit: copy.edit.map(|edit| edit.as_str()),
                    }
                })
                .collect(),
            Self::Skills => openalpaca_core::bundled::set_aside_skill_folders(dir)?
                .into_iter()
                .map(|(path, version)| {
                    let key = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    Copy {
                        archived_as: format!("{key}.{stamp}"),
                        path,
                        version,
                        edit: None,
                    }
                })
                .collect(),
        })
    }
}

/// One set-aside copy: a template file, or a skill's folder.
struct Copy {
    path: PathBuf,
    version: ShippedVersion,
    /// The Installation Manual's step whose result it is (V-2a).
    edit: Option<&'static str>,
    /// Its name under `.archived/`.
    archived_as: String,
}

impl Copy {
    fn describe(&self) -> String {
        let version = match self.version {
            ShippedVersion::Current => "current",
            ShippedVersion::Earlier => "earlier",
        };
        match self.edit {
            Some(edit) => format!("version: {version}, edit: {edit}"),
            None => format!("version: {version}"),
        }
    }
}

/// A row of either listing route, as far as a tidy reads it.
#[derive(Debug, Deserialize)]
pub(crate) struct Row {
    #[serde(default)]
    not_used: Vec<NotUsed>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct NotUsed {
    reason: String,
    #[serde(default)]
    path: Option<String>,
}

/// A path as printed: on one line, every hidden character as `<U+XXXX>`.
fn shown(path: &Path) -> String {
    openalpaca_core::utils::text::escape_hidden_line(&path.display().to_string()).text
}

/// `path` as the file system resolves it, so `/var/…` and `/private/var/…`
/// compare equal; as given when it cannot be resolved.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The copies the running daemon lists as unedited copies it does not use.
async fn daemon_set_aside(kind: Kind, client: &DaemonClient) -> Result<Vec<PathBuf>> {
    let rows: Vec<Row> = client.get(kind.route()).await.with_context(|| {
        format!(
            "The running daemon did not answer GET {}; nothing was moved",
            kind.route()
        )
    })?;
    Ok(rows
        .into_iter()
        .flat_map(|row| row.not_used)
        .filter(|entry| entry.reason == "unedited_copy")
        .filter_map(|entry| entry.path)
        .map(|path| canonical(Path::new(&path)))
        .collect())
}

/// The git work tree `path` is in, if any: the nearest of `path` and its
/// ancestors, as the file system resolves them, that holds an entry named
/// `.git` — a directory, or the file a linked worktree or a submodule has.
///
/// No git is run, since git may not be installed; the test is a few
/// `lstat` calls. It reads no `.git` entry's contents and none of git's
/// environment or configuration, so it can be wrong both ways: it refuses
/// where a `.git` entry is not a repository git would use (or a ceiling
/// would stop git short of it), and it misses a work tree with no `.git`
/// in it or above it — one git reaches only through `GIT_DIR` or
/// `--git-dir`, as a bare-repository dotfiles setup does — where `--apply`
/// still moves.
fn git_work_tree(path: &Path) -> Option<PathBuf> {
    // The nearest part of `path` that exists, resolved; a part that does
    // not exist holds no `.git`.
    let resolved = path.ancestors().find_map(|part| part.canonicalize().ok())?;
    resolved
        .ancestors()
        .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
        .map(Path::to_path_buf)
}

/// The git work tree a tidy would move inside ([`work_tree_of`]), with the
/// path its refusal names.
#[derive(Debug, PartialEq, Eq)]
struct WorkTree {
    /// The path named, as given: the config directory, or the folder.
    named: PathBuf,
    /// The work tree, as resolved.
    root: PathBuf,
    /// How `named` stands to `root`.
    place: Place,
}

/// How the path a refusal names stands to its work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// A path inside it: the config directory.
    Inside,
    /// A link resolving into it: the `agents/` or `skills/` folder.
    LinkInto,
    /// The work tree itself: the folder, holding its own `.git` entry.
    Itself,
}

impl WorkTree {
    /// How `named` stands to `root`, in the refusal.
    fn relation(&self) -> String {
        match self.place {
            Place::Inside => format!("inside {}", shown(&self.root)),
            Place::LinkInto => format!("a link into {}", shown(&self.root)),
            Place::Itself => "a git work tree itself".to_string(),
        }
    }

    /// How `named` stands to `root`, in the listing's last line.
    fn listed_relation(&self) -> String {
        match self.place {
            Place::Inside => format!("inside the git work tree {}", shown(&self.root)),
            Place::LinkInto => format!("a link into the git work tree {}", shown(&self.root)),
            Place::Itself => "a git work tree itself".to_string(),
        }
    }
}

/// The git work tree a tidy of `dir`, the `agents/` or `skills/` of
/// `config_dir`, would move inside, with the path to name for it: the
/// config directory, as given, when it is inside one; else `dir`, as given,
/// when it is a link resolving into one or a work tree itself, a folder of
/// the config directory's own holding a `.git` entry (TIDY-LINKED). `None`
/// otherwise, and for a missing `dir`. The config directory is in no work
/// tree by then, so `dir`, its child, is in one only by being a link into
/// one or by holding its own `.git`; a `.git` deeper down, in a skill's own
/// folder, is not looked for here: [`tidy_lines`] leaves that one copy.
fn work_tree_of(config_dir: &Path, dir: &Path) -> Option<WorkTree> {
    if let Some(root) = git_work_tree(config_dir) {
        return Some(WorkTree {
            named: config_dir.to_path_buf(),
            root,
            place: Place::Inside,
        });
    }
    let meta = std::fs::symlink_metadata(dir).ok()?;
    let place = if meta.file_type().is_symlink() {
        Place::LinkInto
    } else {
        Place::Itself
    };
    git_work_tree(dir).map(|root| WorkTree {
        named: dir.to_path_buf(),
        root,
        place,
    })
}

/// What a tidy of `config_dir`'s `agents/` or `skills/` prints, having
/// moved what it moved: a listing, without `apply`; with it, a line per
/// copy moved or left and, when one moved, the warning last. `daemon` is
/// the running daemon, if one runs; `now` names the archive. With nothing
/// to tidy, it says so first, `apply` or not (TIDY-1). With a copy to move
/// and `config_dir` inside a git work tree, or the folder a link into one or
/// one itself ([`work_tree_of`]), `apply` is refused before anything moves
/// or the daemon is asked. A copy whose own folder holds a `.git` entry is
/// left, with a line saying why, and the others tidied (TIDY-COPY-GIT);
/// when the listing would leave every copy, its last line says `apply`
/// would move none.
pub(crate) async fn tidy_lines(
    kind: Kind,
    config_dir: &Path,
    daemon: Option<&DaemonClient>,
    apply: bool,
    now: DateTime<Utc>,
) -> Result<Vec<String>> {
    let dir = config_dir.join(kind.dir());
    let work_tree = work_tree_of(config_dir, &dir);
    let stamp = now.format("%Y%m%d%H%M%S").to_string();
    let copies = kind
        .copies(&dir, &stamp)
        .with_context(|| format!("Cannot read {}", dir.display()))?;
    if copies.is_empty() {
        return Ok(vec![format!(
            "No unedited copies of shipped {} in {}: nothing to tidy.",
            kind.noun(),
            shown(&dir)
        )]);
    }
    if apply && let Some(tree) = &work_tree {
        return Err(anyhow!(
            "`openalpaca {kind} tidy --apply` moves nothing inside a git work tree, and {path} \
             is {relation}: a copy there may be a file the repository holds, as in a \
             development checkout, whose config/ is the source of the built-in ones. Nothing \
             was moved; `openalpaca {kind} tidy` lists the copies, for you to move yourself.",
            kind = kind.dir(),
            path = shown(&tree.named),
            relation = tree.relation(),
        ));
    }
    let daemon_lists = match daemon {
        Some(client) => Some(daemon_set_aside(kind, client).await?),
        None => None,
    };
    let archive = dir.join(".archived");
    let mut lines = Vec::new();
    if !apply {
        lines.push(format!(
            "Unedited copies of shipped {} in {}, not read — {}:",
            kind.noun(),
            shown(&dir),
            kind.in_their_place()
        ));
    }
    let mut moved = 0;
    let mut would_move = 0;
    for copy in copies {
        let target = archive.join(&copy.archived_as);
        let left = daemon_lists
            .as_ref()
            .is_some_and(|listed| !listed.contains(&canonical(&copy.path)));
        const LEFT: &str = "the running daemon does not list it as an unused copy, so it may \
                            read it as yours";
        // A copy whose own folder holds a `.git` entry is left, that copy
        // alone (TIDY-COPY-GIT). Only a skill's folder can: a template is a
        // file, so the test finds nothing there.
        let own_git = std::fs::symlink_metadata(copy.path.join(".git")).is_ok();
        const OWN_GIT: &str = "its folder holds a .git entry; move it yourself if you mean to";
        let left_because = if own_git {
            Some(OWN_GIT)
        } else if left {
            Some(LEFT)
        } else {
            None
        };
        if !apply {
            lines.push(format!("  {} ({})", shown(&copy.path), copy.describe()));
            lines.push(match left_because {
                Some(why) => format!("    would be left: {why}"),
                None => {
                    would_move += 1;
                    format!("    would move to {}", shown(&target))
                }
            });
            continue;
        }
        if let Some(why) = left_because {
            lines.push(format!("Left {}: {why}", shown(&copy.path)));
            continue;
        }
        if std::fs::symlink_metadata(&target).is_ok() {
            lines.push(format!(
                "Left {}: {} already exists",
                shown(&copy.path),
                shown(&target)
            ));
            continue;
        }
        let result =
            std::fs::create_dir_all(&archive).and_then(|()| std::fs::rename(&copy.path, &target));
        if let Err(e) = result {
            lines.push(format!(
                "Could not move {} to {}: {e}",
                shown(&copy.path),
                shown(&target)
            ));
            if moved > 0 {
                lines.push(kind.warning().to_string());
            }
            return Err(anyhow!("{}", lines.join("\n")));
        }
        lines.push(format!("Moved {} to {}", shown(&copy.path), shown(&target)));
        moved += 1;
    }
    if !apply {
        lines.push(match &work_tree {
            Some(tree) => format!(
                "Nothing was moved; `openalpaca {} tidy --apply` moves nothing here: {} is {}.",
                kind.dir(),
                shown(&tree.named),
                tree.listed_relation()
            ),
            // Every listed copy left: `--apply` would move none, so the
            // line promises no move (TIDY-COPY-GIT).
            None if would_move == 0 => format!(
                "Nothing was moved, and `openalpaca {} tidy --apply` would move none of them.",
                kind.dir()
            ),
            None => format!(
                "Nothing was moved; `openalpaca {} tidy --apply` moves them.",
                kind.dir()
            ),
        });
    } else if moved > 0 {
        lines.push(kind.warning().to_string());
    }
    Ok(lines)
}

/// The config directory the daemon reads, as `openalpaca daemon start` and
/// the app hand it to the daemon (`OPENALPACA_CONFIG_DIR`): that variable
/// when it names a directory, else the store's `config/`. Never a
/// directory found by walking up from here: in a development checkout that
/// is the repository's `config/`, whose files are the bundle's source and
/// all unedited copies of it. Named by the variable, it is listed, and
/// `--apply` refused, as inside a git work tree ([`tidy_lines`]).
fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("OPENALPACA_CONFIG_DIR").map(PathBuf::from)
        && dir.is_dir()
    {
        return Ok(dir);
    }
    openalpaca_app_support::paths::runtime_config_dir()
}

/// The running daemon to ask, or None when none runs (a `discovery.json`
/// naming no live daemon is not one). A daemon that runs but cannot be
/// reached is an error: what it reads is then unknown.
async fn running_daemon() -> Result<Option<DaemonClient>> {
    if openalpaca_app_support::daemon_lifecycle::running_daemon_pid().is_none() {
        return Ok(None);
    }
    DaemonClient::connect()
        .await
        .map(Some)
        .context("A daemon is running but cannot be asked which copies it reads; nothing was moved")
}

/// What a tidy acts on: the config directory the daemon reads and the
/// running daemon to ask, if one runs.
pub(crate) async fn target() -> Result<(PathBuf, Option<DaemonClient>)> {
    Ok((config_dir()?, running_daemon().await?))
}

#[cfg(test)]
mod tests;
