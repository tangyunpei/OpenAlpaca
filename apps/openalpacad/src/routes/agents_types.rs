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

/// `GET /v1/agent-templates?window=` (GAP-20, T48). `None` (the key absent,
/// or `?window=`'s value omitted entirely) resolves to the plan's default of
/// `7d` — see `resolve_window`.
#[derive(Debug, Deserialize)]
pub struct ListTemplatesQuery {
    pub window: Option<String>,
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
    /// How many *completed* runs this template has had within `window`
    /// (GAP-20, T48), counted from `subagent_span`: `state != 'running'`, so a
    /// run still in flight is not counted. `0` for a template with no
    /// completed run in the window — never omitted, so the client can render
    /// "0 runs".
    pub run_count: i64,
    /// When the newest of those *completed* runs started (RFC 3339, UTC).
    /// Absent — not null — for a template with no runs in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<String>,
    /// The window `run_count`/`last_run_at` were computed over — `"7d"`,
    /// `"30d"` or `"all"` — so the client can label the row without having to
    /// remember what it asked for.
    pub window: String,
}

impl TemplateResponse {
    /// `runs` is this template's entry from one grouped
    /// `SubagentSpanRepository::run_counts_by_template` query — passed in
    /// rather than looked up here, so listing N templates stays one query.
    /// `window` is the resolved label (`resolve_window`'s first element),
    /// echoed onto every row of the same list.
    pub fn from_template(
        t: &openalpaca_core::agent::AgentTemplate,
        runs: Option<&openalpaca_storage::TemplateRunCount>,
        window: &str,
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
            window: window.to_string(),
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

    /// GAP-20/T48 — a template row carries how many *completed* runs it has
    /// within the window and when the last one started, counted from
    /// `subagent_span` rather than the `agent_task_history` rows P8 deleted
    /// from the task routes, and echoes back the window it was computed over.
    #[test]
    fn a_template_row_carries_its_run_count_last_run_and_window() {
        let runs = TemplateRunCount {
            run_count: 12,
            last_run_at: Some("2026-09-04T09:15:00.000Z".to_string()),
        };
        let v = serde_json::to_value(TemplateResponse::from_template(
            &template(),
            Some(&runs),
            "7d",
        ))
        .unwrap();

        assert_eq!(v["id"], "review_agent");
        assert_eq!(v["run_count"], 12);
        assert_eq!(v["last_run_at"], "2026-09-04T09:15:00.000Z");
        assert_eq!(v["window"], "7d");
    }

    /// A template with no *completed* run in the window reports an explicit
    /// `0` — the panel says "0 runs", it does not hide the row or guess — and
    /// still carries the window it was asked about.
    #[test]
    fn a_template_that_never_ran_in_the_window_reports_zero_runs() {
        let v = serde_json::to_value(TemplateResponse::from_template(&template(), None, "30d"))
            .unwrap();

        assert_eq!(v["run_count"], 0);
        assert!(
            v.get("last_run_at").is_none(),
            "no run means no last-run stamp, not a fabricated one"
        );
        assert_eq!(v["window"], "30d");
    }
}
