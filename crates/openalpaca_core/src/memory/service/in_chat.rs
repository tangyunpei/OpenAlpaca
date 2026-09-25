//! Confirming a suggestion in chat (Learning v2, design 09 §4; owner
//! decision MD-12): the service's half of `memory_confirm`.
//!
//! The model asks and the person's card answers. The service maps exactly
//! one act to origin `user` — a confirm whose [`HumanApproval`] the sandbox
//! made on the one branch where a person approved this call — and only for a
//! **suggestion from the user's own words**: a candidate of origin
//! `extracted_user` that names the note it replaces and cites at least one
//! row the user typed (§4.2 rule 5). An inferred suggestion is confirmable
//! only in the Memory view or the CLI, so a card never presents the
//! assistant's inference as the user's words. Every other call from the same
//! context stays `ModelTool` (G2).
//!
//! The card's text is the daemon's, read here (§4.4), never an argument the
//! model wrote; a Deny withdraws the suggestion (§4.3, LQ2) exactly as the
//! Memory view's *Keep mine* does.

use super::{
    Capabilities, MemoryAccessContext, MemoryError, MemoryService, Surface, UpdateAction,
    UpdateInput, UpdateOutput, WithdrawOutput, cut_bytes,
};
use crate::tools::registry::{HumanApproval, HumanDenial};
use openalpaca_storage::ConversationRepository;
use openalpaca_storage::memory_files::{
    self as files, EffectiveStatus, Note, NoteId, NoteStatus, NoteView, Origin, SourceType,
    knowledge_text,
};

/// The most the card's preview carries (design 09 §4.4).
pub const PREVIEW_MAX_BYTES: usize = 1024;

const PREVIEW_TITLE_BYTES: usize = 80;
const PREVIEW_TEXT_BYTES: usize = 260;
const PREVIEW_REASON_BYTES: usize = 160;

/// What makes a note confirmable in chat, read once (§4.2 rule 5).
pub(super) struct InChatSuggestion {
    /// The note it would replace (S5: exactly one).
    pub target: NoteId,
    /// `YYYY-MM-DD` of the newest row the user typed that it cites.
    pub said_on: String,
}

