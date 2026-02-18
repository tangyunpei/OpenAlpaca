use super::TaskDispatcher;
use crate::agent::registry::DestroyOutcome;
use crate::agent::subagent::SubAgent;
use crate::context::TaskEntryStatus;
use crate::events::SystemEvent;
use chrono::Utc;
use std::collections::HashMap;
use super::super::skill_matcher::SkillMatch;
use super::super::task_planner::TaskDag;
use super::super::task_state::TaskState;
use uuid::Uuid;

impl TaskDispatcher {
    /// Core dispatch logic shared by both heuristic and LLM-planned paths.
    ///
    /// Spawns an agent instance from each matched template. If any spawn fails,
    /// all previously spawned instances are destroyed (rolled back).
    pub(super) fn dispatch_core(
        &self,
        description: &str,
        title: String,
        matches: Vec<SkillMatch>,
        created_by: &str,
        lane_key: &str,
        source: &str,
        dag: Option<TaskDag>,
        workspace_id: Option<String>,
    ) -> Result<String, String> {
        let task_id = Uuid::new_v4().to_string();

        // Register in task_registry
        self.shared_context
            .task_registry
            .register(task_id.clone(), title.clone());

        // Create TaskLane
        let task_lane = self.lane_manager.create_task_lane(&task_id);

        // Spawn an instance from each matched template.
        // If any spawn fails, destroy all previously spawned instances.
        let mut assignments = Vec::new();
        let mut spawned_instances: Vec<(String, SubAgent)> = Vec::new(); // (template_id, instance)
        let now = Utc::now();
        for skill_match in &matches {
            // skill_match.agent_id is a template_id from the skill matcher
            match self
                .shared_context
                .agent_registry
                .spawn_instance(&skill_match.agent_id, task_id.clone())
            {
                Ok(instance) => {
                    task_lane.assign_agent(instance.id.clone());

                    self.bus.publish(SystemEvent::AgentStatusChanged {
                        agent_id: instance.id.clone(),
                        instance_id: instance.id.clone(),
                        template_id: instance.template_id.clone(),
                        status: "spawned".to_string(),
                        current_task_id: Some(task_id.clone()),
                        timestamp: now,
                    });

                    assignments.push(serde_json::json!({
                        "agent_id": skill_match.agent_id,
                        "agent_name": skill_match.agent_name,
                        "matched_skills": skill_match.matched_skills,
                        "role": skill_match.role_description,
                    }));

                    spawned_instances.push((skill_match.agent_id.clone(), instance));
                }
                Err(e) => {
                    // Roll back: destroy all previously spawned instances
                    tracing::warn!(
                        "Failed to spawn agent '{}' for task '{}': {}. Rolling back {} instances.",
                        skill_match.agent_id,
                        task_id,
                        e,
                        spawned_instances.len()
                    );
                    let rollback_now = Utc::now();
                    for (_, inst) in &spawned_instances {
                        let outcome = self.shared_context
                            .agent_registry
                            .destroy_instance(&inst.id);
                        let status = match outcome {
                            DestroyOutcome::ResetToIdle => "idle",
                            _ => "destroyed",
                        };
                        self.bus.publish(SystemEvent::AgentStatusChanged {
                            agent_id: inst.id.clone(),
                            instance_id: inst.id.clone(),
                            template_id: inst.template_id.clone(),
                            status: status.to_string(),
                            current_task_id: None,
                            timestamp: rollback_now,
                        });
                    }
                    // Task never started — remove from registry entirely
                    self.shared_context.task_registry.remove(&task_id);
                    return Err(format!(
                        "Cannot start task — agent '{}' ({}) unavailable: {}",
                        skill_match.agent_name, skill_match.agent_id, e
                    ));
                }
            }
        }

        // Emit TaskCreated
        self.bus.publish(SystemEvent::TaskCreated {
            task_id: task_id.clone(),
            title: title.clone(),
            created_by: created_by.to_string(),
            timestamp: now,
        });

        // Persist task and assignments to DB
        let mut assignment_ids: HashMap<String, String> = HashMap::new();

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
            };

