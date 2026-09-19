//! Multimodal attachment handling for the orchestrator message pipeline.

use super::attachment_adapt::{self, Fate};
use super::handler_helpers::warn_withheld;
use super::{Orchestrator, wrap_untrusted_context};
use crate::gateway::{HandleRequest, ResolvedAttachment, SkippedAttachment};
use base64::Engine as _;
use openalpaca_llm::{ContentPart, ImageSource};
use std::sync::Arc;

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
        for att in &attachments {
            let is_image = att.mime_type.starts_with("image/");
            // With no answering model at all nothing is adapted: the parts go
            // out as they always did and the router reports the real problem.
            let fate = match &model {
                None => Fate::Native,
                Some(m) if is_image => attachment_adapt::image_fate(m),
                Some(m) => {
                    attachment_adapt::document_fate(m, att.extracted_text.as_deref(), max_chars)
                }
            };
            let model_id = model.as_ref().map(|m| m.id.as_str()).unwrap_or("-");

            match fate {
                Fate::Withheld { reason } => {
                    warn_withheld(Some(&att.file_id), model_id, reason);
                    skipped.push(SkippedAttachment {
                        id: att.file_id.clone(),
                        reason: reason.to_string(),
                    });
                    parts.push(ContentPart::Text {
                        text: match is_image {
                            true => attachment_adapt::PLACEHOLDER_IMAGE.to_string(),
                            false => attachment_adapt::PLACEHOLDER_DOCUMENT.to_string(),
                        },
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
                Fate::Native if is_image => match tokio::fs::read(&att.storage_path).await {
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
                        const READ_FAILED: &str = "the file's bytes could not be read from disk";
                        tracing::warn!(
                            file_id = %att.file_id,
                            path = %att.storage_path,
                            "Failed to read image bytes for multimodal input: {e}"
                        );
                        skipped.push(SkippedAttachment {
                            id: att.file_id.clone(),
                            reason: READ_FAILED.to_string(),
                        });
                        parts.push(ContentPart::Text {
                            text: "[image attached — failed to read image bytes]".to_string(),
                        });
                    }
                },
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
        // Add text query as final part
        if !content.trim().is_empty() {
            parts.push(ContentPart::Text {
                text: content.clone(),
            });
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

        // 3. Pass BOTH the text augmented string AND the structured parts
        self.handle_message_internal(request, augmented, force_simple_query, Some(parts))
            .await
    }
}
