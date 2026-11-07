//! Request/response types for agent management endpoints.

use openalpaca_core::agent::AgentConfigFile;
use openalpaca_core::agent::registry::{
    NotUsed, NotUsedReason, TemplateLayer, TemplateProvenance, TemplateState,
};
use openalpaca_core::bundled::ShippedVersion;
use openalpaca_storage::{AgentMetrics, SubAgentConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ListAgentsQuery {
    pub status: Option<String>,
    pub skill: Option<String>,
    pub limit: Option<usize>,
}

/// `GET /v1/agent-templates?window=` (GAP-20, T48). `None` (the key absent)
/// resolves to the plan's default of `7d` — see `resolve_window`. A present
/// but empty value (`?window=`) deserialises as `Some("")` and is refused with
/// `400 UNKNOWN_WINDOW` like any other unknown spelling.
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
    /// Where the template comes from (subagent design §2.7 R-1): `"bundled"`
    /// (the binary), `"file"` (`config/agents/`) or `"plugin"`.
    pub source: String,
    /// The plugin's id for a plugin's template, else null.
    pub plugin: Option<String>,
    /// The file the template is read from (or, withheld, the file that
    /// withholds it), else null.
    pub path: Option<String>,
    /// Whether a file of the person's stands over the bundled template of
    /// this id.
    pub overrides_bundled: bool,
    /// `"ready"`, or `"withheld"`: a file holds the id and cannot be used
    /// (L-4), and a spawn of it is refused naming the file.
    pub state: String,
    /// The definitions of this id that do not run: a bundled template
    /// overridden, unedited copies set aside, duplicates, plugins refused.
    pub not_used: Vec<NotUsedResponse>,
    /// What the daemon found worth saying about the file: why it is
    /// withheld, a confirmation list naming no tool (W-1).
    pub warnings: Vec<String>,
}

/// One entry of a template row's `not_used` (R-1).
#[derive(Debug, Serialize)]
pub struct NotUsedResponse {
    /// `"bundled"`, `"file"` or `"plugin"`.
    pub source: String,
    pub path: Option<String>,
    pub plugin: Option<String>,
    /// `unedited_copy`, `overridden`, `duplicate`, `does_not_parse` or
    /// `plugin_refused`.
    pub reason: String,
    /// For an unedited copy: `"current"` or `"earlier"`.
    pub version: Option<String>,
    /// For an unedited copy the Installation Manual's step made:
    /// `manual_step_7` or `manual_step_8` (V-2a).
    pub edit: Option<String>,
}

fn layer_word(layer: TemplateLayer) -> &'static str {
    match layer {
        TemplateLayer::Bundled => "bundled",
        TemplateLayer::File => "file",
        TemplateLayer::Plugin => "plugin",
    }
}

impl NotUsedResponse {
    fn from_entry(entry: &NotUsed) -> Self {
        let (version, edit) = match entry.reason {
            NotUsedReason::UneditedCopy(copy) => (
                Some(match copy.version {
                    ShippedVersion::Current => "current",
                    ShippedVersion::Earlier => "earlier",
                }),
                copy.edit.map(|edit| edit.as_str()),
            ),
            _ => (None, None),
        };
        Self {
            source: layer_word(entry.layer).to_string(),
            path: entry.path.as_ref().map(|p| p.display().to_string()),
            plugin: entry.plugin.clone(),
            reason: entry.reason.as_str().to_string(),
            version: version.map(str::to_string),
            edit: edit.map(str::to_string),
        }
    }
}

impl TemplateResponse {
    /// `runs` is this template's entry from one grouped
    /// `SubagentSpanRepository::run_counts_by_template` query — passed in
    /// rather than looked up here, so listing N templates stays one query.
    /// `window` is the resolved label (`resolve_window`'s first element),
    /// echoed onto every row of the same list.
    ///
    /// `provenance` is `AgentRegistry::provenance` of the id; None reads as a
    /// file template with nothing to say (a template built in a test).
    pub fn from_template(
        t: &openalpaca_core::agent::AgentTemplate,
        provenance: Option<&TemplateProvenance>,
        runs: Option<&openalpaca_storage::TemplateRunCount>,
        window: &str,
    ) -> Self {
        let persona = openalpaca_core::agent::template::extract_persona(t);
        let fm = &t.frontmatter;
        let plugin = match &t.source {
            openalpaca_core::agent::AgentSource::Plugin { plugin_id, .. } => {
                Some(plugin_id.clone())
            }
            openalpaca_core::agent::AgentSource::Internal => None,
        };
        let layer = provenance.map_or(TemplateLayer::File, |p| p.layer);
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
            source: layer_word(layer).to_string(),
            plugin: if layer == TemplateLayer::Plugin {
                plugin
            } else {
                None
            },
            path: provenance
                .and_then(|p| p.path.as_ref())
                .map(|p| p.display().to_string()),
            overrides_bundled: provenance.is_some_and(TemplateProvenance::overrides_bundled),
            state: match provenance.map(|p| &p.state) {
                Some(TemplateState::Withheld { .. }) => "withheld",
                _ => "ready",
            }
            .to_string(),
            not_used: provenance
                .map(|p| p.not_used.iter().map(NotUsedResponse::from_entry).collect())
                .unwrap_or_default(),
            warnings: provenance.map(|p| p.warnings.clone()).unwrap_or_default(),
        }
    }

    /// The row of an id a file withholds (L-4): no definition runs under
    /// it, so the row carries none — its name is the id, its definition
    /// the parser's defaults — and `state`, `path` and `warnings` say why.
    pub fn withheld(
        id: &str,
        provenance: &TemplateProvenance,
        runs: Option<&openalpaca_storage::TemplateRunCount>,
        window: &str,
    ) -> Option<Self> {
        let stub = openalpaca_core::agent::template::parse_agent_markdown(&format!(
            "---\nid: \"{id}\"\nname: \"{id}\"\ndescription: \"\"\n---\n"
        ))
        .ok()
        .filter(|stub| stub.frontmatter.id == id)?;
        let mut row = Self::from_template(&stub, Some(provenance), runs, window);
        row.persona = String::new();
        Some(row)
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
            None,
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
        let v = serde_json::to_value(TemplateResponse::from_template(
            &template(),
            None,
            None,
            "30d",
        ))
        .unwrap();

        assert_eq!(v["run_count"], 0);
        assert!(
            v.get("last_run_at").is_none(),
            "no run means no last-run stamp, not a fabricated one"
        );
        assert_eq!(v["window"], "30d");
    }
}
