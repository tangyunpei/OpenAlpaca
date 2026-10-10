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
    let home = store::home_root().map_err(|e| WorkingFolderRefusal::Unavailable {
        reason: format!("{e:#}"),
    })?;
    Ok(WorkingFolder::Scratch {
        dir: home
            .join(ContentKind::Scratch.dir_name())
            .join(session_dir_name(id)),
        conversation_id: id.to_string(),
    })
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
}
