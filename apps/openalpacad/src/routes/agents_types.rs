//! Request/response types for agent management endpoints.

use openalpaca_core::agent::AgentConfigFile;
use openalpaca_storage::{AgentMetrics, SubAgentConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ListAgentsQuery {
    pub status: Option<String>,
    pub skill: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct AgentActionRequest {
    pub action: String, // "pause", "resume"
}

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub agent: SubAgentConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<AgentMetrics>,
}

#[derive(Debug, Serialize)]
pub struct AgentConfigResponse {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentConfigRequest {
    pub config: AgentConfigFile,
    pub config_version: u64,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentFromTomlRequest {
    pub toml_content: String,
}

/// JSON representation of an agent template for the REST API.
#[derive(Debug, Serialize)]
pub struct TemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub singleton: bool,
    pub capabilities: Vec<String>,
    pub denied_capabilities: Vec<String>,
    pub temperature: f32,
    pub verbosity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub fallback_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_per_task: Option<f64>,
    pub require_confirmation_for: Vec<String>,
    pub persona: String,
    pub body: String,
    /// How many runs this template has ever had (GAP-20), counted from
    /// `subagent_span`: one row per spawned subagent, opened at spawn time, so
    /// a run still in flight is included. `0` for a template nothing has
    /// spawned — never omitted, so the client can render "0 runs".
    pub run_count: i64,
    /// When the newest of those runs started (RFC 3339, UTC). Absent — not
    /// null — for a template with no runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
}

impl TemplateResponse {
    /// `runs` is this template's entry from one grouped
    /// `SubagentSpanRepository::run_counts_by_template` query — passed in
    /// rather than looked up here, so listing N templates stays one query.
    pub fn from_template(
        t: &openalpaca_core::agent::AgentTemplate,
        runs: Option<&openalpaca_storage::TemplateRunCount>,
    ) -> Self {
        let persona = openalpaca_core::agent::template::extract_persona(t);
        let fm = &t.frontmatter;
        Self {
            id: fm.id.clone(),
            name: fm.name.clone(),
            description: fm.description.clone(),
            icon: fm.icon.clone(),
            singleton: fm.singleton,
            capabilities: fm.capabilities.clone(),
            denied_capabilities: fm.denied_capabilities.clone(),
            temperature: fm.temperature,
            verbosity: fm.verbosity.clone(),
            model: fm.model.clone(),
            fallback_models: fm.fallback_models.clone(),
            max_tool_calls: fm.max_tool_calls,
            timeout_seconds: fm.timeout_seconds,
            max_cost_per_task: fm.max_cost_per_task,
            require_confirmation_for: fm.require_confirmation_for.clone(),
            persona,
            body: t.body.clone(),
            run_count: runs.map(|r| r.run_count).unwrap_or(0),
            last_run_at: runs.and_then(|r| r.last_run_at.clone()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTemplateRequest {
    pub config: AgentConfigFile,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTemplateRequest {
    pub config: AgentConfigFile,
}

/// JSON representation of an active agent instance.
#[derive(Debug, Serialize)]
pub struct InstanceResponse {
    pub id: String,
    pub template_id: String,
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::TemplateRunCount;

    fn template() -> openalpaca_core::agent::AgentTemplate {
        openalpaca_core::agent::template::parse_agent_markdown(
            "---\nid: review_agent\nname: Review\ndescription: Reviews things\n---\n\n## Persona\n\nYou review.\n",
        )
        .expect("parses")
    }

    /// GAP-20 — a template row carries how many runs it has and when the last
    /// one started, counted from `subagent_span` rather than the
    /// `agent_task_history` rows P8 deleted from the task routes.
    #[test]
    fn a_template_row_carries_its_run_count_and_last_run() {
        let runs = TemplateRunCount {
            run_count: 12,
            last_run_at: Some("2026-09-04T09:15:00.000Z".to_string()),
        };
        let v = serde_json::to_value(TemplateResponse::from_template(&template(), Some(&runs)))
            .unwrap();

        assert_eq!(v["id"], "review_agent");
        assert_eq!(v["run_count"], 12);
        assert_eq!(v["last_run_at"], "2026-09-04T09:15:00.000Z");
    }

    /// A template nothing has ever spawned reports an explicit `0` — the panel
    /// says "0 runs", it does not hide the row or guess.
    #[test]
    fn a_template_that_never_ran_reports_zero_runs() {
        let v = serde_json::to_value(TemplateResponse::from_template(&template(), None)).unwrap();

        assert_eq!(v["run_count"], 0);
        assert!(
            v.get("last_run_at").is_none(),
            "no run means no last-run stamp, not a fabricated one"
        );
    }
}
