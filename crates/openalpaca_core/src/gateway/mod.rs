pub mod persistence;
mod router;

pub use router::{
    ContinuedFrom, DelegationInfo, Gateway, GatewayRequest, GatewayResponse, HandleRequest,
    HandleResult, MessageHandler, ResolvedAttachment, SkippedAttachment, TurnContinuity,
};

/// One `memory_used` entry (design §7a): a memory note the turn delivered —
/// id, the revision read, and `pinned` | `recall` | `tool`. Travels
/// `HandleResult` → `GatewayResponse` → the SSE `done` frame like
/// `attachments_skipped`, and is persisted nowhere.
pub use crate::memory::service::MemoryUsed;
