//! Multimodal attachment handling for the orchestrator message pipeline.

use super::attachment_adapt::{self, Fate};
use super::handler_helpers::warn_withheld;
use super::{Orchestrator, wrap_untrusted_context};
use crate::gateway::{HandleRequest, ResolvedAttachment, SkippedAttachment};
use base64::Engine as _;
use openalpaca_llm::{ContentPart, ImageSource};
use std::sync::Arc;

/// What an attachment is, for the purpose of deciding its fate.
///
/// A2: this used to be a bare `is_image` boolean, so every non-image file —
/// an audio clip included — was judged by `supports_document`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentKind {
    Image,
    Audio,
    Document,
}

impl AttachmentKind {
    fn of(mime_type: &str) -> Self {
        if mime_type.starts_with("image/") {
            Self::Image
        } else if mime_type.starts_with("audio/") {
            Self::Audio
        } else {
            Self::Document
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Image => attachment_adapt::PLACEHOLDER_IMAGE,
            Self::Audio => attachment_adapt::PLACEHOLDER_AUDIO,
            Self::Document => attachment_adapt::PLACEHOLDER_DOCUMENT,
        }
    }
}

/// The turn's own attachments, on their way down the routing ladder.
///
/// A1: the parts are built once, by the one place that knows each file's id,
/// and every arm of `handle_message_internal` either **sends** them to its
/// model or **records** them skipped. `carried` is exactly the ids whose
/// content is in `files` — an attachment the adaptation already withheld is
/// not in it, because it is already on the turn's skipped list.
///
/// The turn's **question** is kept apart from the files because the two tiers
/// that send them phrase it differently: the main loop sends the message as
/// typed, the skill tier sends the query its intent parser took out of it
/// (`/echo what is this?` → `what is this?`). Appending it here would have
/// handed the skill's model the slash line it never used to see.
pub(in crate::orchestrator) struct TurnAttachments {
    /// The adapted attachment parts, with no question text.
    pub files: Vec<ContentPart>,
    /// The turn's message, as typed.
    pub question: String,
    pub carried: Vec<String>,
}

impl TurnAttachments {
    /// The files followed by `question` — the user message a model-answering
    /// tier sends. An empty question adds nothing (an attachment-only turn).
    pub fn message_parts(&self, question: &str) -> Vec<ContentPart> {
        let mut parts = self.files.clone();
        if !question.trim().is_empty() {
            parts.push(ContentPart::Text {
                text: question.to_string(),
            });
        }
        parts
    }
}

/// A1 — why a turn's attachments did not reach a model: one sentence per path
/// that answers without them. They travel verbatim to the client on
/// `done.attachments_skipped`, so they read as sentences, not as codes, and
/// they live together here because the partition they keep is one rule.
pub(in crate::orchestrator) mod skipped {
    pub(in crate::orchestrator) const TASK_OPS: &str =
        "this turn was answered as a task command, which never reaches a model";
    pub(in crate::orchestrator) const STEER: &str =
        "this turn was a /steer command, injected into a running workflow, not answered";
    pub(in crate::orchestrator) const NO_MODEL_TIER: &str =
        "this turn was answered without reaching a model";
    pub(in crate::orchestrator) const SOCIAL: &str =
        "this turn took the social fast path, whose prompt carries no attachments";
    pub(in crate::orchestrator) const DIRECT_SEND: &str =
        "this turn was a direct send, which is executed without a model";
    pub(in crate::orchestrator) const SKILL_REFUSED: &str =
        "this turn was refused before a model was reached";
    pub(in crate::orchestrator) const PLUGIN_SKILL: &str =
        "this skill comes from a plugin, whose protocol carries a plain query and no files";
}

