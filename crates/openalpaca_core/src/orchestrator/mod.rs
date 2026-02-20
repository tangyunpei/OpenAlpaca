//! Orchestrator module: the central message handler.
//!
//! Routes user messages through intent classification, skill matching,
//! and task dispatch pipelines.

pub mod dispatcher;
pub mod intent;
pub mod replanner;
pub mod skill_catalog;
pub mod skill_matcher;
pub mod task_planner;
pub mod task_state;

mod bootstrap;
mod context_builder;
mod extraction;
mod handlers;
mod memory_ops;
mod query_handler;
mod skill_handler;
mod summary;
mod task_ops;

#[cfg(test)]
mod tests;

use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntry};
use crate::daemon_config::DaemonConfig;
use crate::lane::LaneManager;
use crate::middleware::bootstrap::BootstrapDocument;
use crate::middleware::identity::IdentityDocument;
use crate::middleware::prompt::SystemPersona;
use crate::middleware::user::UserDocument;
use crate::runner::LoopConfig;
use crate::security::gate::SecurityGate;
use crate::security::policy::Principal;
use crate::tools::ToolRegistry;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use openalpaca_llm::{ChatMessage, LlmRouter, Role};
use openalpaca_storage::{Database, Task};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

use dispatcher::TaskDispatcher;
use intent::IntentParser;

/// Metadata from an LLM call, stored by query/skill handlers and
/// read by the bridge to propagate into `HandleResult`.
///
/// This avoids threading metadata through all internal `Result<String, String>`
/// return paths — the orchestrator keeps returning `Result<String, String>`
/// internally, and metadata flows through this side-channel keyed by request_id
/// to avoid races between concurrent requests.
pub struct LlmMetadata {
    pub model: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
}

/// The Orchestrator: unified message handler for all user interactions.
///
/// Intent-based routing:
/// - SimpleQuery → LLM call (or echo stub if no LLM configured)
/// - TaskQuery → query task registry
/// - ComplexTask → dispatch to agents via TaskDispatcher
/// - TaskControl → manage task lifecycle
pub struct Orchestrator {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub bus: EventBus,
    pub system_persona: Arc<RwLock<SystemPersona>>,
    pub user_document: Arc<RwLock<Option<UserDocument>>>,
    pub identity_document: Arc<RwLock<Option<IdentityDocument>>>,
    pub llm_router: Option<Arc<LlmRouter>>,
    pub loop_config: LoopConfig,
    pub security_gate: Arc<SecurityGate>,
    pub tool_registry: Arc<ToolRegistry>,
    intent_parser: IntentParser,
    task_dispatcher: TaskDispatcher,
    db: Option<Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    /// Per-lane turn counter for extraction frequency gating.
    extraction_turn_counter: Mutex<HashMap<String, usize>>,
    /// Path to USER.md for writing extraction results. Set via `set_user_path()`.
    user_path: RwLock<Option<std::path::PathBuf>>,
    /// Path to IDENTITY.md for writing identity updates. Set via `set_identity_path()`.
    identity_path: RwLock<Option<std::path::PathBuf>>,
    /// Skill catalog for progressive skill loading and invocation.
    pub skill_catalog: Arc<skill_catalog::SkillCatalog>,
    /// Bootstrap document — `Some` = first-run onboarding active, `None` = normal operation.
    pub bootstrap_document: Arc<RwLock<Option<BootstrapDocument>>>,
    /// Path to BOOTSTRAP.md on disk (for deletion on completion).
    bootstrap_path: RwLock<Option<std::path::PathBuf>>,
    /// Daemon-level config (memory limits, costs, execution defaults, etc.).
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
    /// Atomic guard to prevent concurrent bootstrap completion (race condition fix).
    bootstrap_completing: AtomicBool,
    /// Per-request LLM metadata from query/skill handlers → bridge.
    /// Keyed by request_id to avoid races between concurrent requests.
    /// Populated after LLM response, removed by bridge after reading.
    pub llm_metadata_map: DashMap<Uuid, LlmMetadata>,
}

/// Full conversation context for prompt building and summary update.
pub(super) struct ConversationContext {
    pub(super) summary: Option<String>,
    pub(super) recent_messages: Vec<ChatMessage>,
    /// Raw (id, role, content) tuples for the "older" window — used by maybe_update_summary().
    pub(super) older_window: Vec<(i64, String, String)>,
    /// Current summary version from conversations table (for optimistic locking in update).
    pub(super) summary_version: i64,
    /// Last message ID that was summarized.
    pub(super) last_summarized_id: i64,
    /// Previous summary text (for incremental update).
    pub(super) old_summary_text: String,
}

