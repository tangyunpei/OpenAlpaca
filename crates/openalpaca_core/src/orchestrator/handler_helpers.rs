//! Helper methods for the orchestrator message handler: delegation metadata,
//! skill invocation telemetry, and multimodal adaptation.

use super::{ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use chrono::Utc;
use openalpaca_llm::ContentPart;
use uuid::Uuid;

use super::dispatcher::DispatchOutcome;

impl Orchestrator {
    /// Record structured delegation metadata for the bridge to attach to the
    /// response (mirrors `llm_metadata_map`: inserted here, drained by the
    /// daemon's `build_result`).
    pub(super) fn record_delegation(&self, request_id: Uuid, outcome: &DispatchOutcome) {
        self.delegation_map.insert(
            request_id,
            crate::gateway::DelegationInfo {
                task_id: outcome.task_id.clone(),
                title: outcome.title.clone(),
            },
        );
    }

    /// Invoke a skill with route-telemetry capture and IntentClassified emission.
    ///
    /// Used by the deterministic skill tier in `handle_message_internal`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn invoke_skill_with_telemetry(
        &self,
        request_id: Uuid,
        source: &str,
        skill_name: &str,
        query: &str,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        stream_id: Option<&str>,
    ) -> Result<String, String> {
        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: "skill_invocation".to_string(),
            timestamp: Utc::now(),
        });

        // Capture route metadata for telemetry (re-route is cheap, no side effects)
        let route_result = self.skill_router.route(query, &self.skill_catalog);
        let was_auto_selected = route_result.selected.as_deref() == Some(skill_name);
        let route_score = route_result
            .scores
            .iter()
            .find(|s| s.skill_id == skill_name)
            .map(|s| s.score);

        self.handle_skill_invocation(
            request_id,
            source,
            skill_name,
            query,
            lane_key,
            ctx,
            owner_id,
            scope_ctx,
            route_score,
            was_auto_selected,
            stream_id,
        )
        .await
    }

    /// Adapt multimodal content parts for a model's capabilities.
    ///
    /// Replaces unsupported content types with text placeholders based on
    /// the model's capability flags in the registry.
    pub(super) fn adapt_parts_for_model(
        &self,
        parts: Vec<ContentPart>,
        model_id: &str,
    ) -> Vec<ContentPart> {
        let router = match &self.llm_router {
            Some(r) => r,
            None => return parts,
        };
        let registry = router.model_registry();

        let supports_image = registry.supports_image(model_id);
        let supports_audio = registry.supports_audio(model_id);
        let supports_document = registry.supports_document(model_id);

        parts
            .into_iter()
            .map(|part| match &part {
                ContentPart::Image { .. } if !supports_image => ContentPart::Text {
                    text: "[image attached — model does not support vision]".to_string(),
                },
                ContentPart::Audio { .. } if !supports_audio => ContentPart::Text {
                    text: "[audio attached — model does not support audio input]".to_string(),
                },
                ContentPart::Document { .. } if !supports_document => ContentPart::Text {
                    text: "[document attached — model does not support document input]".to_string(),
                },
                _ => part,
            })
            .collect()
    }
}
