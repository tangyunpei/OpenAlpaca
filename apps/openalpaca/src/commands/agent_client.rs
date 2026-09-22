//! Agent wire operations shared by the CLI and the configuration TUI.

use anyhow::Result;
use dialoguer::{Input, theme::ColorfulTheme};
use openalpaca_core::agent::config::{
    AgentCapabilitiesConfig, AgentConfigFile, AgentConstraintsConfig, AgentLlmConfigFile,
    AgentMeta, AgentPresetConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::DaemonClient;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AgentDetailItem {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub skills: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct AgentConfigResponse {
    pub config: Value,
    pub config_version: u64,
}

/// Each UI chooses its own defaults before constructing this value.
pub(super) struct AgentDraft<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub model: &'a str,
    pub skills: &'a [String],
    pub max_cost: f64,
    /// Always sent — `AgentPresetConfig.persona` is not optional.
    pub persona: &'a str,
    pub temperature: Option<f64>,
}

impl AgentDraft<'_> {
    /// The body `POST /v1/agents` accepts, built from the daemon's own
    /// `AgentConfigFile` so the compiler owns the contract.
    pub fn config(&self) -> Value {
        let file = AgentConfigFile {
            agent: AgentMeta {
                id: self.id.to_owned(),
                name: self.name.to_owned(),
                description: self.description.to_owned(),
                icon: None,
            },
            capabilities: AgentCapabilitiesConfig {
                assigned: self.skills.to_vec(),
                denied: None,
            },
            preset: AgentPresetConfig {
                persona: self.persona.to_owned(),
                temperature: self.temperature.map(|value| value as f32),
                verbosity: None,
            },
            constraints: (self.max_cost > 0.0).then_some(AgentConstraintsConfig {
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: Some(self.max_cost),
                require_confirmation_for: None,
                allowed_capabilities: None,
                denied_capabilities: None,
                allowed_models: None,
                denied_models: None,
                auto_approve: None,
                denied_sections: None,
                max_context_tokens: None,
            }),
            llm: (!self.model.is_empty()).then(|| AgentLlmConfigFile {
                model: Some(self.model.to_owned()),
                fallback_models: None,
                overrides: None,
            }),
        };
        serde_json::to_value(file).expect("AgentConfigFile always serialises")
    }
}

/// The daemon's own cap (`MAX_AGENT_ID_LEN` in
/// `crates/openalpaca_core/src/agent/config_service.rs`). Kept here so the
/// wizard re-prompts instead of letting the daemon refuse the finished form.
const MAX_AGENT_ID_LEN: usize = 64;

/// The daemon turns the id into `<config_dir>/<id>.toml`, so an agent id has to
/// be a safe file name on its own. This is the daemon's grammar verbatim
/// (`crates/openalpaca_core/src/agent/config_service.rs`): every id this accepts
/// the daemon accepts, and every id it rejects the daemon rejects.
pub(super) fn validate_agent_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Agent id cannot be empty".to_owned());
    }
    if id.len() > MAX_AGENT_ID_LEN {
        return Err(format!(
            "Agent id is too long: {} bytes, maximum is {MAX_AGENT_ID_LEN}",
            id.len()
        ));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("Agent id may contain only letters, digits, '-' and '_'".to_owned());
    }
    Ok(())
}

/// An id suggestion derived from the agent's name: lowercased, with every run
/// of anything else collapsed to a single hyphen, cut to the daemon's length
/// cap. Empty when nothing survives — the prompt then offers no default rather
/// than one the daemon would refuse.
pub(super) fn default_agent_id(name: &str) -> String {
    let mut derived = String::new();
    for ch in name.chars() {
        if derived.len() >= MAX_AGENT_ID_LEN {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            derived.push(ch.to_ascii_lowercase());
        } else if !derived.ends_with('-') {
            derived.push('-');
        }
    }
    derived.trim_matches('-').to_owned()
}