pub(super) fn role_label(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        system_persona: SystemPersona,
        llm_router: Option<Arc<LlmRouter>>,
        loop_config: LoopConfig,
        security_gate: Arc<SecurityGate>,
        tool_registry: Arc<ToolRegistry>,
        db: Option<Database>,
        embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
        skill_catalog: Arc<skill_catalog::SkillCatalog>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        let task_dispatcher = TaskDispatcher::new(
            shared_context.clone(),
            lane_manager.clone(),
            bus.clone(),
            llm_router.clone(),
            security_gate.clone(),
            tool_registry.clone(),
            db.clone(),
            embedder.clone(),
            daemon_config.clone(),
        );
        Self {
            shared_context,
            lane_manager,
            bus,
            system_persona: Arc::new(RwLock::new(system_persona)),
            user_document: Arc::new(RwLock::new(None)),
            identity_document: Arc::new(RwLock::new(None)),
            llm_router,
            loop_config,
            security_gate,
            tool_registry,
            intent_parser: IntentParser,
            task_dispatcher,
            db,
            embedder,
            extraction_turn_counter: Mutex::new(HashMap::new()),
            user_path: RwLock::new(None),
            identity_path: RwLock::new(None),
            skill_catalog,
            bootstrap_document: Arc::new(RwLock::new(None)),
            bootstrap_path: RwLock::new(None),
            daemon_config,
            bootstrap_completing: AtomicBool::new(false),
            llm_metadata_map: DashMap::new(),
        }
    }

    /// Set the path to USER.md for extraction writes.
    pub fn set_user_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.user_path.write() {
            *guard = Some(path);
        }
    }

    /// Replace the active user document (from USER.md reload or bootstrap).
    pub fn update_user_document(&self, doc: Option<UserDocument>) {
        match self.user_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("User document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    pub fn update_system_persona(&self, persona: SystemPersona) {
        match self.system_persona.write() {
            Ok(mut guard) => {
                *guard = persona;
            }
            Err(poisoned) => {
                tracing::warn!("System persona lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = persona;
            }
        }
    }

    /// Replace the active identity document (from IDENTITY.md reload or bootstrap).
    ///
    /// If the identity has a non-empty name, also updates `system_persona.name`
    /// so that `PromptAssembler::assemble()` uses the chosen name.
    pub fn update_identity_document(&self, doc: Option<IdentityDocument>) {
        // Update system persona name if identity provides one
        if let Some(ref identity) = doc
            && !identity.name.is_empty()
        {
            match self.system_persona.write() {
                Ok(mut guard) => {
                    guard.name = identity.name.clone();
                }
                Err(poisoned) => {
                    tracing::warn!(
                        "System persona lock poisoned during identity name update; recovering"
                    );
                    let mut guard = poisoned.into_inner();
                    guard.name = identity.name.clone();
                }
            }
        }

        match self.identity_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("Identity document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    /// Set the path to IDENTITY.md for writes.
    pub fn set_identity_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.identity_path.write() {
            *guard = Some(path);
        }
    }

    /// Replace the active bootstrap document (from BOOTSTRAP.md load or deletion).
    pub fn update_bootstrap_document(&self, doc: Option<BootstrapDocument>) {
        match self.bootstrap_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("Bootstrap document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    /// Set the path to BOOTSTRAP.md for deletion on completion.
    pub fn set_bootstrap_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.bootstrap_path.write() {
            *guard = Some(path);
        }
    }

    /// Check if bootstrap mode is active.
    pub fn is_bootstrapping(&self) -> bool {
        self.bootstrap_document
            .read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

pub(super) fn principal_id(principal: &Principal) -> String {
    match principal {
        Principal::System => "system".to_string(),
        Principal::User { global_id } => global_id.clone(),
        Principal::External { provider, id } => format!("{}:{}", provider, id),
    }
}

pub(super) fn task_entry_to_json(entry: &TaskEntry) -> String {
    let mut obj = serde_json::json!({
        "task_id": entry.task_id,
        "title": entry.title,
        "status": entry.status.as_str(),
        "progress_current": entry.progress_current,
        "progress_total": entry.progress_total,
        "created_at": entry.created_at.to_rfc3339(),
        "updated_at": entry.updated_at.to_rfc3339(),
    });
    if let Some(ref dag) = entry.dag_summary {
        obj.as_object_mut().unwrap().insert(
            "dag_summary".to_string(),
            serde_json::json!({
                "total_nodes": dag.total_nodes,
                "completed_nodes": dag.completed_nodes,
                "running_nodes": dag.running_nodes,
                "failed_nodes": dag.failed_nodes,
            }),
        );
    }
    obj.to_string()
}

pub(super) fn db_task_to_json(task: &Task) -> String {
    let mut obj = serde_json::json!({
        "task_id": task.id,
        "title": task.title,
        "status": task.status.as_str(),
        "progress_current": task.progress_current,
        "progress_total": task.progress_total,
        "result_summary": task.result_summary,
        "created_at": task.created_at.to_rfc3339(),
        "updated_at": task.updated_at.to_rfc3339(),
    });

    // Parse state_json to extract DAG node details if available
    if let Some(ref sj) = task.state_json
        && let Ok(state) = serde_json::from_str::<task_state::TaskState>(sj)
        && let Some(ref dag) = state.dag
    {
        let nodes_summary: Vec<serde_json::Value> = dag
            .nodes
            .iter()
            .map(|n| {
                serde_json::json!({
                    "node_id": n.node_id,
                    "title": n.title,
                    "agent_id": n.agent_id,
                    "status": n.status,
                })
            })
            .collect();
        obj.as_object_mut()
            .unwrap()
            .insert("dag_nodes".to_string(), serde_json::json!(nodes_summary));
    }

    obj.to_string()
}
