use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;

/// The `usage` object of one SSE frame, when it has one.
///
/// `None` means the frame said nothing about tokens — which is not the same as
/// "zero tokens", so the caller decides what an absent count means.
fn parse_usage(frame: &serde_json::Value) -> Option<Usage> {
    let u = frame["usage"].as_object()?;
    Some(Usage {
        input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        output_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        cache_read_input_tokens: u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32,
        ..Default::default()
    })
}

/// The OpenAI-compatible spelling of one finish reason.
fn finish_reason_of(raw: &str) -> FinishReason {
    match raw {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolUse,
        "length" => FinishReason::MaxTokens,
        _ => FinishReason::Stop,
    }
}

/// **Every** event one SSE frame carries, in wire order (V1).
///
/// The unit of work is the frame, not the first field of it that matches. The
/// parser used to `return` from inside the field checks, so whatever the frame
/// said after that point was dropped — and Ollama puts a whole tool call
/// (`id`, `name` *and* the complete `arguments`) in a single frame, so every
/// local-model tool call reached the loop with `input: {}`. A frame can also
/// carry content beside a tool call, two tool calls at once, or a tool call
/// beside `finish_reason`; all of them lost something. Nothing is
/// provider-special-cased here: this parser is shared by every
/// OpenAI-compatible base, and the shape is legal for all of them.
fn frame_events(json: &serde_json::Value) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    let choice = &json["choices"][0];
    let delta = &choice["delta"];

    if let Some(text) = delta["content"].as_str()
        && !text.is_empty()
    {
        out.push(StreamEvent::TextDelta {
            text: text.to_string(),
        });
    }

    // Both spellings — Ollama streams `delta.reasoning` (M1).
    // `ThinkingDelta` is the event Anthropic's extended thinking already
    // uses, so nothing new reaches the wire: the text simply stops being
    // dropped.
    if let Some(reasoning) = super::response::reasoning_text(delta) {
        out.push(StreamEvent::ThinkingDelta {
            thinking: reasoning,
        });
    }

    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;

            // The start and the arguments are two events, and one frame may
            // well carry both: the start first, then what the accumulator
            // appends to it.
            if let Some(id) = tc["id"].as_str() {
                let name = tc["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                out.push(StreamEvent::ToolUseStart {
                    index,
                    id: id.to_string(),
                    name,
                });
            }

            if let Some(args) = tc["function"]["arguments"].as_str()
                && !args.is_empty()
            {
                out.push(StreamEvent::InputJsonDelta {
                    index,
                    partial_json: args.to_string(),
                });
            }
        }
    }

    if let Some(finish_reason_str) = choice["finish_reason"].as_str() {
        // Ollama puts usage on this frame when `stream_options.include_usage`
        // was asked for; when it is absent the trailing usage-only frame
        // carries it instead, and the zero here costs nothing (the collector
        // sums).
        out.push(StreamEvent::Usage(parse_usage(json).unwrap_or_default()));
        out.push(StreamEvent::Done {
            finish_reason: finish_reason_of(finish_reason_str),
        });
    } else if choice.is_null()
        && let Some(usage) = parse_usage(json)
    {
        // The usage-only frame: OpenAI sends totals in a trailing chunk whose
        // `choices` is empty, after the one that carried `finish_reason`.
        // Without this arm those tokens are dropped and the turn is booked at
        // zero (L5).
        out.push(StreamEvent::Usage(usage));
    }

    out
}

/// Parse an OpenAI SSE byte stream into a stream of `StreamEvent`s.
///
/// The unfold state carries a queue of events the last frame produced: a frame
/// is decoded once, in full, and the queue is drained before another line is
/// read. That is what makes the parser total over frames that say more than
/// one thing (V1).
pub(super) fn parse_openai_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    let text_stream = crate::providers::utf8::utf8_chunks(byte_stream);
    let pending: std::collections::VecDeque<StreamEvent> = std::collections::VecDeque::new();
    futures_util::stream::unfold(
        (Box::pin(text_stream), String::new(), pending),
        |(mut stream, mut buffer, mut pending)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((Ok(event), (stream, buffer, pending)));
                }

                // Nothing queued: decode lines until one frame yields events.
                let mut saw_done_marker = false;
                while pending.is_empty() {
                    let Some(line_end) = buffer.find('\n') else {
                        break;
                    };
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
                        saw_done_marker = true;
                        break;
                    }

                    let json: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    pending.extend(frame_events(&json));
                }

                if !pending.is_empty() {
                    continue;
                }
                if saw_done_marker {
                    return None;
                }

                match stream.next().await {
                    Some(Ok(text)) => {
                        buffer.push_str(&text);
                    }
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(e.to_string())),
                            (stream, buffer, pending),
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
