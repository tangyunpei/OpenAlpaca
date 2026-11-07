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
struct Row {
    #[serde(default)]
    not_used: Vec<NotUsed>,
}

#[derive(Debug, Deserialize)]
struct NotUsed {
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

/// What a tidy of `config_dir`'s `agents/` or `skills/` prints, having
/// moved what it moved: a listing, without `apply`; with it, a line per
/// copy moved or left and, when one moved, the warning last. `daemon` is
/// the running daemon, if one runs; `now` names the archive.
pub(crate) async fn tidy_lines(
    kind: Kind,
    config_dir: &Path,
    daemon: Option<&DaemonClient>,
    apply: bool,
    now: DateTime<Utc>,
) -> Result<Vec<String>> {
    let dir = config_dir.join(kind.dir());
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
    for copy in copies {
        let target = archive.join(&copy.archived_as);
        let left = daemon_lists
            .as_ref()
            .is_some_and(|listed| !listed.contains(&canonical(&copy.path)));
        const LEFT: &str = "the running daemon does not list it as an unused copy, so it may \
                            read it as yours";
        if !apply {
            lines.push(format!("  {} ({})", shown(&copy.path), copy.describe()));
            lines.push(if left {
                format!("    would be left: {LEFT}")
            } else {
                format!("    would move to {}", shown(&target))
            });
            continue;
        }
        if left {
            lines.push(format!("Left {}: {LEFT}", shown(&copy.path)));
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
        lines.push(format!(
            "Nothing was moved; `openalpaca {} tidy --apply` moves them.",
            kind.dir()
        ));
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
/// all unedited copies of it.
fn config_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("OPENALPACA_CONFIG_DIR").map(PathBuf::from)
        && dir.is_dir()
    {
        return Ok(dir);
    }
    openalpaca_storage::store::runtime_config_dir()
}

/// The running daemon to ask, or None when none runs (a `discovery.json`
/// naming no live daemon is not one). A daemon that runs but cannot be
/// reached is an error: what it reads is then unknown.
fn running_daemon() -> Result<Option<DaemonClient>> {
    if openalpaca_storage::daemon_lifecycle::running_daemon_pid().is_none() {
        return Ok(None);
    }
    DaemonClient::connect()
        .map(Some)
        .context("A daemon is running but cannot be asked which copies it reads; nothing was moved")
}

/// What a tidy acts on: the config directory the daemon reads and the
/// running daemon to ask, if one runs.
pub(crate) fn target() -> Result<(PathBuf, Option<DaemonClient>)> {
    Ok((config_dir()?, running_daemon()?))
}

#[cfg(test)]
mod tests;
