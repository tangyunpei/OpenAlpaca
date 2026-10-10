//! The working folder of a tool call (storage design §10): the project's own
//! folder when the turn carries a registered project, else the conversation's
//! scratch folder under the home store. One function decides it for every
//! tool that acts on files or runs a command, and for the approval card that
//! names it (R34).
//!
//! [`working_folder`] is pure. It reads at most four small files (a project's
//! `project.json`, through `projects::lookup`: as written, then by its
//! canonical form when that differs) and creates nothing, so asking where a
//! call would act, as the card does, never makes a folder.

use std::fmt;
use std::path::{Path, PathBuf};

use openalpaca_storage::store::{self, ContentKind, projects};

use crate::session_log::session_dir_name;
use crate::tools::registry::ToolContext;

/// Where a tool call's files and commands live (design §10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingFolder {
    /// The turn's registered project: its canonical root (`Project::root`).
    Project { root: PathBuf },
    /// No project: `<home>/scratch/<session_dir_name(conversation_id)>`.
    Scratch {
        dir: PathBuf,
        conversation_id: String,
    },
}

impl WorkingFolder {
    /// The folder itself.
    pub fn path(&self) -> &Path {
        match self {
            Self::Project { root } => root,
            Self::Scratch { dir, .. } => dir,
        }
    }

    /// Whether the folder is a project's — where R32's rules bind.
    pub fn is_project(&self) -> bool {
        matches!(self, Self::Project { .. })
    }

    /// The folder, ready to act in (storage design §10, R26). A scratch
    /// folder is made on first use — `<home>/scratch` and the conversation's
    /// folder, one level at a time, each private (0700 on unix) and each
    /// checked to be a folder of its own before the next is made, so a link
    /// planted at either name is refused and nothing is made through it. A
    /// project's root must still lead to itself — a directory that is its own
    /// canonical form (`projects::root_is_missing`, ST-23), so a folder moved
    /// away since the lookup is never acted in through a link left at its
    /// root. Every tool that acts in the folder calls this first; the approval
    /// previews and the R28 note never do, so a conversation that never acts
    /// gets no folder.
    pub fn ensure(&self) -> Result<&Path, WorkingFolderRefusal> {
        match self {
            WorkingFolder::Project { root } => {
                // S0.1's one test of `missing` (ST-23): not a directory, or
                // no longer its own canonical form.
                if projects::root_is_missing(root) {
                    Err(WorkingFolderRefusal::Unavailable {
                        reason: format!(
                            "the project folder {} is missing: it is no longer a directory, or it \
                             no longer leads to itself",
                            root.display()
                        ),
                    })
                } else {
                    Ok(root.as_path())
                }
            }
            WorkingFolder::Scratch { dir, .. } => {
                let unavailable = |reason: String| WorkingFolderRefusal::Unavailable { reason };
                let scratch = dir.parent().ok_or_else(|| {
                    unavailable(format!("{} has no parent folder", dir.display()))
                })?;
                // The home store itself is made the way every store writer
                // makes it; only the two levels below it are private.
                if let Some(home) = scratch.parent() {
                    std::fs::create_dir_all(home)
                        .map_err(|e| unavailable(format!("{}: {e}", home.display())))?;
                }
                make_private_dir(scratch).map_err(unavailable)?;
                make_private_dir(dir).map_err(unavailable)?;
                Ok(dir.as_path())
            }
        }
    }
}

/// Why a call has no working folder. `Display` is the one sentence the model,
/// the card and the report read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingFolderRefusal {
    /// A tool reached through `execute` (no context) — R26.
    NoContext,
    /// R27: the project is registered and missing — its root is not a
    /// directory, or no longer leads to itself (ST-7, ST-23; `Project::missing`).
    ProjectMissing { root: String },
    /// The turn's root names no folder under `projects/` any more — re-pointed
    /// or deleted while the conversation ran.
    NotAProject { root: String },
    /// R44: neither a project nor a conversation id.
    NoConversation,
    /// `projects/` or the home root could not be read, or the scratch folder
    /// could not be made.
    Unavailable { reason: String },
}

impl fmt::Display for WorkingFolderRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoContext => f.write_str(
                "This tool acts in the conversation's working folder, and this call carries no \
                 conversation to name one.",
            ),
            Self::ProjectMissing { root } => write!(
                f,
                "The project folder {root} is missing — its folder is gone or has moved: re-point \
                 the project to where its folder is now (Settings → Connection → Projects → \
                 Re-point…, or openalpaca store rebase {root} <new path>), or delete its data \
                 (Delete data…, or openalpaca store purge {root}). No file tool or command runs \
                 until then."
            ),
            Self::NotAProject { root } => write!(
                f,
                "{}",
                projects::ProjectError::NotRegistered { root: root.clone() }
            ),
            Self::NoConversation => f.write_str(
                "This call has neither a project nor a conversation, so it has no working \
                 folder: file tools and commands run only inside a conversation or a project.",
            ),
            Self::Unavailable { reason } => {
                write!(f, "The working folder could not be read: {reason}")
            }
        }
    }
}

