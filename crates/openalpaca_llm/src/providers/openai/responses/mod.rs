//! OpenAI's Responses API (`POST /v1/responses`), the wire the OpenAI arm
//! uses for a request that calls functions on OpenAI's own API (C7b;
//! [`super::OpenAiProvider::uses_responses`]).
//!
//! What it follows, read 2026-11-12 on developers.openai.com: the
//! function-calling guide (`api/docs/guides/function-calling`), the
//! reasoning guide (`.../guides/reasoning`), the migration guide
//! (`.../guides/migrate-to-responses`), "Keep the prefix unchanged"
//! (`.../guides/preserved-reasoning`) and the Responses API reference
//! (`api/reference/resources/responses/methods/create`, and
//! `.../responses/streaming-events` for [`streaming`]).
//!
//! - Nothing is stored on OpenAI's side: `store` "Defaults to true when
//!   omitted", keeping the response "for at least 30 days", so every request
//!   says `store: false`. Each request carries the whole conversation as
//!   input items, as Chat Completions' messages did; no
//!   `previous_response_id`, no conversation object.
//! - In that stateless mode a reasoning item comes back with its
//!   `encrypted_content` "by default", and goes back the same way: the turn's
//!   output items, verbatim and in order, before its tool outputs
//!   ([`EchoedTurn::from_response_items`]). No `include` is sent: the API
//!   "doesn't require it".
//! - System messages stay input items with the `system` role, each in its
//!   place, the ephemeral notice last — never `instructions`: a change before
//!   a reasoning item is a prefix change, after which "the API will drop
//!   reasoning items".

mod streaming;

pub(super) use streaming::parse_responses_sse;

use crate::error::LlmError;
use crate::types::*;

/// The Responses API body for `request`.
pub(crate) fn build_request_body(
    default_model: &str,
    default_max_tokens: u32,
    request: &ChatRequest,
) -> serde_json::Value {
    let model = request.model.as_deref().unwrap_or(default_model);
    let max_tokens = request.max_tokens.unwrap_or(default_max_tokens);

    let mut input: Vec<serde_json::Value> = Vec::new();
    for msg in request.messages.iter() {
        match msg.role {
            Role::System => input.push(serde_json::json!({
                "role": "system",
                "content": msg.content,
            })),
            Role::User => input.push(serde_json::json!({
                "role": "user",
                "content": input_content(msg),
            })),
            Role::Assistant => {
                if let Some(items) = echoed_items(msg) {
                    input.extend(items);
                    continue;
                }
                if !msg.content.is_empty() || msg.tool_calls.is_none() {
                    input.push(serde_json::json!({
                        "role": "assistant",
                        "content": msg.content,
                    }));
                }
                for tc in msg.tool_calls.iter().flatten() {
                    input.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": tc.id,
                        "name": tc.name,
                        "arguments": tc.arguments.to_string(),
                    }));
                }
            }
            // A failed call's result keeps its `[tool_error]` text: the wire
            // has no error field, as on Chat Completions (N-TOOL-4).
            Role::Tool => input.push(serde_json::json!({
                "type": "function_call_output",
                "call_id": msg.tool_call_id.as_deref().unwrap_or_default(),
                "output": input_content(msg),
            })),
        }
    }

    // Ephemeral system notice: the last input item, as on Chat Completions
    // (spec P0).
    if let Some(ref notice) = request.ephemeral_system_notice {
        input.push(serde_json::json!({
            "role": "system",
            "content": notice,
        }));
    }

    let mut body = serde_json::json!({
        "model": model,
        "max_output_tokens": max_tokens,
        "input": input,
        "store": false,
    });

    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    // The same reading of `thinking` as Chat Completions' `reasoning_effort`
    // (M2), in the Responses form; `Disabled` sends nothing, as the cloud arm
    // always has.
    let effort = match request.thinking {
        Some(ThinkingConfig::Enabled { .. }) => Some("high"),
        Some(ThinkingConfig::Adaptive) => Some("medium"),
        Some(ThinkingConfig::Disabled) | None => None,
    };
    if let Some(effort) = effort {
        body["reasoning"] = serde_json::json!({ "effort": effort });
        body.as_object_mut().unwrap().remove("temperature");
    }

    if !request.tools.is_empty() {
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(crate::providers::openai_tools::openai_responses_function_tool)
            .collect();
        body["tools"] = serde_json::Value::Array(tools);

        if let Some(ref choice) = request.tool_choice {
            body["tool_choice"] = match choice {
                ToolChoice::Auto => serde_json::json!("auto"),
                ToolChoice::Any => serde_json::json!("required"),
                ToolChoice::Tool(name) => serde_json::json!({
                    "type": "function",
                    "name": name,
                }),
            };
        }
    }

    body
}

