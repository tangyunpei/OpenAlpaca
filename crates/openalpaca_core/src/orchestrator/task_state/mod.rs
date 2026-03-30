//! Working memory for tasks: structured state persisted as JSON in task.state_json.

pub(crate) mod outcome;
mod state;
mod workspace;

#[cfg(test)]
mod tests;

pub use outcome::{ArtifactPointer, TaskOutcome};
pub use state::{StepState, TaskConstraints, TaskState};
pub use workspace::{TaskWorkspace, WorkspaceEntry, WorkspaceEntryType};
