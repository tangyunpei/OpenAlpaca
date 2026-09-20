use crate::bus::EventBus;
use crate::events::SystemEvent;
use chrono::Utc;
use std::sync::Arc;

/// RAII guard that cleans up an agent instance on drop.
///
/// For **non-singleton** instances: calls `destroy_instance()` to remove
/// the ephemeral instance from the registry entirely.
///
/// For **singleton** instances (like lead_agent): calls `destroy_instance()`
/// which resets the singleton to Idle so it can be reused.
///
/// This ensures the instance is not permanently stuck in Busy state if the
/// subagent loop panics or returns early without cleanup.
pub(crate) struct AgentBusyGuard {
    instance_id: String,
    template_id: String,
    agent_registry: Arc<crate::agent::registry::AgentRegistry>,
    bus: EventBus,
    /// Set to true once the instance has been explicitly cleaned up.
    /// Prevents double-cleanup in the normal (non-panic) code path.
    restored: bool,
}

impl AgentBusyGuard {
    pub(crate) fn new(
        instance_id: String,
        template_id: String,
        agent_registry: Arc<crate::agent::registry::AgentRegistry>,
        bus: EventBus,
    ) -> Self {
        Self {
            instance_id,
            template_id,
            agent_registry,
            bus,
            restored: false,
        }
    }

    pub(crate) fn restore(&mut self) {
        if !self.restored {
            self.restored = true;
            // Resolve the display name before destroying — a non-singleton
            // instance is removed from the registry entirely, so the lookup
            // must happen first or it comes back empty (GAP-07).
            let name = self
                .agent_registry
                .get_instance(&self.instance_id)
                .map(|a| a.name)
                .unwrap_or_default();
            let outcome = self.agent_registry.destroy_instance(&self.instance_id);
            let status = match outcome {
                crate::agent::registry::DestroyOutcome::ResetToIdle => "idle",
                _ => "destroyed",
            };
            self.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: self.instance_id.clone(),
                instance_id: self.instance_id.clone(),
                template_id: self.template_id.clone(),
                name,
                status: status.to_string(),
                current_task_id: None,
                timestamp: Utc::now(),
            });
        }
    }
}

impl Drop for AgentBusyGuard {
    fn drop(&mut self) {
        if !self.restored {
            tracing::warn!(
                instance_id = %self.instance_id,
                "AgentBusyGuard dropped without explicit restore — destroying instance"
            );
            self.restore();
        }
    }
}
