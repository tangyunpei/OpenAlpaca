//! The Responses API's server-sent events (C7b; the streaming-events
//! reference, developers.openai.com/api/reference/resources/responses/
//! streaming-events, read 2026-11-12).
//!
//! Each frame is one typed event whose `data` names its `type`. A tool call
//! is announced (`response.output_item.added`), its arguments stream as
//! deltas (`response.function_call_arguments.delta`), and the whole call
//! arrives again in `response.function_call_arguments.done` and
//! `response.output_item.done`; `response.completed` (or
//! `response.incomplete`) carries the whole response, its output and its
//! usage. A frame yields every event it carries, and nothing twice: the
//! parser remembers, per output index, which call it has started and whose
//! arguments or text it has already passed on, and the later whole forms
//! fill in only what was missed. So a call that arrives whole in one
//! `output_item.done`, or only inside `response.completed`, is still a call
//! with its arguments, and streamed arguments are never doubled into JSON
//! that cannot be read.

use super::{finish_of, message_text, reasoning_text, usage_of};
use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashSet, VecDeque};

/// What one stream has passed on so far, per output index.
#[derive(Default)]
struct Seen {
    /// Calls whose `ToolUseStart` went out.
    started: HashSet<usize>,
    /// Calls with arguments passed on.
    arguments: HashSet<usize>,
    /// Messages with text passed on.
    text: HashSet<usize>,
    /// Reasoning items with text passed on.
    thinking: HashSet<usize>,
    /// The whole items, from `response.output_item.done` — the kept turn's
    /// source: a reasoning item's `encrypted_content` "in
    /// `response.output_item.added` may be incomplete" (the reference).
    done: BTreeMap<usize, serde_json::Value>,
}

impl Seen {
    /// The events a whole item at `index` adds to what was passed on.
    fn fill(&mut self, index: usize, item: &serde_json::Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        match item["type"].as_str() {
            Some("function_call") => {
                if self.started.insert(index) {
                    out.push(StreamEvent::ToolUseStart {
                        index,
                        id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                    });
                }
                out.extend(self.arguments(index, &item["arguments"]));
            }
            Some("message") if !self.text.contains(&index) => {
                let text = message_text(item);
                if !text.is_empty() {
                    self.text.insert(index);
                    out.push(StreamEvent::TextDelta { text });
                }
            }
            Some("reasoning") if !self.thinking.contains(&index) => {
                let thinking = reasoning_text(item);
                if !thinking.is_empty() {
                    self.thinking.insert(index);
                    out.push(StreamEvent::ThinkingDelta { thinking });
                }
            }
            _ => {}
        }
        out
    }

    /// A call's whole arguments, passed on only when none were: normally a
    /// JSON string; a server sending the object itself has it written out
    /// as text, which the collector reads back (N-TOOL-5).
    fn arguments(&mut self, index: usize, arguments: &serde_json::Value) -> Option<StreamEvent> {
        if self.arguments.contains(&index) {
            return None;
        }
        let text = match arguments {
            serde_json::Value::String(text) => text.clone(),
            object @ serde_json::Value::Object(_) => object.to_string(),
            _ => String::new(),
        };
        (!text.is_empty()).then(|| {
            self.arguments.insert(index);
            StreamEvent::InputJsonDelta {
                index,
                partial_json: text,
            }
        })
    }

    /// The end of the response: whatever its output holds that no event
    /// passed on, the kept turn, the usage and the finish.
    fn finish(&mut self, response: &serde_json::Value) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        for (index, item) in response["output"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            out.extend(self.fill(index, item));
            self.done.entry(index).or_insert_with(|| item.clone());
        }
        let items: Vec<serde_json::Value> = self.done.values().cloned().collect();
        let called = items.iter().any(|item| item["type"] == "function_call");
        if let Some(turn) = EchoedTurn::from_response_items(items) {
            out.push(StreamEvent::Turn(turn));
        }
        out.push(StreamEvent::Usage(usage_of(response)));
        out.push(StreamEvent::Done {
            finish_reason: finish_of(response, called),
        });
        out
    }

    /// Every event one frame's `data` carries, in order.
    fn events(&mut self, json: &serde_json::Value) -> Vec<StreamEvent> {
        let index = json["output_index"].as_u64().unwrap_or(0) as usize;
        let delta = || json["delta"].as_str().unwrap_or_default().to_string();
        match json["type"].as_str().unwrap_or_default() {
            "response.output_item.added" => {
                let item = &json["item"];
                if item["type"] == "function_call" {
                    self.fill(index, item)
                } else {
                    Vec::new()
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let text = delta();
                if text.is_empty() {
                    return Vec::new();
                }
                self.text.insert(index);
                vec![StreamEvent::TextDelta { text }]
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let thinking = delta();
                if thinking.is_empty() {
                    return Vec::new();
                }
                self.thinking.insert(index);
                vec![StreamEvent::ThinkingDelta { thinking }]
            }
            "response.function_call_arguments.delta" => {
                let partial_json = delta();
                if partial_json.is_empty() {
                    return Vec::new();
                }
                self.arguments.insert(index);
                vec![StreamEvent::InputJsonDelta {
                    index,
                    partial_json,
                }]
            }
            "response.function_call_arguments.done" => self
                .arguments(index, &json["arguments"])
                .into_iter()
                .collect(),
            "response.output_item.done" => {
                let item = &json["item"];
                let out = self.fill(index, item);
                self.done.insert(index, item.clone());
                out
            }
            "response.completed" | "response.incomplete" => self.finish(&json["response"]),
            "response.failed" => vec![StreamEvent::Error {
                message: json["response"]["error"]["message"]
                    .as_str()
                    .unwrap_or("the response failed")
                    .to_string(),
            }],
            "error" => vec![StreamEvent::Error {
                message: json["message"]
                    .as_str()
                    .unwrap_or("the stream reported an error")
                    .to_string(),
            }],
            _ => Vec::new(),
        }
    }
}

/// Parse a Responses API SSE byte stream into `StreamEvent`s.
///
/// Like Chat Completions' parser (V1), a frame is decoded once, in full,
/// and the queue it filled is drained before another line is read. The
/// stream ends when the server closes it; the API sends no `[DONE]` line.
pub(in crate::providers::openai) fn parse_responses_sse(
    byte_stream: impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> impl futures_util::Stream<Item = Result<StreamEvent, LlmError>> + Send {
    let text_stream = crate::providers::utf8::utf8_chunks(byte_stream);
    futures_util::stream::unfold(
        (
            Box::pin(text_stream),
            String::new(),
            VecDeque::<StreamEvent>::new(),
            Seen::default(),
        ),
        |(mut stream, mut buffer, mut pending, mut seen)| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((Ok(event), (stream, buffer, pending, seen)));
                }

                while pending.is_empty() {
                    let Some(line_end) = buffer.find('\n') else {
                        break;
                    };
                    let line = buffer[..line_end].trim_end().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let Ok(json) = serde_json::from_str::<serde_json::Value>(data.trim_start())
                    else {
                        continue;
                    };
                    pending.extend(seen.events(&json));
                }

                if !pending.is_empty() {
                    continue;
                }

                match stream.next().await {
                    Some(Ok(text)) => buffer.push_str(&text),
                    Some(Err(e)) => {
                        return Some((
                            Err(LlmError::Stream(crate::providers::transport_message(&e))),
                            (stream, buffer, pending, seen),
                        ));
                    }
                    None => return None,
                }
            }
        },
    )
}
