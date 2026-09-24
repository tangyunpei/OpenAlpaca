//! The mutations of design §6 that touch an existing note — `update`
//! (edit / archive / confirm), `forget` and `undo` — under the revision
//! check, the G2 authority rules and the per-partition writer queue. Each
//! is one single-note mutation of the file layer; the index follows.

use super::{
    ForgetInput, ForgetItem, ForgetItemStatus, ForgetOutput, MemoryAccessContext, MemoryChange,
    MemoryError, MemoryService, Retained, UpdateAction, UpdateInput, UpdateOutput,
};
use openalpaca_storage::memory_files::{
    self as files, ForgetStatus, MAX_NOTE_BYTES, MAX_TITLE_CHARS, NoteId, NotePatch, Origin,
    SourceType, is_valid_fact_key,
};

impl MemoryService {
    /// `memory_update` (design §7a): edit, archive or confirm under the
    /// revision check and the G2 rules.
    pub fn update(
        &self,
        ctx: &MemoryAccessContext,
        input: UpdateInput,
    ) -> Result<UpdateOutput, MemoryError> {
        Self::require(ctx.capabilities.write, "memory_write")?;
        if !files::is_revision(&input.expected_revision) {
            return Err(MemoryError::invalid(
                "expected_revision is not a 64-character hex revision",
            ));
        }
        let resolved = self.resolve(ctx)?;
        let Some((holder, _)) = Self::find_note(&resolved, &input.id)? else {
            return Err(MemoryError::NotFound(input.id));
        };
        let store_scope = holder.scope().clone();
        let (action, change) = match input.action {
            UpdateAction::Edit(patch) => {
                let patch = NotePatch {
                    title: patch.title,
                    body: patch.body,
                    aliases: patch.aliases,
                    tags: patch.tags,
                    fact_key: patch.fact_key,
                    valid_from: patch.valid_from,
                    valid_to: patch.valid_to,
                    review_after: patch.review_after,
                };
                if patch.is_empty() {
                    return Err(MemoryError::invalid("an edit needs a non-empty patch"));
                }
                if let Some(title) = &patch.title
                    && (title.trim().is_empty() || title.chars().count() > MAX_TITLE_CHARS)
                {
                    return Err(MemoryError::invalid(format!(
                        "title must be 1 to {MAX_TITLE_CHARS} characters"
                    )));
                }
                if let Some(body) = &patch.body
                    && body.len() > MAX_NOTE_BYTES
                {
                    return Err(MemoryError::invalid(format!(
                        "body is {} bytes, above the 32 KiB ({MAX_NOTE_BYTES} byte) note limit",
                        body.len()
                    )));
                }
                if let Some(Some(key)) = &patch.fact_key
                    && !is_valid_fact_key(key)
                {
                    return Err(MemoryError::invalid(format!(
                        "fact_key `{key}` does not match [a-z0-9][a-z0-9._-]{{0,63}}"
                    )));
                }
                (files::UpdateAction::Edit(patch), "updated")
            }
            UpdateAction::Archive => (files::UpdateAction::Archive, "archived"),
            UpdateAction::Confirm => (files::UpdateAction::Confirm, "confirmed"),
        };
        let _write = self
            .slots
            .begin_write(&Self::slot_key(&store_scope, &ctx.owner_key));
        let partition = Self::open_partition(&store_scope, &ctx.owner_id)?;
        let updated =
            partition.update(&input.id, &input.expected_revision, action, ctx.actor(None))?;
        self.reindex(&partition, false)?;
        let origin = partition
            .load()?
            .get(&input.id)
            .map(|v| v.derived.origin)
            .unwrap_or(Origin::External);
        let change = updated.written.then(|| MemoryChange {
            partition_key: partition.partition_key().to_string(),
            note_id: updated.id,
            change,
            revision: Some(updated.revision.clone()),
            origin,
            surface: Some(ctx.surface),
        });
        if let Some(change) = &change {
            self.publish(change);
        }
        Ok(UpdateOutput {
            id: updated.id,
            revision: updated.revision,
            status: updated.status,
            change,
        })
    }