impl MemoryService {
    /// The date of the newest **user-authored** row among `note`'s cited
    /// messages (design 09 §1.2, §4.4), from one primary-key read of the
    /// cited ids; `None` when it cites none or the daemon has no database.
    fn newest_user_row_date(&self, note: &Note) -> Result<Option<String>, MemoryError> {
        let Some(db) = &self.db else {
            return Ok(None);
        };
        let ids: Vec<i64> = note
            .sources
            .iter()
            .filter(|s| s.kind == SourceType::Message)
            .filter_map(|s| s.id.parse::<i64>().ok())
            .collect();
        let rows = ConversationRepository::new(db)
            .learning_rows_by_ids(&ids)
            .map_err(MemoryError::Internal)?;
        Ok(rows
            .iter()
            .filter(|row| crate::memory::learning::is_user_authored_row(row))
            .max_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)))
            .map(|row| row.created_at.chars().take(10).collect()))
    }

    /// Rule 5 of design 09 §4.2 over one read note: a candidate naming the
    /// one note it would replace, of origin `extracted_user`, citing a row
    /// the user typed. Anything else is `not_a_suggestion`, naming where it
    /// can be confirmed instead.
    pub(super) fn in_chat_suggestion(
        &self,
        view: &NoteView,
    ) -> Result<InChatSuggestion, MemoryError> {
        let id = view.note.id;
        let refuse = |why: String| MemoryError::NotASuggestion {
            id,
            message: format!(
                "note {id} {why}. Only a suggestion learned from the user's own words is confirmed in chat; the user confirms or keeps any other in the Memory view, or with `openalpaca memory confirm {id}` or `openalpaca memory undo {id}`."
            ),
        };
        if view.note.status != NoteStatus::Candidate || view.note.supersedes.is_empty() {
            return Err(refuse(
                "is not a suggestion: it names no note it would replace, or is no longer a candidate"
                    .to_string(),
            ));
        }
        let [target] = view.note.supersedes.as_slice() else {
            return Err(refuse(format!(
                "names {} notes it would replace, and a suggestion replaces one",
                view.note.supersedes.len()
            )));
        };
        if view.derived.origin != Origin::ExtractedUser {
            return Err(refuse(match view.derived.origin {
                Origin::ExtractedOther => {
                    "was inferred from the conversation, not learned from the user's own words"
                        .to_string()
                }
                other => format!(
                    "is not from the user's own words as learning wrote it (its origin is `{other}`)"
                ),
            }));
        }
        let Some(said_on) = self.newest_user_row_date(&view.note)? else {
            return Err(refuse("cites no message the user typed".to_string()));
        };
        Ok(InChatSuggestion {
            target: *target,
            said_on,
        })
    }

    /// The card's text for `memory_confirm` (design 09 §4.4): the suggestion
    /// and the note it replaces **at the revisions the model named**, both
    /// read here, at most [`PREVIEW_MAX_BYTES`]. A moved suggestion or target
    /// is `revision_conflict`, a gone one `not_found`, a target no longer in
    /// force `invalid_input`, and anything but a suggestion from the user's
    /// words `not_a_suggestion` — each before any card is raised.
    pub fn suggestion_preview(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
        expected_revision: &str,
        target_revision: &str,
    ) -> Result<String, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        check_revisions(expected_revision, target_revision)?;
        let resolved = self.resolve(ctx)?;
        let Some((holder, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        let suggestion = self.in_chat_suggestion(&view)?;
        if view.revision != expected_revision {
            return Err(MemoryError::RevisionConflict {
                id,
                current_revision: view.revision,
            });
        }
        let loaded = holder.load()?;
        let target = loaded
            .get(&suggestion.target)
            .ok_or(MemoryError::NotFound(suggestion.target))?;
        if target.revision != target_revision {
            return Err(MemoryError::RevisionConflict {
                id: suggestion.target,
                current_revision: target.revision.clone(),
            });
        }
        if target.derived.effective_status != EffectiveStatus::Active {
            return Err(MemoryError::invalid(format!(
                "note {}, which note {id} would replace, is no longer active (it is {}): there is nothing left for this suggestion to replace",
                suggestion.target, target.derived.effective_status
            )));
        }
        Ok(render_preview(target, &view.note, &suggestion.said_on))
    }

    /// Whether `id` is a suggestion from the user's own words — what
    /// `memory_update action: confirm`'s refusal names instead of the
    /// Memory view (design 09 §4.1). Reads; records nothing.
    pub fn is_in_chat_suggestion(&self, ctx: &MemoryAccessContext, id: NoteId) -> bool {
        let Ok(resolved) = self.resolve(ctx) else {
            return false;
        };
        matches!(
            Self::find_note(&resolved, &id),
            Ok(Some((_, view))) if self.in_chat_suggestion(&view).is_ok()
        )
    }

    /// `memory_confirm` (design 09 §4.2 rule 5): a person approved this call
    /// on its card, so the confirm of this one suggestion is theirs. Requires
    /// a `ModelTool` context and a suggestion from the user's own words;
    /// performs the confirm as a user-directed write — origin `user`,
    /// `memory_changed.surface` `"user"` — bound to the target's revision
    /// (S5), then S6's dismissals. Nothing else this context does changes.
    pub fn confirm_suggestion(
        &self,
        ctx: &MemoryAccessContext,
        approval: &HumanApproval,
        id: NoteId,
        expected_revision: &str,
        target_revision: &str,
    ) -> Result<UpdateOutput, MemoryError> {
        check_revisions(expected_revision, target_revision)?;
        let user = self.in_chat_user_context(ctx, id, expected_revision)?;
        tracing::info!(
            note = %id,
            confirmation = approval.request_id(),
            "memory: a suggestion confirmed on its approval card"
        );
        self.update(
            &user,
            UpdateInput {
                id,
                expected_revision: expected_revision.to_string(),
                action: UpdateAction::Confirm {
                    target_revision: Some(target_revision.to_string()),
                },
                reason: None,
            },
        )
    }

    /// A Deny on `memory_confirm`'s card (design 09 §4.3, owner answer LQ2):
    /// the suggestion is **dismissed durably** — withdrawn exactly as the
    /// Memory view's *Keep mine* withdraws it, so its target is untouched and
    /// whatever else the same message taught stays learned.
    pub fn dismiss_suggestion(
        &self,
        ctx: &MemoryAccessContext,
        denial: &HumanDenial,
        id: NoteId,
        expected_revision: &str,
    ) -> Result<WithdrawOutput, MemoryError> {
        let user = self.in_chat_user_context(ctx, id, expected_revision)?;
        let resolved = self.resolve(&user)?;
        let Some((holder, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        tracing::info!(
            note = %id,
            confirmation = denial.request_id(),
            "memory: a suggestion dismissed on its approval card"
        );
        self.withdraw(&user, &holder.scope().clone(), &view, expected_revision)
    }

    /// The one user-directed context an in-chat answer writes through: the
    /// tool's owner and project, `UserSurface`, after the checks rule 5
    /// makes on the note as it is now.
    fn in_chat_user_context(
        &self,
        ctx: &MemoryAccessContext,
        id: NoteId,
        expected_revision: &str,
    ) -> Result<MemoryAccessContext, MemoryError> {
        if ctx.surface != Surface::ModelTool {
            return Err(MemoryError::invalid(
                "an in-chat answer comes from a tool call's context; a user-directed surface confirms or undoes directly",
            ));
        }
        Self::require(ctx.capabilities.write, "memory_write")?;
        if !files::is_revision(expected_revision) {
            return Err(MemoryError::invalid(
                "expected_revision is not a 64-character hex revision",
            ));
        }
        let resolved = self.resolve(ctx)?;
        let Some((_, view)) = Self::find_note(&resolved, &id)? else {
            return Err(MemoryError::NotFound(id));
        };
        self.in_chat_suggestion(&view)?;
        if view.revision != expected_revision {
            return Err(MemoryError::RevisionConflict {
                id,
                current_revision: view.revision,
            });
        }
        Ok(MemoryAccessContext {
            surface: Surface::UserSurface,
            capabilities: Capabilities::ALL,
            ..ctx.clone()
        })
    }

    /// Records that `id` was raised on `lane_key`'s card (design 09 §4.5):
    /// it is not listed in that lane's `<memory_suggestions>` again until
    /// the daemon restarts, whatever the answer was.
    pub fn mark_raised(&self, lane_key: &str, id: NoteId) {
        self.raised
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(lane_key.to_string())
            .or_default()
            .insert(id);
    }

    /// The suggestions raised on `lane_key` since the daemon started.
    pub fn raised_on(&self, lane_key: &str) -> std::collections::HashSet<NoteId> {
        self.raised
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(lane_key)
            .cloned()
            .unwrap_or_default()
    }
}

fn check_revisions(expected_revision: &str, target_revision: &str) -> Result<(), MemoryError> {
    if !files::is_revision(expected_revision) {
        return Err(MemoryError::invalid(
            "expected_revision is not a 64-character hex revision",
        ));
    }
    if !files::is_revision(target_revision) {
        return Err(MemoryError::invalid(
            "target_revision is not a 64-character hex revision",
        ));
    }
    Ok(())
}

/// `text` on one line — runs of whitespace, newlines included, as one space
/// — cut to `max` bytes with an ellipsis when it was longer.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match cut_bytes(&flat, max) {
        (cut, true) => format!("{}…", cut.trim_end()),
        (whole, false) => whole,
    }
}

