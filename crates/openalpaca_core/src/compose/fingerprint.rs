//! Deterministic hash helpers for layer fingerprints (spec section Component 1).
//!
//! Phase 1 implementations are intentionally minimal — just enough bits so that
//! layer fingerprints differ when their fields differ. Phase 2 and Phase 3 will
//! expand each helper to fully cover the spec's fingerprint contract.

use std::sync::Arc;

use openalpaca_llm::{ChatMessage, Role, ToolDefinition};

use super::types::{ConnectorSummary, SystemBlock};

/// Maps `Role` to a stable byte tag for fingerprinting.
///
/// `Role` does not implement `Copy` and is not `#[repr(u8)]`, so we hand-roll
/// a canonical mapping here rather than using `as u8`.
fn role_tag(role: &Role) -> u8 {
    match role {
        Role::System => 0,
        Role::User => 1,
        Role::Assistant => 2,
        Role::Tool => 3,
    }
}

/// blake3 over length-prefixed tool names + descriptions + parameters (canonical JSON).
/// Order-sensitive (different order => different hash).
pub fn hash_tool_set(tools: &[ToolDefinition]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(tools.len() as u64).to_le_bytes());
    for tool in tools {
        // Phase 1 stub: hash the name only. Phase 2 expands to description +
        // parameters (canonical JSON) per spec.
        h.update(&(tool.name.len() as u64).to_le_bytes());
        h.update(tool.name.as_bytes());
    }
    h.finalize().into()
}

/// blake3 over a connector-status list. Phase 1 captures count only.
pub fn hash_connector_status(statuses: &[ConnectorSummary]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(statuses.len() as u64).to_le_bytes());
    // Phase 2 expands to per-status id + status + sendable bits.
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
/// Phase 1: captures role tag + content length + content bytes. Phase 3
/// expands to include multimodal `parts` serialization per spec.
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
        }
    }
    h.finalize().into()
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
