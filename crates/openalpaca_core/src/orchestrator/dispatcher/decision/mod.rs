//! Dispatch-decision vocabulary for the tool-path routing record.
//!
//! The planner-era analysis layer (`analyze_plan` and the pipeline/DAG
//! modes) was deleted in Routing V2 Phase 5; what remains is the vocabulary
//! used by `record_tool_dispatch_decision`. Historical DB rows may still
//! carry the retired strings (`dag_parallel`, `sequential_pipeline`,
//! `planner_explicit`, ...) — readers treat the columns as free-form text.

use serde::{Deserialize, Serialize};

/// The execution mode selected by the dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    LeadAgent,
}

impl std::fmt::Display for DispatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeadAgent => write!(f, "lead_agent"),
        }
    }
}

/// Why a particular execution mode was chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    /// The model called the `start_workflow` tool (Routing V2 tool mode).
    ModelToolCall,
}

impl std::fmt::Display for DecisionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelToolCall => write!(f, "model_tool_call"),
        }
    }
}

#[cfg(test)]
mod tests;