/// The preview of design 09 §4.4, from the notes as read: who wrote the
/// text being replaced decides how it is named — the owner's own (saved,
/// edited by hand, imported) is "yours", anything learning wrote is "the
/// current note".
fn render_preview(target: &NoteView, suggestion: &Note, said_on: &str) -> String {
    let (named, label, kept) = match target.derived.origin {
        Origin::User => ("your saved note", "Yours", "Your note is kept"),
        Origin::External => ("your note (edited by hand)", "Yours", "Your note is kept"),
        Origin::Imported => ("your imported note", "Yours", "Your note is kept"),
        _ => ("the note", "Current", "The current note is kept"),
    };
    let reason = super::suggestions::replace_reason(&suggestion.body)
        .map(|r| one_line(&r, PREVIEW_REASON_BYTES));
    let when = format!("from what you said on {said_on}");
    let why = match reason {
        Some(reason) => format!("Why: {reason} ({when})"),
        None => format!("{}{}.", when[..1].to_uppercase(), &when[1..]),
    };
    let text = format!(
        "Replace {named} “{}” with “{}”?\n{label}: {}\nNew: {}\n{why}\n{kept} as history in the Memory view.",
        one_line(&target.note.title, PREVIEW_TITLE_BYTES),
        one_line(&suggestion.title, PREVIEW_TITLE_BYTES),
        one_line(&knowledge_text(&target.note.body), PREVIEW_TEXT_BYTES),
        one_line(&knowledge_text(&suggestion.body), PREVIEW_TEXT_BYTES),
    );
    cut_bytes(&text, PREVIEW_MAX_BYTES).0
}