/// An assistant message's kept Responses turn, item for item, when its
/// `function_call`s are still the message's tool calls; `None` otherwise —
/// and always for a turn another wire returned — so the message is rebuilt
/// from its text and tool calls.
fn echoed_items(msg: &ChatMessage) -> Option<Vec<serde_json::Value>> {
    let turn = msg
        .echo
        .as_ref()
        .filter(|turn| turn.wire() == EchoWire::OpenAiResponses)?;
    let calls: Vec<&str> = msg
        .tool_calls
        .iter()
        .flatten()
        .map(|tc| tc.id.as_str())
        .collect();
    (turn.function_call_ids() == calls).then(|| turn.blocks().to_vec())
}

/// A user message's or a tool result's content: its text, or its parts as
/// `input_text` and `input_image` items.
fn input_content(msg: &ChatMessage) -> serde_json::Value {
    let parts = match &msg.parts {
        Some(parts) if !parts.is_empty() => parts,
        _ => return serde_json::Value::String(msg.content.clone()),
    };

    let text = |text: String| serde_json::json!({ "type": "input_text", "text": text });
    let items: Vec<serde_json::Value> = parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text: t } => (!t.trim().is_empty()).then(|| text(t.clone())),
            ContentPart::Image { source, detail } => Some(match source {
                ImageSource::Base64 { media_type, data } => serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:{};base64,{}", media_type, data.as_str()),
                    "detail": detail.as_deref().unwrap_or("auto"),
                }),
                ImageSource::Url { url } => serde_json::json!({
                    "type": "input_image",
                    "image_url": url,
                    "detail": detail.as_deref().unwrap_or("auto"),
                }),
                ImageSource::FileAsset {
                    file_id,
                    media_type,
                } => text(format!(
                    "[image file_id={} not resolved — media_type={}]",
                    file_id, media_type
                )),
            }),
            // The Responses API's input content is text, image or file (the
            // reference); an audio part is said, not dropped.
            ContentPart::Audio { format, .. } => Some(text(format!(
                "[audio input ({format}) is not sent on OpenAI's Responses API]"
            ))),
            ContentPart::Document {
                filename,
                mime_type,
                extracted_text,
                ..
            } => Some(text(match extracted_text {
                Some(t) => format!("[Document: {} ({})]\n{}", filename, mime_type, t),
                None => format!(
                    "[Document: {} ({}) — no extracted text available]",
                    filename, mime_type
                ),
            })),
            ContentPart::FileRef {
                file_id,
                filename,
                mime_type,
            } => Some(text(format!(
                "[File reference: {} ({}) id={}]",
                filename, mime_type, file_id
            ))),
        })
        .collect();

    if items.is_empty() {
        if !msg.content.trim().is_empty() {
            return serde_json::Value::String(msg.content.clone());
        }
        return serde_json::Value::String("[empty message]".to_string());
    }
    serde_json::Value::Array(items)
}

/// The usage of a Responses API answer: input and output tokens as
/// reported — output includes reasoning, which is "billed as output tokens"
/// (the reasoning guide) — and the cached input as the cache read, as on
/// Chat Completions. `input_tokens_details.cache_write_tokens` is not read
/// (pending-decisions §1, *Rev 58*'s cache-write item).
pub(super) fn usage_of(response: &serde_json::Value) -> Usage {
    let u = &response["usage"];
    let count = |v: &serde_json::Value| v.as_u64().unwrap_or(0) as u32;
    Usage {
        input_tokens: count(&u["input_tokens"]),
        output_tokens: count(&u["output_tokens"]),
        cache_read_input_tokens: count(&u["input_tokens_details"]["cached_tokens"]),
        ..Default::default()
    }
}

