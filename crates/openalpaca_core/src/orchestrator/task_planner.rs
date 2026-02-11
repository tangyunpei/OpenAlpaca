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
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt = Self::build_system_prompt(idle_agents);

        let history_tail = if history.len() > 12 {
            &history[history.len() - 12..]
        } else {
            history
        };
        let mut messages = Vec::with_capacity(4 + history_tail.len());
        messages.push(ChatMessage::system(&system_prompt));

        // Inject summary before history
        if let Some(summary) = session_summary {
            messages.push(ChatMessage::system(&format!(
                "### SESSION SUMMARY ###\n{}",
                summary
            )));
        }

        // Inject active tasks block
        if let Some(tasks_block) = active_tasks_block {
            messages.push(ChatMessage::system(tasks_block));
        }

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
## Response Format
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.
The JSON object MUST contain exactly these four keys: "classification", "title", "assignments", "reasoning".
Do NOT include keys like "available_agents" or repeat the agent list.

Simple query example:
{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a casual greeting"}

Complex task example:
{"classification": "complex_task", "title": "Research Rust async patterns", "assignments": [{"agent_id": "...", "agent_name": "...", "role_description": "...", "matched_skills": ["..."]}], "reasoning": "User needs research done"}

## Rules
- "classification" MUST be either "simple_query" or "complex_task"
- Use exact agent_id values from the list above
- Title: imperative, max 50 chars (e.g. "Research Rust async patterns")
- Only classify as complex_task if agent work is needed
- **Agents run as a sequential pipeline**: agent 1 runs first, then agent 2 receives agent 1's output as context, and so on. Order matters — list them in execution order.
- Use multiple agents when the task has distinct stages requiring different skills (e.g. agent with file_read reads a file → agent with text_generate writes a polished summary using the file content).
- Use a single agent when one agent can handle the entire task alone (e.g. a file_read agent can also summarize what it reads via its LLM).
- Casual messages, greetings, short phrases, numbers, or anything that doesn't require agent tools should be "simple_query".
- If an active task already covers the user's request, classify as "simple_query" and explain the existing task in your reasoning.
"#,
        );

        prompt
    }

    /// Parse the LLM response into a TaskPlan.
    fn parse_response(content: &str) -> Result<TaskPlan, PlanError> {
        let json_str = Self::extract_json(content);

        // Primary: direct parse
        if let Ok(plan) = serde_json::from_str::<TaskPlan>(json_str) {
            return Ok(plan);
        }

        // Fallback: LLM may have wrapped the plan in a parent object (e.g. {"available_agents": ..., "classification": ...})
        // Try extracting known fields from a loose Value
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
            if let Some(classification) = obj.get("classification").and_then(|v| v.as_str()) {
                return Ok(TaskPlan {
                    classification: classification.to_string(),
                    title: obj.get("title").and_then(|v| v.as_str()).map(String::from),
                    assignments: obj
                        .get("assignments")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default(),
                    reasoning: obj.get("reasoning").and_then(|v| v.as_str()).map(String::from),
                });
            }
        }

        Err(PlanError::MalformedResponse(format!(
            "Failed to parse JSON: missing field `classification` (input: {})",
            &content.chars().take(200).collect::<String>()
        )))
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
        assert!(prompt.contains("If an active task already covers"));
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

    #[test]
    fn test_parse_response_with_extra_fields() {
        // LLM sometimes echoes back agent info alongside the classification
        let json = r#"{
            "available_agents": [{"agent_id": "writing_agent", "name": "Writer"}],
            "classification": "simple_query",
            "title": null,
            "assignments": [],
            "reasoning": "Greeting detected"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "simple_query");
        assert!(plan.assignments.is_empty());
    }

    #[test]
    fn test_parse_response_no_classification_at_all() {
        let json = r#"{"available_agents": [{"agent_id": "writing_agent"}]}"#;
        let result = TaskPlanner::parse_response(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            PlanError::MalformedResponse(msg) => {
                assert!(msg.contains("missing field `classification`"));
            }
            _ => panic!("Expected MalformedResponse"),
        }
    }
}
