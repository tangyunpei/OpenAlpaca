//! Helper methods for the orchestrator message handler: delegation metadata,
//! skill invocation telemetry, and multimodal adaptation.

use super::attachment_adapt::{self, AnsweringModel, Fate};
use super::{ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use chrono::Utc;
use openalpaca_llm::ContentPart;
use uuid::Uuid;

impl Orchestrator {
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
        turn: &mut crate::gateway::HandleResult,
        request_id: Uuid,
        source: &str,
        skill_name: &str,
        query: &str,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        stream_id: Option<&str>,
        // M6/S5 — the client behind this turn cannot answer a confirmation.
        unattended: bool,
        // K1 — where this turn's text deltas go while the model is still
        // producing them. `None` for every caller with nobody watching a
        // stream (scheduled skills, connectors, the follow-up runner).
        turn_sink: Option<&crate::chat::TurnSinkHandle>,
        // A1 — the turn's own attachments, already adapted. `None` for every
        // caller that is not a chat turn with files on it.
        attachments: Option<&super::handler_attachments::TurnAttachments>,
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
            let requirements = self.tool_registry.skill_requirements(&entry.frontmatter);
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
                // A1 — the refusal is written here, without a model.
                self.skip_turn_attachments(
                    turn,
                    attachments,
                    super::handler_attachments::skipped::SKILL_REFUSED,
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
            turn,
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
            unattended,
            turn_sink,
            attachments,
        )
        .await
    }

    /// U1 — the model this turn will actually be answered by, and what it
    /// takes natively. `None` when there is no router or nothing is routable,
    /// which every caller must read as "change nothing".
    pub(super) fn answering_model(&self, pinned: Option<&str>) -> Option<AnsweringModel> {
        attachment_adapt::answering_model(self.llm_router.as_ref()?, pinned)
    }

    /// `[upload.governance] max_extracted_text_chars` — the cap extraction
    /// itself applies, and therefore the bound U2's labelled text block is
    /// carried under. One number, one place.
    pub(super) fn attachment_text_cap(&self) -> usize {
        self.daemon_config
            .load()
            .upload
            .governance
            .max_extracted_text_chars
    }

    /// Adapt multimodal content parts for the model that will answer.
    ///
    /// Not "the configured default": on a local-only install that id is not in
    /// the registry at all, and every capability question answered `false`
    /// (U1). The caller resolves the model once with
    /// [`Orchestrator::answering_model`] and passes it here, so this and the
    /// context-window code read the same ladder.
    ///
    /// An image or audio part the model cannot take becomes a placeholder; a
    /// document becomes its extracted text, labelled and bounded (U2), and a
    /// placeholder only when there is no text. Every withholding is logged
    /// (U3) — the ids of the turn's *own* attachments are reported by
    /// `handle_message_with_attachments`, which knows them; a part replayed
    /// out of history carries an id only when it is a document.
    pub(super) fn adapt_parts_for_model(
        &self,
        parts: Vec<ContentPart>,
        model: &AnsweringModel,
    ) -> Vec<ContentPart> {
        let max_chars = self.attachment_text_cap();
        parts
            .into_iter()
            .map(|part| match part {
                ContentPart::Image { .. } => match attachment_adapt::image_fate(model) {
                    Fate::Native => part,
                    _ => {
                        warn_withheld(None, &model.id, attachment_adapt::REASON_NO_IMAGE);
                        ContentPart::Text {
                            text: attachment_adapt::PLACEHOLDER_IMAGE.to_string(),
                        }
                    }
                },
                // A history-replayed audio part carries no extracted text (the
                // part has no field for one), so A2's shared tail lands on the
                // placeholder exactly as before.
                ContentPart::Audio { .. } => {
                    match attachment_adapt::audio_fate(model, None, max_chars) {
                        Fate::Native => part,
                        _ => {
                            warn_withheld(None, &model.id, attachment_adapt::REASON_NO_AUDIO);
                            ContentPart::Text {
                                text: attachment_adapt::PLACEHOLDER_AUDIO.to_string(),
                            }
                        }
                    }
                }
                ContentPart::Document {
                    file_id,
                    filename,
                    mime_type,
                    extracted_text,
                } => {
                    let fate = attachment_adapt::document_fate(
                        model,
                        extracted_text.as_deref(),
                        max_chars,
                    );
                    match fate {
                        Fate::Native => ContentPart::Document {
                            file_id,
                            filename,
                            mime_type,
                            extracted_text,
                        },
                        Fate::AsText { cut_at } => {
                            if let Some(total) = cut_at {
                                tracing::warn!(
                                    file_id = %file_id,
                                    model = %model.id,
                                    kept_chars = max_chars,
                                    total_chars = total,
                                    "Attachment text cut to the extraction cap before the model saw it"
                                );
                            }
                            ContentPart::Text {
                                text: attachment_adapt::document_as_text(
                                    &filename,
                                    &mime_type,
                                    extracted_text.as_deref().unwrap_or_default(),
                                    max_chars,
                                ),
                            }
                        }
                        Fate::Withheld { reason } => {
                            warn_withheld(Some(&file_id), &model.id, reason);
                            ContentPart::Text {
                                text: attachment_adapt::PLACEHOLDER_DOCUMENT.to_string(),
                            }
                        }
                    }
                }
                other => other,
            })
            .collect()
    }
}

/// U3(a) — one WARN naming the attachment, the model and the reason.
pub(super) fn warn_withheld(file_id: Option<&str>, model: &str, reason: &str) {
    tracing::warn!(
        file_id = file_id.unwrap_or("-"),
        model = %model,
        reason = %reason,
        "Attachment withheld from the model"
    );
}
