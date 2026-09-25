//! The mutations of design §6 that touch an existing note — `update`
//! (edit / archive / confirm), `forget` and `undo` — under the revision
//! check, the G2 authority rules and the per-partition writer queue. Each
//! is one single-note mutation of the file layer; the index follows.
//!
//! Learning v2 (design 09 §3.2–§3.3) adds two things here: confirming a
//! suggestion binds its target's revision (S5) and then dismisses every
//! other pending suggestion for that target (S6); and `undo` is a
//! **withdrawal** — the one note goes, its tombstone keeps no link and
//! suppresses no source, so what it replaced comes back and whatever else
//! the same message taught stays learned (LR12).

use super::{
    ForgetInput, ForgetItem, ForgetItemStatus, ForgetOutput, MemoryAccessContext, MemoryChange,
    MemoryError, MemoryService, Retained, UpdateAction, UpdateInput, UpdateOutput, WithdrawOutput,
};
use openalpaca_storage::memory_files::{
    self as files, EffectiveStatus, ForgetMode, ForgetStatus, LoadedPartition, MAX_NOTE_BYTES,
    MAX_TITLE_CHARS, NoteId, NotePatch, NoteStatus, NoteView, Origin, Partition, SourceType,
    is_valid_fact_key,
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
        let Some((holder, before)) = Self::find_note(&resolved, &input.id)? else {
            return Err(MemoryError::NotFound(input.id));
        };
        let store_scope = holder.scope().clone();
        // The notes a suggestion would replace (design 09 §3.1): read before
        // the confirm, for S6's dismissals and the result's `superseded`.
        let suggestion_targets: Vec<NoteId> = match input.action {
            UpdateAction::Confirm { .. } if before.note.status == NoteStatus::Candidate => {
                before.note.supersedes.clone()
            }
            _ => Vec::new(),
        };
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
            UpdateAction::Confirm { target_revision } => {
                if let Some(rev) = &target_revision
                    && !files::is_revision(rev)
                {
                    return Err(MemoryError::invalid(
                        "target_revision is not a 64-character hex revision",
                    ));
                }
                (
                    files::UpdateAction::Confirm { target_revision },
                    "confirmed",
                )
            }
        };
        let _write = self
            .slots
            .begin_write(&Self::slot_key(&store_scope, &ctx.owner_key));
        let partition = self.open_partition(&store_scope, &ctx.owner_id)?;
        let (updated, mut view) = partition.update_viewed(
            &input.id,
            &input.expected_revision,
            action,
            ctx.actor(None),
        )?;
        // S6: the confirm is written; every other pending suggestion for the
        // same note is now moot, and each is withdrawn on its own (design
        // 09 §3.2). A confirm that failed returned above and dismissed
        // nothing.
        let dismissed = if suggestion_targets.is_empty() {
            Vec::new()
        } else {
            self.dismiss_others(ctx, &partition, &mut view, input.id, &suggestion_targets)
        };
        self.reindex_view(&partition, &view, false)?;
        let origin = view
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
            superseded: suggestion_targets,
            dismissed,
        })
    }

    /// S6 (design 09 §3.2): withdraws every pending suggestion in
    /// `partition` other than `confirmed` that names one of `targets`, each
    /// as its own single-note mutation after the confirm has been written.
    /// A withdrawal suppresses nothing, so a dismissed suggestion citing the
    /// row the confirmed note cites leaves that note recallable. One that
    /// moved or went since is left alone; the confirm stands whatever
    /// happens to these, so nothing here fails it. `view` is the partition
    /// right after the confirm, and is brought up to date with each
    /// dismissal.
    fn dismiss_others(
        &self,
        ctx: &MemoryAccessContext,
        partition: &Partition,
        view: &mut LoadedPartition,
        confirmed: NoteId,
        targets: &[NoteId],
    ) -> Vec<NoteId> {
        let others: Vec<(NoteId, String, Origin)> = view
            .views
            .iter()
            .filter(|v| {
                v.note.id != confirmed
                    && v.note.status == NoteStatus::Candidate
                    && v.note.supersedes.iter().any(|t| targets.contains(t))
            })
            .map(|v| (v.note.id, v.revision.clone(), v.derived.origin))
            .collect();
        let mut dismissed = Vec::new();
        for (id, revision, origin) in others {
            match partition.forget_viewed(&[(id, revision)], ForgetMode::Withdraw) {
                Ok((out, after)) => {
                    *view = after;
                    if out.first().map(|o| &o.status) != Some(&ForgetStatus::Forgotten) {
                        continue;
                    }
                    self.publish(&MemoryChange {
                        partition_key: partition.partition_key().to_string(),
                        note_id: id,
                        change: "forgotten",
                        revision: None,
                        origin,
                        surface: Some(ctx.surface),
                    });
                    dismissed.push(id);
                }
                Err(e) => {
                    tracing::warn!(
                        note = %id,
                        "memory: a suggestion made moot by a confirm could not be dismissed: {e}"
                    );
                    // A failure part-way may have left a tombstone: read
                    // what is there now, for the index.
                    if let Ok(fresh) = partition.load() {
                        *view = fresh;
                    }
                }
            }
        }
        dismissed
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
            // Design 09 §13.3: forgetting a conversation summary forgets the
            // summary, not what was said. It cites the conversation's own
            // rows, so a forget's suppression would turn every note those
            // rows taught `source_forgotten`; its tombstone suppresses none.
            // An episode candidate the extractor proposed is no summary.
            let mode = if files::is_conversation_summary(
                view.note.kind,
                view.derived.origin,
                view.note.status,
            ) {
                ForgetMode::Withdraw
            } else {
                ForgetMode::Forget
            };
            let _write = self
                .slots
                .begin_write(&Self::slot_key(&store_scope, &ctx.owner_key));
            let partition = self.open_partition(&store_scope, &ctx.owner_id)?;
            let (outcomes, after) = partition.forget_viewed(&[(*id, expected.clone())], mode)?;
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
            self.reindex_view(&partition, &after, false)?;
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

    /// Deleting or clearing a conversation (design 09 §13.3, LQ4) "also
    /// forgets the episodes whose sources all lie in that session": every
    /// conversation summary of `owner_id`, in the home partition and — when
    /// the conversation had one — its project's, whose `sources` are all
    /// `message` rows among `message_ids`. An episode the user saved, a note
    /// learned from the same rows and a summary that also cites another
    /// conversation's row all stay. Each is forgotten as `memory_forget`
    /// forgets a summary, on the user's own surface: the deletion is the
    /// user's act. Returns how many were forgotten.
    ///
    /// The caller reads `message_ids` before the rows go; the lookup is the
    /// index's `note_source`, and each note is re-read before it is judged.
    pub fn forget_conversation_summaries(
        &self,
        owner_id: &str,
        project_root: Option<&std::path::Path>,
        message_ids: &[i64],
    ) -> Result<usize, MemoryError> {
        if message_ids.is_empty() {
            return Ok(0);
        }
        let hint = match project_root {
            Some(root) => super::WorkspaceHint::Root(root),
            None => super::WorkspaceHint::None,
        };
        let ctx = self.context(
            owner_id,
            hint,
            super::Surface::UserSurface,
            super::Capabilities::ALL,
            super::Attribution::default(),
        )?;
        let resolved = self.resolve(&ctx)?;
        let rows: std::collections::HashSet<String> =
            message_ids.iter().map(|id| id.to_string()).collect();
        let source_keys: Vec<String> = message_ids
            .iter()
            .map(|id| format!("message:{id}"))
            .collect();
        let mut targets: Vec<(NoteId, String)> = Vec::new();
        for p in resolved.all() {
            if matches!(p.health(), files::PartitionHealth::Quarantined(_)) {
                continue;
            }
            self.touch(p)?;
            let key = p.partition_key().to_string();
            for row in self
                .index
                .notes_citing(std::slice::from_ref(&key), &source_keys)?
            {
                let (Some(kind), Some(origin), Some(status)) = (
                    files::NoteKind::parse(&row.kind),
                    Origin::parse(&row.origin),
                    NoteStatus::parse(&row.status),
                ) else {
                    continue;
                };
                if !files::is_conversation_summary(kind, origin, status) {
                    continue;
                }
                let Ok(id) = NoteId::parse(&row.id) else {
                    continue;
                };
                let super::recall::Verified::Fresh { note, revision, .. } =
                    super::recall::verify(p, &row.relative_path, id, &row.revision)
                else {
                    continue;
                };
                let all_here = !note.sources.is_empty()
                    && note
                        .sources
                        .iter()
                        .all(|s| s.kind == SourceType::Message && rows.contains(&s.id));
                if all_here && files::is_conversation_summary(note.kind, origin, note.status) {
                    targets.push((id, revision));
                }
            }
        }
        let mut forgotten = 0;
        for batch in targets.chunks(20) {
            let out = self.forget(
                &ctx,
                ForgetInput {
                    targets: batch.to_vec(),
                    reason: Some("its conversation was deleted".to_string()),
                },
            )?;
            forgotten += out
                .results
                .iter()
                .filter(|r| r.status == ForgetItemStatus::Forgotten)
                .count();
        }
        Ok(forgotten)
    }

    /// `undo` (design §6 *recently learned*; design 09 §3.3): a
    /// **withdrawal** by id and expected revision — "keep my note; take back
    /// what learning did". It accepts a note learned from the user's words
    /// (`extracted_user`) and any candidate, whatever its origin, which is
    /// how a suggestion is dismissed (the Memory view's *Keep mine*,
    /// `openalpaca memory undo`). Anything else — a note the user saved,
    /// edited or confirmed — is `not_undoable`: forget is the verb for that.
    ///
    /// The tombstone keeps no link and suppresses no source, so a replaced
    /// note comes back — in a chain A ← B ← C, undoing C brings B back and A
    /// stays superseded by B — and every other note the same message taught
    /// stays learned and recallable.
    pub fn undo(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
        expected_revision: &str,
    ) -> Result<WithdrawOutput, MemoryError> {
        Self::require(ctx.capabilities.forget, "memory_forget")?;
        if !files::is_revision(expected_revision) {
            return Err(MemoryError::invalid(
                "expected_revision is not a 64-character hex revision",
            ));
        }
        let resolved = self.resolve(ctx)?;
        let Some((holder, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        if view.derived.origin != Origin::ExtractedUser && view.note.status != NoteStatus::Candidate
        {
            return Err(MemoryError::NotUndoable {
                id,
                origin: view.derived.origin,
            });
        }
        self.withdraw(ctx, &holder.scope().clone(), &view, expected_revision)
    }

    /// One withdrawal (design 09 §3.3, ruling LR12) of `view` in the
    /// partition at `store_scope`, under that partition's writer slot.
    pub(super) fn withdraw(
        &self,
        ctx: &MemoryAccessContext,
        store_scope: &openalpaca_storage::store::StoreScope,
        view: &NoteView,
        expected_revision: &str,
    ) -> Result<WithdrawOutput, MemoryError> {
        let id = view.note.id;
        let _write = self
            .slots
            .begin_write(&Self::slot_key(store_scope, &ctx.owner_key));
        let partition = self.open_partition(store_scope, &ctx.owner_id)?;
        // What this note retires now, so the answer can name what came back.
        let retired: Vec<NoteId> = partition
            .load()?
            .views
            .iter()
            .filter(|v| v.derived.superseded_by.contains(&id))
            .map(|v| v.note.id)
            .collect();
        let (out, loaded) = partition
            .forget_viewed(&[(id, expected_revision.to_string())], ForgetMode::Withdraw)?;
        match out.first().map(|o| &o.status) {
            Some(ForgetStatus::Forgotten) => {}
            Some(ForgetStatus::RevisionConflict { current_revision }) => {
                return Err(MemoryError::RevisionConflict {
                    id,
                    current_revision: current_revision.clone(),
                });
            }
            Some(ForgetStatus::NotFound) | None => return Err(MemoryError::NotFound(id)),
        }
        self.reindex_view(&partition, &loaded, false)?;
        let restored = retired
            .into_iter()
            .filter(|t| {
                loaded
                    .get(t)
                    .is_some_and(|v| v.derived.effective_status != EffectiveStatus::Superseded)
            })
            .collect();
        let change = MemoryChange {
            partition_key: partition.partition_key().to_string(),
            note_id: id,
            change: "forgotten",
            revision: None,
            origin: view.derived.origin,
            surface: Some(ctx.surface),
        };
        self.publish(&change);
        Ok(WithdrawOutput {
            id,
            restored,
            change,
        })
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
