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

use super::recall::{Verified, verify};
use super::{
    Capabilities, MemoryAccessContext, MemoryError, MemoryService, Surface, UpdateAction,
    UpdateInput, UpdateOutput, WithdrawOutput, cut_bytes,
};
use crate::tools::registry::{HumanApproval, HumanDenial};
use openalpaca_storage::ConversationRepository;
use openalpaca_storage::memory_files::{
    self as files, EffectiveStatus, Note, NoteId, NoteStatus, NoteView, Origin, Partition,
    PartitionHealth, SourceType, knowledge_text,
};
use std::collections::HashSet;

/// The most the card's preview carries (design 09 §4.4).
pub const PREVIEW_MAX_BYTES: usize = 1024;

const PREVIEW_TITLE_BYTES: usize = 80;
const PREVIEW_TEXT_BYTES: usize = 260;
const PREVIEW_REASON_BYTES: usize = 160;

/// At most this many suggestions are re-read for one `<memory_suggestions>`
/// block (design 09 §4.5).
pub const PENDING_SUGGESTIONS_READ_MAX: usize = 10;

/// The card's text for one `memory_confirm` call and the note it replaces
/// (design 09 §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionPreview {
    pub text: String,
    pub target: NoteId,
}

/// One suggestion a turn offering `memory_confirm` is told about (design 09
/// §4.5): what the tool needs — both ids and revisions — and what the line
/// says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSuggestion {
    pub id: NoteId,
    pub revision: String,
    pub title: String,
    /// The note it would replace, at its current revision.
    pub target: NoteId,
    pub target_revision: String,
    pub target_title: String,
    pub target_origin: Origin,
    /// `YYYY-MM-DD` of the newest row the user typed that it cites.
    pub said_on: String,
}

impl PendingSuggestion {
    /// The line `<memory_suggestions>` carries (design 09 §4.5): the ids and
    /// revisions `memory_confirm` takes, then both titles and the date,
    /// titles cut to [`SUGGESTION_LINE_TITLE_BYTES`]. Data, not an
    /// instruction — when to call is in the tool's description.
    pub fn render(&self) -> String {
        let who = match self.target_origin {
            Origin::User => "saved by you",
            Origin::External => "edited by you",
            Origin::Imported => "imported by you",
            _ => "learned earlier",
        };
        format!(
            "[memory-suggestion id={} revision={} replaces={} replaces_revision={}] “{}” ({who}) → “{}” (from what you said on {})\n",
            self.id,
            self.revision,
            self.target,
            self.target_revision,
            one_line(&self.target_title, SUGGESTION_LINE_TITLE_BYTES),
            one_line(&self.title, SUGGESTION_LINE_TITLE_BYTES),
            self.said_on,
        )
    }
}

