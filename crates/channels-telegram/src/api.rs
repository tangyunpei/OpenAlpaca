use reqwest::Client;

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

    /// Call getMe to verify the bot token.
    pub async fn get_me(&self) -> Result<User, TelegramApiError> {
        let resp: ApiResponse<User> = self
            .client
            .get(format!("{}/getMe", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        resp.result.ok_or_else(|| {
            TelegramApiError::Api(resp.description.unwrap_or_else(|| "unknown error".into()))
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

        let resp: ApiResponse<Vec<Update>> = self.client.get(&url).send().await?.json().await?;
        Ok(resp.result.unwrap_or_default())
    }

    /// Send a text message.
    pub async fn send_message(
        &self,
        params: &SendMessageParams,
    ) -> Result<TelegramMessage, TelegramApiError> {
        let resp: ApiResponse<TelegramMessage> = self
            .client
            .post(format!("{}/sendMessage", self.base_url))
            .json(params)
            .send()
            .await?
            .json()
            .await?;
        resp.result.ok_or_else(|| {
            TelegramApiError::Api(resp.description.unwrap_or_else(|| "send failed".into()))
        })
    }

    /// Edit a message's text.
    pub async fn edit_message_text(
        &self,
        params: &EditMessageParams,
    ) -> Result<TelegramMessage, TelegramApiError> {
        let resp: ApiResponse<TelegramMessage> = self
            .client
            .post(format!("{}/editMessageText", self.base_url))
            .json(params)
            .send()
            .await?
            .json()
            .await?;
        resp.result.ok_or_else(|| {
            TelegramApiError::Api(resp.description.unwrap_or_else(|| "edit failed".into()))
        })
    }

    /// Send a chat action (typing indicator, etc.).
    pub async fn send_chat_action(
        &self,
        chat_id: &str,
        action: &str,
    ) -> Result<(), TelegramApiError> {
        let resp: ApiResponse<bool> = self
            .client
            .post(format!("{}/sendChatAction", self.base_url))
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "action": action,
            }))
            .send()
            .await?
            .json()
            .await?;
        if resp.ok {
            Ok(())
        } else {
            Err(TelegramApiError::Api(
                resp.description
                    .unwrap_or_else(|| "sendChatAction failed".into()),
            ))
        }
    }

    /// Delete the webhook (required before long-polling).
    pub async fn delete_webhook(&self) -> Result<(), TelegramApiError> {
        let resp: ApiResponse<bool> = self
            .client
            .post(format!("{}/deleteWebhook", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        if resp.ok {
            Ok(())
        } else {
            Err(TelegramApiError::Api(
                resp.description
                    .unwrap_or_else(|| "deleteWebhook failed".into()),
            ))
        }
    }

    /// Set bot commands.
    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<(), TelegramApiError> {
        let resp: ApiResponse<bool> = self
            .client
            .post(format!("{}/setMyCommands", self.base_url))
            .json(&serde_json::json!({ "commands": commands }))
            .send()
            .await?
            .json()
            .await?;
        if resp.ok {
            Ok(())
        } else {
            Err(TelegramApiError::Api(
                resp.description
                    .unwrap_or_else(|| "setMyCommands failed".into()),
            ))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TelegramApiError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Telegram API error: {0}")]
    Api(String),
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
                "description": "Unauthorized"
            })))
            .mount(&server)
            .await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let result = api.get_me().await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unauthorized"));
    }
}
