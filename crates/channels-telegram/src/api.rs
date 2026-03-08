use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::types::*;

/// Telegram Bot API HTTP client.
pub struct TelegramApi {
    client: Client,
    base_url: String,
}

impl TelegramApi {
    pub fn new(client: Client, token: &str) -> Self {
        Self {
            client,
            base_url: format!("https://api.telegram.org/bot{token}"),
        }
    }

    /// Create with a custom base URL (for testing with mock server).
    pub fn with_base_url(client: Client, base_url: String) -> Self {
        Self { client, base_url }
    }

    /// Parse a Telegram API response, handling rate limits and error codes.
    fn parse_response<T: DeserializeOwned>(
        body: &str,
        fallback_desc: &str,
    ) -> Result<ApiResponse<T>, TelegramApiError> {
        let resp: ApiResponse<T> = serde_json::from_str(body)
            .map_err(|e| TelegramApiError::Parse(format!("{e}: {body}")))?;

        if resp.error_code == Some(429) {
            return Err(TelegramApiError::RateLimited {
                retry_after: resp.retry_after.unwrap_or(5),
            });
        }

        if !resp.ok && resp.result.is_none() {
            return Err(TelegramApiError::Api {
                error_code: resp.error_code,
                description: resp
                    .description
                    .clone()
                    .unwrap_or_else(|| fallback_desc.into()),
            });
        }

        Ok(resp)
    }

    /// Call getMe to verify the bot token.
    pub async fn get_me(&self) -> Result<User, TelegramApiError> {
        let body = self
            .client
            .get(format!("{}/getMe", self.base_url))
            .send()
            .await?
            .text()
            .await?;
        let resp = Self::parse_response::<User>(&body, "getMe failed")?;
        resp.result.ok_or_else(|| TelegramApiError::Api {
            error_code: resp.error_code,
            description: resp.description.unwrap_or_else(|| "unknown error".into()),
        })
    }

