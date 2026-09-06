use super::super::task_state::TaskState;
use super::update_state_with_retry;
use super::usage;
use super::{
    DispatchOutcome, TaskDispatcher, finalize_task_with_outcome, format_task_result,
    persist_completion_report, spawn_task_memory_extraction,
};
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::SubAgent;
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::runner::lead_agent::run_lead_agent;
use crate::runner::steering::SteeringInbox;
use chrono::Utc;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How a dispatch's persist step writes the run's row.
///
/// Every dispatch but one mints a fresh id, so the row cannot exist yet. The
/// exception is D5's `start`, which re-launches a row the client already holds
/// an id for — see [`TaskRepository::upsert_queued`].
///
/// [`TaskRepository::upsert_queued`]: openalpaca_storage::repository::TaskRepository::upsert_queued
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowWrite {
    /// A fresh id: `INSERT`.
    Create,
    /// D5's `start`: create-or-update on the caller's id.
    Relaunch,
}

impl TaskDispatcher {
    /// Dispatch a task using the Lead Agent orchestration pattern.
    /// Spawns a lead agent instance from the "lead_agent" template (singleton),
    /// registers the task, and runs the lead agent execution loop.
    pub(crate) fn dispatch_lead_agent(
        &self,
        description: &str,
        title: String,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace: MemoryScopeContext,
    ) -> Result<DispatchOutcome, String> {
        self.dispatch_lead_agent_inner(
            Uuid::new_v4().to_string(),
            description,
            title,
            created_by,
            lane_key,
            source,
            workspace,
            None,
            RowWrite::Create,
        )
    }

