//! C7a: a tool loop sends each assistant turn's thinking back unchanged.
//!
//! "If you run a tool-use loop, pass the `thinking` blocks from each
//! assistant response back to the API complete and unmodified when you
//! return tool results, including blocks whose `thinking` field is empty.
//! Echo the assistant message as received rather than filtering its content
//! blocks by type or rebuilding it: the API rejects edited, reordered, or
//! partially dropped thinking blocks with a 400 error."
//! (platform.claude.com/docs/en/models/opus-5-5/migration-guide, read
//! 2026-11-12.) A streamed thinking block ends with one `signature_delta`
//! "just before the block's `content_block_stop`"; a `redacted_thinking`
//! block carries its `data` whole and goes back unchanged
//! (.../build-with-claude/thinking, read 2026-11-12). Every provider here is
//! answered by a loopback [`MockHttpServer`].

use super::*;
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// The first response of a tool loop, as the Messages API returns it on
/// Claude Opus 5.5: an empty-text thinking block (the default `display` is
/// `"omitted"`), a summarized one, a redacted one, text, then a progress
/// update's thinking block right before the `tool_use` it introduces.
fn thinking_turn_blocks() -> serde_json::Value {
    serde_json::json!([
        {"type": "thinking", "thinking": "", "signature": "EqQBsig-A=="},
        {"type": "thinking", "thinking": "Search first.", "signature": "EqQBsig-B=="},
        {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4a"},
        {"type": "text", "text": "Looking."},
        {"type": "thinking", "thinking": "", "signature": "EqQBsig-C=="},
        {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "rust"}},
    ])
}

fn whole_response(model: &str, content: serde_json::Value, stop_reason: &str) -> String {
    serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason,
        "usage": {"input_tokens": 12, "output_tokens": 30},
    })
    .to_string()
}

fn final_answer(model: &str) -> String {
    whole_response(
        model,
        serde_json::json!([{"type": "text", "text": "Found it."}]),
        "end_turn",
    )
}

/// The same turn as Server-Sent Events, the way the thinking page shows a
/// stream: each block opens, its deltas follow, a thinking block's single
/// `signature_delta` comes last, and a redacted block arrives whole.
fn thinking_turn_frames() -> Vec<String> {
    let frame = |event: &str, data: serde_json::Value| format!("event: {event}\ndata: {data}\n\n");
    let delta = |index: u32, delta: serde_json::Value| {
        frame(
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": index, "delta": delta}),
        )
    };
    let start = |index: u32, block: serde_json::Value| {
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": index, "content_block": block}),
        )
    };
    let stop = |index: u32| {
        frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": index}),
        )
    };
    vec![
        frame(
            "message_start",
            serde_json::json!({"type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": "claude-opus-5-5", "usage": {"input_tokens": 12}}}),
        ),
        start(
            0,
            serde_json::json!({"type": "thinking", "thinking": "", "signature": ""}),
        ),
        delta(
            0,
            serde_json::json!({"type": "thinking_delta", "thinking": ""}),
        ),
        delta(
            0,
            serde_json::json!({"type": "signature_delta", "signature": "EqQBsig-A=="}),
        ),
        stop(0),
        start(
            1,
            serde_json::json!({"type": "thinking", "thinking": "", "signature": ""}),
        ),
        delta(
            1,
            serde_json::json!({"type": "thinking_delta", "thinking": "Search "}),
        ),
        delta(
            1,
            serde_json::json!({"type": "thinking_delta", "thinking": "first."}),
        ),
        delta(
            1,
            serde_json::json!({"type": "signature_delta", "signature": "EqQBsig-B=="}),
        ),
        stop(1),
        start(
            2,
            serde_json::json!({"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4a"}),
        ),
        stop(2),
        start(3, serde_json::json!({"type": "text", "text": ""})),
        delta(3, serde_json::json!({"type": "text_delta", "text": "Look"})),
        delta(3, serde_json::json!({"type": "text_delta", "text": "ing."})),
        stop(3),
        start(
            4,
            serde_json::json!({"type": "thinking", "thinking": "", "signature": ""}),
        ),
        delta(
            4,
            serde_json::json!({"type": "thinking_delta", "thinking": ""}),
        ),
        delta(
            4,
            serde_json::json!({"type": "signature_delta", "signature": "EqQBsig-C=="}),
        ),
        stop(4),
        start(
            5,
            serde_json::json!({"type": "tool_use", "id": "toolu_1", "name": "search", "input": {}}),
        ),
        delta(
            5,
            serde_json::json!({"type": "input_json_delta", "partial_json": "{\"q\": "}),
        ),
        delta(
            5,
            serde_json::json!({"type": "input_json_delta", "partial_json": "\"rust\"}"}),
        ),
        stop(5),
        frame(
            "message_delta",
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 30}}),
        ),
        frame("message_stop", serde_json::json!({"type": "message_stop"})),
    ]
}