/// How a Responses API answer ended: cut at `max_output_tokens`, a call, or
/// an answer.
pub(super) fn finish_of(response: &serde_json::Value, called: bool) -> FinishReason {
    if response["status"] == "incomplete"
        && response["incomplete_details"]["reason"] == "max_output_tokens"
    {
        FinishReason::MaxTokens
    } else if called {
        FinishReason::ToolUse
    } else {
        FinishReason::Stop
    }
}

/// A `message` item's text: its `output_text` parts, and a refusal's text.
pub(super) fn message_text(item: &serde_json::Value) -> String {
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| match part["type"].as_str() {
            Some("output_text") => part["text"].as_str(),
            Some("refusal") => part["refusal"].as_str(),
            _ => None,
        })
        .collect()
}

/// A `reasoning` item's readable text, when the API sent any: its summary
/// (only when one was asked for, which this arm never does) or its
/// reasoning text.
pub(super) fn reasoning_text(item: &serde_json::Value) -> String {
    ["summary", "content"]
        .iter()
        .flat_map(|key| item[*key].as_array().into_iter().flatten())
        .filter_map(|part| part["text"].as_str())
        .collect()
}

/// A `function_call` item as a [`ToolCall`], named by its `call_id` — what
/// its `function_call_output` must name — not by the item's `id`. Arguments
/// that cannot be read are warned about once and refused by the registry,
/// never run as `{}` (N-TOOL-5).
pub(super) fn tool_call_of(item: &serde_json::Value) -> ToolCall {
    let name = item["name"].as_str().unwrap_or_default().to_string();
    let arguments = crate::tool_arguments::read_tool_arguments_field(&name, &item["arguments"]);
    ToolCall {
        id: item["call_id"].as_str().unwrap_or_default().to_string(),
        name,
        arguments,
    }
}

/// A whole Responses API answer, read.
pub(super) fn parse_response(
    default_model: &str,
    body: serde_json::Value,
) -> Result<ChatResponse, LlmError> {
    // A body that says the response failed (its `error`, "returned when the
    // model fails to generate a Response"; its codes, the reference): a
    // server error is classed as the router classes a 5xx, a rate limit as a
    // 429 without `Retry-After` — the 1,000 ms this arm already assumes
    // there, its own figure, which no page gives — and anything else as a
    // refusal of the request.
    if body["status"] == "failed" {
        let error = &body["error"];
        let message = error["message"]
            .as_str()
            .unwrap_or("the response failed")
            .to_string();
        return Err(match error["code"].as_str() {
            Some("rate_limit_exceeded") => LlmError::RateLimited {
                retry_after_ms: 1_000,
            },
            Some("server_error") => LlmError::Api {
                status: 500,
                message,
            },
            _ => LlmError::Api {
                status: 400,
                message,
            },
        });
    }

    let model = body["model"].as_str().unwrap_or(default_model).to_string();
    let output: Vec<serde_json::Value> = body["output"].as_array().cloned().unwrap_or_default();

    let mut content = String::new();
    let mut thinking = String::new();
    let mut tool_calls = Vec::new();
    for item in &output {
        match item["type"].as_str() {
            Some("message") => content.push_str(&message_text(item)),
            Some("reasoning") => thinking.push_str(&reasoning_text(item)),
            Some("function_call") => tool_calls.push(tool_call_of(item)),
            _ => {}
        }
    }

    let parts = (!content.is_empty()).then(|| {
        vec![ContentPart::Text {
            text: content.clone(),
        }]
    });

    Ok(ChatResponse {
        finish_reason: finish_of(&body, !tool_calls.is_empty()),
        usage: usage_of(&body),
        content,
        tool_calls,
        model,
        thinking: (!thinking.is_empty()).then_some(thinking),
        echo: EchoedTurn::from_response_items(output),
        parts,
    })
}

#[cfg(test)]
mod tests;