    /// GAP-06's `rerun`: a **new** run carrying an old one's goal, with the
    /// provenance link back to it (`task.source_task_id`).
    ///
    /// The asymmetry with [`Self::dispatch_lead_agent_with_id`] is deliberate.
    /// A re-run is a second run of the same work, and both rows have to survive
    /// — the original's result is the thing the user is comparing against — so
    /// it gets a new id and answers `201`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dispatch_lead_agent_rerun(
        &self,
        description: &str,
        title: String,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace: MemoryScopeContext,
        source_task_id: &str,
    ) -> Result<DispatchOutcome, String> {
        self.dispatch_lead_agent_inner(
            Uuid::new_v4().to_string(),
            description,
            title,
            created_by,
            lane_key,
            source,
            workspace,
            Some(source_task_id.to_string()),
            RowWrite::Create,
        )
    }

    /// D5's `start`: run a stored row **under its own id**.
    ///
    /// The caller must already hold the id's run slot
    /// ([`SharedContext::claim_run_slot`](crate::context::SharedContext::claim_run_slot))
    /// and must release it if this returns `Err` — the claim is what stops two
    /// simultaneous `start`s from putting two lead agents on one task id.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dispatch_lead_agent_with_id(
        &self,
        task_id: &str,
        description: &str,
        title: String,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace: MemoryScopeContext,
    ) -> Result<DispatchOutcome, String> {
        self.dispatch_lead_agent_inner(
            task_id.to_string(),
            description,
            title,
            created_by,
            lane_key,
            source,
            workspace,
            None,
            RowWrite::Relaunch,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_lead_agent_inner(
        &self,
        task_id: String,
        description: &str,
        title: String,
        created_by: &str,
        lane_key: &str,
        source: &str,
        workspace: MemoryScopeContext,
        source_task_id: Option<String>,
        row_write: RowWrite,
    ) -> Result<DispatchOutcome, String> {
        let now = Utc::now();

        // Spawn a lead agent instance from the singleton template.
        // Prefer templates with "orchestration" capability, fall back to any template.
        let lead_agent = {
            let templates = self
                .shared_context
                .agent_registry
                .find_templates_by_capability("orchestration");
            let mut spawned = None;
            for t in &templates {
                if let Ok(agent) = self
                    .shared_context
                    .agent_registry
                    .spawn_instance(&t.frontmatter.id, task_id.clone())
                {
                    spawned = Some(agent);
                    break;
                }
            }
            if spawned.is_none() {
                // Fallback: try spawning from any available template
                for t in self.shared_context.agent_registry.list_templates() {
                    if let Ok(agent) = self
                        .shared_context
                        .agent_registry
                        .spawn_instance(&t.frontmatter.id, task_id.clone())
                    {
                        spawned = Some(agent);
                        break;
                    }
                }
            }
            spawned.ok_or_else(|| {
                "No agents available to act as Lead Agent. All agents are busy.".to_string()
            })?
        };

        // Register in task_registry
        self.shared_context
            .task_registry
            .register(task_id.clone(), title.clone());

        // Create TaskLane
        self.lane_manager.create_task_lane(&task_id);

        // Emit status change — lead agent instance just spawned
        self.bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: lead_agent.id.clone(),
            instance_id: lead_agent.id.clone(),
            template_id: lead_agent.template_id.clone(),
            name: lead_agent.name.clone(),
            status: "spawned".to_string(),
            current_task_id: Some(task_id.clone()),
            timestamp: now,
        });

        // Emit TaskCreated
        self.bus.publish(SystemEvent::TaskCreated {
            task_id: task_id.clone(),
            title: title.clone(),
            created_by: created_by.to_string(),
            timestamp: now,
        });

        // §5.1: the session this run was started from, resolved once at
        // dispatch. It is what makes the completion report land in the
        // conversation that asked for the work, even when the user has opened
        // another one by the time the run finishes.
        let session_id = self.db.as_ref().and_then(|db| {
            openalpaca_storage::ConversationRepository::new(db)
                .active_session_id(lane_key)
                .unwrap_or_else(|e| {
                    tracing::warn!(%lane_key, "Failed to resolve the lane's active session: {e}");
                    None
                })
        });

        // Persist task to DB
        if let Some(ref db) = self.db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let task = openalpaca_storage::Task {
                id: task_id.clone(),
                title: title.clone(),
                description: Some(description.to_string()),
                status: openalpaca_storage::TaskStatus::Queued,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: created_by.to_string(),
                source_lane: lane_key.to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
                state_json: None,
                state_version: 0,
                outcome_json: None,
                outcome_kind: None,
                artifact_count: 0,
                // §4.7 item 3: the project this run belonged to. The request's
                // workspace root and nothing else — `workspace_id` on the same
                // context falls back to the daemon CWD for memory scoping, and
                // recording *that* would claim a Telegram run belonged to
                // whatever repository the daemon started in (R22).
                workspace_id: workspace.request_workspace_root.clone(),
                // Set only by `rerun`, which is the only dispatch that copies
                // another run's goal onto a new id (GAP-06).
                source_task_id: source_task_id.clone(),
                session_id: session_id.clone(),
            };
            let persisted = match row_write {
                RowWrite::Create => repo.create(&task),
                // D5 — the row is already there under this id, and the run
                // about to start replaces whatever the last one left on it.
                RowWrite::Relaunch => repo.upsert_queued(&task),
            };
            if let Err(e) = persisted {
                tracing::warn!("Failed to persist lead agent task to DB: {e}");
            }

            // Initialize state_json with workspace
            let step_info = vec![(
                lead_agent.id.clone(),
                lead_agent.name.clone(),
                "lead_orchestrator".to_string(),
            )];
            let initial_state = TaskState::initial(description, &step_info);
            let state_json = initial_state.to_json();
            match repo.update_state(&task_id, &state_json, 0) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(task_id = %task_id, "State init version conflict, retrying");
                    let _ = repo.update_state(&task_id, &state_json, 1);
                }
                Err(e) => {
                    tracing::error!(task_id = %task_id, error = %e, "Failed to initialize task state");
                }
            }
        }

        // Spawn the lead agent execution
        self.spawn_lead_agent_execution(
            task_id.clone(),
            title.clone(),
            description.to_string(),
            lead_agent,
            lane_key.to_string(),
            source.to_string(),
            created_by.to_string(),
            workspace,
            session_id,
        );

        let ack = format!(
            "I've created a task and assigned it to the Lead Agent for dynamic orchestration:\n\n\
             - Lead Agent will analyze, delegate to subagents, and synthesize results\n\n\
             Task: {}\nYou'll see the results here when the task completes.",
            title
        );
        Ok(DispatchOutcome {
            task_id,
            title,
            ack,
        })
    }

    /// Spawn the lead agent execution in a background tokio task.
    /// The lead agent runs a full agentic loop with `spawn_subagent` tool access.
    #[allow(clippy::too_many_arguments)]
    fn spawn_lead_agent_execution(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        lead_agent: SubAgent,
        lane_key: String,
        source: String,
        created_by: String,
        workspace: MemoryScopeContext,
        session_id: Option<String>,
    ) {
        let Some(router) = self.require_router(&task_id) else {
            // Nothing will run, so nothing will clean up after it: release the
            // run slot D5's `start` claimed before dispatching, or that id
            // answers "already running" until the daemon restarts. A no-op for
            // every other dispatch — those register their token below.
            self.shared_context.remove_cancellation_token(&task_id);
            return;
        };

        let bus = self.bus.clone();
        let ctx = self.shared_context.clone();
        let lane_manager = self.lane_manager.clone();
        let db = self.db.clone();
        let embedder = self.embedder.clone();
        let tool_registry = self.tool_registry.clone();
        let daemon_config = self.daemon_config.clone();
        let connector_block = self.connector_guidance_block();
        let broker = self.confirmation_broker.read().ok().and_then(|g| g.clone());
        let followup_runner = self.followup_runner.read().ok().and_then(|g| g.clone());
        let skill_catalog = self.skill_catalog.clone();
        let context_manager = self.context_manager.clone();
        let compose_engine = self.compose_engine.clone();

        // Create cancellation token for this task
        let cancel_token = CancellationToken::new();
        ctx.register_cancellation_token(&task_id, cancel_token.clone());

        // Routing V2: attach the workflow to its lane unconditionally — the
        // lane attachment backs the StartWorkflowTool per-lane cap and the
        // workflow-context block, which must bind in every mode/flag combo.
        ctx.register_workflow_for_lane(&lane_key, &task_id);

        // The steering inbox itself stays flag-gated: with steering off no
        // inbox registers and steer paths report "not steerable".
        let steering_inbox = {
            let cfg = daemon_config.load();
            if cfg.orchestrator.routing.steering_enabled {
                let inbox = Arc::new(SteeringInbox::new(
                    cfg.orchestrator.routing.steering_inbox_cap,
                ));
                ctx.register_steering_inbox(&task_id, inbox.clone());
                Some(inbox)
            } else {
                None
            }
        };

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();

            tracing::info!(
                task_id = %task_id,
                lead_agent = %lead_agent.id,
                "Lead agent background execution starting"
            );

            // Update task status → Running
            ctx.task_registry
                .update_status(&task_id, TaskEntryStatus::Running);
            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                title: task_title.clone(),
                status: "running".to_string(),
                progress_current: Some(0),
                progress_total: None, // Lead agent has dynamic progress
                timestamp: Utc::now(),
            });
            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
            }

            // The lead's own lane (plan Phase 4, GAP-09). Its span id is
            // derived from the task rather than a fresh UUID: there is exactly
            // one lead lane per run, so the id stays reconstructible from the
            // task alone. Opened after the status flip, so a reader that sees
            // a running task always sees a lane for it.
            let lead_span_id = format!("lead::{task_id}");
            crate::runner::span::open_span(
                db.as_ref(),
                &bus,
                &task_id,
                &lead_span_id,
                &lead_agent.template_id,
                &lead_agent.id,
                &description,
            );

            // Mark step 0 as running now (before the agentic loop) so started_at is accurate
            if let Some(ref db) = db
                && !update_state_with_retry(
                    db,
                    &task_id,
                    |s| s.mark_step_running(0),
                    "lead_agent_mark_step_running",
                )
                .await
            {
                tracing::error!("Failed to persist lead_agent_mark_step_running for task '{}'", task_id);
            }

            tracing::info!(task_id = %task_id, "Task status: queued → running");

            // Run the lead agent
            let result = run_lead_agent(
                &lead_agent,
                &description,
                router.clone(),
                tool_registry,
                ctx.clone(),
                bus.clone(),
                db.clone(),
                embedder.clone(),
                &task_id,
                &created_by,
                &lane_key,
                &source,
                &daemon_config,
                workspace.clone(),
                Some(cancel_token),
                steering_inbox.clone(),
                &connector_block,
                broker,
                skill_catalog,
                context_manager,
                compose_engine,
            )
            .await;

            // The cancellation token is NOT released here (R45). It is also
            // the run slot `Orchestrator::start_task` claims, and everything
            // below — lane teardown, the steering drain, the state write, the
            // completion report, the span close — happens while the row still
            // says `running`. Releasing it now would leave that whole stretch
            // unclaimed on a non-terminal row, and a `start` arriving in it
            // would re-queue the row under a second lead agent that this run's
            // own `finalize_task_with_outcome` would then write over. It is
            // released immediately after that finalize instead.

            // Remove the task lane — task lanes are per-execution and would
            // otherwise accumulate for the daemon's lifetime (slow leak).
            lane_manager.remove_task_lane(&task_id);

            // Routing V2: detach the lane attachment unconditionally — it
            // was registered unconditionally at dispatch, independent of the
            // steering flag.
            ctx.deregister_workflow_for_lane(&lane_key, &task_id);

            // Detach steering. Close FIRST so a concurrent push gets
            // Err(Closed) instead of landing after the drain, then
            // deregister and convert leftovers to `unprocessed_steering`
            // follow-up rows — injected as a context block on the lane's
            // next main-loop turn (`query_handler/unprocessed_steering.rs`,
            // which marks them done), never auto-run (claim_next only
            // claims kind='followup').
            if let Some(ref inbox) = steering_inbox {
                let leftovers = inbox.close_and_drain();
                ctx.remove_steering_inbox(&task_id);
                if !leftovers.is_empty() {
                    if let Some(ref db) = db {
                        let repo =
                            openalpaca_storage::repository::FollowupRepository::new(db);
                        for msg in leftovers {
                            let principal_json = match serde_json::to_string(&msg.principal)
                            {
                                Ok(json) => json,
                                Err(e) => {
                                    tracing::warn!(
                                        task_id = %task_id,
                                        "Dropping steering leftover with unserializable principal: {e}"
                                    );
                                    continue;
                                }
                            };
                            match repo.queue(
                                &lane_key,
                                "unprocessed_steering",
                                &msg.text,
                                &principal_json,
                                msg.workspace_path.as_deref(),
                                Some(&task_id),
                            ) {
                                Ok(followup_id) => {
                                    bus.publish(SystemEvent::FollowupQueued {
                                        lane_key: lane_key.clone(),
                                        followup_id,
                                        kind: "unprocessed_steering".to_string(),
                                        timestamp: Utc::now(),
                                    });
                                }
                                Err(e) => tracing::warn!(
                                    task_id = %task_id,
                                    "Failed to queue steering leftover as follow-up: {e}"
                                ),
                            }
                        }
                    } else {
                        tracing::warn!(
                            task_id = %task_id,
                            dropped = leftovers.len(),
                            "Dropping unprocessed steering messages: no database"
                        );
                    }
                }
            }

            let now = Utc::now();
            let runtime_secs = start_time.elapsed().as_secs() as i64;

            tracing::info!(
                task_id = %task_id,
                success = result.success,
                rounds = result.loop_result.rounds_used,
                subagents = result.subagents_spawned,
                runtime_secs = runtime_secs,
                finish_reason = ?result.loop_result.finish_reason,
                "Lead agent execution returned"
            );

            // Update state_json: mark lead agent step completed or failed
            if let Some(ref db) = db {
                let success = result.success;
                let summary_text: String = result.final_content.chars().take(500).collect();
                let error_msg = format!("{:?}", result.loop_result.finish_reason);
                if !update_state_with_retry(
                    db,
                    &task_id,
                    move |state| {
                        if success {
                            state.mark_step_completed(0, &summary_text);
                        } else {
                            state.mark_step_failed(0, &error_msg);
                        }
                        state.scan_workspace_artifacts(0);
                    },
                    "lead_agent_step_complete",
                )
                .await
                {
                    tracing::error!("Failed to persist lead_agent_step_complete for task '{}'", task_id);
                }
            }

            // Destroy lead agent instance (resets singleton to Idle)
            let outcome = ctx.agent_registry.destroy_instance(&lead_agent.id);
            let destroy_status = match outcome {
                DestroyOutcome::ResetToIdle => "idle",
                _ => "destroyed",
            };
            bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: lead_agent.id.clone(),
                instance_id: lead_agent.id.clone(),
                template_id: lead_agent.template_id.clone(),
                name: lead_agent.name.clone(),
                status: destroy_status.to_string(),
                current_task_id: None,
                timestamp: now,
            });

            // Build final content
            let final_content = if result.success {
                if result.final_content.is_empty() {
                    format!(
                        "Lead agent completed: {} subagents spawned, {} rounds, {} tokens used",
                        result.subagents_spawned,
                        result.loop_result.rounds_used,
                        result.loop_result.total_input_tokens
                            + result.loop_result.total_output_tokens,
                    )
                } else {
                    result.final_content.clone()
                }
            } else {
                format!(
                    "Lead agent failed: {:?}. {} subagents were spawned before failure.",
                    result.loop_result.finish_reason, result.subagents_spawned
                )
            };

            // Persist LLM usage for the lead agent's own loop
            usage::record_llm_usage(
                &router,
                &result.loop_result,
                lead_agent.llm_config.model.as_deref(),
                &lead_agent.id,
                &task_id,
                start_time.elapsed().as_millis() as i64,
                db.as_ref(),
                &bus,
            );

            // Record agent task history
            if let Some(ref db) = db {
                usage::record_agent_history(
                    db,
                    &lead_agent.id,
                    &task_id,
                    "lead_agent",
                    result.success,
                    runtime_secs,
                );
            }

            // Persist final result to conversation before publishing completion,
            // so follow-up turns can immediately read the result from history.
            //
            // Routing V2 §2b: the lead agent's own final message IS the
            // user-facing completion report — persist it verbatim. The legacy
            // template is the fallback for empty final content (budget /
            // cancel / error exits); non-Complete finishes get a one-line
            // status prefix either way.
            if let Some(ref db) = db {
                let report = if result.final_content.is_empty() {
                    format_task_result(&task_title, &final_content, result.success)
                } else {
                    result.final_content.clone()
                };
                let content =
                    match super::outcome::completion_status_line(&result.loop_result.finish_reason)
                    {
                        Some(status) => format!("{status}\n\n{report}"),
                        None => report,
                    };
                // Resolve model name for conversation record
                let default_model = router.default_model();
                let actual_model = result
                    .loop_result
                    .model_used
                    .as_deref()
                    .or(lead_agent.llm_config.model.as_deref())
                    .unwrap_or(&default_model);
                // GAP-23: the report carries the run it closed *and* a
                // `role='artifact'` link per file the run produced — the two
                // things a reloaded transcript cannot otherwise know.
                // §5.3: into the session this run was *started from*, which
                // may since have been archived — not into whatever the lane is
                // currently showing.
                persist_completion_report(
                    db,
                    &lane_key,
                    &source,
                    session_id.as_deref(),
                    content,
                    Some(actual_model.to_string()),
                    result.loop_result.total_input_tokens as i64,
                    result.loop_result.total_output_tokens as i64,
                    runtime_secs,
                    &task_id,
                );
            }

            // Update task status
            if result.success {
                tracing::info!(task_id = %task_id, "Task status: running → completed");
            } else {
                tracing::warn!(
                    task_id = %task_id,
                    finish_reason = ?result.loop_result.finish_reason,
                    "Task status: running → failed"
                );
            }
            // Close the lead's lane *before* the task goes terminal: the
            // timeline reports a span still running on a terminal task as
            // `cancelled`/`"interrupted"`, and closing after the flip would
            // leave a window where a reader saw the lead as interrupted.
            crate::runner::span::close_span(
                db.as_ref(),
                &bus,
                &lead_span_id,
                crate::runner::span::span_state_for(&result.loop_result.finish_reason),
                crate::runner::span::span_detail_for(&result.loop_result.finish_reason).as_deref(),
                Some(result.final_content.as_str()),
            );

            finalize_task_with_outcome(
                &ctx,
                &bus,
                db.as_ref(),
                &task_id,
                &final_content,
                result.success,
            );

            // Now the id is genuinely free (R45): the row is terminal and
            // carries this run's result, so the next `start` on it is refused
            // by R43's guard rather than racing this tail. Nothing between
            // `run_lead_agent` returning and here reads the token — the only
            // other readers are `cancel_task` (a cancel during the tail already
            // had nothing left to stop) and `claim_run_slot` itself.
            ctx.remove_cancellation_token(&task_id);

            // Routing V2: auto-start the next queued follow-up for this lane.
            // Inert unless a runner is wired AND a `followup` row is queued
            // (only the steering-gated `queue_followup` tool writes those).
            if daemon_config.load().orchestrator.routing.followup_autostart
                && let (Some(runner), Some(db)) = (followup_runner, db.as_ref())
            {
                let repo = openalpaca_storage::repository::FollowupRepository::new(db);
                match repo.claim_next(&lane_key) {
                    Ok(Some(row)) => match serde_json::from_str(&row.principal_json) {
                        Ok(principal) => {
                            tracing::info!(
                                followup_id = row.id,
                                lane_key = %lane_key,
                                "Auto-starting queued follow-up"
                            );
                            // The claim re-homed the lane onto the session the
                            // item was promised in (§5.3) — say so, or a second
                            // window keeps showing the conversation that just
                            // stepped down.
                            if let Some(ref session_id) = row.session_id {
                                bus.publish(SystemEvent::SessionChanged {
                                    session_id: session_id.clone(),
                                    lane_key: lane_key.clone(),
                                    status: openalpaca_storage::SESSION_ACTIVE.to_string(),
                                    timestamp: Utc::now(),
                                });
                            }
                            let scope = match row.workspace_path.clone() {
                                Some(path) => crate::security::policy::Scope::Workspace { path },
                                None => crate::security::policy::Scope::Global,
                            };
                            runner.spawn_followup(crate::orchestrator::FollowupItem {
                                id: row.id,
                                lane_key: row.lane_key,
                                content: row.content,
                                principal,
                                scope,
                                workspace_path: row.workspace_path,
                                source_task_id: row.source_task_id,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                followup_id = row.id,
                                "Cancelling follow-up with unparseable principal: {e}"
                            );
                            let _ = repo.mark_cancelled(row.id);
                        }
                    },
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(lane_key = %lane_key, "Failed to claim follow-up: {e}");
                    }
                }
            }

            // Memory extraction from lead agent output (non-blocking)
            if let Some(ref db) = db {
                spawn_task_memory_extraction(
                    db,
                    &router,
                    &embedder,
                    &daemon_config,
                    &created_by,
                    &task_id,
                    &description,
                    &final_content,
                    "lead_agent",
                    result.success,
                    workspace.workspace_id,
                );
            }

            tracing::info!(
                "Lead agent execution for task '{}' finished: success={}, subagents={}, rounds={}, runtime={}s",
                task_id,
                result.success,
                result.subagents_spawned,
                result.loop_result.rounds_used,
                runtime_secs
            );
        });
    }
}
