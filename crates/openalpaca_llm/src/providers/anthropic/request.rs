use crate::types::*;

/// Build Anthropic content blocks from a ChatMessage.
///
/// If the message has multimodal `parts`, builds an array of content blocks
/// in Anthropic's format. If parts is None, returns a plain string value.
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
            ContentPart::Image { source, .. } => match source {
                ImageSource::Base64 { media_type, data } => Some(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data.as_str(),
                    }
                })),
                ImageSource::Url { url } => Some(serde_json::json!({
                    "type": "image",
                    "source": {
                        "type": "url",
                        "url": url,
                    }
                })),
                ImageSource::FileAsset { file_id, media_type } => {
                    Some(serde_json::json!({
                        "type": "text",
                        "text": format!("[image file_id={} not resolved — media_type={}]", file_id, media_type),
                    }))
                }
            },
            ContentPart::Audio { .. } => {
                Some(serde_json::json!({
                    "type": "text",
                    "text": "[audio content — not supported by this model]",
                }))
            }
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
        return serde_json::Value::Array(vec![serde_json::json!({
            "type": "text",
            "text": "[empty message]",
        })]);
    }

    serde_json::Value::Array(blocks)
}

pub(super) fn build_request_body(
    default_model: &str,
    default_max_tokens: u32,
    request: &ChatRequest,
) -> serde_json::Value {
    let model = request.model.as_deref().unwrap_or(default_model);
    let max_tokens = request.max_tokens.unwrap_or(default_max_tokens);

    // Extract system message (Anthropic uses top-level system field)
    let mut system_text = String::new();
    let mut messages = Vec::new();

    for msg in request.messages.iter() {
        match msg.role {
            Role::System => {
                if !system_text.is_empty() {
                    system_text.push('\n');
                }
                system_text.push_str(&msg.content);
            }
            Role::User => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": build_message_content(msg),
                }));
            }
            Role::Assistant => {
                if let Some(ref tool_calls) = msg.tool_calls {
                    let mut content: Vec<serde_json::Value> = match &msg.parts {
                        Some(parts) if !parts.is_empty() => {
                            match build_message_content(msg) {
                                serde_json::Value::Array(blocks) => blocks,
                                val => vec![val],
                            }
                        }
                        _ if !msg.content.is_empty() => {
                            vec![serde_json::json!({"type": "text", "text": msg.content})]
                        }
                        _ => vec![],
                    };
                    for tc in tool_calls {
                        content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content,
                    }));
                } else {
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": build_message_content(msg),
                    }));
                }
            }
            Role::Tool => {
                let tool_content = build_message_content(msg);
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": msg.tool_call_id,
                        "content": tool_content,
                    }],
                }));
            }
        }
    }

    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    if !system_text.is_empty() {
        if request.enable_caching {
            body["system"] = serde_json::json!([{
                "type": "text",
                "text": system_text,
                "cache_control": CacheControl::ephemeral()
            }]);
        } else {
            body["system"] = serde_json::Value::String(system_text);
        }
    }

    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut tool = serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                });
                if let Some(ref examples) = t.input_examples {
                    tool["input_examples"] = serde_json::json!(examples);
                }
                // Cache breakpoint on the last tool
                if request.enable_caching && i == request.tools.len() - 1 {
                    tool["cache_control"] = serde_json::json!(CacheControl::ephemeral());
                }
                tool
            })
            .collect();
        body["tools"] = serde_json::Value::Array(tools);

        if let Some(ref choice) = request.tool_choice {
            body["tool_choice"] = match choice {
                ToolChoice::Auto => serde_json::json!({"type": "auto"}),
                ToolChoice::Any => serde_json::json!({"type": "any"}),
                ToolChoice::Tool(name) => serde_json::json!({"type": "tool", "name": name}),
            };
        }
    }

    // Extended thinking
    if let Some(ref thinking) = request.thinking {
        match thinking {
            ThinkingConfig::Enabled { budget_tokens } => {
                if *budget_tokens < 1024 {
                    tracing::warn!(
                        budget_tokens,
                        "budget_tokens < 1024 may produce poor thinking results; Anthropic minimum is 1024"
                    );
                }
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget_tokens,
                });
                // Anthropic requires temperature=1.0 (or unset) for thinking
                body.as_object_mut().unwrap().remove("temperature");
            }
            ThinkingConfig::Adaptive => {
                body["thinking"] = serde_json::json!({ "type": "adaptive" });
                body.as_object_mut().unwrap().remove("temperature");
            }
            ThinkingConfig::Disabled => {
                body["thinking"] = serde_json::json!({ "type": "disabled" });
            }
        }
    }

    // Context management (server-side tool/thinking clearing)
    if let Some(ref ctx_mgmt) = request.context_management {
        body["context_management"] = ctx_mgmt.to_json();
    }

    body
}