            let db_assignments: Vec<openalpaca_storage::TaskAgentAssignment> = matches
                .iter()
                .enumerate()
                .map(|(i, skill_match)| {
                    let id = Uuid::new_v4().to_string();
                    assignment_ids.insert(skill_match.agent_id.clone(), id.clone());
                    openalpaca_storage::TaskAgentAssignment {
                        id,
                        task_id: task_id.clone(),
                        agent_id: skill_match.agent_id.clone(),
                        role: skill_match.role_description.clone(),
                        status: openalpaca_storage::AssignmentStatus::Pending,
                        step_order: Some(i as i32),
                        started_at: None,
                        completed_at: None,
                        result_output: None,
                    }
                })
                .collect();

            if let Err(e) = repo.create_task_with_assignments(&task, &db_assignments) {
                tracing::warn!("Failed to persist task+assignments to DB: {e}");
            }
        }

        // Initialize state_json for working memory
        if let Some(ref db) = self.db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let step_info: Vec<(String, String, String)> = matches.iter()
                .map(|m| (m.agent_id.clone(), m.agent_name.clone(), m.role_description.clone()))
                .collect();
            let mut initial_state = TaskState::initial(description, &step_info);
            initial_state.dag = dag.clone();
            let _ = repo.update_state(&task_id, &initial_state.to_json(), 0);
        }

        // Build agents_with_assignments from spawned instances
        let agents_with_assignments: Vec<(SubAgent, Option<String>, String)> = spawned_instances
            .iter()
            .zip(matches.iter())
            .map(|((template_id, instance), skill_match)| {
                let assign_id = assignment_ids.get(template_id).cloned();
                (instance.clone(), assign_id, skill_match.role_description.clone())
            })
            .collect();

        // All instances were spawned above; this guard handles the (shouldn't happen) case
        if agents_with_assignments.len() < matches.len() {
            tracing::error!(
                "Pipeline assembly failed: expected {} agents but got {}. Destroying all instances.",
                matches.len(),
                agents_with_assignments.len()
            );
            let now = Utc::now();
            for (_, inst) in &spawned_instances {
                let outcome = self.shared_context.agent_registry.destroy_instance(&inst.id);
                let status = match outcome {
                    DestroyOutcome::ResetToIdle => "idle",
                    _ => "destroyed",
                };
                self.bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: inst.id.clone(),
                    instance_id: inst.id.clone(),
                    template_id: inst.template_id.clone(),
                    status: status.to_string(),
                    current_task_id: None,
                    timestamp: now,
                });
            }
            self.shared_context
                .task_registry
                .update_status(&task_id, TaskEntryStatus::Failed);
            if let Some(ref db) = self.db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                if let Err(e) = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed) {
                    tracing::warn!("Failed to update task status to Failed in DB: {e}");
                }
            }
            return Err(
                "Pipeline assembly failed: some agents became unavailable".to_string(),
            );
        }

        // Choose execution path: DAG-parallel or sequential pipeline
        if let Some(dag) = dag {
            self.spawn_dag_execution(
                task_id.clone(),
                title.clone(),
                description.to_string(),
                dag,
                created_by.to_string(),
                lane_key.to_string(),
                source.to_string(),
                workspace_id,
            );
        } else {
            self.spawn_agent_pipeline(
                task_id.clone(),
                title.clone(),
                description.to_string(),
                agents_with_assignments,
                lane_key.to_string(),
                source.to_string(),
                created_by.to_string(),
                workspace_id,
            );
        }

        // Build human-readable response for chat
        let agent_list: Vec<String> = assignments.iter().map(|a| {
            format!("- {} ({})", a["agent_name"].as_str().unwrap_or("Unknown"),
                    a["matched_skills"].as_array().map(|s|
                        s.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")
                    ).unwrap_or_default())
        }).collect();

        Ok(format!(
            "I've created a task and assigned it to the following agents:\n\n{}\n\nTask: {}\nYou'll see the results here when the task completes.",
            agent_list.join("\n"), title
        ))
    }
}
