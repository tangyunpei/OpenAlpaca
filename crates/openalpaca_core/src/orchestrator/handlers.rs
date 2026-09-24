use super::handler_attachments::{TurnAttachments, skipped};
use super::{Orchestrator, principal_id};
use crate::events::SystemEvent;
use crate::gateway::HandleRequest;
use crate::memory::scope_context::MemoryScopeContext;
use crate::security::gate::SecurityGate;
use crate::security::policy::Principal;
use crate::types::Capability;
use chrono::Utc;
use openalpaca_storage::repository::orchestrator_latency::{
    OrchestratorLatencyRecord, OrchestratorLatencyRepository,
};
use std::time::Instant;
use uuid::Uuid;

use super::intent::Intent;

impl Orchestrator {
    /// Public entry point for processing a user message.
    pub async fn handle_message(&self, request: HandleRequest) -> Result<String, String> {
        let model_input_content = request.content.clone();
        self.handle_message_internal(request, model_input_content, false, None)
            .await
    }

    /// Internal message handler that separates the model input from the intent source.
    ///
    /// `request.content` is the intent source — used only for intent
    /// classification checks; `model_input_content` is used for LLM calls and
    /// context building. The two differ on the attachment path, where the model
    /// sees the message augmented with the files' extracted text.
    ///
    /// **A1 — every arm answers for its attachments.** `attachments` is the
    /// turn's own files, already adapted for the model that will answer. An
    /// arm either hands the parts to its model or calls
    /// [`Orchestrator::skip_turn_attachments`] with a reason, so the bridge's
    /// `attachments_used` / `attachments_skipped` stay a partition and `used`
    /// keeps meaning "the model's request really carried it".
    pub(super) async fn handle_message_internal(
        &self,
        request: HandleRequest,
        model_input_content: String,
        force_simple_query: bool,
        attachments: Option<TurnAttachments>,
    ) -> Result<String, String> {
        let HandleRequest {
            request_id,
            source,
            content: intent_source_content,
            principal,
            scope,
            lane_key,
            workspace_path,
            stream_id,
            model_override,
            unattended,
            turn_sink,
        } = request;
        let ack_start = Instant::now();

        // 1. Permission check via SecurityGate (wraps TrustGate)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        SecurityGate::check_access(&principal, &capability, &scope)?;

        // 2. Input sanitization
        let max_input_len = self.daemon_config.load().security.max_input_length;
        let model_input_content =
            SecurityGate::sanitize_input(&model_input_content, Some(max_input_len))?;
        let intent_source_content =
            SecurityGate::sanitize_input(&intent_source_content, Some(max_input_len))?;

        // Extract owner_id from principal (before slash-command early return)
        let owner_id_str = principal_id(&principal);
        let owner_id = match &principal {
            Principal::User { .. } => Some(owner_id_str.as_str()),
            _ => None,
        };

        // Resolve the turn's workspace context. Memory scoping prefers the
        // request-provided path (GUI/CLI) and falls back to the daemon CWD;
        // `request_workspace_root` is set only on the request branch, so
        // content writers never inherit the CWD (R22 — see `for_request`).
        let scope_ctx = MemoryScopeContext::for_request(workspace_path.as_deref());

        // 3. Try slash commands, task queries, and skill invocations first
        let intent = if force_simple_query {
            Intent::SimpleQuery {
                query: intent_source_content.trim().to_string(),
            }
        } else {
            self.intent_parser.parse_with_skills_and_router(
                &intent_source_content,
                &self.skill_catalog,
                &self.skill_router,
            )
        };
        match &intent {
            Intent::TaskQuery { .. } | Intent::TaskControl { .. } => {
                self.bus.publish(SystemEvent::IntentClassified {
                    request_id,
                    intent_type: intent.intent_type().to_string(),
                    timestamp: Utc::now(),
                });
                let result = match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id, &owner_id_str),
                    Intent::TaskControl {
                        task_id: Some(task_id),
                        action,
                    } => self.handle_task_control(&task_id, &action),
                    Intent::TaskControl {
                        task_id: None,
                        action,
                    } => {
                        // Bare /cancel|/pause|/resume: resolve the target from
                        // the lane's active workflows (Routing V2 Phase 3).
                        self.handle_bare_task_control(&action, &lane_key)
                    }
                    _ => unreachable!(),
                };
                // A1 — a task command is answered with no model at all, so the
                // turn's files went nowhere. Say so rather than let the bridge
                // report them used.
                self.skip_turn_attachments(request_id, attachments.as_ref(), skipped::TASK_OPS);
                // Task ops skip the routing ladder entirely, but every routed
                // message must still be observable (Routing V2 Phase 3):
                // emit OrchestrationStage + the latency record before returning.
                self.record_orchestration_stage(
                    request_id,
                    "task_ops".to_string(),
                    ack_start.elapsed().as_millis() as u64,
                    None,
                    None,
                );
                return result;
            }
            _ => {}
        }

        // 4. Build context ONCE for all remaining paths (D6: single dedup location)
        //
        // The dedup key is the **raw** message, not the model input. The
        // gateway persists this turn before the handler runs, so the last
        // recent row *is* this turn; `build_context` drops it by comparing it
        // to the current query. On the attachment path the model input is the
        // augmented string (the files' extracted text wrapped around the
        // question) and never equalled the stored content, so the turn's own
        // message came back as history — the attachment reached the model
        // twice, and U2 would have sent the whole document twice.
        let ctx = self.build_context(&lane_key, &intent_source_content);

        // 5. Compute result — deterministic tiers, then the main loop.
        //    Track timing for observability (OrchestrationStage metrics).
        let mode: String;

        let result: Result<String, String> = if !force_simple_query
            && self.daemon_config.load().orchestrator.routing.steering_enabled
            && let Some(steer_text) = intent_source_content.strip_prefix("/steer ")
        {
            // Deterministic steering override (Routing V2): guaranteed
            // injection into the lane's running workflow, bypassing the
            // model. With steering_enabled=false (rollback) this arm is
            // skipped and "/steer ..." routes like any other message.
            mode = "steered".to_string();
            // A1 — a steer is a deterministic injection, not a turn a model
            // answers; nothing carries the files into the running workflow.
            self.skip_turn_attachments(request_id, attachments.as_ref(), skipped::STEER);
            self.handle_steer_prefix(
                request_id,
                steer_text,
                &principal,
                &scope,
                &lane_key,
                workspace_path.clone(),
            )
        } else if let Intent::SkillInvocation {
            ref skill_name,
            ref query,
        } = intent
        {
            // Deterministic skill tier: slash commands and router-selected
            // skills execute directly, before the bootstrap check and the
            // main loop (Routing V2 Phase 0.5).
            mode = "skill_command".to_string();
            self.invoke_skill_with_telemetry(
                request_id,
                &source,
                skill_name,
                query,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                stream_id.as_deref(),
                // S5: the deterministic skill tier is a *client-facing* tier
                // — a `/slash` from a piped CLI and every scheduled skill
                // reach the model here, not through the main loop — so the
                // declaration has to arrive here too, or the turn sits out
                // the whole confirmation timeout with nobody to answer.
                unattended,
                // K1: and for the same reason, the turn's live rail. A
                // `/slash` answer used to reach the client as one delta after
                // the whole generation — a silent minute on a local model —
                // while the main loop two arms below streamed.
                turn_sink.as_ref(),
                // A1: and for the same reason again, the turn's own files. A
                // `/slash` with an attachment reached the model as the bare
                // question — no file, no name, and `attachments_used` saying
                // it had arrived.
                attachments.as_ref(),
            )
            .await
        } else if let Some(reply) = self.withdrawn_skill_reply(&intent_source_content) {
            // A `/slash` naming a skill a **disabled plugin** used to provide.
            // The catalog's live indices were scrubbed at T2, so the command
            // missed above and the message would otherwise have fallen through
            // to the main loop as ordinary chat; the tombstone answers with the
            // plugin that owns it instead (extension design §10 case 5(a)).
            mode = "skill_withdrawn".to_string();
            // A1 — the tombstone is a fixed sentence, written without a model.
            self.skip_turn_attachments(request_id, attachments.as_ref(), skipped::NO_MODEL_TIER);
            Ok(reply)
        } else if self.is_bootstrapping() {
            mode = "bootstrap".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                attachments.as_ref(),
                stream_id.as_deref(),
                turn_sink.as_ref(),
                Some(super::query_handler::LoopOverrides::ModelOnly {
                    model_override: model_override.clone(),
                }),
            )
            .await
        } else if force_simple_query {
            mode = "forced_simple_query".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                attachments.as_ref(),
                stream_id.as_deref(),
                turn_sink.as_ref(),
                Some(super::query_handler::LoopOverrides::ModelOnly {
                    model_override: model_override.clone(),
                }),
            )
            .await
        } else if self.llm_router.is_some()
            && matches!(intent, Intent::SimpleQuery { .. })
            && self.intent_parser.is_social_message(&intent_source_content)
            && {
                let sendable_for_guard: Vec<String> = self
                    .connector_sender
                    .read()
                    .ok()
                    .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
                    .unwrap_or_default();
                super::query_handler::detect_active_send_hints(&ctx.recent_messages, &sendable_for_guard)
                    == super::query_handler::ActiveSendHints::default()
            }
        {
            // Social fast path: ultra-light prompt for "ok", "thanks", "好的" etc.
            mode = "social_fast_path".to_string();
            // A1 — this arm *does* reach a model, but with a prompt of its own
            // that carries no parts. The turn's files did not reach it.
            self.skip_turn_attachments(request_id, attachments.as_ref(), skipped::SOCIAL);
            self.handle_social_query(
                request_id,
                &model_input_content,
                &lane_key,
                &ctx,
                model_override.clone(),
                turn_sink.as_ref(),
            )
            .await
        } else {
            // Routing V2 main loop: the front door for everything that
            // survived the deterministic tier and the social branch.
            // Chat vs. task vs. steer is the model's tool choice
            // (`start_workflow` / `steer_workflow` via the per-request core
            // tool set assembled inside handle_simple_query).
            mode = "main_loop".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                attachments.as_ref(),
                stream_id.as_deref(),
                turn_sink.as_ref(),
                Some(super::query_handler::LoopOverrides::MainLoop {
                    workspace_path: workspace_path.clone(),
                    model_override: model_override.clone(),
                    // M6: the turn's own declaration, carried into the tools
                    // it may call and the workflow it may start.
                    unattended,
                }),
            )
            .await
        };

        let ack_ms = ack_start.elapsed().as_millis() as u64;

        // Emit OrchestrationStage event + persist the latency record
        self.record_orchestration_stage(request_id, mode, ack_ms, None, None);

        // 6 + 7. The summary update and the memory extraction run after the
        // answer, as one tracked background job (`BackgroundJobs`): never on
        // the response path, stopped by the shutdown token, and waited for
        // by whoever needs the turn's extraction to have landed.
        {
            use super::extraction::{ExtractionTurn, TurnExtractionDeps, extract_turn_memories};
            use super::summary::update_summary_background;

            let summary = match (self.db.clone(), self.llm_router.clone()) {
                (Some(db), Some(router)) => {
                    Some((db, router, self.daemon_config.clone(), lane_key.clone()))
                }
                _ => None,
            };

            // MD-2, plan 3.4: the extractor's input is the message **as
            // typed** (`intent_source_content`), never `model_input_content`
            // — on an attachment turn that carries the files' text, and an
            // uploaded document must not mint an `extracted_user` note.
            let extraction = match (
                &result,
                owner_id,
                self.db.clone(),
                self.llm_router.clone(),
                self.memory_service(),
            ) {
                (Ok(response), Some(owner), Some(db), Some(router), Some(service)) => Some((
                    TurnExtractionDeps {
                        db,
                        router,
                        daemon_config: self.daemon_config.clone(),
                        memory_service: service,
                        turn_counter: self.extraction_turn_counter.clone(),
                        cancel: self.background.cancel_token(),
                    },
                    ExtractionTurn {
                        owner_id: owner.to_string(),
                        lane_key: lane_key.clone(),
                        source: source.clone(),
                        request_id,
                        project_root: scope_ctx
                            .request_workspace_root
                            .as_deref()
                            .map(std::path::PathBuf::from),
                        user_message: intent_source_content.clone(),
                        assistant_response: response.clone(),
                    },
                )),
                _ => None,
            };

            // NOTE: ctx is intentionally moved into this job — do not reference it after this block
            self.background.spawn(async move {
                let summary_fut = async {
                    if let Some((db, router, config, lane)) = summary {
                        update_summary_background(db, router, config, lane, ctx).await;
                    }
                };
                let extract_fut = async {
                    if let Some((deps, turn)) = extraction {
                        extract_turn_memories(&deps, turn).await;
                    }
                };
                tokio::join!(summary_fut, extract_fut);
            });
        }

        // 8. Check if bootstrap onboarding is complete
        self.maybe_complete_bootstrap().await;

        result
    }

    /// Publish an `OrchestrationStage` event and persist the matching
    /// latency record (best-effort). Shared by the routing ladder and the
    /// task-ops early return so every routed message is observable.
    fn record_orchestration_stage(
        &self,
        request_id: Uuid,
        mode: String,
        ack_ms: u64,
        fallback_reason: Option<String>,
        auto_promotion_reason: Option<String>,
    ) {
        self.bus.publish(SystemEvent::OrchestrationStage {
            request_id,
            mode: mode.clone(),
            ack_ms,
            fallback_reason: fallback_reason.clone(),
            auto_promotion_reason: auto_promotion_reason.clone(),
            timestamp: Utc::now(),
        });

        if let Some(ref db) = self.db {
            let repo = OrchestratorLatencyRepository::new(db);
            if let Err(e) = repo.record(&OrchestratorLatencyRecord {
                id: None,
                request_id: request_id.to_string(),
                mode,
                ack_ms,
                fallback_reason,
                auto_promotion_reason,
                timestamp: None,
            }) {
                tracing::debug!("Failed to persist orchestrator latency: {e}");
            }
        }
    }
}
