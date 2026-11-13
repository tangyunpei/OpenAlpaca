use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;
use std::collections::{BTreeMap, VecDeque};

/// One content block of a streamed turn, put back together from its events
/// so that a turn with thinking can be sent back as it came (C7a).
enum TurnBlock {
    Thinking {
        thinking: String,
        signature: String,
    },
    Redacted {
        data: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
    /// A block this arm does not put back together (a server tool's, say):
    /// a turn holding one is not echoed.
    Other,
}

impl TurnBlock {
    fn opened(block: &serde_json::Value) -> Self {
        let text = |field: &str| block[field].as_str().unwrap_or_default().to_string();
        match block["type"].as_str() {
            Some("thinking") => Self::Thinking {
                thinking: text("thinking"),
                signature: text("signature"),
            },
            Some("redacted_thinking") => Self::Redacted { data: text("data") },
            Some("text") => Self::Text { text: text("text") },
            Some("tool_use") => Self::ToolUse {
                id: text("id"),
                name: text("name"),
                partial_json: String::new(),
            },
            _ => Self::Other,
        }
    }

    /// The block as the Messages API writes it in a whole response.
    fn into_json(self) -> Option<serde_json::Value> {
        Some(match self {
            Self::Thinking {
                thinking,
                signature,
            } => {
                serde_json::json!({"type": "thinking", "thinking": thinking, "signature": signature})
            }
            Self::Redacted { data } => {
                serde_json::json!({"type": "redacted_thinking", "data": data})
            }
            Self::Text { text } => serde_json::json!({"type": "text", "text": text}),
            Self::ToolUse {
                id,
                name,
                partial_json,
            } => {
                let input: serde_json::Value = if partial_json.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&partial_json).ok()?
                };
                serde_json::json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            Self::Other => return None,
        })
    }
}

/// What the parser carries between frames.
struct SseState<S> {
    stream: S,
    buffer: String,
    block_index: usize,
    /// The turn's blocks so far, by their `index`.
    blocks: BTreeMap<usize, TurnBlock>,
    /// Events a frame yielded that are still to be handed on.
    pending: VecDeque<StreamEvent>,
}

/// The turn's blocks as one event, when it carried thinking and every block
/// could be put back together; `None` otherwise.
fn turn_event(blocks: &mut BTreeMap<usize, TurnBlock>) -> Option<StreamEvent> {
    if !blocks
        .values()
        .any(|b| matches!(b, TurnBlock::Thinking { .. } | TurnBlock::Redacted { .. }))
    {
        blocks.clear();
        return None;
    }
    let blocks: Option<Vec<serde_json::Value>> = std::mem::take(blocks)
        .into_values()
        .map(TurnBlock::into_json)
        .collect();
    EchoedTurn::from_blocks(blocks?).map(StreamEvent::Turn)
}

