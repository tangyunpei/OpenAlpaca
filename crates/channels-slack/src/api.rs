use reqwest::Client;
use serde::Deserialize;

/// Slack Web API client.
pub struct SlackApi {
    client: Client,
    bot_token: String,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct SlackApiResponse {
    pub ok: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuthTestResponse {
    pub ok: bool,
    pub user_id: Option<String>,
    pub user: Option<String>,
    pub team_id: Option<String>,
    pub team: Option<String>,
    pub bot_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionsOpenResponse {
    pub ok: bool,
    pub url: Option<String>,
    pub error: Option<String>,
}

impl SlackApi {
    pub fn new(client: Client, bot_token: &str) -> Self {
        Self {
            client,
            bot_token: bot_token.to_string(),
            base_url: "https://slack.com/api".to_string(),
        }
    }

    pub fn with_base_url(client: Client, bot_token: &str, base_url: String) -> Self {
        Self {
            client,
            bot_token: bot_token.to_string(),
            base_url,
        }
    }

    /// Post a message to a Slack channel.
    pub async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<String, SlackApiError> {
        let mut body = serde_json::json!({
            "channel": channel,
            "text": text,
        });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = serde_json::Value::String(ts.to_string());
        }

        let resp: SlackApiResponse = self
            .client
            .post(format!("{}/chat.postMessage", self.base_url))
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        if resp.ok {
            Ok(resp.ts.unwrap_or_default())
        } else {
            Err(SlackApiError::Api(
                resp.error.unwrap_or_else(|| "unknown".into()),
            ))
        }
    }

    /// Update an existing message.
    pub async fn update_message(
        &self,
        channel: &str,
        ts: &str,
        text: &str,
    ) -> Result<(), SlackApiError> {
        let resp: SlackApiResponse = self
            .client
            .post(format!("{}/chat.update", self.base_url))
            .bearer_auth(&self.bot_token)
            .json(&serde_json::json!({
                "channel": channel,
                "ts": ts,
                "text": text,
            }))
            .send()
            .await?
            .json()
            .await?;

        if resp.ok {
            Ok(())
        } else {
            Err(SlackApiError::Api(
                resp.error.unwrap_or_else(|| "unknown".into()),
            ))
        }
    }

    /// Test authentication and get bot info.
    pub async fn auth_test(&self) -> Result<AuthTestResponse, SlackApiError> {
        let resp: AuthTestResponse = self
            .client
            .post(format!("{}/auth.test", self.base_url))
            .bearer_auth(&self.bot_token)
            .send()
            .await?
            .json()
            .await?;

        if resp.ok {
            Ok(resp)
        } else {
            Err(SlackApiError::Api("auth.test failed".into()))
        }
    }
}

/// Open a Socket Mode connection using the app-level token.
pub async fn connections_open(
    client: &Client,
    app_token: &str,
    base_url: &str,
) -> Result<String, SlackApiError> {
    let resp: ConnectionsOpenResponse = client
        .post(format!("{base_url}/apps.connections.open"))
        .bearer_auth(app_token)
        .send()
        .await?
        .json()
        .await?;

    if resp.ok {
        resp.url.ok_or(SlackApiError::Api("no url returned".into()))
    } else {
        Err(SlackApiError::Api(
            resp.error.unwrap_or_else(|| "unknown".into()),
        ))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SlackApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Slack API error: {0}")]
    Api(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_post_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1234567890.123456",
                "channel": "C123"
            })))
            .mount(&server)
            .await;

        let api = SlackApi::with_base_url(Client::new(), "xoxb-test", server.uri());
        let ts = api.post_message("C123", "hello", None).await.unwrap();
        assert_eq!(ts, "1234567890.123456");
    }

    #[tokio::test]
    async fn test_post_message_with_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "ts": "1234567890.654321",
                "channel": "C123"
            })))
            .mount(&server)
            .await;

        let api = SlackApi::with_base_url(Client::new(), "xoxb-test", server.uri());
        let ts = api
            .post_message("C123", "threaded reply", Some("1234567890.123456"))
            .await
            .unwrap();
        assert!(!ts.is_empty());
    }

    #[tokio::test]
    async fn test_auth_test() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/auth.test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "user_id": "U123",
                "user": "testbot",
                "team_id": "T123",
                "team": "Test Team",
                "bot_id": "B123"
            })))
            .mount(&server)
            .await;

        let api = SlackApi::with_base_url(Client::new(), "xoxb-test", server.uri());
        let resp = api.auth_test().await.unwrap();
        assert_eq!(resp.user.as_deref(), Some("testbot"));
        assert_eq!(resp.bot_id.as_deref(), Some("B123"));
    }

    #[tokio::test]
    async fn test_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat.postMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error": "channel_not_found"
            })))
            .mount(&server)
            .await;

        let api = SlackApi::with_base_url(Client::new(), "xoxb-test", server.uri());
        let result = api.post_message("C999", "hello", None).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("channel_not_found")
        );
    }
}
