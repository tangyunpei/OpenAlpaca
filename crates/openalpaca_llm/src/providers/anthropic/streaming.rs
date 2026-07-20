use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;

/// Parse an Anthropic SSE byte stream into a stream of `StreamEvent`s.
///
/// Anthropic SSE format:
///   event: <event_type>\n
///   data: <json>\n
///   \n
pub(super) fn parse_anthropic_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    let text_stream = crate::providers::utf8::utf8_chunks(byte_stream);
    futures_util::stream::unfold(
        (Box::pin(text_stream), String::new(), 0_usize),
        |(mut stream, mut buffer, mut block_index)| async move {
            loop {
                // Try to extract a complete SSE frame from the buffer
                while let Some(frame_end) = buffer.find("\n\n") {
                    let frame = buffer[..frame_end].to_string();
                    buffer = buffer[frame_end + 2..].to_string();

                    let mut event_type = None;
                    let mut data = None;
                    for line in frame.lines() {
                        if let Some(val) = line.strip_prefix("event: ") {
                            event_type = Some(val.to_string());
                        } else if let Some(val) = line.strip_prefix("data: ") {
                            data = Some(val.to_string());
                        }
                    }

                    let event_type = match event_type {
                        Some(t) => t,
                        None => continue,
                    };

                    // Terminal event
                    if event_type == "message_stop" {
                        return None;
                    }

                    let data = match data {
                        Some(d) => d,
                        None => continue,
                    };

                    let json: serde_json::Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    match event_type.as_str() {
                        "content_block_start" => {
                            let block = &json["content_block"];
                            let block_type = block["type"].as_str().unwrap_or("");
                            if block_type == "tool_use" {
                                let id =
                                    block["id"].as_str().unwrap_or_default().to_string();
                                let name =
                                    block["name"].as_str().unwrap_or_default().to_string();
                                let idx = json["index"]
                                    .as_u64()
                                    .unwrap_or(block_index as u64)
                                    as usize;
                                block_index = idx + 1;
                                return Some((
                                    Ok(StreamEvent::ToolUseStart {
                                        index: idx,
                                        id,
                                        name,
                                    }),
                                    (stream, buffer, block_index),
                                ));
                            }
                            // text and thinking blocks don't need start events
                        }
                        "content_block_delta" => {
                            let delta = &json["delta"];
                            let delta_type = delta["type"].as_str().unwrap_or("");
                            match delta_type {
                                "text_delta" => {
                                    if let Some(text) = delta["text"].as_str() {
                                        return Some((
                                            Ok(StreamEvent::TextDelta {
                                                text: text.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(thinking) = delta["thinking"].as_str() {
                                        return Some((
                                            Ok(StreamEvent::ThinkingDelta {
                                                thinking: thinking.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(pj) = delta["partial_json"].as_str() {
                                        let idx =
                                            json["index"].as_u64().unwrap_or(0) as usize;
                                        return Some((
                                            Ok(StreamEvent::InputJsonDelta {
                                                index: idx,
                                                partial_json: pj.to_string(),
                                            }),
                                            (stream, buffer, block_index),
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                        "message_delta" => {
                            let stop_reason =
                                json["delta"]["stop_reason"].as_str().unwrap_or("end_turn");
                            let usage = if let Some(u) = json["usage"].as_object() {
                                Usage {
                                    input_tokens: 0,
                                    output_tokens: u
                                        .get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                        as u32,
                                    ..Default::default()
                                }
                            } else {
                                Usage::default()
                            };
                            let done_frame = format!(
                                "event: _done\ndata: {{\"finish_reason\":\"{}\"}}\n\n",
                                stop_reason
                            );
                            buffer = done_frame + &buffer;
                            return Some((
                                Ok(StreamEvent::Usage(usage)),
                                (stream, buffer, block_index),
                            ));
                        }
                        "_done" => {
                            let stop_reason =
                                json["finish_reason"].as_str().unwrap_or("end_turn");
                            let finish_reason = match stop_reason {
                                "end_turn" => FinishReason::Stop,
                                "tool_use" => FinishReason::ToolUse,
                                "max_tokens" => FinishReason::MaxTokens,
                                _ => FinishReason::Stop,
                            };
                            return Some((
                                Ok(StreamEvent::Done { finish_reason }),
                                (stream, buffer, block_index),
                            ));
                        }
                        "message_start" => {
                            if let Some(usage) = json["message"]["usage"].as_object() {
                                let input_tokens = usage
                                    .get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32;
                                let cache_creation = usage
                                    .get("cache_creation_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32;
                                let cache_read = usage
                                    .get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32;
                                if input_tokens > 0 || cache_creation > 0 || cache_read > 0 {
                                    return Some((
                                        Ok(StreamEvent::Usage(Usage {
                                            input_tokens: input_tokens
                                                + cache_creation
                                                + cache_read,
                                            output_tokens: 0,
                                            cache_creation_input_tokens: cache_creation,
                                            cache_read_input_tokens: cache_read,
                                        })),
                                        (stream, buffer, block_index),
                                    ));
                                }
                            }
                        }
                        "error" => {
                            let message = json["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown streaming error")
                                .to_string();
                            return Some((
                                Ok(StreamEvent::Error { message }),
                                (stream, buffer, block_index),
                            ));
                        }
                        // ping, content_block_stop, etc. — ignore
                        _ => {}
                    }
                }

                // Need more data from the byte stream
                match stream.next().await {
                    Some(Ok(text)) => {
                        buffer.push_str(&text);
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(e.to_string())),
                            (stream, buffer, block_index),
                        ));
                    }
                    None => {
                        return None;
                    }
                }
            }
        },
    )
}
