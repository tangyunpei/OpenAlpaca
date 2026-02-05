//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

use crate::agent::subagent::AgentStatus;
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::lane::LaneManager;
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

use super::skill_matcher::SkillMatcher;

/// Dispatches complex tasks by matching skills to agents and creating task lanes.
pub struct TaskDispatcher {
    shared_context: Arc<SharedContext>,
    lane_manager: Arc<LaneManager>,
    bus: EventBus,
    skill_matcher: SkillMatcher,
}

impl TaskDispatcher {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
    ) -> Self {
        Self {
            shared_context,
            lane_manager,
            bus,
            skill_matcher: SkillMatcher,
        }
    }

    /// Dispatch a complex task:
    /// 1. Match required skills to idle agents
    /// 2. Create task entry in registry
    /// 3. Create TaskLane
    /// 4. Assign agents to lane, update their status to Busy
    /// 5. Emit TaskCreated + AgentStatusChanged events
    /// 6. Return JSON response
    pub fn dispatch(
        &self,
        request_id: Uuid,
        _source: &str,
        description: &str,
        required_skills: &[String],
        created_by: &str,
    ) -> Result<String, String> {
        // 1. Find matching agents
        let matches = self
            .skill_matcher
            .match_skills(required_skills, &self.shared_context.agent_registry)?;

        // 2. Generate task_id and title
        let task_id = Uuid::new_v4().to_string();
        let title = if description.len() > 60 {
            format!("{}...", &description[..57])
        } else {
            description.to_string()
        };

        // 3. Register in task_registry
        self.shared_context
            .task_registry
            .register(task_id.clone(), title.clone());

        // 4. Create TaskLane
        let task_lane = self.lane_manager.create_task_lane(&task_id);

        // 5. Assign agents
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

        // 6. Emit TaskCreated
        self.bus.publish(SystemEvent::TaskCreated {
            task_id: task_id.clone(),
            title: title.clone(),
            created_by: created_by.to_string(),
            timestamp: now,
        });

        // 7. Return JSON response
        let response = serde_json::json!({
            "request_id": request_id.to_string(),
            "task_id": task_id,
            "title": title,
            "status": "queued",
            "assignments": assignments,
        });

        Ok(response.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentPreset, AgentStatus, Skill, SubAgent};

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            name: format!("Agent {}", id),
            description: Some(format!("{} agent", id)),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .into_iter()
                .map(|s| Skill {
                    name: s.to_string(),
                    category: "test".to_string(),
                    proficiency: 1.0,
                })
                .collect(),
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
        }
    }

    fn setup(agents: Vec<SubAgent>) -> TaskDispatcher {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
        }
        let lane_mgr = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        TaskDispatcher::new(ctx, lane_mgr, bus)
    }

    #[test]
    fn test_creates_task_and_lane() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let result = dispatcher.dispatch(
            Uuid::new_v4(),
            "cli",
            "Search for Rust docs",
            &["web_search".to_string()],
            "user1",
        );

        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "queued");
        assert!(json["task_id"].as_str().is_some());
        assert_eq!(json["assignments"].as_array().unwrap().len(), 1);

        // Verify task registered
        assert_eq!(dispatcher.shared_context.task_registry.count(), 1);
        // Verify lane created
        assert_eq!(dispatcher.lane_manager.task_count(), 1);
    }

    #[test]
    fn test_fails_no_matching() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let result = dispatcher.dispatch(
            Uuid::new_v4(),
            "cli",
            "Generate text",
            &["text_generate".to_string()],
            "user1",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_assigns_multiple() {
        let dispatcher = setup(vec![
            make_agent("a1", vec!["web_search"]),
            make_agent("a2", vec!["text_generate"]),
        ]);
        let result = dispatcher
            .dispatch(
                Uuid::new_v4(),
                "cli",
                "Research and write report",
                &["web_search".to_string(), "text_generate".to_string()],
                "user1",
            )
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["assignments"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_title_short() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let desc = "Short task";
        let result = dispatcher
            .dispatch(
                Uuid::new_v4(),
                "cli",
                desc,
                &["web_search".to_string()],
                "user1",
            )
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(json["title"], "Short task");
    }

    #[test]
    fn test_title_long() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let desc = "A".repeat(100);
        let result = dispatcher
            .dispatch(
                Uuid::new_v4(),
                "cli",
                &desc,
                &["web_search".to_string()],
                "user1",
            )
            .unwrap();

        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let title = json["title"].as_str().unwrap();
        assert_eq!(title.len(), 60); // "AAA...57 chars...AAA..."
        assert!(title.ends_with("..."));
    }
}
