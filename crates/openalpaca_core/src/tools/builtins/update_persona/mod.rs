mod common;
mod identity;
mod soul;
mod user;

use crate::bus::EventBus;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Runtime context for all persona document tools.
#[derive(Clone)]
pub struct PersonaToolContext {
    pub soul_path: PathBuf,
    pub user_path: PathBuf,
    pub identity_path: PathBuf,
    pub backup_dir: PathBuf,
    pub bus: EventBus,
    pub max_backups: Option<usize>,
}

/// Known top-level fields in the update_persona tool arguments.
const KNOWN_TOP_LEVEL_FIELDS: &[&str] = &["target", "mode", "content_b64", "sections"];

/// Common lifecycle for all persona document updates.
#[async_trait]
trait PersonaHandler: Send + Sync {
    fn target_name(&self) -> &'static str;
    fn document_path(&self) -> &Path;
    fn backup_dir(&self) -> &Path;
    fn backup_prefix(&self) -> &'static str;
    fn max_backups(&self) -> Option<usize>;
    fn tmp_filename(&self) -> &'static str;
    fn validate_replace(&self, content_b64: &str) -> Result<String, String>;
    async fn validate_sections(
        &self,
        sections: &serde_json::Value,
    ) -> Result<(String, Option<Vec<String>>), String>;
    fn publish_event(
        &self,
        mode: &str,
        hash: &str,
        backup_path: &Option<String>,
        modified_sections: Option<Vec<String>>,
    );
}

struct PersonaUpdateTool {
    soul: soul::SoulHandler,
    user: user::UserHandler,
    identity: identity::IdentityHandler,
}

#[async_trait]
impl BuiltInTool for PersonaUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
                if key == "owner_id" {
                    continue;
                }
                if !KNOWN_TOP_LEVEL_FIELDS.contains(&key.as_str()) {
                    return Err(format!(
                        "Unknown field '{}'. Allowed fields: {}",
                        key,
                        KNOWN_TOP_LEVEL_FIELDS.join(", ")
                    ));
                }
            }
        }

        let target = arguments
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: target (\"soul\", \"user\", or \"identity\")")?;

        let handler: &dyn PersonaHandler = match target {
            "soul" => &self.soul,
            "user" => &self.user,
            "identity" => &self.identity,
            other => {
                return Err(format!(
                    "Invalid target '{}'. Must be \"soul\", \"user\", or \"identity\".",
                    other
                ))
            }
        };

        let mode = arguments
            .get("mode")
            .and_then(|v| v.as_str())
            .ok_or("Missing required parameter: mode (\"replace\" or \"sections\")")?;

        let (validated, modified_sections) = match mode {
            "replace" => {
                let b64 = arguments
                    .get("content_b64")
                    .and_then(|v| v.as_str())
                    .ok_or(
                        "Missing required parameter: content_b64 (required for replace mode)",
                    )?;
                (handler.validate_replace(b64)?, None)
            }
            "sections" => {
                let sections = arguments
                    .get("sections")
                    .ok_or("Missing required parameter: sections (required for sections mode)")?;
                let (content, mods) = handler.validate_sections(sections).await?;
                (content, mods)
            }
            other => {
                return Err(format!(
                    "Invalid mode '{}'. Must be \"replace\" or \"sections\".",
                    other
                ))
            }
        };

        let hash = common::sha256(&validated);
        let backup = common::backup_if_exists(
            handler.document_path(),
            handler.backup_dir(),
            handler.backup_prefix(),
        )
        .await?;
        common::prune_backups(
            handler.backup_dir(),
            handler.max_backups(),
            handler.backup_prefix(),
        )
        .await;
        common::atomic_write(handler.document_path(), &validated, handler.tmp_filename()).await?;
        handler.publish_event(mode, &hash, &backup, modified_sections.clone());

        let extra = modified_sections.map(|ms| {
            (
                "modified_sections",
                serde_json::Value::Array(ms.into_iter().map(serde_json::Value::String).collect()),
            )
        });
        Ok(common::result_json(
            handler.target_name(),
            mode,
            handler.document_path(),
            &hash,
            validated.len(),
            backup,
            extra,
        ))
    }
}

pub(super) fn update_persona_tool(ctx: PersonaToolContext) -> RegisteredTool {
    let tool = PersonaUpdateTool {
        soul: soul::SoulHandler {
            soul_path: ctx.soul_path,
            backup_dir: ctx.backup_dir.clone(),
            bus: ctx.bus.clone(),
            max_backups: ctx.max_backups,
        },
        user: user::UserHandler {
            user_path: ctx.user_path,
            backup_dir: ctx.backup_dir.clone(),
            bus: ctx.bus.clone(),
            max_backups: ctx.max_backups,
        },
        identity: identity::IdentityHandler {
            identity_path: ctx.identity_path,
            backup_dir: ctx.backup_dir,
            bus: ctx.bus,
            max_backups: ctx.max_backups,
        },
    };

    RegisteredTool {
        definition: ToolDefinition {
            name: "update_persona".to_string(),
            description: "Safely update one of the three persona documents: SOUL.md (system \
                personality and guidelines), USER.md (user profile and preferences), \
                or IDENTITY.md (the agent's self-identity). Two modes: 'replace' for \
                full document replacement (provide base64-encoded markdown in \
                content_b64), or 'sections' for targeted field patches (provide a \
                sections object with only the fields to update). All updates are \
                validated against the document schema, backed up with timestamps, and \
                written atomically. Returns a JSON object with status, content hash, \
                and backup path."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "enum": ["soul", "user", "identity"],
                        "description": "Which persona document to update: 'soul' (SOUL.md), 'user' (USER.md), or 'identity' (IDENTITY.md)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["replace", "sections"],
                        "description": "Update mode: 'replace' for full content replacement, 'sections' for targeted field patches"
                    },
                    "content_b64": {
                        "type": "string",
                        "description": "Base64-encoded full markdown content (required for 'replace' mode)"
                    },
                    "sections": {
                        "type": "object",
                        "description": "Section patches as a JSON object (required for 'sections' mode). Valid fields depend on target: soul has title/summary/core_truths/boundaries/vibe/continuity; user has identity/communication_style/expertise/projects/preferences/notes; identity has name/creature/vibe/emoji/avatar."
                    }
                },
                "required": ["target", "mode"]
            }),
            strict: Some(true),
            input_examples: Some(vec![
                serde_json::json!({
                    "target": "soul",
                    "mode": "sections",
                    "sections": { "vibe": "calm and thoughtful", "summary": "A reflective assistant" }
                }),
                serde_json::json!({
                    "target": "user",
                    "mode": "sections",
                    "sections": { "preferences": "Prefers concise responses" }
                }),
                serde_json::json!({
                    "target": "identity",
                    "mode": "replace",
                    "content_b64": "LS0tCm5hbWU6IEV4YW1wbGUK..."
                }),
            ]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(tool)),
    }
}

#[cfg(test)]
mod tests;