impl std::error::Error for WorkingFolderRefusal {}

/// The working folder of the call `ctx` describes (design §10). Pure: reads
/// four small files at most (`projects::lookup`, S0.2) and creates nothing.
///
/// 1. A non-blank `request_workspace_root`: an absolute one is looked up
///    under `projects/` — a present project is its root, a missing one
///    [`WorkingFolderRefusal::ProjectMissing`] (read from
///    `Project::missing`, the flag `projects::root_is_missing` decides —
///    never re-derived here), an unrecorded one
///    [`WorkingFolderRefusal::NotAProject`]; a relative one is not a
///    project either.
/// 2. Else a non-blank `conversation_id`: that conversation's scratch folder.
/// 3. Else [`WorkingFolderRefusal::NoConversation`].
pub fn working_folder(ctx: &ToolContext) -> Result<WorkingFolder, WorkingFolderRefusal> {
    if let Some(root) = ctx
        .request_workspace_root
        .as_deref()
        .filter(|root| !root.trim().is_empty())
    {
        return project_folder(root);
    }
    let Some(id) = ctx
        .conversation_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    else {
        return Err(WorkingFolderRefusal::NoConversation);
    };
    let dir = scratch_dir(id).map_err(|e| WorkingFolderRefusal::Unavailable {
        reason: format!("{e:#}"),
    })?;
    Ok(WorkingFolder::Scratch {
        dir,
        conversation_id: id.to_string(),
    })
}

/// `<home>/scratch/<session_dir_name(conversation_id)>` — the working folder
/// of a conversation with no project (storage design §10), as a path:
/// nothing is created. `session_dir_name` makes the id one safe path segment,
/// the mapping `sessions/<id>/` uses.
pub fn scratch_dir(conversation_id: &str) -> anyhow::Result<PathBuf> {
    Ok(store::home_root()?
        .join(ContentKind::Scratch.dir_name())
        .join(session_dir_name(conversation_id)))
}

/// Removes a conversation's scratch folder: it goes with the conversation
/// (storage design §10). `Ok(false)` when nothing stood at its name. Never
/// follows a link, at either of the two levels [`WorkingFolder::ensure`]
/// makes: `<home>/scratch` itself must be a folder of its own — a link or a
/// file standing there is refused, and nothing behind it is touched, since
/// the system would otherwise resolve it on the way to the conversation's
/// folder — and at the folder's own name a link (or a file) is removed
/// itself, a directory with everything in it.
pub fn remove_scratch(conversation_id: &str) -> std::io::Result<bool> {
    let dir = scratch_dir(conversation_id).map_err(|e| std::io::Error::other(format!("{e:#}")))?;
    let scratch = dir
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("{} has no parent folder", dir.display())))?;
    match std::fs::symlink_metadata(scratch) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::other(format!(
                "{} is not a folder of its own: a link or a file stands at its name",
                scratch.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        std::fs::remove_dir_all(&dir)?;
    } else if meta.file_type().is_symlink() {
        // A link to a directory is unlinked as a file on unix and taken by
        // `remove_dir` on Windows; neither follows it.
        std::fs::remove_file(&dir).or_else(|_| std::fs::remove_dir(&dir))?;
    } else {
        std::fs::remove_file(&dir)?;
    }
    Ok(true)
}

/// Who reads R28's line (the lead's ruling R-T22): the lead holds no tool
/// that writes a file and hands a result file for the project to a
/// subagent, so its line says where such a file comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteReader {
    /// A workflow's lead agent.
    Lead,
    /// Any other loop: the main loop, a skill, a nested skill, a subagent.
    Loop,
}

