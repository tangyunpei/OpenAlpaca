use super::TaskDispatcher;
use crate::agent::subagent::{AgentStatus, SubAgent};
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

        // Assign agents
        let mut assignments = Vec::new();
        let now = Utc::now();
        for skill_match in &matches {
            task_lane.assign_agent(skill_match.agent_id.clone());

            // Update agent status to Busy
            self.shared_context.agent_registry.update_status(
                &skill_match.agent_id,
                AgentStatus::Busy {
                    task_id: task_id.clone(),
                },
            );

            // Emit AgentStatusChanged
            self.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: skill_match.agent_id.clone(),
                status: "busy".to_string(),
                current_task_id: Some(task_id.clone()),
                timestamp: now,
            });

            assignments.push(serde_json::json!({
                "agent_id": skill_match.agent_id,
                "agent_name": skill_match.agent_name,
                "matched_skills": skill_match.matched_skills,
                "role": skill_match.role_description,
            }));
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

        // Collect agents with their assignment IDs and role descriptions for the pipeline
        let agents_with_assignments: Vec<(SubAgent, Option<String>, String)> = matches
            .iter()
            .filter_map(|skill_match| {
                let agent = self.shared_context.agent_registry.get(&skill_match.agent_id)?;
                let assign_id = assignment_ids.get(&skill_match.agent_id).cloned();
                Some((agent, assign_id, skill_match.role_description.clone()))
            })
            .collect();

        // Verify all agents were collected (guard against race between availability check and registry lookup)
        if agents_with_assignments.len() < matches.len() {
            tracing::error!(
                "Pipeline assembly failed: expected {} agents but got {}. Releasing all agents.",
                matches.len(),
                agents_with_assignments.len()
            );
            // Release all agents that were set to Busy
            let now = Utc::now();
            for skill_match in &matches {
                self.shared_context
                    .agent_registry
                    .update_status(&skill_match.agent_id, AgentStatus::Idle);
                self.bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: skill_match.agent_id.clone(),
                    status: "idle".to_string(),
                    current_task_id: None,
                    timestamp: now,
                });
            }
            self.shared_context
                .task_registry
                .update_status(&task_id, TaskEntryStatus::Failed);
            // Also update DB to keep it in sync with in-memory registry
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