fn provider_at(server: &MockHttpServer) -> AnthropicProvider {
    AnthropicProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("claude-opus-5-5".to_string()),
        None,
    )
    .with_api_base(format!("{}/v1", server.base_url))
}

fn search_tool() -> ToolDefinition {
    ToolDefinition {
        name: "search".to_string(),
        description: "Search.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"q": {"type": "string"}},
        }),
        ..Default::default()
    }
}

fn request(messages: Vec<ChatMessage>) -> ChatRequest {
    ChatRequest {
        messages: Arc::new(messages),
        tools: Arc::new(vec![search_tool()]),
        model: None,
        temperature: None,
        max_tokens: None,
        tool_choice: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        ephemeral_system_notice: None,
    }
}

async fn sent_bodies(server: &MockHttpServer) -> Vec<serde_json::Value> {
    server
        .requests()
        .await
        .iter()
        .map(|r| serde_json::from_str(&r.body).expect("a JSON body"))
        .collect()
}

/// A tool loop's second request sends the first response's assistant turn
/// back block for block — empty-text and redacted thinking, signatures and
/// order included — from a whole response.
#[tokio::test]
async fn a_tool_loop_echoes_a_whole_responses_thinking_byte_for_byte() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let server = MockHttpServer::start(move |_| {
        if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            MockResponse::json(whole_response(
                "claude-opus-5-5",
                thinking_turn_blocks(),
                "tool_use",
            ))
        } else {
            MockResponse::json(final_answer("claude-opus-5-5"))
        }
    })
    .await;
    let provider = provider_at(&server);

    let user = ChatMessage::user("find it");
    let first = provider
        .chat(request(vec![user.clone()]))
        .await
        .expect("the mock answers");
    assert_eq!(first.tool_calls.len(), 1);
    let looped = vec![
        user,
        ChatMessage::assistant_with_tools(&first),
        ChatMessage::tool_result("toolu_1", "found"),
    ];
    provider
        .chat(request(looped))
        .await
        .expect("the mock answers");

    let sent = sent_bodies(&server).await;
    assert_eq!(sent.len(), 2);
    let assistant = &sent[1]["messages"][1];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], thinking_turn_blocks());
}

/// The same from a streamed response: the blocks are put back together from
/// their deltas, `signature_delta` included, in their index order.
#[tokio::test]
async fn a_tool_loop_echoes_a_streamed_responses_thinking_byte_for_byte() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let server = MockHttpServer::start(move |_| {
        if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            MockResponse::sse(thinking_turn_frames()).every(Duration::ZERO)
        } else {
            MockResponse::json(final_answer("claude-opus-5-5"))
        }
    })
    .await;
    let provider = provider_at(&server);

    let user = ChatMessage::user("find it");
    let stream = provider
        .chat_streaming_with_key("sk-test-not-a-key", request(vec![user.clone()]))
        .await
        .expect("the stream opens");
    let first = crate::collect_stream(stream, "claude-opus-5-5".to_string())
        .await
        .expect("the stream completes");
    assert_eq!(first.tool_calls.len(), 1);
    assert_eq!(first.content, "Looking.");
    let looped = vec![
        user,
        ChatMessage::assistant_with_tools(&first),
        ChatMessage::tool_result("toolu_1", "found"),
    ];
    provider
        .chat(request(looped))
        .await
        .expect("the mock answers");

    let sent = sent_bodies(&server).await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["messages"][1]["content"], thinking_turn_blocks());
}

