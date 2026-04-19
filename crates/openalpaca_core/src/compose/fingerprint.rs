//! Deterministic hash helpers for layer fingerprints (spec section Component 1).
//!
//! Phase 1 implementations are intentionally minimal — just enough bits so that
//! layer fingerprints differ when their fields differ. Phase 2 and Phase 3 will
//! expand each helper to fully cover the spec's fingerprint contract.

use std::sync::Arc;

use openalpaca_llm::{ChatMessage, ContentPart, Role, ToolDefinition};

use super::types::{ConnectorSummary, SystemBlock};

/// Maps `Role` to a stable byte tag for fingerprinting.
///
/// `Role` does not implement `Copy` and is not `#[repr(u8)]`, so we hand-roll
/// a canonical mapping here rather than using `as u8`. Phase 2+ fingerprint
/// helpers MUST use this helper; do not redefine.
pub(crate) fn role_tag(role: &Role) -> u8 {
    match role {
        Role::System => 0,
        Role::User => 1,
        Role::Assistant => 2,
        Role::Tool => 3,
    }
}

/// blake3 over length-prefixed tool names + descriptions + parameters (canonical JSON).
/// Order-sensitive (different order => different hash).
///
/// Per spec section Component 1 Layer-2 fingerprint: the tool-set hash covers
/// name, description, and `parameters` serialized to canonical JSON bytes,
/// with a separator byte between adjacent tools so two tools cannot "bleed"
/// into each other via concatenation collisions.
pub fn hash_tool_set(tools: &[ToolDefinition]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(tools.len() as u64).to_le_bytes());
    for tool in tools {
        h.update(&(tool.name.len() as u64).to_le_bytes());
        h.update(tool.name.as_bytes());
        h.update(&(tool.description.len() as u64).to_le_bytes());
        h.update(tool.description.as_bytes());
        // Serialize `parameters` (serde_json::Value) with serde_json::to_vec.
        // On failure (e.g. a map with non-string keys, which Value disallows
        // by construction), treat as empty bytes — still deterministic.
        let params_bytes = serde_json::to_vec(&tool.parameters).unwrap_or_default();
        h.update(&(params_bytes.len() as u64).to_le_bytes());
        h.update(&params_bytes);
        // Separator byte prevents name+description|params|name... collisions
        // across adjacent tools.
        h.update(&[0xff]);
    }
    h.finalize().into()
}

/// blake3 over a connector-status list. Order-independent: statuses are sorted
/// by `id` before hashing so two callers presenting the same set in a
/// different order produce the same fingerprint.
pub fn hash_connector_status(statuses: &[ConnectorSummary]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    let mut sorted: Vec<&ConnectorSummary> = statuses.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    h.update(&(sorted.len() as u64).to_le_bytes());
    for s in sorted {
        h.update(&(s.id.len() as u64).to_le_bytes());
        h.update(s.id.as_bytes());
        h.update(&(s.status.len() as u64).to_le_bytes());
        h.update(s.status.as_bytes());
        h.update(&[s.sendable as u8]);
    }
    h.finalize().into()
}

/// blake3 over an ordered list of system blocks.
pub fn hash_raw_blocks(blocks: &[SystemBlock]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(blocks.len() as u64).to_le_bytes());
    for b in blocks {
        h.update(&(b.name.len() as u64).to_le_bytes());
        h.update(b.name.as_bytes());
        h.update(&(b.content.len() as u64).to_le_bytes());
        h.update(b.content.as_bytes());
        h.update(&[b.priority as u8]);
    }
    h.finalize().into()
}

