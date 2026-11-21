mod common;
mod identity;
mod soul;
mod user;
mod writer;

pub use writer::{UserDocumentRead, UserDocumentWriter, UserDocumentWritten};

use crate::bus::EventBus;
use crate::tools::builtins::annotations_for_builtin;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::hash::sha256_hex;
use std::path::{Path, PathBuf};

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

/// The persona document an update targets, a closed set of three; each
/// variant dispatches the common update lifecycle to its own handler.
enum PersonaHandler<'a> {
    Soul(&'a soul::SoulHandler),
    User(&'a user::UserHandler),
    Identity(&'a identity::IdentityHandler),
}

impl PersonaHandler<'_> {
    fn target_name(&self) -> &'static str {
        match self {
            Self::Soul(h) => h.target_name(),
            Self::User(h) => h.target_name(),
            Self::Identity(h) => h.target_name(),
        }
    }

    fn document_path(&self) -> &Path {
        match self {
            Self::Soul(h) => h.document_path(),
            Self::User(h) => h.document_path(),
            Self::Identity(h) => h.document_path(),
        }
    }

    fn backup_dir(&self) -> &Path {
        match self {
            Self::Soul(h) => h.backup_dir(),
            Self::User(h) => h.backup_dir(),
            Self::Identity(h) => h.backup_dir(),
        }
    }

    fn backup_prefix(&self) -> &'static str {
        match self {
            Self::Soul(h) => h.backup_prefix(),
            Self::User(h) => h.backup_prefix(),
            Self::Identity(h) => h.backup_prefix(),
        }
    }

    fn max_backups(&self) -> Option<usize> {
        match self {
            Self::Soul(h) => h.max_backups(),
            Self::User(h) => h.max_backups(),
            Self::Identity(h) => h.max_backups(),
        }
    }

    fn tmp_filename(&self) -> &'static str {
        match self {
            Self::Soul(h) => h.tmp_filename(),
            Self::User(h) => h.tmp_filename(),
            Self::Identity(h) => h.tmp_filename(),
        }
    }

    fn validate_replace(&self, content_b64: &str) -> Result<String, String> {
        match self {
            Self::Soul(h) => h.validate_replace(content_b64),
            Self::User(h) => h.validate_replace(content_b64),
            Self::Identity(h) => h.validate_replace(content_b64),
        }
    }

    async fn validate_sections(
        &self,
        sections: &serde_json::Value,
    ) -> Result<(String, Option<Vec<String>>), String> {
        match self {
            Self::Soul(h) => h.validate_sections(sections).await,
            Self::User(h) => h.validate_sections(sections).await,
            Self::Identity(h) => h.validate_sections(sections).await,
        }
    }

    fn publish_event(
        &self,
        mode: &str,
        hash: &str,
        backup_path: &Option<String>,
        modified_sections: Option<Vec<String>>,
    ) {
        match self {
            Self::Soul(h) => h.publish_event(mode, hash, backup_path, modified_sections),
            Self::User(h) => h.publish_event(mode, hash, backup_path, modified_sections),
            Self::Identity(h) => h.publish_event(mode, hash, backup_path, modified_sections),
        }
    }
}

struct PersonaUpdateTool {
    soul: soul::SoulHandler,
    user: user::UserHandler,
    identity: identity::IdentityHandler,
}

impl BuiltInTool for PersonaUpdateTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // Reject unknown top-level fields
        if let Some(obj) = arguments.as_object() {
            for key in obj.keys() {
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

        let handler = match target {
            "soul" => PersonaHandler::Soul(&self.soul),
            "user" => PersonaHandler::User(&self.user),
            "identity" => PersonaHandler::Identity(&self.identity),
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

        let hash = sha256_hex(validated.as_bytes());
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
                        "description": "Section patches (required for 'sections' mode). Valid fields depend on target. soul: title, summary, core_truths, boundaries, vibe, continuity. user: identity, communication_style, expertise, projects, preferences, notes. identity: name, creature, vibe, emoji, avatar.",
                        "properties": {
                            "title":               {"type": "string", "description": "Document title (soul)"},
                            "summary":             {"type": "string", "description": "Brief summary (soul)"},
                            "core_truths":         {"type": "array", "items": {"type": "string"}, "description": "Core truth statements (soul)"},
                            "boundaries":          {"type": "array", "items": {"type": "string"}, "description": "Boundary rules (soul)"},
                            "vibe":                {"type": "string", "description": "Personality vibe (soul, identity)"},
                            "continuity":          {"type": "array", "items": {"type": "string"}, "description": "Continuity notes (soul)"},
                            "identity":            {"type": "string", "description": "JSON-encoded key-value pairs for user identity fields. Example: {\"name\": \"Alice\", \"role\": \"engineer\"}"},
                            "communication_style": {"type": "string", "description": "Communication style description (user)"},
                            "expertise":           {"type": "string", "description": "Areas of expertise (user)"},
                            "projects":            {"type": "string", "description": "Active projects (user)"},
                            "preferences":         {"type": "string", "description": "User preferences (user)"},
                            "notes":               {"type": "string", "description": "Freeform notes (user)"},
                            "name":                {"type": "string", "description": "Agent name (identity)"},
                            "creature":            {"type": "string", "description": "Creature/avatar type (identity)"},
                            "emoji":               {"type": "string", "description": "Representative emoji (identity)"},
                            "avatar":              {"type": "string", "description": "Avatar description (identity)"}
                        }
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
        backend: ToolBackend::built_in(tool),
        provides_capabilities: vec!["persona_write".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("update_persona"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests;