/// One SSE frame that ends the turn yields each event it carries: the
/// turn's blocks, then the usage, then the end (CLAUDE.md's stream-frame
/// rule, for the Anthropic stream where C7a touches it).
#[tokio::test]
async fn the_frame_that_ends_a_thinking_turn_yields_the_turn_its_usage_and_its_end() {
    use futures_util::StreamExt;

    let raw: String = thinking_turn_frames().concat();
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    let events: Vec<StreamEvent> = parse_anthropic_sse(byte_stream)
        .map(|e| e.expect("no stream error"))
        .collect()
        .await;

    let tail: Vec<&str> = events
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|e| match e {
            StreamEvent::Turn(_) => "turn",
            StreamEvent::Usage(_) => "usage",
            StreamEvent::Done { .. } => "done",
            _ => "other",
        })
        .collect();
    assert_eq!(tail, ["turn", "usage", "done"]);
    let turn = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Turn(turn) => Some(turn.clone()),
            _ => None,
        })
        .expect("a turn event");
    assert_eq!(
        serde_json::Value::Array(turn.blocks().to_vec()),
        thinking_turn_blocks()
    );
}

/// A thinking block whose signature never came cannot go back (an empty
/// signature is refused), so the turn is sent as it was before C7a, without
/// any of its thinking — never some of it.
#[tokio::test]
async fn a_turn_with_a_thinking_block_missing_its_signature_goes_back_without_thinking() {
    use futures_util::StreamExt;

    let frames: Vec<String> = thinking_turn_frames()
        .into_iter()
        .filter(|f| !f.contains("EqQBsig-B=="))
        .collect();
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(frames.concat()))]);
    let events: Vec<StreamEvent> = parse_anthropic_sse(byte_stream)
        .map(|e| e.expect("no stream error"))
        .collect()
        .await;

    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Turn(_))),
        "no turn is kept for an echo"
    );
}

/// A streamed turn holding a block the arm does not put back together (a
/// server tool's `server_tool_use`) keeps none of its thinking for an echo:
/// the turn could not go back whole, and it never goes back in part.
#[tokio::test]
async fn a_streamed_turn_with_a_server_tool_block_keeps_no_turn() {
    use futures_util::StreamExt;

    let frame = |event: &str, data: serde_json::Value| format!("event: {event}\ndata: {data}\n\n");
    let frames = [
        frame(
            "message_start",
            serde_json::json!({"type": "message_start", "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "content": [],
                "model": "claude-opus-5-5", "usage": {"input_tokens": 12}}}),
        ),
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        frame(
            "content_block_delta",
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "signature_delta", "signature": "EqQBsig-A=="}}),
        ),
        frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": 0}),
        ),
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 1,
                "content_block": {"type": "server_tool_use", "id": "srvtoolu_1",
                    "name": "web_search", "input": {}}}),
        ),
        frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": 1}),
        ),
        frame(
            "content_block_start",
            serde_json::json!({"type": "content_block_start", "index": 2,
                "content_block": {"type": "text", "text": "Done."}}),
        ),
        frame(
            "content_block_stop",
            serde_json::json!({"type": "content_block_stop", "index": 2}),
        ),
        frame(
            "message_delta",
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 30}}),
        ),
        frame("message_stop", serde_json::json!({"type": "message_stop"})),
    ];
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(frames.concat()))]);
    let events: Vec<StreamEvent> = parse_anthropic_sse(byte_stream)
        .map(|e| e.expect("no stream error"))
        .collect()
        .await;

    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::Done { .. })),
        "the stream ends"
    );
    assert!(
        !events.iter().any(|e| matches!(e, StreamEvent::Turn(_))),
        "no turn is kept for an echo"
    );
}

/// What the arm sends for a model without thinking: a tool loop whose first
/// response carried none goes exactly as before C7a (characterization).
#[test]
fn a_tool_loop_without_thinking_is_sent_as_before() {
    for model in ["claude-opus-4-6", "claude-sonnet-4-6"] {
        let provider =
            AnthropicProvider::new("test-key".to_string(), Some(model.to_string()), None);
        let first = provider
            .parse_response(serde_json::from_str(&whole_response(
                model,
                serde_json::json!([
                    {"type": "text", "text": "Looking."},
                    {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "rust"}},
                ]),
                "tool_use",
            ))
            .unwrap())
            .unwrap();
        let body = provider.build_request_body(&request(vec![
            ChatMessage::system("Be brief."),
            ChatMessage::user("find it"),
            ChatMessage::assistant_with_tools(&first),
            ChatMessage::tool_result("toolu_1", "found"),
        ]));

        assert_eq!(
            body,
            serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "system": "Be brief.",
                "messages": [
                    {"role": "user", "content": "find it"},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "Looking."},
                        {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "rust"}},
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1", "content": "found"},
                    ]},
                ],
                "tools": [{
                    "name": "search",
                    "description": "Search.",
                    "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}},
                }],
            }),
            "{model}"
        );
    }
}