/// A title on a `<memory_suggestions>` line keeps at most this much.
pub const SUGGESTION_LINE_TITLE_BYTES: usize = 60;

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
    ) -> Result<SuggestionPreview, MemoryError> {
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
        Ok(SuggestionPreview {
            text: render_preview(target, &view.note, &suggestion.said_on),
            target: suggestion.target,
        })
    }

    /// The pending suggestions a turn that offers `memory_confirm` is told
    /// about (design 09 §4.5): suggestions from the user's own words
    /// (§4.2 rule 5) whose target is still in force, newest first, one per
    /// target, at most `limit`, leaving out any whose id or target is in
    /// `skip` — what was already raised on this lane.
    ///
    /// **On the reply path, so it reads the index, not the partitions**
    /// (ruling LR18): the candidates come from the index's `supersedes`
    /// column, and only the listed files — each suggestion and its target —
    /// are re-read and re-hashed, as recall does, at most
    /// [`PENDING_SUGGESTIONS_READ_MAX`] suggestions per call. A partition is
    /// re-parsed only when its stat scan or a re-read finds it changed. Not a
    /// recall: nothing is recorded as used.
    pub fn pending_suggestions(
        &self,
        ctx: &MemoryAccessContext,
        limit: usize,
        skip: &HashSet<NoteId>,
    ) -> Result<Vec<PendingSuggestion>, MemoryError> {
        Self::require(ctx.capabilities.read, "memory_read")?;
        let resolved = self.resolve(ctx)?;
        let partitions: Vec<&Partition> = resolved
            .all()
            .into_iter()
            .filter(|p| !matches!(p.health(), PartitionHealth::Quarantined(_)))
            .collect();
        for p in &partitions {
            self.touch(p)?;
        }
        let keys: Vec<String> = partitions
            .iter()
            .map(|p| p.partition_key().to_string())
            .collect();
        let mut out: Vec<PendingSuggestion> = Vec::new();
        let mut dirty: HashSet<String> = HashSet::new();
        let mut seen_targets: HashSet<(String, String)> = HashSet::new();
        let mut read = 0usize;
        for row in self.index.suggestion_rows(&keys)? {
            if out.len() >= limit || read >= PENDING_SUGGESTIONS_READ_MAX {
                break;
            }
            // Rule 5's first cut, from the index alone: an inferred
            // suggestion is never listed here.
            if row.origin != Origin::ExtractedUser.as_str() {
                continue;
            }
            let [target] = row.supersedes.as_slice() else {
                continue;
            };
            let (Ok(id), Ok(target_id)) = (NoteId::parse(&row.id), NoteId::parse(target)) else {
                continue;
            };
            // Newest first, so the first row per target is the one listed;
            // a target already raised keeps its older restatements out too.
            if !seen_targets.insert((row.partition_key.clone(), target.clone()))
                || skip.contains(&id)
                || skip.contains(&target_id)
            {
                continue;
            }
            let Some(p) = partitions
                .iter()
                .copied()
                .find(|p| p.partition_key() == row.partition_key)
            else {
                continue;
            };
            read += 1;
            let Some(target) = super::suggestions::verified_target(self, p, target, &mut dirty)?
            else {
                continue;
            };
            let (note, revision) = match verify(p, &row.relative_path, id, &row.revision) {
                Verified::Fresh {
                    note,
                    revision,
                    stale,
                    ..
                } => {
                    if stale {
                        // Changed behind the service's back: `external`, not
                        // the user's words as learning wrote them.
                        dirty.insert(p.partition_key().to_string());
                        continue;
                    }
                    (*note, revision)
                }
                Verified::Gone => {
                    dirty.insert(p.partition_key().to_string());
                    continue;
                }
            };
            if note.status != NoteStatus::Candidate || note.supersedes != [target.id] {
                continue;
            }
            let Some(said_on) = self.newest_user_row_date(&note)? else {
                continue;
            };
            out.push(PendingSuggestion {
                id,
                revision,
                title: note.title,
                target: target.id,
                target_revision: target.revision,
                target_title: target.title,
                target_origin: target.origin,
                said_on,
            });
        }
        for key in dirty {
            if let Some(p) = partitions.iter().find(|p| p.partition_key() == key) {
                self.reindex(p, true)?;
            }
        }
        Ok(out)
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

    /// Records that a card was raised on `lane_key` for suggestion `id`,
    /// which would replace `target` (design 09 §4.5): neither it nor another
    /// suggestion for the same note is listed in that lane's
    /// `<memory_suggestions>` again until the daemon restarts, whatever the
    /// answer was.
    pub fn mark_raised(&self, lane_key: &str, id: NoteId, target: NoteId) {
        self.raised
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(lane_key.to_string())
            .or_default()
            .extend([id, target]);
    }

    /// The suggestions raised on `lane_key` since the daemon started, and the
    /// notes they would replace — what [`Self::pending_suggestions`] skips.
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

/// `text` on one line — runs of whitespace as one space, and none of the
/// characters an approval's escape shows ([`crate::utils::text::one_line`]:
/// the CLI prints this raw at a terminal, where an ESC sequence could rewrite
/// what the person approves; a bidi override reorders it anywhere; and a
/// variation selector, U+2800 or U+FFFC draws as nothing or as blank space a
/// run of which wraps) — cut to `max` bytes with an ellipsis when it was
/// longer. Dropped, not escaped: the preview is OpenAlpaca's prose, a
/// summary cut to a budget for reading, and the note's exact text, which
/// the person may want to check, is in the Memory view and
/// `openalpaca memory show`.
fn one_line(text: &str, max: usize) -> String {
    let flat = crate::utils::text::one_line(text);
    match cut_bytes(&flat, max) {
        (cut, true) => format!("{}…", cut.trim_end()),
        (whole, false) => whole,
    }
}

/// The preview of design 09 §4.4, from the notes as read: who wrote the
/// text being replaced decides how it is named — the owner's own (saved,
/// edited by hand, imported) is "yours", anything learning wrote is "the
/// current note".
///
/// Its fields drop every character an approval's escape would show
/// ([`one_line`]), and when any was dropped one last line says how many and
/// where they are shown ([`left_out_line`]): `memory_confirm`'s arguments
/// are three ids, so this preview is the only text on its card, and a drop
/// nobody is told about is the danger the escape exists for. No cut reaches
/// that line.
fn render_preview(target: &NoteView, suggestion: &Note, said_on: &str) -> String {
    let (named, label, kept) = match target.derived.origin {
        Origin::User => ("your saved note", "Yours", "Your note is kept"),
        Origin::External => ("your note (edited by hand)", "Yours", "Your note is kept"),
        Origin::Imported => ("your imported note", "Yours", "Your note is kept"),
        _ => ("the note", "Current", "The current note is kept"),
    };
    let raw_reason = super::suggestions::replace_reason(&suggestion.body);
    let target_text = knowledge_text(&target.note.body);
    let suggested_text = knowledge_text(&suggestion.body);
    let left_out = left_out_line(
        [
            target.note.title.as_str(),
            suggestion.title.as_str(),
            raw_reason.as_deref().unwrap_or(""),
        ],
        [target_text.as_str(), suggested_text.as_str()],
    );
    let reason = raw_reason.map(|r| one_line(&r, PREVIEW_REASON_BYTES));
    let when = format!("from what you said on {said_on}");
    let why = match reason {
        Some(reason) => format!("Why: {reason} ({when})"),
        None => format!("{}{}.", when[..1].to_uppercase(), &when[1..]),
    };
    let text = format!(
        "Replace {named} “{}” with “{}”?\n{label}: {}\nNew: {}\n{why}\n{kept} as history in the Memory view.",
        one_line(&target.note.title, PREVIEW_TITLE_BYTES),
        one_line(&suggestion.title, PREVIEW_TITLE_BYTES),
        one_line(&target_text, PREVIEW_TEXT_BYTES),
        one_line(&suggested_text, PREVIEW_TEXT_BYTES),
    );
    match left_out {
        None => cut_bytes(&text, PREVIEW_MAX_BYTES).0,
        Some(line) => {
            let (kept, _) = cut_bytes(&text, PREVIEW_MAX_BYTES - line.len() - 1);
            format!("{kept}\n{line}")
        }
    }
}

/// The preview's last line when its fields dropped anything an approval's
/// escape shows, or `None`: how many characters, counted as the Memory
/// view's Suggestions card counts the same five fields — the titles and the
/// reason with the one-line escape, the two texts with the arguments' — in
/// the whole of each, not only the part a byte budget kept. That card shows
/// each of them as `<U+XXXX>`. The counterpart of an approval's notice for
/// a text that drops: it keeps R2's drop (spec item 4) and tells the person
/// what the drop left out and where it is (A6, A8).
fn left_out_line(lines: [&str; 3], texts: [&str; 2]) -> Option<String> {
    use crate::utils::text::{escape_hidden, escape_hidden_line};
    let hidden = lines
        .iter()
        .map(|t| escape_hidden_line(t).hidden)
        .chain(texts.iter().map(|t| escape_hidden(t).hidden))
        .sum::<usize>();
    match hidden {
        0 => None,
        1 => Some(
            "This preview leaves out 1 hidden character; the Memory view shows it as <U+…>."
                .to_string(),
        ),
        n => Some(format!(
            "This preview leaves out {n} hidden characters; the Memory view shows them as <U+…>."
        )),
    }
}
