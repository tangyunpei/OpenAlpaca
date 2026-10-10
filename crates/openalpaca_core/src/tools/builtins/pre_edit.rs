//! The pre-edit copy (ST-17, R46): before a file tool replaces or edits a
//! file in the project, its current bytes are kept under the conversation's
//! `sessions/<session>/snapshots/`, committed by a `file_snapshot` record, and
//! the change does not happen unless that succeeded. Moved out of
//! `file_ops.rs` and judged against the call's project instead of the
//! daemon's startup folder; `file_write` uses it, and `file_edit` (S4b).

use std::path::{Path, PathBuf};

use crate::session_log::SnapshotSpec;
use crate::tools::registry::ToolContext;

/// Which verb a refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditVerb {
    Overwrite,
    // Constructed by `file_edit` (S4b), which removes this attribute.
    #[allow(dead_code)]
    Edit,
}

impl EditVerb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Overwrite => "overwrite",
            Self::Edit => "edit",
        }
    }
}

/// What the copy step found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreEdit {
    /// Nothing stands at the target, or it is not a regular file: nothing to keep.
    NewFile,
    /// The current bytes are in `sessions/<session>/snapshots/`, committed by a
    /// `file_snapshot` record, before the caller writes.
    Kept,
}

/// Whether a change of `target` may go ahead without losing bytes, writing
/// nothing — the card's half and the body's first: `Ok(true)` a file is there
/// and will be replaced, `Ok(false)` a new file (or a folder, which the write
/// itself refuses); `Err` R46 or ST-17's cap.
///
/// R46 is asked first: with no conversation log, raising the cap would not
/// help. Its discriminator is `ctx.session_log` — the copy needs the log's
/// emit side, and `conversation_id` is not it.
pub(super) fn check_pre_edit(
    path: &str,
    target: &Path,
    ctx: &ToolContext,
    snapshot_max_bytes: u64,
    verb: EditVerb,
) -> Result<bool, String> {
    let Ok(metadata) = std::fs::metadata(target) else {
        return Ok(false);
    };
    if !metadata.is_file() {
        return Ok(false);
    }
    let verb = verb.as_str();
    if ctx.session_log.is_none() {
        return Err(format!(
            "Refusing to {verb} '{path}': it already exists, and no copy of its current content \
             can be kept here — a skill or a plugin agent keeps no conversation log. The file is \
             unchanged. Write the result to a new file, or ask for the change in the \
             conversation itself."
        ));
    }
    if metadata.len() > snapshot_max_bytes {
        return Err(format!(
            "Refusing to {verb} '{path}': it is {} bytes, above the {snapshot_max_bytes} byte \
             [orchestrator.sessions] snapshot_max_bytes limit, so no pre-edit snapshot can be \
             kept for it. The file is unchanged. Raise snapshot_max_bytes, or copy the file aside \
             yourself first.",
            metadata.len()
        ));
    }
    Ok(true)
}

/// ST-17 + R46, the body's half: [`check_pre_edit`], then the copy. `target`
/// is the resolved path under `project_root`. A target outside it, or inside
/// the home store (a project never overlaps it, R13), is never copied — and
/// the change is refused rather than made uncopied. Fail closed: `Err` means
/// the caller must not write.
pub(super) async fn keep_pre_edit_copy(
    path: &str,
    target: &Path,
    project_root: &Path,
    ctx: &ToolContext,
    snapshot_max_bytes: u64,
    verb: EditVerb,
) -> Result<PreEdit, String> {
    if !check_pre_edit(path, target, ctx, snapshot_max_bytes, verb)? {
        return Ok(PreEdit::NewFile);
    }
    let word = verb.as_str();
    let copy_failed = |why: &str| {
        format!(
            "Refusing to {word} '{path}': its pre-edit snapshot could not be taken ({why}). The \
             file is unchanged."
        )
    };
    // Through any link, and only then judged.
    let source = tokio::fs::canonicalize(target)
        .await
        .map_err(|e| copy_failed(&e.to_string()))?;
    if !is_snapshotable(&source, project_root, home_store().as_deref()) {
        return Err(copy_failed("it is not a file inside the project folder"));
    }
    let Some(log) = ctx.session_log.as_ref() else {
        // `check_pre_edit` refused this already (R46); kept for the type.
        return Err(copy_failed("this call has no conversation log"));
    };
    let spec = SnapshotSpec {
        source,
        path: path.to_string(),
        task_id: ctx.task_id.clone(),
        span_id: None,
        agent: ctx
            .agent_instance_id
            .clone()
            .or_else(|| ctx.agent_id.clone()),
    };
    log.snapshot(spec)
        .await
        .map(|_| PreEdit::Kept)
        .map_err(|e| copy_failed(&e))
}

