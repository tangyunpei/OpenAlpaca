//! Multimodal attachment handling for the orchestrator message pipeline.

use super::{Orchestrator, wrap_untrusted_context};
use crate::gateway::ResolvedAttachment;
use crate::security::policy::{Principal, Scope};
use base64::Engine as _;
use openalpaca_llm::{ContentPart, ImageSource};
use std::sync::Arc;
use uuid::Uuid;

impl Orchestrator {
    /// Handle a user message with file attachments.
    ///
    /// Injects attachment context as low-trust blocks before delegating to
    /// the standard `handle_message` pipeline.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_message_with_attachments(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        attachments: Vec<ResolvedAttachment>,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
        stream_id: Option<String>,
    ) -> Result<String, String> {
        // 1. Build structured ContentParts from attachments
        let mut parts: Vec<ContentPart> = Vec::new();
        for att in &attachments {
            if att.mime_type.starts_with("image/") {
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
                        tracing::warn!(
                            file_id = %att.file_id,
                            path = %att.storage_path,
                            "Failed to read image bytes for multimodal input: {e}"
                        );
                        parts.push(ContentPart::Text {
                            text: "[image attached — failed to read image bytes]".to_string(),
                        });
                    }
                }
            } else {
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
        // Add text query as final part
        if !content.trim().is_empty() {
            parts.push(ContentPart::Text {
                text: content.clone(),
            });
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
        augmented.push_str(&content);

        let force_simple_query = content.trim().is_empty() && !attachments.is_empty();

        // 3. Pass BOTH the text augmented string AND the structured parts
        self.handle_message_internal(
            request_id,
            source,
            augmented,
            content,
            force_simple_query,
            Some(parts),
            principal,
            scope,
            lane_key,
            workspace_path,
            stream_id,
        )
        .await
    }
}