/// blake3 over an `Option<Arc<str>>`, using a leading tag byte so that
/// `None` and `Some("")` cannot collide.
pub fn hash_opt_arc_str(opt: &Option<Arc<str>>) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    match opt {
        None => {
            h.update(&[0u8]);
        }
        Some(s) => {
            h.update(&[1u8]);
            h.update(&(s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        }
    }
    h.finalize().into()
}

/// blake3 over an `Option<ChatMessage>` for Layer 4 fingerprinting.
///
/// Phase 3: covers role tag + content + multimodal `parts` so the history
/// fingerprint busts when any of those change (including text-only vs
/// part-populated transitions, which providers render differently).
pub fn hash_opt_msg(opt: &Option<ChatMessage>) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    match opt {
        None => {
            h.update(&[0u8]);
        }
        Some(m) => {
            h.update(&[1u8]);
            h.update(&[role_tag(&m.role)]);
            h.update(&(m.content.len() as u64).to_le_bytes());
            h.update(m.content.as_bytes());
            match &m.parts {
                None => {
                    h.update(&[0u8]);
                }
                Some(parts) => {
                    h.update(&[1u8]);
                    h.update(&(parts.len() as u64).to_le_bytes());
                    for part in parts {
                        hash_content_part(part, &mut h);
                    }
                }
            }
        }
    }
    h.finalize().into()
}

/// Hash a single `ContentPart` into the supplied hasher. All variants of the
/// current `openalpaca_llm::ContentPart` enum are covered; each is tagged with
/// a distinct leading byte so variant changes can never collide across a
/// single hasher invocation.
fn hash_content_part(part: &ContentPart, h: &mut blake3::Hasher) {
    match part {
        ContentPart::Text { text } => {
            h.update(&[0u8]);
            h.update(&(text.len() as u64).to_le_bytes());
            h.update(text.as_bytes());
        }
        ContentPart::Image { source, detail } => {
            h.update(&[1u8]);
            // Use Debug formatting as a deterministic canonical serializer for
            // `ImageSource` — the enum has multiple variants (Base64 / Url /
            // FileAsset) and Debug output discriminates between them reliably.
            let s = format!("{:?}", source);
            h.update(&(s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
            match detail {
                None => h.update(&[0u8]),
                Some(d) => {
                    h.update(&[1u8]);
                    h.update(&(d.len() as u64).to_le_bytes());
                    h.update(d.as_bytes())
                }
            };
        }
        ContentPart::Audio { data, format } => {
            h.update(&[2u8]);
            // Hash the raw audio bytes — they identify the audio.
            h.update(&(data.len() as u64).to_le_bytes());
            h.update(data.as_bytes());
            h.update(&(format.len() as u64).to_le_bytes());
            h.update(format.as_bytes());
        }
        ContentPart::Document {
            file_id,
            filename,
            mime_type,
            extracted_text,
        } => {
            h.update(&[3u8]);
            h.update(&(file_id.len() as u64).to_le_bytes());
            h.update(file_id.as_bytes());
            h.update(&(filename.len() as u64).to_le_bytes());
            h.update(filename.as_bytes());
            h.update(&(mime_type.len() as u64).to_le_bytes());
            h.update(mime_type.as_bytes());
            match extracted_text {
                None => h.update(&[0u8]),
                Some(t) => {
                    h.update(&[1u8]);
                    h.update(&(t.len() as u64).to_le_bytes());
                    h.update(t.as_bytes())
                }
            };
        }
        ContentPart::FileRef {
            file_id,
            filename,
            mime_type,
        } => {
            h.update(&[4u8]);
            h.update(&(file_id.len() as u64).to_le_bytes());
            h.update(file_id.as_bytes());
            h.update(&(filename.len() as u64).to_le_bytes());
            h.update(filename.as_bytes());
            h.update(&(mime_type.len() as u64).to_le_bytes());
            h.update(mime_type.as_bytes());
        }
    }
}

/// Combine the four per-layer fingerprints into a single composite fingerprint.
pub fn combine_composite(
    persona: &[u8; 32],
    static_prompt: &[u8; 32],
    dynamic_context: &[u8; 32],
    history: &[u8; 32],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(persona);
    h.update(static_prompt);
    h.update(dynamic_context);
    h.update(history);
    h.finalize().into()
}
