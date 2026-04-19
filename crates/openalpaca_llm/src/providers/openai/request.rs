use crate::types::*;

/// Build OpenAI content value from a ChatMessage.
pub(super) fn build_message_content(msg: &ChatMessage) -> serde_json::Value {
    let parts = match &msg.parts {
        Some(parts) if !parts.is_empty() => parts,
        _ => return serde_json::Value::String(msg.content.clone()),
    };

    let blocks: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => {
                if text.trim().is_empty() {
                    None
                } else {
                    Some(serde_json::json!({ "type": "text", "text": text }))
                }
            }
            ContentPart::Image { source, detail } => {
                let url = match source {
                    ImageSource::Base64 { media_type, data } => {
                        format!("data:{};base64,{}", media_type, data.as_str())
                    }
                    ImageSource::Url { url } => url.clone(),
                    ImageSource::FileAsset { file_id, media_type } => {
                        return Some(serde_json::json!({
                            "type": "text",
                            "text": format!("[image file_id={} not resolved — media_type={}]", file_id, media_type),
                        }));
                    }
                };
                let detail_val = detail.as_deref().unwrap_or("auto");
                Some(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": url,
                        "detail": detail_val,
                    }
                }))
            }
            ContentPart::Audio { data, format } => Some(serde_json::json!({
                "type": "input_audio",
                "input_audio": {
                    "data": data.as_str(),
                    "format": format,
                }
            })),
            ContentPart::Document {
                filename,
                mime_type,
                extracted_text,
                ..
            } => {
                if let Some(text) = extracted_text {
                    Some(serde_json::json!({
                        "type": "text",
                        "text": format!("[Document: {} ({})]\n{}", filename, mime_type, text),
                    }))
                } else {
                    Some(serde_json::json!({
                        "type": "text",
                        "text": format!("[Document: {} ({}) — no extracted text available]", filename, mime_type),
                    }))
                }
            }
            ContentPart::FileRef {
                file_id,
                filename,
                mime_type,
            } => Some(serde_json::json!({
                "type": "text",
                "text": format!("[File reference: {} ({}) id={}]", filename, mime_type, file_id),
            })),
        })
        .collect();

    if blocks.is_empty() {
        if !msg.content.trim().is_empty() {
            return serde_json::Value::String(msg.content.clone());
        }
        return serde_json::Value::String("[empty message]".to_string());
    }

    serde_json::Value::Array(blocks)
}

pub(crate) fn build_request_body(
    default_model: &str,
    default_max_tokens: u32,
    request: &ChatRequest,
) -> serde_json::Value {
    let model = request.model.as_deref().unwrap_or(default_model);
    let max_tokens = request.max_tokens.unwrap_or(default_max_tokens);

    let mut messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|msg| {
            let role = match msg.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };

            let content = match msg.role {
                Role::Assistant => serde_json::Value::String(msg.content.clone()),
                _ => build_message_content(msg),
            };

            let mut obj = serde_json::json!({
                "role": role,
                "content": content,
            });

            if let Some(ref tool_calls) = msg.tool_calls {
                let tcs: Vec<serde_json::Value> = tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments.to_string(),
                            }
                        })
                    })
                    .collect();
                obj["tool_calls"] = serde_json::Value::Array(tcs);
            }

            if let Some(ref tool_call_id) = msg.tool_call_id {
                obj["tool_call_id"] = serde_json::Value::String(tool_call_id.clone());
            }

            obj
        })
        .collect();

    // Ephemeral system notice: append as tail system-role message (spec P0).
    if let Some(ref notice) = request.ephemeral_system_notice {
        messages.push(serde_json::json!({
            "role": "system",
            "content": notice,
        }));
    }

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    if let Some(ref thinking) = request.thinking {
        match thinking {
            ThinkingConfig::Enabled { .. } => {
                body["reasoning_effort"] = serde_json::json!("high");
                body.as_object_mut().unwrap().remove("temperature");
            }
            ThinkingConfig::Adaptive => {
                body["reasoning_effort"] = serde_json::json!("medium");
                body.as_object_mut().unwrap().remove("temperature");
            }
            ThinkingConfig::Disabled => {}
        }
    }

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|t| {
                let mut function = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                });
                if let Some(true) = t.strict {
                    function["strict"] = serde_json::json!(true);
                }
                serde_json::json!({
                    "type": "function",
                    "function": function,
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(tools);

        if let Some(ref choice) = request.tool_choice {
            body["tool_choice"] = match choice {
                ToolChoice::Auto => serde_json::json!("auto"),
                ToolChoice::Any => serde_json::json!("required"),
                ToolChoice::Tool(name) => serde_json::json!({
                    "type": "function",
                    "function": {"name": name}
                }),
            };
        }
    }

    body
}