    /// `memory_forget` (design §6, *Forget contract*): ids with expected
    /// revisions, each its own single-note mutation, tombstone first.
    pub fn forget(
        &self,
        ctx: &MemoryAccessContext,
        input: ForgetInput,
    ) -> Result<ForgetOutput, MemoryError> {
        Self::require(ctx.capabilities.forget, "memory_forget")?;
        if input.targets.is_empty() || input.targets.len() > 20 {
            return Err(MemoryError::invalid("targets takes 1 to 20 entries"));
        }
        for (_, rev) in &input.targets {
            if !files::is_revision(rev) {
                return Err(MemoryError::invalid(
                    "expected_revision is not a 64-character hex revision",
                ));
            }
        }
        let resolved = self.resolve(ctx)?;
        let mut results = Vec::with_capacity(input.targets.len());
        let mut changes = Vec::new();
        let mut cited_messages = false;
        let mut cited_tasks = false;
        for (id, expected) in &input.targets {
            let Some((holder, view)) = Self::find_note(&resolved, id)? else {
                results.push(ForgetItem {
                    id: *id,
                    status: ForgetItemStatus::NotFound,
                });
                continue;
            };
            let store_scope = holder.scope().clone();
            let _write = self
                .slots
                .begin_write(&Self::slot_key(&store_scope, &ctx.owner_key));
            let partition = Self::open_partition(&store_scope, &ctx.owner_id)?;
            let outcomes = partition.forget(&[(*id, expected.clone())])?;
            for outcome in outcomes {
                let status = match outcome.status {
                    ForgetStatus::Forgotten => {
                        cited_messages |= view
                            .note
                            .sources
                            .iter()
                            .any(|s| s.kind == SourceType::Message);
                        cited_tasks |= view.note.sources.iter().any(|s| s.kind == SourceType::Task);
                        let change = MemoryChange {
                            partition_key: partition.partition_key().to_string(),
                            note_id: outcome.id,
                            change: "forgotten",
                            revision: None,
                            origin: view.derived.origin,
                            surface: Some(ctx.surface),
                        };
                        self.publish(&change);
                        changes.push(change);
                        ForgetItemStatus::Forgotten
                    }
                    ForgetStatus::RevisionConflict { current_revision } => {
                        ForgetItemStatus::RevisionConflict { current_revision }
                    }
                    ForgetStatus::NotFound => ForgetItemStatus::NotFound,
                };
                results.push(ForgetItem {
                    id: outcome.id,
                    status,
                });
            }
            self.reindex(&partition, false)?;
        }
        let forgotten = results
            .iter()
            .filter(|r| r.status == ForgetItemStatus::Forgotten)
            .count();
        let retained = Retained {
            transcript: cited_messages,
            session_logs: forgotten > 0,
            note: retained_note(forgotten, cited_messages, cited_tasks),
        };
        Ok(ForgetOutput {
            results,
            retained,
            changes,
        })
    }

    /// `undo` (design §6, *recently learned*): a forget by id and expected
    /// revision, refused once the origin is no longer `extracted_user`.
    pub fn undo(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
        expected_revision: &str,
    ) -> Result<ForgetOutput, MemoryError> {
        Self::require(ctx.capabilities.forget, "memory_forget")?;
        let resolved = self.resolve(ctx)?;
        let Some((_, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        if view.derived.origin != Origin::ExtractedUser {
            return Err(MemoryError::NotUndoable {
                id,
                origin: view.derived.origin,
            });
        }
        let out = self.forget(
            ctx,
            ForgetInput {
                targets: vec![(id, expected_revision.to_string())],
                reason: Some("undo".into()),
            },
        )?;
        if let Some(ForgetItem {
            status: ForgetItemStatus::RevisionConflict { current_revision },
            ..
        }) = out.results.first()
        {
            return Err(MemoryError::RevisionConflict {
                id,
                current_revision: current_revision.clone(),
            });
        }
        Ok(out)
    }
}

fn retained_note(forgotten: usize, cited_messages: bool, cited_tasks: bool) -> String {
    if forgotten == 0 {
        return "Nothing was forgotten.".to_string();
    }
    let mut note = String::from(
        "The note files, their history and their index rows are gone, and the sources they cited will not be re-extracted.",
    );
    if cited_messages {
        note.push_str(
            " The transcript messages the note cited remain in the conversation history; delete them there if they must go too.",
        );
    }
    if cited_tasks {
        note.push_str(" The task rows the note cited remain in the task list.");
    }
    note.push_str(
        " Session logs and any external backup are separate data and are not touched by a forget.",
    );
    note
}