    /// Long-poll for updates.
    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: u32,
    ) -> Result<Vec<Update>, TelegramApiError> {
        let mut url = format!("{}/getUpdates?timeout={timeout}", self.base_url);
        if let Some(off) = offset {
            url.push_str(&format!("&offset={off}"));
        }

        let body = self.client.get(&url).send().await?.text().await?;
        let resp = Self::parse_response::<Vec<Update>>(&body, "getUpdates failed")?;
        Ok(resp.result.unwrap_or_default())
    }

    /// Send a text message.
    pub async fn send_message(
        &self,
        params: &SendMessageParams,
    ) -> Result<TelegramMessage, TelegramApiError> {
        let body = self
            .client
            .post(format!("{}/sendMessage", self.base_url))
            .json(params)
            .send()
            .await?
            .text()
            .await?;
        let resp = Self::parse_response::<TelegramMessage>(&body, "send failed")?;
        resp.result.ok_or_else(|| TelegramApiError::Api {
            error_code: resp.error_code,
            description: resp.description.unwrap_or_else(|| "send failed".into()),
        })
    }

    /// Edit a message's text.
    pub async fn edit_message_text(
        &self,
        params: &EditMessageParams,
    ) -> Result<TelegramMessage, TelegramApiError> {
        let body = self
            .client
            .post(format!("{}/editMessageText", self.base_url))
            .json(params)
            .send()
            .await?
            .text()
            .await?;
        let resp = Self::parse_response::<TelegramMessage>(&body, "edit failed")?;
        resp.result.ok_or_else(|| TelegramApiError::Api {
            error_code: resp.error_code,
            description: resp.description.unwrap_or_else(|| "edit failed".into()),
        })
    }

    /// Send a chat action (typing indicator, etc.).
    pub async fn send_chat_action(
        &self,
        chat_id: &str,
        action: &str,
    ) -> Result<(), TelegramApiError> {
        let body = self
            .client
            .post(format!("{}/sendChatAction", self.base_url))
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "action": action,
            }))
            .send()
            .await?
            .text()
            .await?;
        Self::parse_response::<bool>(&body, "sendChatAction failed")?;
        Ok(())
    }

    /// Delete the webhook (required before long-polling).
    pub async fn delete_webhook(&self) -> Result<(), TelegramApiError> {
        let body = self
            .client
            .post(format!("{}/deleteWebhook", self.base_url))
            .send()
            .await?
            .text()
            .await?;
        Self::parse_response::<bool>(&body, "deleteWebhook failed")?;
        Ok(())
    }

    /// Send a text message with automatic rate-limit retry.
    pub async fn send_message_with_retry(
        &self,
        params: &SendMessageParams,
    ) -> Result<TelegramMessage, TelegramApiError> {
        with_rate_limit_retry(|| self.send_message(params)).await
    }

    /// Set bot commands.
    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<(), TelegramApiError> {
        let body = self
            .client
            .post(format!("{}/setMyCommands", self.base_url))
            .json(&serde_json::json!({ "commands": commands }))
            .send()
            .await?
            .text()
            .await?;
        Self::parse_response::<bool>(&body, "setMyCommands failed")?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TelegramApiError {
    /// Network-level error (DNS, connection, timeout)
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Response body could not be parsed as JSON
    #[error("parse error: {0}")]
    Parse(String),

    /// Telegram API returned ok: false
    #[error("Telegram API error {}: {description}", error_code.map(|c| c.to_string()).unwrap_or_default())]
    Api {
        error_code: Option<i32>,
        description: String,
    },

    /// Rate limited (HTTP 429 or error_code 429)
    #[error("rate limited: retry after {retry_after}s")]
    RateLimited { retry_after: u64 },
}

/// Retry a Telegram API call on rate-limit errors.
/// Max 3 retries, respects the `retry_after` value from Telegram (capped at 60s).
async fn with_rate_limit_retry<T, F, Fut>(mut f: F) -> Result<T, TelegramApiError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, TelegramApiError>>,
{
    for attempt in 0..3u32 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(TelegramApiError::RateLimited { retry_after }) => {
                let wait = retry_after.clamp(1, 60);
                tracing::warn!(
                    attempt = attempt + 1,
                    retry_after = wait,
                    "telegram: rate limited, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(e) => return Err(e),
        }
    }
    f().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_get_me() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "id": 123,
                    "is_bot": true,
                    "first_name": "TestBot",
                    "username": "test_bot"
                }
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let user = api.get_me().await.unwrap();
        assert_eq!(user.id, 123);
        assert_eq!(user.first_name, "TestBot");
        assert!(user.is_bot);
    }

    #[tokio::test]
    async fn test_get_updates() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getUpdates"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": [{
                    "update_id": 1,
                    "message": {
                        "message_id": 10,
                        "from": {"id": 42, "is_bot": false, "first_name": "User"},
                        "chat": {"id": 42, "type": "private"},
                        "text": "hello"
                    }
                }]
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let updates = api.get_updates(None, 1).await.unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].update_id, 1);
    }

    #[tokio::test]
    async fn test_send_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 99,
                    "from": {"id": 123, "is_bot": true, "first_name": "Bot"},
                    "chat": {"id": 42, "type": "private"},
                    "text": "hi there"
                }
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let params = SendMessageParams {
            chat_id: "42".into(),
            text: "hi there".into(),
            reply_to_message_id: None,
            message_thread_id: None,
            parse_mode: None,
        };
        let msg = api.send_message(&params).await.unwrap();
        assert_eq!(msg.message_id, 99);
    }

    #[tokio::test]
    async fn test_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "error_code": 401,
                "description": "Unauthorized"
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let result = api.get_me().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            TelegramApiError::Api {
                error_code,
                description,
            } => {
                assert_eq!(error_code, Some(401));
                assert!(description.contains("Unauthorized"));
            }
            other => panic!("expected Api error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_rate_limited_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getMe"))
            .respond_with(ResponseTemplate::new(429).set_body_json(serde_json::json!({
                "ok": false,
                "error_code": 429,
                "description": "Too Many Requests: retry after 30",
                "retry_after": 30
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let result = api.get_me().await;
        assert!(matches!(
            result,
            Err(TelegramApiError::RateLimited { retry_after: 30 })
        ));
    }

    #[tokio::test]
    async fn test_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/getMe"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let result = api.get_me().await;
        assert!(matches!(result, Err(TelegramApiError::Parse(_))));
    }
}