/// R28 (storage design §10): the one line a turn's context carries about
/// where its file tools and commands act — the project's folder, the
/// conversation's scratch folder, or, for a project whose folder is gone or
/// that no folder under `projects/` records, the refusal's own sentence, so
/// the model knows before it tries. `None` when there is nothing to name
/// (no context, neither a project nor a conversation, an unreadable store).
/// Pure, like [`working_folder`]: it creates nothing.
///
/// The lead's ruling R-T22: `held` is the names of the tools the loop is
/// offered, and the line names only the file tools among them
/// ([`FOLDER_FILE_TOOLS`], in that order), says a shell command starts in
/// the folder only when `shell_execute` is held, and — with no project —
/// says `file_write` saves a by-product only when it is held. On a project
/// turn a loop that holds neither `file_write` nor `file_edit` is told it
/// writes no file there; the lead is told, too, that a subagent does.
pub fn working_folder_note<'a>(
    ctx: &ToolContext,
    held: impl IntoIterator<Item = &'a str>,
    reader: NoteReader,
) -> Option<String> {
    let held: Vec<&str> = held.into_iter().collect();
    let holds = |name: &str| held.contains(&name);
    let shell = holds("shell_execute");
    let line = match working_folder(ctx) {
        Ok(WorkingFolder::Project { root }) => {
            let mut line = format!(
                "Your working folder is the project folder {}.",
                root.display()
            );
            let names = named(&FOLDER_FILE_TOOLS, &holds);
            match (names.is_empty(), shell) {
                (false, true) => line.push_str(&format!(
                    " Paths in {names} are relative to it, and a shell command starts in it."
                )),
                (false, false) => line.push_str(&format!(" Paths in {names} are relative to it.")),
                (true, true) => line.push_str(" A shell command starts in it."),
                (true, false) => {}
            }
            if !holds("file_write") && !holds("file_edit") {
                line.push_str(" You hold no tool that writes a file in the project");
                line.push_str(match reader {
                    NoteReader::Lead => {
                        ": a file for the project is written by a subagent whose template grants \
                         file_write."
                    }
                    NoteReader::Loop => ".",
                });
            }
            line
        }
        Ok(WorkingFolder::Scratch { dir, .. }) => {
            let mut line = format!(
                "This conversation has no project. Your working folder is its scratch folder {}",
                dir.display()
            );
            // `file_write` saves a by-product and `file_edit` is refused
            // there (it changes a project's file), so neither is relative to
            // the scratch folder.
            let names = named(&["file_read", "file_list", "send"], &holds);
            match (names.is_empty(), shell) {
                (false, true) => line.push_str(&format!(
                    ": paths in {names} are relative to it, and a shell command starts in it."
                )),
                (false, false) => line.push_str(&format!(": paths in {names} are relative to it.")),
                (true, true) => line.push_str(": a shell command starts in it."),
                (true, false) => line.push('.'),
            }
            if holds("file_write") {
                line.push_str(
                    " file_write saves a by-product in OpenAlpaca's artifact store instead.",
                );
            }
            line
        }
        Err(
            refusal @ (WorkingFolderRefusal::ProjectMissing { .. }
            | WorkingFolderRefusal::NotAProject { .. }),
        ) => refusal.to_string(),
        Err(_) => return None,
    };
    Some(format!("<working_folder>{line}</working_folder>"))
}

/// The file tools R28's line can name, in the order it names them (R-T22).
pub const FOLDER_FILE_TOOLS: [&str; 5] =
    ["file_read", "file_list", "file_write", "file_edit", "send"];

/// The names in `among` that `holds` answers for, as one list: "a", "a and
/// b", "a, b and c". Empty when none is held.
fn named(among: &[&str], holds: &impl Fn(&str) -> bool) -> String {
    let names: Vec<&str> = among.iter().copied().filter(|name| holds(name)).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// One level of a scratch folder: made private when absent (never
/// recursively), then required to be a directory of its own — a link or a
/// file standing at its name is refused, not followed.
fn make_private_dir(path: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{} could not be made: {e}", path.display())),
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "{} is not a folder of its own: a link or a file stands at its name",
            path.display()
        ))
    }
}