/// Ask for the agent id, offering the name-derived default that Enter accepts.
/// Re-prompts until the id is one the daemon can safely turn into a file name.
pub(super) fn prompt_agent_id(theme: &ColorfulTheme, name: &str) -> Result<String> {
    let mut prompt = Input::<String>::with_theme(theme)
        .with_prompt("Agent id (letters, digits, '-' and '_')")
        .validate_with(|id: &String| validate_agent_id(id));
    let suggested = default_agent_id(name);
    if !suggested.is_empty() {
        prompt = prompt.default(suggested);
    }
    Ok(prompt.interact_text()?)
}

pub(super) async fn models(client: &DaemonClient) -> Result<Vec<String>> {
    let models: Vec<Value> = client.get("/v1/models").await?;
    Ok(models
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect())
}

pub(super) async fn config(client: &DaemonClient, id: &str) -> Result<AgentConfigResponse> {
    client.get(&format!("/v1/agents/{id}/config")).await
}

pub(super) async fn save_config(
    client: &DaemonClient,
    id: &str,
    config: &Value,
    version: u64,
) -> Result<Value> {
    client
        .put(
            &format!("/v1/agents/{id}/config"),
            &json!({ "config": config, "config_version": version }),
        )
        .await
}

pub(super) async fn create(client: &DaemonClient, config: &Value) -> Result<Value> {
    client
        .post("/v1/agents", &json!({ "config": config }))
        .await
}

pub(super) async fn action(client: &DaemonClient, id: &str, action: &str) -> Result<Value> {
    client
        .post(
            &format!("/v1/agents/{id}/action"),
            &json!({ "action": action }),
        )
        .await
}

