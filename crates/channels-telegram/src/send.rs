use crate::api::{TelegramApi, TelegramApiError};
use crate::types::SendMessageParams;

/// Default character limit for Telegram messages.
pub const DEFAULT_CHUNK_LIMIT: usize = 4000;

/// Send a reply, chunking if needed.
///
/// Returns the message IDs of the sent chunks.
pub async fn send_reply(
    api: &TelegramApi,
    chat_id: &str,
    text: &str,
    reply_to: Option<i64>,
    thread_id: Option<i64>,
    chunk_limit: usize,
) -> Result<Vec<i64>, TelegramApiError> {
    let limit = if chunk_limit == 0 {
        DEFAULT_CHUNK_LIMIT
    } else {
        chunk_limit
    };

    let chunks = openalpaca_delivery::chunk_text(text, limit);
    let mut message_ids = Vec::new();

    for (i, chunk) in chunks.iter().enumerate() {
        let params = SendMessageParams {
            chat_id: chat_id.to_string(),
            text: chunk.clone(),
            // Only reply to the original message on the first chunk
            reply_to_message_id: if i == 0 { reply_to } else { None },
            message_thread_id: thread_id,
            parse_mode: None,
        };

        let msg = api.send_message(&params).await?;
        message_ids.push(msg.message_id);
    }

    Ok(message_ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_send_mock(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/sendMessage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "result": {
                    "message_id": 1,
                    "from": {"id": 123, "is_bot": true, "first_name": "Bot"},
                    "chat": {"id": 42, "type": "private"},
                    "text": "ok"
                }
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn test_send_short_message() {
        let server = MockServer::start().await;
        setup_send_mock(&server).await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let ids = send_reply(&api, "42", "short msg", None, None, DEFAULT_CHUNK_LIMIT)
            .await
            .unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[tokio::test]
    async fn test_send_long_message_chunks() {
        let server = MockServer::start().await;
        setup_send_mock(&server).await;

        let api = TelegramApi::with_base_url(Client::new(), server.uri());
        let long_text = "a".repeat(5000);
        let ids = send_reply(&api, "42", &long_text, Some(10), None, 4000)
            .await
            .unwrap();
        assert_eq!(ids.len(), 2);
    }
}
