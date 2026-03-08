use serde::{Deserialize, Serialize};

/// Telegram Bot API response wrapper.
#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

/// Telegram Update object.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub edited_message: Option<TelegramMessage>,
    pub callback_query: Option<CallbackQuery>,
}

/// Telegram Message object.
#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
    pub reply_to_message: Option<Box<TelegramMessage>>,
    pub message_thread_id: Option<i64>,
    pub photo: Option<Vec<PhotoSize>>,
    pub document: Option<Document>,
    pub date: Option<i64>,
}

/// Telegram Chat object.
#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
    pub username: Option<String>,
}

/// Telegram User object.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

/// Telegram PhotoSize object.
#[derive(Debug, Clone, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub file_size: Option<i64>,
}

/// Telegram Document object.
#[derive(Debug, Clone, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// Telegram CallbackQuery object.
#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<TelegramMessage>,
    pub data: Option<String>,
}

/// Telegram BotCommand object.
#[derive(Debug, Clone, Serialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

/// Parameters for sendMessage API call.
#[derive(Debug, Serialize)]
pub struct SendMessageParams {
    pub chat_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
}

/// Parameters for editMessageText API call.
#[derive(Debug, Serialize)]
pub struct EditMessageParams {
    pub chat_id: String,
    pub message_id: i64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_update_with_message() {
        let json = r#"{
            "update_id": 123,
            "message": {
                "message_id": 1,
                "from": {"id": 42, "is_bot": false, "first_name": "Alice"},
                "chat": {"id": 100, "type": "private"},
                "text": "hello",
                "date": 1700000000
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        assert_eq!(update.update_id, 123);
        let msg = update.message.unwrap();
        assert_eq!(msg.text.as_deref(), Some("hello"));
        assert_eq!(msg.chat.chat_type, "private");
        assert_eq!(msg.from.unwrap().first_name, "Alice");
    }

    #[test]
    fn test_deserialize_update_with_callback() {
        let json = r#"{
            "update_id": 124,
            "callback_query": {
                "id": "cb1",
                "from": {"id": 42, "is_bot": false, "first_name": "Alice"},
                "data": "button_click"
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        let cb = update.callback_query.unwrap();
        assert_eq!(cb.data.as_deref(), Some("button_click"));
    }

    #[test]
    fn test_serialize_send_params() {
        let params = SendMessageParams {
            chat_id: "123".into(),
            text: "hello".into(),
            reply_to_message_id: Some(42),
            message_thread_id: None,
            parse_mode: None,
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["chat_id"], "123");
        assert_eq!(json["reply_to_message_id"], 42);
        assert!(json.get("message_thread_id").is_none());
    }

    #[test]
    fn test_deserialize_group_chat() {
        let json = r#"{
            "update_id": 125,
            "message": {
                "message_id": 2,
                "from": {"id": 42, "is_bot": false, "first_name": "Bob"},
                "chat": {"id": -100, "type": "supergroup", "title": "My Group"},
                "text": "@bot help"
            }
        }"#;
        let update: Update = serde_json::from_str(json).unwrap();
        let msg = update.message.unwrap();
        assert_eq!(msg.chat.chat_type, "supergroup");
        assert_eq!(msg.chat.title.as_deref(), Some("My Group"));
    }
}