/// Step 1 of [`working_folder`]: the request's root, as `projects/` records it.
/// `lookup` matches a recorded root as written first (S0.2, ST-23), so a root
/// whose folder was renamed by case only, or moved with a link left at the old
/// path, names its own project — missing — and never the folder it now reaches.
fn project_folder(root: &str) -> Result<WorkingFolder, WorkingFolderRefusal> {
    let path = Path::new(root);
    if !path.is_absolute() {
        return Err(WorkingFolderRefusal::NotAProject {
            root: root.to_string(),
        });
    }
    match projects::lookup(path) {
        Ok(Some(project)) if !project.missing => Ok(WorkingFolder::Project { root: project.root }),
        Ok(Some(project)) => Err(WorkingFolderRefusal::ProjectMissing {
            root: project.root_string(),
        }),
        Ok(None) => Err(WorkingFolderRefusal::NotAProject {
            root: root.to_string(),
        }),
        Err(e) => Err(WorkingFolderRefusal::Unavailable {
            reason: format!("{e:#}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{GuardedProject, HomeStoreGuard};

    /// Every tool R28's line can name: the file tools and the shell.
    const EVERY_FOLDER_TOOL: [&str; 6] = [
        "file_read",
        "file_list",
        "file_write",
        "file_edit",
        "send",
        "shell_execute",
    ];

    #[test]
    fn a_project_turn_works_in_the_registered_project_root() {
        let gp = GuardedProject::new();

        let folder = working_folder(&gp.ctx()).unwrap();

        assert_eq!(
            gp.root,
            gp.project.path().canonicalize().unwrap(),
            "the root register recorded is the folder, canonical"
        );
        assert_eq!(
            folder,
            WorkingFolder::Project {
                root: gp.root.clone()
            }
        );
        assert!(folder.is_project());
        assert_eq!(folder.path(), gp.root.as_path());
    }

    #[test]
    fn a_turn_with_no_project_works_in_its_conversations_scratch_folder_and_creates_nothing() {
        let gp = GuardedProject::new();

        let folder = working_folder(&gp.scratch_ctx()).unwrap();

        let dir = gp.home_root().join("scratch").join("conv-test");
        assert_eq!(
            folder,
            WorkingFolder::Scratch {
                dir: dir.clone(),
                conversation_id: "conv-test".to_string(),
            }
        );
        assert!(!folder.is_project());
        assert_eq!(folder.path(), dir.as_path());
        assert!(
            !gp.home_root().join("scratch").exists(),
            "asking where a call acts makes no folder"
        );
    }

    /// The id becomes one path segment (`session_dir_name`), so no id names a
    /// folder outside `scratch/`.
    #[test]
    fn a_conversation_id_is_one_path_segment_under_scratch() {
        let gp = GuardedProject::new();
        let mut ctx = gp.scratch_ctx();
        ctx.conversation_id = Some("../../escape".to_string());

        let folder = working_folder(&ctx).unwrap();

        assert_eq!(
            folder.path().parent(),
            Some(gp.home_root().join("scratch").as_path())
        );
    }

    /// R27 (ST-7): a registered project whose folder is gone is refused with
    /// the one sentence naming both ways out.
    #[test]
    fn a_project_whose_folder_is_gone_is_refused_with_the_repoint_sentence() {
        let gp = GuardedProject::new();
        std::fs::remove_dir_all(&gp.root).unwrap();

        let refusal = working_folder(&gp.ctx()).unwrap_err();

        let root = gp.root.to_string_lossy().into_owned();
        assert_eq!(
            refusal,
            WorkingFolderRefusal::ProjectMissing { root: root.clone() }
        );
        assert_eq!(
            refusal.to_string(),
            format!(
                "The project folder {root} is missing — its folder is gone or has moved: re-point \
                 the project to where its folder is now (Settings → Connection → Projects → \
                 Re-point…, or openalpaca store rebase {root} <new path>), or delete its data \
                 (Delete data…, or openalpaca store purge {root}). No file tool or command runs \
                 until then."
            )
        );
    }

    /// A root no folder under `projects/` records is refused even though the
    /// turn has a conversation: a stale project root never quietly becomes a
    /// scratch folder.
    #[test]
    fn a_root_no_project_records_is_not_a_project() {
        let gp = GuardedProject::new();
        let other = tempfile::tempdir().unwrap();
        let root = other
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let mut ctx = gp.ctx();
        ctx.request_workspace_root = Some(root.clone());

        let refusal = working_folder(&ctx).unwrap_err();

        assert_eq!(
            refusal,
            WorkingFolderRefusal::NotAProject { root: root.clone() }
        );
        assert_eq!(
            refusal.to_string(),
            format!(
                "{root} is not a project OpenAlpaca knows: no folder under projects/ records it"
            )
        );
    }

    #[test]
    fn a_relative_root_is_not_a_project() {
        let gp = GuardedProject::new();
        let mut ctx = gp.scratch_ctx();
        ctx.request_workspace_root = Some("relative/dir".to_string());

        assert_eq!(
            working_folder(&ctx).unwrap_err(),
            WorkingFolderRefusal::NotAProject {
                root: "relative/dir".to_string()
            }
        );
    }

    /// R44: neither a project nor a conversation. A blank root is no project
    /// and a blank id no conversation. Nothing here reads a store.
    #[test]
    fn with_neither_a_project_nor_a_conversation_there_is_no_working_folder() {
        let blank = ToolContext {
            request_workspace_root: Some("  ".to_string()),
            conversation_id: Some(String::new()),
            ..Default::default()
        };
        for ctx in [ToolContext::default(), blank] {
            let refusal = working_folder(&ctx).unwrap_err();
            assert_eq!(refusal, WorkingFolderRefusal::NoConversation);
            assert_eq!(
                refusal.to_string(),
                "This call has neither a project nor a conversation, so it has no working \
                 folder: file tools and commands run only inside a conversation or a project."
            );
        }
    }

    /// The home store cannot be resolved: both arms say so, in one sentence.
    #[test]
    fn an_unusable_home_store_makes_the_folder_unavailable() {
        let _env = HomeStoreGuard::set(Path::new("relative-store"));
        let reason = "OPENALPACA_HOME_STORE must be an absolute path, got 'relative-store'";

        let scratch = ToolContext {
            conversation_id: Some("conv-test".to_string()),
            ..Default::default()
        };
        let refusal = working_folder(&scratch).unwrap_err();
        assert_eq!(
            refusal,
            WorkingFolderRefusal::Unavailable {
                reason: reason.to_string()
            }
        );
        assert_eq!(
            refusal.to_string(),
            format!("The working folder could not be read: {reason}")
        );

        let project = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            request_workspace_root: Some(
                project
                    .path()
                    .canonicalize()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..Default::default()
        };
        assert_eq!(
            working_folder(&ctx).unwrap_err(),
            WorkingFolderRefusal::Unavailable {
                reason: reason.to_string()
            }
        );
    }

    /// R26's refusal for a tool reached with no context (S3 returns it).
    #[test]
    fn a_call_with_no_context_is_told_why_in_one_sentence() {
        assert_eq!(
            WorkingFolderRefusal::NoContext.to_string(),
            "This tool acts in the conversation's working folder, and this call carries no \
             conversation to name one."
        );
    }

    // ── S3: the scratch folder's life, and the R28 note ─────────────────
    // (S2's `use crate::test_util::GuardedProject` is in scope; the full path
    // is kept so the block reads alone.)

    #[test]
    fn a_scratch_folder_is_named_after_the_conversation_under_scratch() {
        let gp = crate::test_util::GuardedProject::new();
        assert_eq!(
            scratch_dir("conv-test").unwrap(),
            gp.home_root().join("scratch").join("conv-test")
        );
        // `session_dir_name` makes an id one safe path segment: a slash never
        // reaches the file system, so no id climbs out of `scratch/`.
        assert_eq!(
            scratch_dir("../x").unwrap(),
            gp.home_root().join("scratch").join(".._x")
        );
        assert_eq!(
            working_folder(&gp.scratch_ctx()).unwrap().path(),
            scratch_dir("conv-test").unwrap().as_path()
        );
    }

    #[test]
    fn ensure_makes_a_private_scratch_folder_on_first_use_and_only_then() {
        let gp = crate::test_util::GuardedProject::new();
        let dir = gp.home_root().join("scratch").join("conv-test");
        let folder = working_folder(&gp.scratch_ctx()).unwrap();
        assert!(!dir.exists(), "naming the folder makes nothing");

        assert_eq!(folder.ensure().unwrap(), dir.as_path());
        assert!(dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for level in [dir.parent().unwrap(), dir.as_path()] {
                let mode = std::fs::metadata(level).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "{} is {mode:o}", level.display());
            }
        }

        std::fs::write(dir.join("kept.txt"), "kept").unwrap();
        assert_eq!(
            folder.ensure().unwrap(),
            dir.as_path(),
            "a second call finds it"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("kept.txt")).unwrap(),
            "kept"
        );
    }

    /// A link where `scratch/` or the conversation's folder should be would
    /// move the working folder somewhere else; it is refused, and nothing is
    /// made through it.
    #[cfg(unix)]
    #[test]
    fn ensure_refuses_a_link_where_a_scratch_folder_should_be() {
        let gp = crate::test_util::GuardedProject::new();
        let elsewhere = tempfile::tempdir().unwrap();

        // A link at `<home>/scratch` itself.
        std::os::unix::fs::symlink(elsewhere.path(), gp.home_root().join("scratch")).unwrap();
        let err = working_folder(&gp.scratch_ctx())
            .unwrap()
            .ensure()
            .expect_err("a link is not the scratch folder");
        assert!(
            matches!(&err, WorkingFolderRefusal::Unavailable { reason }
                if reason.contains("is not a folder of its own")),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read_dir(elsewhere.path()).unwrap().count(),
            0,
            "nothing was made through the link"
        );

        // A link at `<home>/scratch/<id>`.
        std::fs::remove_file(gp.home_root().join("scratch")).unwrap();
        std::fs::create_dir(gp.home_root().join("scratch")).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path(),
            gp.home_root().join("scratch").join("conv-test"),
        )
        .unwrap();
        let err = working_folder(&gp.scratch_ctx())
            .unwrap()
            .ensure()
            .expect_err("a link is not the conversation's folder");
        assert!(
            matches!(&err, WorkingFolderRefusal::Unavailable { reason }
                if reason.contains("is not a folder of its own")),
            "{err:?}"
        );
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
    }

    #[test]
    fn ensure_answers_a_project_folder_gone_since_the_lookup_as_unavailable() {
        let gp = crate::test_util::GuardedProject::new();
        let folder = working_folder(&gp.ctx()).unwrap();
        assert_eq!(folder.ensure().unwrap(), gp.root.as_path());
        std::fs::remove_dir_all(&gp.root).unwrap();
        let err = folder.ensure().expect_err("the folder is gone");
        assert!(
            matches!(&err, WorkingFolderRefusal::Unavailable { reason }
                if reason.contains("is missing: it is no longer a directory")),
            "{err:?}"
        );
    }

    /// ST-23 (S0, the lead's ruling R-T17): a project whose folder moved with
    /// a link left at its recorded root is missing — the lookup says so before
    /// any tool acts (R27) — and a folder `ensure` found before the move is not
    /// acted in through the link. Its own scene, not `GuardedProject`'s, whose
    /// `TempDir` owns the root.
    #[cfg(unix)]
    #[test]
    fn a_project_moved_behind_a_link_is_missing_and_never_acted_in_through_it() {
        let home = tempfile::tempdir().unwrap();
        let scene = tempfile::tempdir().unwrap();
        let _guard = HomeStoreGuard::set(&home.path().canonicalize().unwrap());
        let root = scene.path().canonicalize().unwrap().join("app");
        std::fs::create_dir(&root).unwrap();
        projects::register(&root).unwrap();
        let ctx = ToolContext {
            owner_id: Some("owner-test".to_string()),
            request_workspace_root: Some(root.display().to_string()),
            conversation_id: Some("conv-test".to_string()),
            ..Default::default()
        };
        let folder = working_folder(&ctx).unwrap();
        assert_eq!(folder.ensure().unwrap(), root.as_path());

        let moved = root.with_file_name("app-moved");
        std::fs::rename(&root, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &root).unwrap();

        let err = folder
            .ensure()
            .expect_err("the root no longer leads to itself");
        assert!(
            matches!(&err, WorkingFolderRefusal::Unavailable { reason }
                if reason.contains("is missing: it is no longer a directory")),
            "{err:?}"
        );
        let missing = WorkingFolderRefusal::ProjectMissing {
            root: root.display().to_string(),
        };
        assert_eq!(working_folder(&ctx), Err(missing.clone()));
        assert!(
            missing
                .to_string()
                .contains("is missing — its folder is gone or has moved:")
        );
        assert_eq!(
            working_folder_note(&ctx, EVERY_FOLDER_TOOL, NoteReader::Loop),
            Some(format!("<working_folder>{missing}</working_folder>"))
        );
    }

    #[test]
    fn remove_scratch_removes_the_conversations_folder_and_no_other() {
        let gp = crate::test_util::GuardedProject::new();
        let scratch = gp.home_root().join("scratch");
        for id in ["conv-a", "conv-b"] {
            std::fs::create_dir_all(scratch.join(id).join("sub")).unwrap();
            std::fs::write(scratch.join(id).join("sub/f.txt"), id).unwrap();
        }

        assert!(remove_scratch("conv-a").unwrap());
        assert!(!scratch.join("conv-a").exists());
        assert_eq!(
            std::fs::read_to_string(scratch.join("conv-b/sub/f.txt")).unwrap(),
            "conv-b"
        );
        assert!(
            !remove_scratch("conv-a").unwrap(),
            "a second removal finds nothing"
        );
        assert!(!remove_scratch("never-used").unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn remove_scratch_removes_a_link_and_never_what_it_points_at() {
        let gp = crate::test_util::GuardedProject::new();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::write(elsewhere.path().join("precious.txt"), "precious").unwrap();
        let scratch = gp.home_root().join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), scratch.join("conv-link")).unwrap();

        assert!(remove_scratch("conv-link").unwrap());
        assert!(std::fs::symlink_metadata(scratch.join("conv-link")).is_err());
        assert_eq!(
            std::fs::read_to_string(elsewhere.path().join("precious.txt")).unwrap(),
            "precious"
        );
    }

    /// A link at `<home>/scratch` itself — the level `ensure` refuses to make
    /// anything through — is never followed by a removal either: the folder
    /// of the same name behind it, and the link, are left as they are, and
    /// the refusal names the link.
    #[cfg(unix)]
    #[test]
    fn remove_scratch_never_follows_a_link_where_the_scratch_folder_should_be() {
        let gp = crate::test_util::GuardedProject::new();
        let elsewhere = tempfile::tempdir().unwrap();
        std::fs::create_dir(elsewhere.path().join("conv-x")).unwrap();
        std::fs::write(elsewhere.path().join("conv-x/precious.txt"), "precious").unwrap();
        let scratch = gp.home_root().join("scratch");
        std::os::unix::fs::symlink(elsewhere.path(), &scratch).unwrap();

        let err = remove_scratch("conv-x").expect_err("a link is not the scratch folder");
        assert!(
            err.to_string().contains("is not a folder of its own"),
            "{err}"
        );
        assert!(
            err.to_string().contains(&scratch.display().to_string()),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(elsewhere.path().join("conv-x/precious.txt")).unwrap(),
            "precious"
        );
        assert!(
            std::fs::symlink_metadata(&scratch)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link at scratch/ is left as it is"
        );
    }

    #[test]
    fn the_note_names_the_project_folder() {
        let gp = crate::test_util::GuardedProject::new();
        assert_eq!(
            working_folder_note(&gp.ctx(), EVERY_FOLDER_TOOL, NoteReader::Loop),
            Some(format!(
                "<working_folder>Your working folder is the project folder {}. Paths in \
                 file_read, file_list, file_write, file_edit and send are relative to it, and a \
                 shell command starts in it.</working_folder>",
                gp.root.display()
            ))
        );
    }

    #[test]
    fn the_note_names_the_scratch_folder_and_makes_nothing() {
        let gp = crate::test_util::GuardedProject::new();
        let dir = gp.home_root().join("scratch").join("conv-test");
        assert_eq!(
            working_folder_note(&gp.scratch_ctx(), EVERY_FOLDER_TOOL, NoteReader::Loop),
            Some(format!(
                "<working_folder>This conversation has no project. Your working folder is its \
                 scratch folder {}: paths in file_read, file_list and send are relative to it, \
                 and a shell command starts in it. file_write saves a by-product in OpenAlpaca's \
                 artifact store instead.</working_folder>",
                dir.display()
            ))
        );
        assert!(
            !gp.home_root().join("scratch").exists(),
            "a note makes no folder"
        );
    }

    #[test]
    fn the_note_carries_the_sentence_of_a_missing_or_unknown_project() {
        let gp = crate::test_util::GuardedProject::new();
        std::fs::remove_dir_all(&gp.root).unwrap();
        let missing = WorkingFolderRefusal::ProjectMissing {
            root: gp.root.display().to_string(),
        };
        assert_eq!(
            working_folder_note(&gp.ctx(), EVERY_FOLDER_TOOL, NoteReader::Loop),
            Some(format!("<working_folder>{missing}</working_folder>"))
        );

        let unknown = tempfile::tempdir().unwrap();
        let root = unknown.path().canonicalize().unwrap().display().to_string();
        let ctx = ToolContext {
            request_workspace_root: Some(root.clone()),
            conversation_id: Some("conv-test".to_string()),
            ..Default::default()
        };
        assert_eq!(
            working_folder_note(&ctx, EVERY_FOLDER_TOOL, NoteReader::Loop),
            Some(format!(
                "<working_folder>{root} is not a project OpenAlpaca knows: no folder under \
                 projects/ records it</working_folder>"
            ))
        );
    }

    /// A call with an owner and a task but neither a project nor a
    /// conversation — since S3.13 (the lead's ruling R-T2) no dispatched run
    /// is one, but a dispatcher with no database still builds it: no folder
    /// to name.
    #[test]
    fn the_note_says_nothing_without_a_folder_to_name() {
        assert_eq!(
            working_folder_note(&ToolContext::default(), EVERY_FOLDER_TOOL, NoteReader::Loop),
            None
        );
        let parked = ToolContext {
            owner_id: Some("owner-test".to_string()),
            task_id: Some("task-parked".to_string()),
            ..Default::default()
        };
        assert_eq!(
            working_folder_note(&parked, EVERY_FOLDER_TOOL, NoteReader::Loop),
            None
        );
    }

    /// The lead's ruling R-T22 (the whole-stage review's d6-surfaces-1): the
    /// line names only the file tools the loop holds, says a shell command
    /// starts in the folder only when the loop holds `shell_execute`, and on
    /// a project turn tells a loop that holds neither `file_write` nor
    /// `file_edit` that it writes no file there. A loop holding `file_read`
    /// alone gets neither a write clause nor the shell's.
    #[test]
    fn the_project_note_names_only_the_tools_the_loop_holds() {
        let gp = crate::test_util::GuardedProject::new();
        let root = gp.root.display();
        let note =
            |held: &[&str]| working_folder_note(&gp.ctx(), held.iter().copied(), NoteReader::Loop);

        assert_eq!(
            note(&["file_read"]),
            Some(format!(
                "<working_folder>Your working folder is the project folder {root}. Paths in \
                 file_read are relative to it. You hold no tool that writes a file in the \
                 project.</working_folder>"
            ))
        );
        assert_eq!(
            note(&["file_list", "artifact_read", "file_read"]),
            Some(format!(
                "<working_folder>Your working folder is the project folder {root}. Paths in \
                 file_read and file_list are relative to it. You hold no tool that writes a file \
                 in the project.</working_folder>"
            )),
            "named in one order whatever the surface's, and only the folder's tools"
        );
        assert_eq!(
            note(&["file_edit"]),
            Some(format!(
                "<working_folder>Your working folder is the project folder {root}. Paths in \
                 file_edit are relative to it.</working_folder>"
            )),
            "a loop that edits files writes in the project"
        );
        assert_eq!(
            note(&["shell_execute"]),
            Some(format!(
                "<working_folder>Your working folder is the project folder {root}. A shell \
                 command starts in it. You hold no tool that writes a file in the \
                 project.</working_folder>"
            ))
        );
        assert_eq!(
            note(&["start_workflow", "memory_search"]),
            Some(format!(
                "<working_folder>Your working folder is the project folder {root}. You hold no \
                 tool that writes a file in the project.</working_folder>"
            ))
        );
    }

    /// R-T22: the lead holds no tool that writes a file, and its line says
    /// where a file for the project comes from — a subagent.
    #[test]
    fn the_leads_project_note_says_a_subagent_writes_a_file_for_the_project() {
        let gp = crate::test_util::GuardedProject::new();
        assert_eq!(
            working_folder_note(&gp.ctx(), ["file_read", "file_list"], NoteReader::Lead),
            Some(format!(
                "<working_folder>Your working folder is the project folder {}. Paths in \
                 file_read and file_list are relative to it. You hold no tool that writes a file \
                 in the project: a file for the project is written by a subagent whose template \
                 grants file_write.</working_folder>",
                gp.root.display()
            ))
        );
        // A lead whose template was edited to grant a writer is told no
        // such thing.
        assert_eq!(
            working_folder_note(&gp.ctx(), ["file_read", "file_write"], NoteReader::Lead),
            Some(format!(
                "<working_folder>Your working folder is the project folder {}. Paths in \
                 file_read and file_write are relative to it.</working_folder>",
                gp.root.display()
            ))
        );
    }

    /// R-T22 with no project: the scratch line names the readers and `send`
    /// it holds, the shell only when held, and `file_write`'s by-product
    /// only when held; it never says the loop writes no file — with no
    /// project there is no project to write into — and the lead's line is
    /// the same.
    #[test]
    fn the_scratch_note_names_only_the_tools_the_loop_holds() {
        let gp = crate::test_util::GuardedProject::new();
        let dir = gp.home_root().join("scratch").join("conv-test");
        let dir = dir.display();
        let note = |held: &[&str], reader| {
            working_folder_note(&gp.scratch_ctx(), held.iter().copied(), reader)
        };

        assert_eq!(
            note(&["file_read"], NoteReader::Loop),
            Some(format!(
                "<working_folder>This conversation has no project. Your working folder is its \
                 scratch folder {dir}: paths in file_read are relative to it.</working_folder>"
            ))
        );
        assert_eq!(
            note(&["file_read", "file_list"], NoteReader::Lead),
            Some(format!(
                "<working_folder>This conversation has no project. Your working folder is its \
                 scratch folder {dir}: paths in file_read and file_list are relative to \
                 it.</working_folder>"
            ))
        );
        assert_eq!(
            note(&["shell_execute", "file_write"], NoteReader::Loop),
            Some(format!(
                "<working_folder>This conversation has no project. Your working folder is its \
                 scratch folder {dir}: a shell command starts in it. file_write saves a \
                 by-product in OpenAlpaca's artifact store instead.</working_folder>"
            ))
        );
        assert_eq!(
            note(&[], NoteReader::Loop),
            Some(format!(
                "<working_folder>This conversation has no project. Your working folder is its \
                 scratch folder {dir}.</working_folder>"
            ))
        );
    }
}