/// The home store, so a file inside it is never copied: `snapshots/` and
/// `results/` live there, and a session that copied its own log would grow
/// the very bytes the caps are about.
fn home_store() -> Option<PathBuf> {
    openalpaca_storage::store::home_root()
        .ok()
        .map(|root| root.canonicalize().unwrap_or(root))
}

/// Whether a resolved target is a project file worth copying: inside the
/// project root and outside the home store, both after link resolution.
fn is_snapshotable(target: &Path, project_root: &Path, store: Option<&Path>) -> bool {
    let Ok(root) = project_root.canonicalize() else {
        return false;
    };
    if !target.starts_with(&root) {
        return false;
    }
    !store.is_some_and(|store| target.starts_with(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_log::{SNAPSHOTS_DIR, SessionLogLimits, SessionLogService};
    use crate::test_util::GuardedProject;
    use std::fs;
    use std::sync::Arc;

    /// The default `[orchestrator.sessions] snapshot_max_bytes`.
    const CAP: u64 = 10 * 1024 * 1024;

    fn service(dir: &Path) -> Arc<SessionLogService> {
        SessionLogService::new(
            dir.to_path_buf(),
            None,
            SessionLogLimits::default(),
            "test".to_string(),
        )
        .into_arc()
    }

    /// The project turn of `gp`, with a conversation log for `session`.
    fn logged(gp: &GuardedProject, service: &SessionLogService, session: &str) -> ToolContext {
        ToolContext {
            session_id: Some(session.to_string()),
            session_log: Some(service.handle_for(session)),
            agent_instance_id: Some("code_agent::a1".to_string()),
            ..gp.ctx()
        }
    }

    /// Only an existing regular file has a pre-edit state: a new file and a
    /// folder have nothing to keep and nothing to refuse, log or no log.
    #[test]
    fn a_new_file_or_a_folder_has_nothing_to_keep() {
        let gp = GuardedProject::new();
        fs::create_dir_all(gp.root.join("docs")).unwrap();
        for target in [gp.root.join("fresh.md"), gp.root.join("docs")] {
            assert_eq!(
                check_pre_edit("x", &target, &gp.ctx(), CAP, EditVerb::Overwrite),
                Ok(false)
            );
        }
    }

    /// R46: where no conversation log can keep a copy — a skill or a plugin
    /// agent — an existing file is refused, by both verbs, and left as it is.
    #[test]
    fn without_a_conversation_log_an_existing_file_is_refused_by_both_verbs() {
        let gp = GuardedProject::new();
        let target = gp.root.join("notes.md");
        fs::write(&target, "old").unwrap();
        for (verb, word) in [(EditVerb::Overwrite, "overwrite"), (EditVerb::Edit, "edit")] {
            assert_eq!(
                check_pre_edit("notes.md", &target, &gp.ctx(), CAP, verb),
                Err(format!(
                    "Refusing to {word} 'notes.md': it already exists, and no copy of its current \
                     content can be kept here — a skill or a plugin agent keeps no conversation \
                     log. The file is unchanged. Write the result to a new file, or ask for the \
                     change in the conversation itself."
                ))
            );
        }
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    }

    /// ST-17: above the cap the change is refused, naming the key to raise.
    /// (A runtime, because a log handle starts its writer task.)
    #[tokio::test]
    async fn above_the_cap_the_file_is_refused_naming_the_limit() {
        let gp = GuardedProject::new();
        let sessions = tempfile::tempdir().unwrap();
        let service = service(sessions.path());
        let ctx = logged(&gp, &service, "sess-cap");
        let target = gp.root.join("big.txt");
        fs::write(&target, "far more than eight bytes").unwrap();

        assert_eq!(
            check_pre_edit("big.txt", &target, &ctx, 8, EditVerb::Edit),
            Err(
                "Refusing to edit 'big.txt': it is 25 bytes, above the 8 byte \
                 [orchestrator.sessions] snapshot_max_bytes limit, so no pre-edit snapshot can be \
                 kept for it. The file is unchanged. Raise snapshot_max_bytes, or copy the file \
                 aside yourself first."
                    .to_string()
            )
        );
        assert_eq!(
            check_pre_edit("big.txt", &target, &ctx, CAP, EditVerb::Overwrite),
            Ok(true)
        );
    }

    /// ST-17: the current bytes are in the conversation's `snapshots/`,
    /// committed, before the caller writes — and the copy changes nothing.
    #[tokio::test]
    async fn a_kept_copy_is_in_the_conversations_log_before_the_caller_writes() {
        let gp = GuardedProject::new();
        let sessions = tempfile::tempdir().unwrap();
        let service = service(sessions.path());
        let ctx = logged(&gp, &service, "sess-k");
        let target = gp.root.join("docs").join("report.md");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "the old draft").unwrap();

        let kept = keep_pre_edit_copy(
            "docs/report.md",
            &target,
            &gp.root,
            &ctx,
            CAP,
            EditVerb::Overwrite,
        )
        .await;

        assert_eq!(kept, Ok(PreEdit::Kept));
        assert!(ctx.session_log.as_ref().unwrap().flush().await);
        let images: Vec<_> = fs::read_dir(sessions.path().join("sess-k").join(SNAPSHOTS_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(images.len(), 1, "{images:?}");
        assert_eq!(fs::read_to_string(&images[0]).unwrap(), "the old draft");
        assert_eq!(fs::read_to_string(&target).unwrap(), "the old draft");

        let fresh = gp.root.join("fresh.md");
        assert_eq!(
            keep_pre_edit_copy("fresh.md", &fresh, &gp.root, &ctx, CAP, EditVerb::Overwrite).await,
            Ok(PreEdit::NewFile)
        );
    }

    /// Fail closed: a target that is not a file inside the project is never
    /// copied, and the change it guards is refused (unreachable after R31's
    /// resolver; a backstop).
    #[tokio::test]
    async fn a_target_outside_the_project_is_refused_not_copied() {
        let gp = GuardedProject::new();
        let sessions = tempfile::tempdir().unwrap();
        let service = service(sessions.path());
        let ctx = logged(&gp, &service, "sess-o");
        let outside = tempfile::tempdir().unwrap();
        let elsewhere = outside.path().join("elsewhere.txt");
        fs::write(&elsewhere, "not the project's").unwrap();

        let refused = keep_pre_edit_copy(
            "elsewhere.txt",
            &elsewhere,
            &gp.root,
            &ctx,
            CAP,
            EditVerb::Overwrite,
        )
        .await
        .unwrap_err();

        assert_eq!(
            refused,
            "Refusing to overwrite 'elsewhere.txt': its pre-edit snapshot could not be taken (it \
             is not a file inside the project folder). The file is unchanged."
        );
        assert!(!sessions.path().join("sess-o").join(SNAPSHOTS_DIR).exists());
    }

    /// The store holds the copies themselves. A file inside it is never one,
    /// whatever the project root happens to be. (Moved from `file_ops.rs`.)
    #[test]
    fn a_file_inside_the_store_is_never_imaged() {
        let work = tempfile::tempdir().unwrap();
        let store = work.path().join(".openalpaca");
        fs::create_dir_all(store.join("sessions/s/results")).unwrap();
        let spill = store.join("sessions/s/results/000001-t-dump.txt");
        fs::write(&spill, "a spilled result").unwrap();
        let plain = work.path().join("ordinary.txt");
        fs::write(&plain, "a workspace file").unwrap();
        let store = store.canonicalize().unwrap();

        assert!(
            is_snapshotable(&plain.canonicalize().unwrap(), work.path(), Some(&store)),
            "an ordinary project file is copied"
        );
        assert!(
            !is_snapshotable(&spill.canonicalize().unwrap(), work.path(), Some(&store)),
            "the store's own bytes are never copied"
        );
        assert!(
            !is_snapshotable(Path::new("/etc/hosts"), work.path(), Some(&store)),
            "and nothing outside the project is"
        );
    }
}