/// An assistant message whose kept turn no longer matches its tool calls
/// goes without thinking, and so does every later turn of the request: a
/// block left out between two that are kept breaks the run the API checks
/// ("Leave out the oldest blocks, the newest, or all of them, but none in
/// between", .../preserved-thinking/change-tools-or-instructions, read
/// 2026-11-12).
#[test]
fn a_kept_turn_that_no_longer_matches_its_message_goes_without_thinking_and_so_do_later_ones() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let turn = |id: &str, sig: &str| {
        let blocks = vec![
            serde_json::json!({"type": "thinking", "thinking": "", "signature": sig}),
            serde_json::json!({"type": "tool_use", "id": id, "name": "search", "input": {"q": "rust"}}),
        ];
        let response = ChatResponse {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: id.to_string(),
                name: "search".to_string(),
                arguments: serde_json::json!({"q": "rust"}),
            }],
            model: "claude-opus-5-5".to_string(),
            usage: Usage::default(),
            finish_reason: FinishReason::ToolUse,
            thinking: None,
            parts: None,
            echo: EchoedTurn::from_blocks(blocks),
        };
        ChatMessage::assistant_with_tools(&response)
    };
    let mut changed = turn("toolu_1", "sig-1");
    changed.tool_calls.as_mut().unwrap()[0].id = "toolu_other".to_string();

    let body = provider.build_request_body(&request(vec![
        ChatMessage::user("find it"),
        turn("toolu_0", "sig-0"),
        ChatMessage::tool_result("toolu_0", "found"),
        changed,
        ChatMessage::tool_result("toolu_other", "found"),
        turn("toolu_2", "sig-2"),
        ChatMessage::tool_result("toolu_2", "found"),
    ]));

    let thinking_in = |i: usize| {
        body["messages"][i]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"] == "thinking")
            .count()
    };
    assert_eq!(thinking_in(1), 1, "the first turn keeps its thinking");
    assert_eq!(thinking_in(3), 0, "the changed turn goes without");
    assert_eq!(thinking_in(5), 0, "and so does every later turn");
}

/// A turn that called no tool goes back whole through
/// [`ChatMessage::assistant_answer`], the message the loop's `MaxTokens`
/// continuation, its steering drain at the completion guard and its answer
/// guard's round push: every thinking block with its signature, in its
/// order, text included — and a turn of thinking blocks alone (its text
/// never came) goes back as those blocks. A turn that called a tool goes
/// back from `assistant_answer` as its text alone, without thinking.
#[test]
fn an_answer_that_called_no_tool_goes_back_with_all_its_thinking() {
    let provider = AnthropicProvider::new("test-key".to_string(), None, None);
    let answered = |content: serde_json::Value, stop_reason: &str| {
        provider
            .parse_response(
                serde_json::from_str(&whole_response("claude-opus-5-5", content, stop_reason))
                    .unwrap(),
            )
            .unwrap()
    };
    let sent_back = |response: &ChatResponse| {
        provider.build_request_body(&request(vec![
            ChatMessage::user("write it"),
            ChatMessage::assistant_answer(response),
            ChatMessage::user("Continue from where you left off."),
        ]))["messages"][1]
            .clone()
    };

    let with_text = serde_json::json!([
        {"type": "thinking", "thinking": "", "signature": "EqQBsig-A=="},
        {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4a"},
        {"type": "text", "text": "The first half"},
        {"type": "thinking", "thinking": "Then the rest.", "signature": "EqQBsig-B=="},
    ]);
    let truncated = answered(with_text.clone(), "max_tokens");
    assert_eq!(truncated.finish_reason, FinishReason::MaxTokens);
    let assistant = sent_back(&truncated);
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(assistant["content"], with_text);

    let thinking_alone = serde_json::json!([
        {"type": "thinking", "thinking": "", "signature": "EqQBsig-C=="},
        {"type": "thinking", "thinking": "Still thinking.", "signature": "EqQBsig-D=="},
    ]);
    let assistant = sent_back(&answered(thinking_alone.clone(), "max_tokens"));
    assert_eq!(assistant["content"], thinking_alone);

    let assistant = sent_back(&answered(thinking_turn_blocks(), "tool_use"));
    assert_eq!(assistant["content"], "Looking.");
}
