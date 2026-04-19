//! Layer 3 — Dynamic Context (Phase 1 stub).
//!
//! Real implementation lands in Phase 3 (wraps `ContextManager`-produced
//! `ContextBundle`). This stub returns an empty output with a fingerprint
//! derived from the query hash + path tag + mode tag so engine cache
//! plumbing differentiates cache entries across queries and execution paths.

use super::types::{DynamicContextInput, DynamicContextMode, DynamicContextOutput};
use crate::prompt_ctx::ExecutionPath;

fn mode_tag(mode: DynamicContextMode) -> u8 {
    match mode {
        DynamicContextMode::Default => 0,
        DynamicContextMode::Skip => 1,
    }
}

fn path_tag(path: &ExecutionPath) -> u8 {
    match path {
        ExecutionPath::SimpleQuery => 0,
        ExecutionPath::SocialQuery => 1,
        ExecutionPath::SkillInvocation { .. } => 2,
        ExecutionPath::PipelineStep { .. } => 3,
        ExecutionPath::DagNode { .. } => 4,
        ExecutionPath::LeadAgent => 5,
    }
}

pub fn compute(input: &DynamicContextInput) -> DynamicContextOutput {
    let mut h = blake3::Hasher::new();
    let query_hash = blake3::hash(input.query.as_bytes());
    h.update(query_hash.as_bytes());
    h.update(&[path_tag(&input.path)]);
    h.update(&[mode_tag(input.mode)]);
    DynamicContextOutput {
        context_messages: Vec::new(),
        additional_system_blocks: Vec::new(),
        fingerprint: h.finalize().into(),
    }
}
