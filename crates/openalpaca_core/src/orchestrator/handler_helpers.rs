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

    /// The `/slash` tier's tombstone answer (extension design §10 case 5(a)).
    ///
    /// `SkillCatalog::remove` scrubs the command and alias indices, so after a
    /// plugin's T2 a `/slash` for one of its skills resolves to nothing and
    /// falls through to the main loop as ordinary chat. The tombstone — a
    /// separate map, consulted **only on that miss** — names the plugin and its
    /// current state instead of the entry that is gone.
    pub(super) fn withdrawn_skill_reply(&self, content: &str) -> Option<String> {
        let command = content.trim().strip_prefix('/')?;
        let command = command.split_whitespace().next()?;
        let tomb = self.skill_catalog.tombstone(command)?;
        tracing::warn!(
            skill = %tomb.skill_id,
            plugin = %tomb.plugin_id,
            "Slash command names a skill withdrawn with its plugin"
        );
        Some(self.tool_registry.withdrawn_contribution_refusal(
            "Skill",
            &tomb.skill_id,
            &tomb.plugin_id,
        ))
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

        // **The explicit-invocation refusal** (extension design §7.5, §6.2
        // #12). The user named this skill, so the withdrawal changes what they
        // asked for and the answer is chat: refuse, naming skill, requirement,
        // extension and remedy. The deterministic tier returns directly with no
        // fallback, so this message *is* the answer — it is returned as
        // **`Ok(reply)`**, the reply text, never as `Err`, so it does not
        // depend on whatever `handlers.rs` does with an `Err`.
        //
        // Auto-routed skills never reach here unsatisfiable: the router drops
        // them from candidacy on the same predicate.
        if let Some(entry) = self.skill_catalog.get(skill_name) {
            let requirements = self
                .tool_registry
                .skill_requirements(&entry.frontmatter);
            if !requirements.is_satisfiable() {
                // The announcement the surface assembly this short-circuits
                // would have made (§7.2, `Moment::SurfaceAssembly`), scoped to
                // the request exactly as the invocation sites scope theirs.
                let scope = request_id.to_string();
                for (extension, subject) in requirements.attributions() {
                    self.tool_registry.extensions().note_withheld(
                        extension,
                        subject,
                        crate::tools::extensions::Moment::SurfaceAssembly,
                        None,
                        Some(&scope),
                    );
                }
                tracing::warn!(
                    skill = skill_name,
                    "Refusing explicitly invoked skill: a required capability is wholly withheld"
                );
                return Ok(requirements.refusal(skill_name));
            }
        }

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
