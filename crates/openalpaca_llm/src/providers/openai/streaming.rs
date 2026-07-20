use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;

/// Parse an OpenAI SSE byte stream into a stream of `StreamEvent`s.
pub(super) fn parse_openai_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    let text_stream = crate::providers::utf8::utf8_chunks(byte_stream);
    futures_util::stream::unfold(
        (Box::pin(text_stream), String::new()),
        |(mut stream, mut buffer)| async move {
            loop {
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim_end().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    let data = match line.strip_prefix("data: ") {
                        Some(d) => d,
                        None => continue,
                    };

                    if data == "[DONE]" {
                        return None;
                    }

                    let json: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let choice = &json["choices"][0];
                    let delta = &choice["delta"];

                    if let Some(text) = delta["content"].as_str()
                        && !text.is_empty()
                    {
                        return Some((
                            Ok(StreamEvent::TextDelta {
                                text: text.to_string(),
                            }),
                            (stream, buffer),
                        ));
                    }

                    if let Some(reasoning) = delta["reasoning_content"].as_str()
                        && !reasoning.is_empty()
                    {
                        return Some((
                            Ok(StreamEvent::ThinkingDelta {
                                thinking: reasoning.to_string(),
                            }),
                            (stream, buffer),
                        ));
                    }

                    if let Some(tool_calls) = delta["tool_calls"].as_array() {
                        for tc in tool_calls {
                            let index = tc["index"].as_u64().unwrap_or(0) as usize;

                            if let Some(id) = tc["id"].as_str() {
                                let name = tc["function"]["name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string();
                                return Some((
                                    Ok(StreamEvent::ToolUseStart {
                                        index,
                                        id: id.to_string(),
                                        name,
                                    }),
                                    (stream, buffer),
                                ));
                            }

                            if let Some(args) = tc["function"]["arguments"].as_str()
                                && !args.is_empty()
                            {
                                return Some((
                                    Ok(StreamEvent::InputJsonDelta {
                                        index,
                                        partial_json: args.to_string(),
                                    }),
                                    (stream, buffer),
                                ));
                            }
                        }
                    }

                    if let Some(finish_reason_str) = choice["finish_reason"].as_str() {
                        let usage = if let Some(u) = json["usage"].as_object() {
                            Usage {
                                input_tokens: u
                                    .get("prompt_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                output_tokens: u
                                    .get("completion_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                cache_read_input_tokens: u
                                    .get("prompt_tokens_details")
                                    .and_then(|d| d.get("cached_tokens"))
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0)
                                    as u32,
                                ..Default::default()
                            }
                        } else {
                            Usage::default()
                        };

                        let done_line = format!(
                            "data: {}\n",
                            serde_json::json!({"_done": finish_reason_str})
                        );
                        buffer = done_line + &buffer;

                        return Some((
                            Ok(StreamEvent::Usage(usage)),
                            (stream, buffer),
                        ));
                    }

                    if let Some(fr_str) = json["_done"].as_str() {
                        let finish_reason = match fr_str {
                            "stop" => FinishReason::Stop,
                            "tool_calls" => FinishReason::ToolUse,
                            "length" => FinishReason::MaxTokens,
                            _ => FinishReason::Stop,
                        };
                        return Some((
                            Ok(StreamEvent::Done { finish_reason }),
                            (stream, buffer),
                        ));
                    }
                }

                match stream.next().await {
                    Some(Ok(text)) => {
                        buffer.push_str(&text);
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(e.to_string())),
                            (stream, buffer),
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