/// Parse an Anthropic SSE byte stream into a stream of `StreamEvent`s.
///
/// Anthropic SSE format:
///   event: <event_type>\n
///   data: <json>\n
///   \n
///
/// Besides the deltas, a turn that carried thinking yields, once its blocks
/// are all in (at `message_delta`), one [`StreamEvent::Turn`] holding every
/// block in index order — `thinking` blocks with the text of their
/// `thinking_delta`s and the `signature` of their `signature_delta`,
/// `redacted_thinking` blocks whole — before the turn's usage and its end.
pub(super) fn parse_anthropic_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    let text_stream = crate::providers::utf8::utf8_chunks(byte_stream);
    futures_util::stream::unfold(
        SseState {
            stream: Box::pin(text_stream),
            buffer: String::new(),
            block_index: 0_usize,
            blocks: BTreeMap::new(),
            pending: VecDeque::new(),
        },
        |mut state| async move {
            loop {
                if let Some(event) = state.pending.pop_front() {
                    return Some((Ok(event), state));
                }

                // Try to extract a complete SSE frame from the buffer
                while let Some(frame_end) = state.buffer.find("\n\n") {
                    let frame = state.buffer[..frame_end].to_string();
                    state.buffer = state.buffer[frame_end + 2..].to_string();

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
                            let idx =
                                json["index"].as_u64().unwrap_or(state.block_index as u64) as usize;
                            state.blocks.insert(idx, TurnBlock::opened(block));
                            if block_type == "tool_use" {
                                let id = block["id"].as_str().unwrap_or_default().to_string();
                                let name = block["name"].as_str().unwrap_or_default().to_string();
                                state.block_index = idx + 1;
                                return Some((
                                    Ok(StreamEvent::ToolUseStart {
                                        index: idx,
                                        id,
                                        name,
                                    }),
                                    state,
                                ));
                            }
                            // text and thinking blocks need no start events
                        }
                        "content_block_delta" => {
                            let delta = &json["delta"];
                            let delta_type = delta["type"].as_str().unwrap_or("");
                            let block = json["index"]
                                .as_u64()
                                .and_then(|i| state.blocks.get_mut(&(i as usize)));
                            match delta_type {
                                "text_delta" => {
                                    if let Some(text) = delta["text"].as_str() {
                                        if let Some(TurnBlock::Text { text: kept }) = block {
                                            kept.push_str(text);
                                        }
                                        return Some((
                                            Ok(StreamEvent::TextDelta {
                                                text: text.to_string(),
                                            }),
                                            state,
                                        ));
                                    }
                                }
                                "thinking_delta" => {
                                    if let Some(thinking) = delta["thinking"].as_str() {
                                        if let Some(TurnBlock::Thinking {
                                            thinking: kept, ..
                                        }) = block
                                        {
                                            kept.push_str(thinking);
                                        }
                                        return Some((
                                            Ok(StreamEvent::ThinkingDelta {
                                                thinking: thinking.to_string(),
                                            }),
                                            state,
                                        ));
                                    }
                                }
                                // The block's one signature, just before its
                                // `content_block_stop` (C7a).
                                "signature_delta" => {
                                    if let (
                                        Some(signature),
                                        Some(TurnBlock::Thinking {
                                            signature: kept, ..
                                        }),
                                    ) = (delta["signature"].as_str(), block)
                                    {
                                        kept.push_str(signature);
                                    }
                                }
                                "input_json_delta" => {
                                    if let Some(pj) = delta["partial_json"].as_str() {
                                        if let Some(TurnBlock::ToolUse { partial_json, .. }) = block
                                        {
                                            partial_json.push_str(pj);
                                        }
                                        let idx = json["index"].as_u64().unwrap_or(0) as usize;
                                        return Some((
                                            Ok(StreamEvent::InputJsonDelta {
                                                index: idx,
                                                partial_json: pj.to_string(),
                                            }),
                                            state,
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
                            state.buffer = done_frame + &state.buffer;
                            // The turn's blocks are all in: a turn with
                            // thinking is handed on whole, then its usage.
                            if let Some(turn) = turn_event(&mut state.blocks) {
                                state.pending.push_back(StreamEvent::Usage(usage));
                                return Some((Ok(turn), state));
                            }
                            return Some((Ok(StreamEvent::Usage(usage)), state));
                        }
                        "_done" => {
                            let stop_reason = json["finish_reason"].as_str().unwrap_or("end_turn");
                            let finish_reason = match stop_reason {
                                "end_turn" => FinishReason::Stop,
                                "tool_use" => FinishReason::ToolUse,
                                "max_tokens" => FinishReason::MaxTokens,
                                _ => FinishReason::Stop,
                            };
                            return Some((Ok(StreamEvent::Done { finish_reason }), state));
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
                                        state,
                                    ));
                                }
                            }
                        }
                        "error" => {
                            let message = json["error"]["message"]
                                .as_str()
                                .unwrap_or("Unknown streaming error")
                                .to_string();
                            return Some((Ok(StreamEvent::Error { message }), state));
                        }
                        // ping, content_block_stop, etc. — ignore
                        _ => {}
                    }
                }

                // Need more data from the byte stream
                match state.stream.next().await {
                    Some(Ok(text)) => {
                        state.buffer.push_str(&text);
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(crate::providers::transport_message(&e))),
                            state,
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