impl Orchestrator {
    /// A1 — an arm that answers a turn **without** its attachments says so.
    ///
    /// The invariant the bridge then keeps is a partition: every id is in
    /// exactly one of `attachments_used` / `attachments_skipped`, and `used`
    /// means the model's request really carried it. A tier that never reaches
    /// a model at all (task ops, `/steer`, the withdrawn-skill tombstone) and
    /// one that reaches a model with a prompt of its own (the social fast
    /// path, a plugin-contributed skill) are both this case.
    pub(super) fn skip_turn_attachments(
        &self,
        request_id: uuid::Uuid,
        attachments: Option<&TurnAttachments>,
        reason: &str,
    ) {
        let Some(attachments) = attachments else {
            return;
        };
        if attachments.carried.is_empty() {
            return;
        }
        for id in &attachments.carried {
            warn_withheld(Some(id), "-", reason);
        }
        self.attachments_skipped_map
            .entry(request_id)
            .or_default()
            .extend(attachments.carried.iter().map(|id| SkippedAttachment {
                id: id.clone(),
                reason: reason.to_string(),
            }));
    }
}

impl Orchestrator {
    /// Handle a user message with file attachments.
    ///
    /// Injects attachment context as low-trust blocks before delegating to
    /// the standard `handle_message` pipeline.
    ///
    /// U1–U3: the parts are built for the model that will *answer* this turn,
    /// not for the configured default, and every attachment that does not
    /// reach it is logged and recorded for the turn's result. This is the one
    /// place that knows each attachment's file id, so it is the one place that
    /// can name them.
    pub async fn handle_message_with_attachments(
        &self,
        request: HandleRequest,
        attachments: Vec<ResolvedAttachment>,
    ) -> Result<String, String> {
        // The request's own content is the intent source; the augmented string
        // built below is what the model sees.
        let content = &request.content;
        let request_id = request.request_id;
        // U1 — the same resolution `handle_simple_query` will budget and route
        // with: this turn's pin (`LoopConfig.model` after the request's
        // override) walked down L3's ladder. `None` — no router, nothing
        // routable — leaves every part as it is and lets `NoRoutableModel`
        // speak on the next call.
        let pinned = request
            .model_override
            .clone()
            .or_else(|| self.loop_config.model.clone());
        let model = self.answering_model(pinned.as_deref());
        let max_chars = self.attachment_text_cap();
        let mut skipped: Vec<SkippedAttachment> = Vec::new();

        // 1. Build structured ContentParts from attachments
        let mut parts: Vec<ContentPart> = Vec::new();
        // A1 — the ids whose content is actually in `parts`, for the arms that
        // cannot carry them.
        let mut carried: Vec<String> = Vec::new();
        for att in &attachments {
            let kind = AttachmentKind::of(&att.mime_type);
            // With no answering model at all nothing is adapted: the parts go
            // out as they always did and the router reports the real problem.
            let fate = match (&model, kind) {
                (None, _) => Fate::Native,
                (Some(m), AttachmentKind::Image) => attachment_adapt::image_fate(m),
                // A2 — audio is judged by `supports_audio`. This asked
                // `document_fate` about every non-image file.
                (Some(m), AttachmentKind::Audio) => {
                    attachment_adapt::audio_fate(m, att.extracted_text.as_deref(), max_chars)
                }
                (Some(m), AttachmentKind::Document) => {
                    attachment_adapt::document_fate(m, att.extracted_text.as_deref(), max_chars)
                }
            };
            let model_id = model.as_ref().map(|m| m.id.as_str()).unwrap_or("-");
            if !matches!(fate, Fate::Withheld { .. }) {
                carried.push(att.file_id.clone());
            }

            match fate {
                Fate::Withheld { reason } => {
                    warn_withheld(Some(&att.file_id), model_id, reason);
                    skipped.push(SkippedAttachment {
                        id: att.file_id.clone(),
                        reason: reason.to_string(),
                    });
                    parts.push(ContentPart::Text {
                        text: kind.placeholder().to_string(),
                    });
                }
                // U2 — the model takes no native document part, but it reads
                // text, and the upload pipeline already extracted some.
                Fate::AsText { cut_at } => {
                    if let Some(total) = cut_at {
                        tracing::warn!(
                            file_id = %att.file_id,
                            model = %model_id,
                            kept_chars = max_chars,
                            total_chars = total,
                            "Attachment text cut to the extraction cap before the model saw it"
                        );
                    }
                    parts.push(ContentPart::Text {
                        text: attachment_adapt::document_as_text(
                            &att.filename,
                            &att.mime_type,
                            att.extracted_text.as_deref().unwrap_or_default(),
                            max_chars,
                        ),
                    });
                }
                Fate::Native if kind == AttachmentKind::Image => {
                    match tokio::fs::read(&att.storage_path).await {
                        Ok(bytes) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                            parts.push(ContentPart::Image {
                                source: ImageSource::Base64 {
                                    media_type: att.mime_type.clone(),
                                    data: Arc::new(b64),
                                },
                                detail: None,
                            });
                        }
                        Err(e) => {
                            const READ_FAILED: &str =
                                "the file's bytes could not be read from disk";
                            tracing::warn!(
                                file_id = %att.file_id,
                                path = %att.storage_path,
                                "Failed to read image bytes for multimodal input: {e}"
                            );
                            // The fate said Native, so the id went onto
                            // `carried` above; nothing of it reached the parts,
                            // so take it back off (A1's partition).
                            carried.retain(|id| id != &att.file_id);
                            skipped.push(SkippedAttachment {
                                id: att.file_id.clone(),
                                reason: READ_FAILED.to_string(),
                            });
                            parts.push(ContentPart::Text {
                                text: "[image attached — failed to read image bytes]".to_string(),
                            });
                        }
                    }
                }
                Fate::Native => {
                    parts.push(ContentPart::Document {
                        file_id: att.file_id.clone(),
                        filename: att.filename.clone(),
                        mime_type: att.mime_type.clone(),
                        extracted_text: att.extracted_text.clone(),
                    });
                    if att.extracted_text.is_none() && !att.mime_type.starts_with("audio/") {
                        parts.push(ContentPart::Text {
                            text: "[document attached — text extraction pending]".to_string(),
                        });
                    }
                }
            }
        }