pub(super) async fn remove(client: &DaemonClient, id: &str) -> Result<Value> {
    client.delete_req(&format!("/v1/agents/{id}")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_requests_preserve_endpoints_payloads_and_optimistic_versions() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = DaemonClient::for_tests(
            &format!("http://{}", listener.local_addr().unwrap()),
            "test",
        );
        let config_value = json!({"name":"agent", "llm":{"model":"local"}});
        let expected_config = config_value.clone();
        let server = tokio::spawn(async move {
            let expectations = [
                (
                    "GET /v1/models HTTP/1.1",
                    Value::Null,
                    json!([{"id":"local"},{"name":"skip"}]),
                    "200 OK",
                ),
                (
                    "GET /v1/agents/a/config HTTP/1.1",
                    Value::Null,
                    json!({"config":expected_config,"config_version":9}),
                    "200 OK",
                ),
                (
                    "PUT /v1/agents/a/config HTTP/1.1",
                    json!({"config":expected_config,"config_version":9}),
                    json!({"error":{"code":"CONFIG_CONFLICT","message":"retry"}}),
                    "409 Conflict",
                ),
                (
                    "POST /v1/agents HTTP/1.1",
                    json!({"config":expected_config}),
                    json!({"agent_id":"a"}),
                    "200 OK",
                ),
                (
                    "POST /v1/agents/a/action HTTP/1.1",
                    json!({"action":"pause"}),
                    json!({"status":"paused"}),
                    "200 OK",
                ),
                (
                    "DELETE /v1/agents/a HTTP/1.1",
                    Value::Null,
                    json!({"status":"archived"}),
                    "200 OK",
                ),
            ];
            for (request_line, body, response, status) in expectations {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                let mut chunk = [0; 2048];
                let header_end = loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&chunk[..count]);
                    if let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
                assert_eq!(headers.lines().next(), Some(request_line));
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while received.len() < header_end + length {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    received.extend_from_slice(&chunk[..count]);
                }
                let actual = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&received[header_end..header_end + length]).unwrap()
                };
                assert_eq!(actual, body);
                let response = response.to_string();
                socket.write_all(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", response.len(), response).as_bytes()).await.unwrap();
            }
        });
        assert_eq!(models(&client).await.unwrap(), vec!["local"]);
        let found = config(&client, "a").await.unwrap();
        assert_eq!(found.config_version, 9);
        let conflict = save_config(&client, "a", &found.config, found.config_version)
            .await
            .unwrap_err();
        assert!(conflict.to_string().contains("CONFIG_CONFLICT"));
        assert_eq!(
            create(&client, &config_value).await.unwrap()["agent_id"],
            "a"
        );
        assert_eq!(
            action(&client, "a", "pause").await.unwrap()["status"],
            "paused"
        );
        assert_eq!(remove(&client, "a").await.unwrap()["status"], "archived");
        server.await.unwrap();
    }

    /// The assertion that would have caught the 422: the body the wizards post
    /// has to be a body `POST /v1/agents` can deserialise.
    #[test]
    fn draft_config_deserialises_into_the_route_body() {
        let skills = vec!["research".to_owned()];
        let draft = AgentDraft {
            id: "researcher",
            name: "Researcher",
            description: "Reads things",
            model: "local",
            skills: &skills,
            max_cost: 1.5,
            persona: "You are a helpful assistant.",
            temperature: Some(0.5),
        };
        let body = json!({ "config": draft.config() });
        let parsed: AgentConfigFile = serde_json::from_value(body["config"].clone()).unwrap();
        assert_eq!(parsed.agent.id, "researcher");
        assert_eq!(parsed.agent.name, "Researcher");
        assert_eq!(parsed.agent.description, "Reads things");
        assert_eq!(parsed.capabilities.assigned, skills);
        assert_eq!(parsed.preset.persona, "You are a helpful assistant.");
        assert_eq!(parsed.preset.temperature, Some(0.5));
        assert_eq!(parsed.constraints.unwrap().max_cost_per_task, Some(1.5_f64));
        assert_eq!(parsed.llm.unwrap().model.as_deref(), Some("local"));
    }

    #[test]
    fn draft_omits_the_optional_sections_and_still_deserialises() {
        let draft = AgentDraft {
            id: "researcher",
            name: "Researcher",
            description: "",
            model: "local",
            skills: &[],
            max_cost: 0.0,
            // The CLI leaves the persona empty; the field is not optional.
            persona: "",
            temperature: None,
        };
        let config = draft.config();
        assert_eq!(
            config["preset"],
            json!({"persona":"", "temperature":null, "verbosity":null})
        );
        assert_eq!(config["constraints"], Value::Null);
        let parsed: AgentConfigFile = serde_json::from_value(config).unwrap();
        assert_eq!(parsed.preset.persona, "");
        assert!(parsed.constraints.is_none());
        assert!(parsed.capabilities.assigned.is_empty());
    }

    /// The wizard's rule has to be the daemon's rule: an id this accepts that
    /// the daemon then refuses would fail after the whole form is filled in.
    #[test]
    fn agent_id_accepts_only_safe_file_names() {
        assert!(validate_agent_id("my-agent_2").is_ok());
        assert!(validate_agent_id(&"a".repeat(MAX_AGENT_ID_LEN)).is_ok());
        for rejected in ["", "../x", "a/b", "a b", "a.b", "a\\b", "a\nb"] {
            assert!(
                validate_agent_id(rejected).is_err(),
                "expected {rejected:?} to be rejected"
            );
        }
        assert!(
            validate_agent_id(&"a".repeat(MAX_AGENT_ID_LEN + 1)).is_err(),
            "an id past the daemon's cap must be refused here too"
        );
    }

    #[test]
    fn default_agent_id_derives_a_valid_suggestion_from_the_name() {
        assert_eq!(default_agent_id("My Agent"), "my-agent");
        assert_eq!(default_agent_id("  Code Review / QA  "), "code-review-qa");
        assert_eq!(default_agent_id("研究员"), "");
        assert!(validate_agent_id(&default_agent_id("My Agent")).is_ok());
        // A long name still yields a suggestion the daemon will take.
        let from_long_name = default_agent_id(&"Very Long Agent Name ".repeat(20));
        assert!(from_long_name.len() <= MAX_AGENT_ID_LEN);
        assert!(validate_agent_id(&from_long_name).is_ok());
    }
}
