//! LLM-based task planning: replaces keyword heuristics with a single LLM call
//! that classifies intent, generates a title, and assigns agents.

use crate::agent::subagent::SubAgent;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use serde::{Deserialize, Serialize};

/// The result of an LLM planning call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub classification: String,
    pub title: Option<String>,
    pub assignments: Vec<PlannedAssignment>,
    pub reasoning: Option<String>,
}

/// An agent assignment decided by the LLM planner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedAssignment {
    pub agent_id: String,
    pub agent_name: String,
    pub role_description: String,
    pub matched_skills: Vec<String>,
}

/// Errors from the task planner.
#[derive(Debug)]
pub enum PlanError {
    MalformedResponse(String),
    LlmError(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::MalformedResponse(msg) => write!(f, "Malformed response: {}", msg),
            PlanError::LlmError(msg) => write!(f, "LLM error: {}", msg),
        }
    }
}

pub struct TaskPlanner;

impl TaskPlanner {
    /// Call the LLM to classify a user message and optionally assign agents.
    pub async fn plan(
        router: &LlmRouter,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt = Self::build_system_prompt(idle_agents);

        let history_tail = if history.len() > 6 {
            &history[history.len() - 6..]
        } else {
            history
        };
        let mut messages = Vec::with_capacity(2 + history_tail.len());
        messages.push(ChatMessage::system(&system_prompt));
        messages.extend_from_slice(history_tail);
        messages.push(ChatMessage::user(user_message));

        let request = RouterRequest {
            model: None,
            messages,
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(1024),
            context: RequestContext::default(),
        };

        let response = router
            .complete(request)
            .await
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        Self::parse_response(&response.content)
    }

    /// Build the system prompt listing available agents.
    fn build_system_prompt(idle_agents: &[SubAgent]) -> String {
        let mut prompt = String::from(
            "You are a task router for OpenAlpaca. Classify the user message and, if it requires work, assign agents.\n\n",
        );

        prompt.push_str("## Available Agents\n");
        if idle_agents.is_empty() {
            prompt.push_str("No agents are currently available.\n");
        } else {
            for agent in idle_agents {
                let desc = agent
                    .description
                    .as_deref()
                    .unwrap_or("No description");
                let skills_str: Vec<String> = agent
                    .skills
                    .iter()
                    .map(|s| format!("{} ({:.1})", s.name, s.proficiency))
                    .collect();
                prompt.push_str(&format!(
                    "- ID: \"{}\", Name: \"{}\", Description: \"{}\", Skills: {}\n",
                    agent.id,
                    agent.name,
                    desc,
                    if skills_str.is_empty() {
                        "none".to_string()
                    } else {
                        skills_str.join(", ")
                    }
                ));
            }
        }

        prompt.push_str(
            r#"
## Response Format (JSON only, no other text)
Simple query: {"classification": "simple_query", "title": null, "assignments": [], "reasoning": "..."}
Complex task:  {"classification": "complex_task", "title": "Concise title", "assignments": [{"agent_id": "...", "agent_name": "...", "role_description": "...", "matched_skills": ["..."]}], "reasoning": "..."}

## Rules
- Use exact agent_id values from the list above
- Title: imperative, max 50 chars (e.g. "Research Rust async patterns")
- Only classify as complex_task if agent work is needed
"#,
        );

        prompt
    }

    /// Parse the LLM response into a TaskPlan.
    fn parse_response(content: &str) -> Result<TaskPlan, PlanError> {
        let json_str = Self::extract_json(content);
        serde_json::from_str::<TaskPlan>(json_str).map_err(|e| {
            PlanError::MalformedResponse(format!(
                "Failed to parse JSON: {} (input: {})",
                e,
                &content.chars().take(200).collect::<String>()
            ))
        })
    }

    /// Extract JSON from a response that may be wrapped in markdown code fences.
    fn extract_json(content: &str) -> &str {
        let trimmed = content.trim();

        // Try ```json ... ``` first
        if let Some(start) = trimmed.find("```json") {
            let after_fence = &trimmed[start + 7..];
            if let Some(end) = after_fence.find("```") {
                return after_fence[..end].trim();
            }
        }

        // Try ``` ... ```
        if let Some(start) = trimmed.find("```") {
            let after_fence = &trimmed[start + 3..];
            if let Some(end) = after_fence.find("```") {
                return after_fence[..end].trim();
            }
        }

        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_query_response() {
        let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting"}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "simple_query");
        assert!(plan.title.is_none());
        assert!(plan.assignments.is_empty());
        assert_eq!(plan.reasoning.as_deref(), Some("This is a greeting"));
    }

    #[test]
    fn test_parse_complex_task_response() {
        let json = r#"{
            "classification": "complex_task",
            "title": "Research Rust async patterns",
            "assignments": [{
                "agent_id": "researcher-01",
                "agent_name": "Researcher",
                "role_description": "Search for information about Rust async patterns",
                "matched_skills": ["web_search", "summarize"]
            }],
            "reasoning": "User wants research, assigning researcher agent"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert_eq!(plan.title.as_deref(), Some("Research Rust async patterns"));
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].agent_id, "researcher-01");
        assert_eq!(plan.assignments[0].matched_skills, vec!["web_search", "summarize"]);
    }

    #[test]
    fn test_parse_response_with_markdown_fences() {
        let content = "```json\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"greeting\"}\n```";
        let plan = TaskPlanner::parse_response(content).unwrap();
        assert_eq!(plan.classification, "simple_query");
    }

    #[test]
    fn test_parse_response_with_plain_fences() {
        let content = "```\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"test\"}\n```";
        let plan = TaskPlanner::parse_response(content).unwrap();
        assert_eq!(plan.classification, "simple_query");
    }

    #[test]
    fn test_parse_malformed_response() {
        let result = TaskPlanner::parse_response("this is not json at all");
        assert!(result.is_err());
        match result.unwrap_err() {
            PlanError::MalformedResponse(msg) => {
                assert!(msg.contains("Failed to parse JSON"));
            }
            _ => panic!("Expected MalformedResponse"),
        }
    }

    #[test]
    fn test_build_system_prompt_with_agents() {
        use crate::agent::subagent::{
            AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill,
        };

        let agents = vec![SubAgent {
            id: "researcher-01".to_string(),
            name: "Researcher".to_string(),
            description: Some("Research agent".to_string()),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![
                Skill {
                    name: "web_search".to_string(),
                    category: "research".to_string(),
                    proficiency: 0.9,
                },
                Skill {
                    name: "summarize".to_string(),
                    category: "research".to_string(),
                    proficiency: 0.8,
                },
            ],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }];

        let prompt = TaskPlanner::build_system_prompt(&agents);
        assert!(prompt.contains("researcher-01"));
        assert!(prompt.contains("Researcher"));
        assert!(prompt.contains("Research agent"));
        assert!(prompt.contains("web_search (0.9)"));
        assert!(prompt.contains("summarize (0.8)"));
    }

    #[test]
    fn test_build_system_prompt_no_agents() {
        let prompt = TaskPlanner::build_system_prompt(&[]);
        assert!(prompt.contains("No agents are currently available"));
    }

    #[test]
    fn test_extract_json_bare() {
        let input = r#"{"classification": "simple_query"}"#;
        assert_eq!(TaskPlanner::extract_json(input), input);
    }

    #[test]
    fn test_extract_json_with_whitespace() {
        let input = "  \n{\"classification\": \"simple_query\"}\n  ";
        assert_eq!(
            TaskPlanner::extract_json(input),
            "{\"classification\": \"simple_query\"}"
        );
    }
}