        // U3(b,c) — the bridge subtracts these ids from `attachments_used` and
        // carries them on the turn's result. Written only when something was
        // actually withheld, and removed by the bridge on the way out.
        if !skipped.is_empty() {
            self.attachments_skipped_map.insert(request_id, skipped);
        }

        // 2. Build text-only augmented string for intent classification
        //    (the intent parser only understands text)
        let mut augmented = String::new();
        for att in &attachments {
            let ctx_block = if let Some(ref text) = att.extracted_text {
                let truncated = text.chars().take(4000).collect::<String>();
                format!(
                    "[File: {} ({})]\n{}",
                    att.filename, att.mime_type, truncated
                )
            } else if att.mime_type.starts_with("image/") || att.mime_type.starts_with("audio/") {
                format!("[File: {} ({})]", att.filename, att.mime_type)
            } else {
                format!(
                    "[File: {} ({})]\n[document attached — text extraction pending]",
                    att.filename, att.mime_type
                )
            };
            let wrapped = wrap_untrusted_context(&ctx_block, "file_attachment", "user_derived");
            augmented.push_str(&wrapped);
            augmented.push('\n');
        }
        augmented.push_str(content);

        let force_simple_query = content.trim().is_empty() && !attachments.is_empty();
        let question = content.clone();

        // 3. Pass BOTH the text augmented string AND the structured parts,
        //    with the ids riding in them (A1): an arm that answers without
        //    the parts records exactly those as skipped.
        self.handle_message_internal(
            request,
            augmented,
            force_simple_query,
            Some(TurnAttachments {
                files: parts,
                question,
                carried,
            }),
        )
        .await
    }
}
