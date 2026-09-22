//! Agent wire operations shared by the CLI and the configuration TUI.

use anyhow::Result;
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
    pub name: &'a str,
    pub description: &'a str,
    pub model: &'a str,
    pub skills: &'a [String],
    pub max_cost: f64,
    pub persona: Option<&'a str>,
    pub temperature: Option<f64>,
}

impl AgentDraft<'_> {
    pub fn config(&self) -> Value {
        let mut config = json!({
            "name": self.name,
            "description": self.description,
            "llm": { "model": self.model },
            "skills": self.skills,
        });
        if self.max_cost > 0.0 {
            config["constraints"] = json!({ "max_cost_per_task": self.max_cost });
        }
        if let Some(persona) = self.persona {
            config["persona"] = json!(persona);
        }
        if let Some(temperature) = self.temperature {
            config["preset"] = json!({ "temperature": temperature });
        }
        config
    }
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

    #[test]
    fn draft_preserves_cli_omissions_and_tui_explicit_defaults() {
        let skills = vec!["research".to_owned()];
        let mut draft = AgentDraft {
            name: "Researcher",
            description: "",
            model: "local",
            skills: &skills,
            max_cost: 0.0,
            persona: None,
            temperature: None,
        };
        assert_eq!(
            draft.config(),
            json!({"name":"Researcher", "description":"", "llm":{"model":"local"}, "skills":["research"]})
        );
        draft.persona = Some("You are a helpful assistant.");
        draft.temperature = Some(0.5);
        draft.max_cost = 1.5;
        let config = draft.config();
        assert_eq!(config["persona"], "You are a helpful assistant.");
        assert_eq!(config["preset"], json!({"temperature":0.5}));
        assert_eq!(config["constraints"], json!({"max_cost_per_task":1.5}));
        // An explicitly empty persona is different from an omitted persona.
        draft.persona = Some("");
        assert_eq!(draft.config()["persona"], "");
    }
}
