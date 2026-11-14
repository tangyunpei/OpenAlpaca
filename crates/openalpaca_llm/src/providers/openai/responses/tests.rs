//! C7b: a request with tools to OpenAI's own API goes through the Responses
//! API (`POST /v1/responses`), every other request and every other
//! OpenAI-compatible server through Chat Completions, as before.
//!
//! The wire facts are the pages read 2026-11-12: the function-calling guide
//! (developers.openai.com/api/docs/guides/function-calling), the reasoning
//! guide (.../guides/reasoning), the migration guide
//! (.../guides/migrate-to-responses), "Keep the prefix unchanged"
//! (.../guides/preserved-reasoning), the streaming guide
//! (.../guides/streaming-responses) and the Responses API reference
//! (.../api/reference/resources/responses/methods/create and
//! .../responses/streaming-events). Every server here is a loopback
//! [`MockHttpServer`]; no test reaches a provider.

use crate::LlmProvider;
use crate::providers::openai::{OpenAiProvider, is_openai_api, parse_responses_sse};
use crate::test_support::{MockHttpServer, MockResponse, loopback_client};
use crate::types::*;
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A tool whose schema strict mode accepts: closed, every property required.
fn closed_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} things"),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"q": {"type": "string"}},
            "required": ["q"],
            "additionalProperties": false,
        }),
        strict: Some(true),
        input_examples: None,
    }
}

/// A strict-marked tool with an optional property: strict mode refuses it.
fn open_tool(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} things"),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"q": {"type": "string"}, "n": {"type": "integer"}},
            "required": ["q"],
        }),
        strict: Some(true),
        input_examples: None,
    }
}

fn request(messages: Vec<ChatMessage>, tools: Vec<ToolDefinition>) -> ChatRequest {
    ChatRequest {
        messages: Arc::new(messages),
        tools: Arc::new(tools),
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

/// The cloud arm as an install builds it: OpenAI's own API, the default
/// `base_url`.
fn cloud() -> OpenAiProvider {
    OpenAiProvider::new(
        "sk-test-not-a-key".to_string(),
        Some("gpt-6.1-sol".to_string()),
        None,
        None,
    )
}

/// A whole Responses API answer with these output items.
fn answer(output: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "resp_1",
        "object": "response",
        "model": "gpt-6.1-sol",
        "status": "completed",
        "output": output,
        "usage": {
            "input_tokens": 120,
            "input_tokens_details": {"cached_tokens": 100},
            "output_tokens": 50,
            "output_tokens_details": {"reasoning_tokens": 30},
            "total_tokens": 170
        }
    })
}

fn reasoning_item(id: &str, encrypted: Option<&str>) -> serde_json::Value {
    let mut item = serde_json::json!({"type": "reasoning", "id": id, "summary": []});
    if let Some(e) = encrypted {
        item["encrypted_content"] = serde_json::json!(e);
    }
    item
}

fn call_item(call_id: &str, name: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "function_call",
        "id": format!("fc_{call_id}"),
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
        "status": "completed"
    })
}

fn message_item(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "status": "completed",
        "phase": "commentary",
        "content": [{"type": "output_text", "text": text, "annotations": []}]
    })
}

// ── Which wire ───────────────────────────────────────────────────────────

/// OpenAI's own API is `https` to `api.openai.com` or one of the regional
/// hosts its data-residency page lists (developers.openai.com/api/docs/
/// guides/your-data, read 2026-11-12), and nothing else: a look-alike host,
/// a path naming it, a loopback server and Ollama keep Chat Completions.
#[test]
fn only_openais_own_hosts_are_the_responses_api() {
    for own in [
        "https://api.openai.com/v1",
        "https://api.openai.com/v1/",
        "https://API.OPENAI.COM/v1",
        "https://us.api.openai.com/v1",
        "https://eu.api.openai.com/v1",
        "https://ae.api.openai.com/v1",
    ] {
        assert!(is_openai_api(own), "{own}");
    }
    for other in [
        "http://api.openai.com/v1",
        "https://api.openai.com.example.net/v1",
        "https://example.net/api.openai.com/v1",
        "https://eu.api.openai.com.evil/v1",
        "http://eu.api.openai.com/v1",
        "https://xx.api.openai.com/v1",
        "https://eu.eu.api.openai.com/v1",
        "https://evil-api.openai.com/v1",
        "http://127.0.0.1:1/v1",
        "http://localhost:11434/v1",
        "not a url",
        "",
    ] {
        assert!(!is_openai_api(other), "{other}");
    }
}

/// A request with tools to OpenAI's own API moves; one with neither tools
/// nor any tool call in its history stays; one whose history holds tool
/// calls moves too, since that is still function calling.
#[test]
fn a_request_with_tools_or_tool_history_is_a_responses_request() {
    let provider = cloud();
    let plain = request(vec![ChatMessage::user("hi")], vec![]);
    assert!(!provider.uses_responses(&plain));

    let with_tools = request(vec![ChatMessage::user("hi")], vec![closed_tool("search")]);
    assert!(provider.uses_responses(&with_tools));

    let called = ChatResponse {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "x"}),
        }],
        model: "gpt-6.1-sol".to_string(),
        usage: Usage::default(),
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
        echo: None,
    };
    let history = request(
        vec![
            ChatMessage::user("hi"),
            ChatMessage::assistant_with_tools(&called),
            ChatMessage::tool_result("call_1", "found"),
        ],
        vec![],
    );
    assert!(provider.uses_responses(&history));

    // The same requests to a server that is not OpenAI's own never move.
    let compatible = OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("local-model".to_string()),
        Some("http://127.0.0.1:1/v1".to_string()),
        None,
    );
    assert!(!compatible.uses_responses(&with_tools));
    assert!(!compatible.uses_responses(&history));
}

// ── The request ─────────────────────────────────────────────────────────

/// The body: `max_output_tokens` (not `max_tokens`), `input` items (not
/// `messages`), `store: false`, and each system message as a `system`
/// input item in its place.
#[test]
fn a_responses_body_names_max_output_tokens_input_and_store_false() {
    let mut req = request(
        vec![
            ChatMessage::system("You are helpful."),
            ChatMessage::user("Hello"),
        ],
        vec![closed_tool("search")],
    );
    req.max_tokens = Some(2048);
    let body = cloud().build_responses_body(&req);

    assert_eq!(body["model"], "gpt-6.1-sol");
    assert_eq!(body["max_output_tokens"], 2048);
    assert_eq!(body["store"], false);
    assert!(body.get("max_tokens").is_none());
    assert!(body.get("messages").is_none());
    assert!(body.get("stream_options").is_none());
    assert!(body.get("include").is_none());
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(
        body["input"],
        serde_json::json!([
            {"role": "system", "content": "You are helpful."},
            {"role": "user", "content": "Hello"},
        ])
    );

    let defaulted = cloud().build_responses_body(&request(
        vec![ChatMessage::user("Hello")],
        vec![closed_tool("search")],
    ));
    assert_eq!(defaulted["max_output_tokens"], 4096);
}

/// The tools in the Responses form: `strict` beside `type`, `name` and
/// `parameters`, `true` only where strict mode accepts the schema and an
/// explicit `false` otherwise — on Responses an omitted `strict` asks the
/// API to make the schema strict.
#[test]
fn the_tools_go_in_the_responses_form_with_strict_at_the_top_level() {
    let mut unmarked = closed_tool("plain");
    unmarked.strict = None;
    let body = cloud().build_responses_body(&request(
        vec![ChatMessage::user("go")],
        vec![closed_tool("search"), open_tool("fetch"), unmarked],
    ));
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(
        tools[0],
        serde_json::json!({
            "type": "function",
            "name": "search",
            "description": "search things",
            "parameters": closed_tool("search").parameters,
            "strict": true,
        })
    );
    assert_eq!(
        tools[1]["strict"], false,
        "a refused schema goes non-strict"
    );
    assert_eq!(
        tools[1]["parameters"],
        open_tool("fetch").parameters,
        "as written"
    );
    assert_eq!(
        tools[2]["strict"], false,
        "an unmarked tool goes non-strict"
    );
    assert!(tools.iter().all(|t| t.get("function").is_none()));
}

/// `tool_choice` in the Responses form: a named function is
/// `{"type": "function", "name": …}`, not under `function`.
#[test]
fn tool_choice_goes_in_the_responses_form() {
    let mut req = request(vec![ChatMessage::user("go")], vec![closed_tool("send")]);
    for (choice, wire) in [
        (ToolChoice::Auto, serde_json::json!("auto")),
        (ToolChoice::Any, serde_json::json!("required")),
        (
            ToolChoice::Tool("send".to_string()),
            serde_json::json!({"type": "function", "name": "send"}),
        ),
    ] {
        req.tool_choice = Some(choice);
        assert_eq!(cloud().build_responses_body(&req)["tool_choice"], wire);
    }
}

/// `thinking` maps to `reasoning.effort` as it maps to `reasoning_effort` on
/// Chat Completions, `temperature` dropped where it is set; `Disabled`
/// sends nothing, as the cloud arm always has.
#[test]
fn thinking_maps_to_reasoning_effort_as_on_chat_completions() {
    let mut req = request(vec![ChatMessage::user("go")], vec![closed_tool("search")]);
    req.temperature = Some(0.3);

    let plain = cloud().build_responses_body(&req);
    assert!(plain.get("reasoning").is_none());
    let temperature = plain["temperature"].as_f64().unwrap();
    assert!((temperature - 0.3).abs() < 1e-6);

    for (thinking, effort) in [
        (
            ThinkingConfig::Enabled {
                budget_tokens: 4000,
            },
            "high",
        ),
        (ThinkingConfig::Adaptive, "medium"),
    ] {
        req.thinking = Some(thinking);
        let body = cloud().build_responses_body(&req);
        assert_eq!(body["reasoning"], serde_json::json!({"effort": effort}));
        assert!(body.get("temperature").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    req.thinking = Some(ThinkingConfig::Disabled);
    let body = cloud().build_responses_body(&req);
    assert!(body.get("reasoning").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

/// The ephemeral notice is the input's last item, a `system` message —
/// never `instructions`, which would change the prefix every round.
#[test]
fn the_ephemeral_notice_is_the_last_input_item() {
    let mut req = request(
        vec![ChatMessage::system("Base."), ChatMessage::user("go")],
        vec![closed_tool("search")],
    );
    req.ephemeral_system_notice = Some("Budget: 2 rounds left.".to_string());
    let body = cloud().build_responses_body(&req);
    assert!(body.get("instructions").is_none());
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input.last().unwrap(),
        &serde_json::json!({"role": "system", "content": "Budget: 2 rounds left."})
    );
}

/// A user message's parts become `input_text` and `input_image` items.
#[test]
fn a_user_messages_parts_become_input_content() {
    let msg = ChatMessage::user_with_parts(vec![
        ContentPart::Text {
            text: "What is this?".to_string(),
        },
        ContentPart::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".to_string(),
                data: Arc::new("AAAA".to_string()),
            },
            detail: None,
        },
    ]);
    let body = cloud().build_responses_body(&request(vec![msg], vec![closed_tool("search")]));
    assert_eq!(
        body["input"][0],
        serde_json::json!({
            "role": "user",
            "content": [
                {"type": "input_text", "text": "What is this?"},
                {"type": "input_image", "image_url": "data:image/png;base64,AAAA", "detail": "auto"},
            ]
        })
    );
}

// ── The round trip ──────────────────────────────────────────────────────

/// A function call comes back as a `ToolCall` named by its `call_id`, and
/// goes back as a `function_call` item followed by its
/// `function_call_output`, linked by that `call_id`.
#[test]
fn a_function_call_and_its_output_round_trip() {
    let provider = cloud();
    let response = provider
        .parse_responses_response(answer(serde_json::json!([call_item(
            "call_1",
            "search",
            r#"{"q":"alpacas"}"#
        )])))
        .expect("parses");
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_1");
    assert_eq!(response.tool_calls[0].name, "search");
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"q": "alpacas"})
    );
    assert!(response.echo.is_none(), "no reasoning item, nothing kept");

    let next = provider.build_responses_body(&request(
        vec![
            ChatMessage::user("find alpacas"),
            ChatMessage::assistant_with_tools(&response),
            ChatMessage::tool_result("call_1", "3 alpacas"),
        ],
        vec![closed_tool("search")],
    ));
    assert_eq!(
        next["input"],
        serde_json::json!([
            {"role": "user", "content": "find alpacas"},
            {"type": "function_call", "call_id": "call_1", "name": "search", "arguments": "{\"q\":\"alpacas\"}"},
            {"type": "function_call_output", "call_id": "call_1", "output": "3 alpacas"},
        ])
    );
}

/// A failed tool's result keeps its `[tool_error]` text, as on Chat
/// Completions: the wire has no error field.
#[test]
fn a_failed_tool_result_goes_back_as_its_marked_text() {
    let body = cloud().build_responses_body(&request(
        vec![
            ChatMessage::user("go"),
            ChatMessage::tool_error("call_9", "[tool_error] boom"),
        ],
        vec![closed_tool("search")],
    ));
    assert_eq!(
        body["input"][1],
        serde_json::json!({"type": "function_call_output", "call_id": "call_9", "output": "[tool_error] boom"})
    );
}

/// Reasoning items go back with the tool outputs: the turn's output items,
/// verbatim and in order, `encrypted_content` included.
#[test]
fn reasoning_items_go_back_with_the_tool_outputs() {
    let provider = cloud();
    let output = serde_json::json!([
        reasoning_item("rs_1", Some("gAAAA-encrypted")),
        message_item("Looking it up."),
        call_item("call_1", "search", r#"{"q":"alpacas"}"#),
    ]);
    let response = provider
        .parse_responses_response(answer(output.clone()))
        .expect("parses");
    let echo = response.echo.clone().expect("a reasoning turn is kept");
    assert_eq!(echo.wire(), EchoWire::OpenAiResponses);
    assert_eq!(echo.function_call_ids(), vec!["call_1"]);
    assert_eq!(response.content, "Looking it up.");

    let next = provider.build_responses_body(&request(
        vec![
            ChatMessage::user("find alpacas"),
            ChatMessage::assistant_with_tools(&response),
            ChatMessage::tool_result("call_1", "3 alpacas"),
        ],
        vec![closed_tool("search")],
    ));
    let mut expected = vec![serde_json::json!({"role": "user", "content": "find alpacas"})];
    expected.extend(output.as_array().unwrap().iter().cloned());
    expected.push(serde_json::json!(
        {"type": "function_call_output", "call_id": "call_1", "output": "3 alpacas"}
    ));
    assert_eq!(next["input"], serde_json::Value::Array(expected));
}

/// A reasoning item without its encrypted content cannot go back with
/// nothing stored, so the turn keeps none and is rebuilt as before.
#[test]
fn a_reasoning_item_without_encrypted_content_keeps_no_turn() {
    let response = cloud()
        .parse_responses_response(answer(serde_json::json!([
            reasoning_item("rs_1", None),
            call_item("call_1", "search", "{}"),
        ])))
        .expect("parses");
    assert!(response.echo.is_none());
}

/// A kept turn whose calls no longer match the message's tool calls goes
/// back rebuilt from the message, without its reasoning.
#[test]
fn a_kept_turn_whose_calls_changed_is_rebuilt() {
    let provider = cloud();
    let mut response = provider
        .parse_responses_response(answer(serde_json::json!([
            reasoning_item("rs_1", Some("gAAAA-encrypted")),
            call_item("call_1", "search", r#"{"q":"a"}"#),
        ])))
        .expect("parses");
    assert!(response.echo.is_some());
    response.tool_calls[0].id = "call_other".to_string();
    let body = provider.build_responses_body(&request(
        vec![
            ChatMessage::user("go"),
            ChatMessage::assistant_with_tools(&response),
        ],
        vec![closed_tool("search")],
    ));
    let text = body.to_string();
    assert!(!text.contains("encrypted"), "{text}");
    assert_eq!(body["input"][1]["call_id"], "call_other");
}

/// Each arm sends back only its own turn. An Anthropic turn (thinking
/// blocks and signatures) reaching the OpenAI arm after a fallback is
/// rebuilt there, on both wires; an OpenAI turn reaching the Anthropic arm
/// is rebuilt there.
#[test]
fn each_arm_sends_back_only_its_own_turn() {
    let anthropic_turn = EchoedTurn::from_blocks(vec![
        serde_json::json!({"type": "thinking", "thinking": "", "signature": "sig-A"}),
        serde_json::json!({"type": "text", "text": "Checking."}),
        serde_json::json!({"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "a"}}),
    ])
    .expect("an Anthropic thinking turn");
    let from_claude = ChatResponse {
        content: "Checking.".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "a"}),
        }],
        model: "claude-opus-5-5".to_string(),
        usage: Usage::default(),
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
        echo: Some(anthropic_turn),
    };
    let messages = vec![
        ChatMessage::user("go"),
        ChatMessage::assistant_with_tools(&from_claude),
        ChatMessage::tool_result("call_1", "found"),
    ];
    let responses =
        cloud().build_responses_body(&request(messages.clone(), vec![closed_tool("search")]));
    let chat = cloud().build_request_body(&request(messages, vec![closed_tool("search")]));
    for body in [responses, chat] {
        let text = body.to_string();
        assert!(
            !text.contains("signature") && !text.contains("sig-A"),
            "{text}"
        );
        assert!(!text.contains("\"thinking\""), "{text}");
    }

    let openai_turn = cloud()
        .parse_responses_response(answer(serde_json::json!([
            reasoning_item("rs_1", Some("gAAAA-encrypted")),
            message_item("Done."),
        ])))
        .expect("parses");
    assert!(openai_turn.echo.is_some());
    let anthropic = crate::providers::anthropic::AnthropicProvider::new(
        "sk-ant-test-not-a-key".to_string(),
        None,
        None,
    );
    let body = anthropic.build_request_body(&request(
        vec![
            ChatMessage::user("go"),
            ChatMessage::assistant_answer(&openai_turn),
            ChatMessage::user("and?"),
        ],
        vec![],
    ));
    let text = body.to_string();
    assert!(
        !text.contains("encrypted") && !text.contains("rs_1"),
        "{text}"
    );
    assert!(body["messages"][1].to_string().contains("Done."), "{text}");
}

/// An Anthropic answer turn — thinking and text, no tool call, what
/// `assistant_answer` carries — reaching the OpenAI arm after a fallback is
/// rebuilt there as its text. With no calls on either side the call-id
/// check passes, so only the arm's wire filter keeps its blocks out.
#[test]
fn an_anthropic_answer_turn_is_rebuilt_on_the_responses_wire() {
    let anthropic_turn = EchoedTurn::from_blocks(vec![
        serde_json::json!({"type": "thinking", "thinking": "", "signature": "sig-A"}),
        serde_json::json!({"type": "text", "text": "Checking."}),
    ])
    .expect("an Anthropic thinking turn");
    let from_claude = ChatResponse {
        content: "Checking.".to_string(),
        tool_calls: vec![],
        model: "claude-opus-5-5".to_string(),
        usage: Usage::default(),
        finish_reason: FinishReason::Stop,
        thinking: None,
        parts: None,
        echo: Some(anthropic_turn),
    };
    let body = cloud().build_responses_body(&request(
        vec![
            ChatMessage::user("go"),
            ChatMessage::assistant_answer(&from_claude),
            ChatMessage::user("and?"),
        ],
        vec![closed_tool("search")],
    ));
    assert_eq!(
        body["input"][1],
        serde_json::json!({"role": "assistant", "content": "Checking."})
    );
    let text = body.to_string();
    assert!(
        !text.contains("signature") && !text.contains("sig-A"),
        "{text}"
    );
}

/// A turn cut at `max_output_tokens` before any text — what a reasoning
/// model can do inside the arm's 4096 default — goes back, in the
/// `MaxTokens` continuation, as its reasoning item when it has one, and as
/// an empty assistant message when it has none: the shape step 19's check
/// meets, which no page read settles. Only a streamed round reaches that
/// continuation; a non-streamed one ends as the router's `EmptyCompletion`
/// (`routing/router/pricing_tests.rs`).
#[test]
fn a_turn_cut_before_any_text_goes_back_as_its_items_or_as_empty_text() {
    let provider = cloud();
    let cut = |output: serde_json::Value| {
        let mut body = answer(output);
        body["status"] = serde_json::json!("incomplete");
        body["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});
        provider.parse_responses_response(body).expect("parses")
    };
    let next = |turn: &ChatResponse| {
        provider.build_responses_body(&request(
            vec![
                ChatMessage::user("go"),
                ChatMessage::assistant_answer(turn),
                ChatMessage::user("continue"),
            ],
            vec![closed_tool("search")],
        ))
    };

    let kept = cut(serde_json::json!([reasoning_item(
        "rs_1",
        Some("gAAAA-encrypted")
    )]));
    assert_eq!(kept.finish_reason, FinishReason::MaxTokens);
    assert_eq!(kept.content, "");
    assert_eq!(
        next(&kept)["input"][1],
        reasoning_item("rs_1", Some("gAAAA-encrypted"))
    );

    let bare = cut(serde_json::json!([]));
    assert!(bare.echo.is_none());
    assert_eq!(
        next(&bare)["input"][1],
        serde_json::json!({"role": "assistant", "content": ""})
    );
}

// ── The whole answer ────────────────────────────────────────────────────

/// Usage: input and output tokens as reported (output includes reasoning,
/// "billed as output tokens", the reasoning guide), cached tokens as the
/// cache read, as Chat Completions' `prompt_tokens_details`.
#[test]
fn a_whole_answer_reads_text_usage_and_model() {
    let response = cloud()
        .parse_responses_response(answer(serde_json::json!([message_item("Hello.")])))
        .expect("parses");
    assert_eq!(response.content, "Hello.");
    assert_eq!(response.model, "gpt-6.1-sol");
    assert_eq!(response.finish_reason, FinishReason::Stop);
    assert_eq!(response.usage.input_tokens, 120);
    assert_eq!(response.usage.output_tokens, 50);
    assert_eq!(response.usage.cache_read_input_tokens, 100);
    assert_eq!(response.usage.cache_creation_input_tokens, 0);
}

/// An answer cut at `max_output_tokens` is `MaxTokens`.
#[test]
fn an_incomplete_answer_at_max_output_tokens_is_max_tokens() {
    let mut body = answer(serde_json::json!([reasoning_item("rs_1", Some("e"))]));
    body["status"] = serde_json::json!("incomplete");
    body["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});
    let response = cloud().parse_responses_response(body).expect("parses");
    assert_eq!(response.finish_reason, FinishReason::MaxTokens);
}

/// Arguments that cannot be read are carried as unreadable, never as `{}`.
#[test]
fn unreadable_arguments_are_never_an_empty_object() {
    let response = cloud()
        .parse_responses_response(answer(serde_json::json!([call_item(
            "call_1", "search", "{\"q\": "
        )])))
        .expect("parses");
    let arguments = &response.tool_calls[0].arguments;
    assert_ne!(arguments, &serde_json::json!({}));
    assert!(
        crate::UnreadableArguments::of(arguments).is_some(),
        "{arguments}"
    );
}

/// A refusal is the turn's text: a `message` item's `refusal` part reads
/// as its answer, beside any `output_text` (the reference's output content
/// is `output_text` or `refusal`).
#[test]
fn a_refusal_part_is_the_answers_text() {
    let refusal = serde_json::json!({
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "refusal", "refusal": "I can't help with that."}]
    });
    let response = cloud()
        .parse_responses_response(answer(serde_json::json!([refusal])))
        .expect("parses");
    assert_eq!(response.content, "I can't help with that.");
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

/// A 200 body whose `status` is `failed` is an error, classed by its
/// `error.code` (the reference lists `server_error`,
/// `rate_limit_exceeded` and others): a server error as a 5xx, a rate
/// limit as one (the arm's own 1,000 ms), anything else as a 400.
#[test]
fn a_failed_body_is_an_error_by_its_code() {
    let failed = |code: &str| {
        let mut body = answer(serde_json::json!([]));
        body["status"] = serde_json::json!("failed");
        body["error"] = serde_json::json!({"code": code, "message": format!("{code} said")});
        cloud().parse_responses_response(body)
    };
    match failed("server_error") {
        Err(crate::error::LlmError::Api { status, message }) => {
            assert_eq!(status, 500);
            assert_eq!(message, "server_error said");
        }
        other => panic!("server_error: {other:?}"),
    }
    match failed("rate_limit_exceeded") {
        Err(crate::error::LlmError::RateLimited { retry_after_ms }) => {
            assert_eq!(retry_after_ms, 1_000);
        }
        other => panic!("rate_limit_exceeded: {other:?}"),
    }
    match failed("invalid_prompt") {
        Err(crate::error::LlmError::Api { status, message }) => {
            assert_eq!(status, 400);
            assert_eq!(message, "invalid_prompt said");
        }
        other => panic!("invalid_prompt: {other:?}"),
    }
}

// ── The stream ──────────────────────────────────────────────────────────

/// One SSE frame per event, as the API writes them.
fn frame(event: serde_json::Value) -> String {
    format!(
        "event: {}\ndata: {}\n\n",
        event["type"].as_str().unwrap(),
        event
    )
}

fn completed(output: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"type": "response.completed", "response": answer(output)})
}

fn added(index: usize, item: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"type": "response.output_item.added", "output_index": index, "item": item})
}

fn done(index: usize, item: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"type": "response.output_item.done", "output_index": index, "item": item})
}

fn args_delta(index: usize, delta: &str) -> serde_json::Value {
    serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": index, "item_id": "fc", "delta": delta})
}

fn args_done(index: usize, arguments: &str) -> serde_json::Value {
    serde_json::json!({"type": "response.function_call_arguments.done", "output_index": index, "item_id": "fc", "arguments": arguments})
}

fn text_delta(index: usize, delta: &str) -> serde_json::Value {
    serde_json::json!({"type": "response.output_text.delta", "output_index": index, "content_index": 0, "item_id": "msg_1", "delta": delta})
}

/// A started call: its `added` item carries no arguments yet.
fn started(call_id: &str, name: &str) -> serde_json::Value {
    let mut item = call_item(call_id, name, "");
    item["status"] = serde_json::json!("in_progress");
    item
}

/// The whole stream in one network chunk: every frame the chunk carries.
async fn events_of(frames: &[serde_json::Value]) -> Vec<StreamEvent> {
    use futures_util::StreamExt;
    let raw: String = frames.iter().cloned().map(frame).collect();
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    parse_responses_sse(byte_stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|e| e.expect("no stream error"))
        .collect()
}

async fn collected(frames: &[serde_json::Value]) -> ChatResponse {
    let raw: String = frames.iter().cloned().map(frame).collect();
    let byte_stream = futures_util::stream::iter(vec![Ok(bytes::Bytes::from(raw))]);
    crate::streaming::collect_stream(
        Box::pin(parse_responses_sse(byte_stream)),
        "mock".to_string(),
    )
    .await
    .expect("the stream collects")
}

/// A whole call in one frame — its `output_item.done` alone, no `added`
/// and no deltas — is still a call, with its arguments, once.
#[tokio::test]
async fn a_whole_call_in_one_frame_keeps_its_arguments() {
    let call = call_item("call_1", "search", r#"{"q":"alpacas"}"#);
    let response = collected(&[done(0, call.clone()), completed(serde_json::json!([call]))]).await;
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_1");
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"q": "alpacas"})
    );
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
}

/// Arguments arrive as deltas and again whole in `arguments.done` and
/// `output_item.done`: they are counted once.
#[tokio::test]
async fn arguments_streamed_as_deltas_are_not_doubled_by_the_done_events() {
    let whole = r#"{"q":"alpacas"}"#;
    let response = collected(&[
        added(0, started("call_1", "search")),
        args_delta(0, r#"{"q":"#),
        args_delta(0, r#""alpacas"}"#),
        args_done(0, whole),
        done(0, call_item("call_1", "search", whole)),
        completed(serde_json::json!([call_item("call_1", "search", whole)])),
    ])
    .await;
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"q": "alpacas"})
    );
}

/// Two calls in one frame both survive, each with its own arguments.
#[tokio::test]
async fn two_calls_in_one_frame_both_survive() {
    let a = call_item("call_a", "search", r#"{"q":"a"}"#);
    let b = call_item("call_b", "fetch", r#"{"q":"b"}"#);
    let response = collected(&[
        added(0, started("call_a", "search")),
        args_delta(0, r#"{"q":"a"}"#),
        added(1, started("call_b", "fetch")),
        args_delta(1, r#"{"q":"b"}"#),
        done(0, a.clone()),
        done(1, b.clone()),
        completed(serde_json::json!([a, b])),
    ])
    .await;
    let calls: Vec<(&str, &serde_json::Value)> = response
        .tool_calls
        .iter()
        .map(|c| (c.id.as_str(), &c.arguments))
        .collect();
    assert_eq!(
        calls,
        vec![
            ("call_a", &serde_json::json!({"q": "a"})),
            ("call_b", &serde_json::json!({"q": "b"})),
        ]
    );
}

/// Two calls carried only by the completion — on this wire the one frame
/// that holds more than one item — both survive, in order, each with its
/// own arguments.
#[tokio::test]
async fn two_calls_only_in_the_completion_both_survive() {
    let a = call_item("call_a", "search", r#"{"q":"a"}"#);
    let b = call_item("call_b", "fetch", r#"{"q":"b"}"#);
    let response = collected(&[completed(serde_json::json!([a, b]))]).await;
    let calls: Vec<(&str, &str, &serde_json::Value)> = response
        .tool_calls
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str(), &c.arguments))
        .collect();
    assert_eq!(
        calls,
        vec![
            ("call_a", "search", &serde_json::json!({"q": "a"})),
            ("call_b", "fetch", &serde_json::json!({"q": "b"})),
        ]
    );
    assert_eq!(response.finish_reason, FinishReason::ToolUse);
}

/// Content and a call in one frame: both survive.
#[tokio::test]
async fn content_and_a_call_in_one_frame_both_survive() {
    let call = call_item("call_1", "search", r#"{"q":"a"}"#);
    let response = collected(&[
        text_delta(0, "Looking."),
        done(0, message_item("Looking.")),
        done(1, call.clone()),
        completed(serde_json::json!([message_item("Looking."), call])),
    ])
    .await;
    assert_eq!(response.content, "Looking.");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"q": "a"})
    );
}

/// A call and the completion in one frame: the call, its usage and the
/// finish all survive.
#[tokio::test]
async fn a_call_and_the_completion_in_one_frame_both_survive() {
    let call = call_item("call_1", "search", r#"{"q":"a"}"#);
    let events = events_of(&[done(0, call.clone()), completed(serde_json::json!([call]))]).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::ToolUseStart { id, .. } if id == "call_1"))
    );
    assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(u) if u.input_tokens == 120 && u.output_tokens == 50 && u.cache_read_input_tokens == 100)));
    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            finish_reason: FinishReason::ToolUse
        })
    ));
}

/// A completion whose items no event announced still yields them: the
/// frame carries the whole response.
#[tokio::test]
async fn a_completion_alone_yields_the_items_it_carries() {
    let call = call_item("call_1", "search", r#"{"q":"a"}"#);
    let response = collected(&[completed(serde_json::json!([message_item("Hi."), call]))]).await;
    assert_eq!(response.content, "Hi.");
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(
        response.tool_calls[0].arguments,
        serde_json::json!({"q": "a"})
    );
}

/// A streamed reasoning turn is kept from `output_item.done`, whose
/// `encrypted_content` is whole — `added`'s "may be incomplete" (the
/// reference) — and its turn reaches the collected response.
#[tokio::test]
async fn a_streamed_reasoning_turn_is_kept_from_the_done_items() {
    let call = call_item("call_1", "search", r#"{"q":"a"}"#);
    let response = collected(&[
        added(0, reasoning_item("rs_1", Some("gAAAA-part"))),
        done(0, reasoning_item("rs_1", Some("gAAAA-whole"))),
        done(1, call.clone()),
        // The completion's copy differs, so only `output_item.done`'s item
        // passes the assertion below.
        completed(serde_json::json!([
            reasoning_item("rs_1", Some("gAAAA-from-completed")),
            call
        ])),
    ])
    .await;
    let echo = response.echo.expect("the turn is kept");
    assert_eq!(echo.wire(), EchoWire::OpenAiResponses);
    assert_eq!(echo.blocks()[0]["encrypted_content"], "gAAAA-whole");
    assert_eq!(echo.function_call_ids(), vec!["call_1"]);
}

/// Cut at `max_output_tokens`: `MaxTokens`, with the usage.
#[tokio::test]
async fn an_incomplete_stream_is_max_tokens() {
    let mut cut = answer(serde_json::json!([]));
    cut["status"] = serde_json::json!("incomplete");
    cut["incomplete_details"] = serde_json::json!({"reason": "max_output_tokens"});
    let response = collected(&[
        text_delta(0, "Partial"),
        serde_json::json!({"type": "response.incomplete", "response": cut}),
    ])
    .await;
    assert_eq!(response.content, "Partial");
    assert_eq!(response.finish_reason, FinishReason::MaxTokens);
    assert_eq!(response.usage.output_tokens, 50);
}

/// A failed response and an `error` event end the stream as an error.
#[tokio::test]
async fn a_failed_response_or_an_error_event_is_an_error() {
    for event in [
        serde_json::json!({"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "boom"}}}),
        serde_json::json!({"type": "error", "code": "server_error", "message": "boom", "param": null}),
    ] {
        let events = events_of(&[event]).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::Error { message } if message.contains("boom"))),
            "{events:?}"
        );
    }
}

/// Reasoning summary text streams as thinking.
#[tokio::test]
async fn reasoning_summary_text_streams_as_thinking() {
    let events = events_of(&[
        serde_json::json!({"type": "response.reasoning_summary_text.delta", "output_index": 0, "item_id": "rs_1", "summary_index": 0, "delta": "Weighing it."}),
        completed(serde_json::json!([])),
    ])
    .await;
    assert!(events.iter().any(
        |e| matches!(e, StreamEvent::ThinkingDelta { thinking } if thinking == "Weighing it.")
    ));
}

/// A streamed refusal is the turn's text as it streams, and once: the
/// completion's whole `refusal` part does not repeat the deltas.
#[tokio::test]
async fn a_streamed_refusal_is_the_answers_text() {
    let refusal = serde_json::json!({
        "type": "message",
        "id": "msg_1",
        "role": "assistant",
        "status": "completed",
        "content": [{"type": "refusal", "refusal": "I can't help with that."}]
    });
    let frames = [
        serde_json::json!({"type": "response.refusal.delta", "output_index": 0, "content_index": 0, "item_id": "msg_1", "delta": "I can't "}),
        serde_json::json!({"type": "response.refusal.delta", "output_index": 0, "content_index": 0, "item_id": "msg_1", "delta": "help with that."}),
        completed(serde_json::json!([refusal])),
    ];
    let texts: Vec<String> = events_of(&frames)
        .await
        .into_iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["I can't ", "help with that."]);
    assert_eq!(collected(&frames).await.content, "I can't help with that.");
}

// ── Over HTTP ───────────────────────────────────────────────────────────

/// The cloud arm pointed at a loopback stand-in for OpenAI's own API.
fn cloud_at(base: &str) -> OpenAiProvider {
    OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("gpt-6.1-sol".to_string()),
        Some(format!("{base}/v1")),
        None,
    )
    .with_responses_api(true)
}

/// A cloud request with tools goes to `/v1/responses` with
/// `max_output_tokens`, the tools and `store: false`, whole and streamed.
#[tokio::test]
async fn a_cloud_request_with_tools_goes_to_the_responses_api() {
    let call = call_item("call_1", "search", r#"{"q":"a"}"#);
    let whole = answer(serde_json::json!([call.clone()])).to_string();
    let frames = vec![
        frame(done(0, call.clone())),
        frame(completed(serde_json::json!([call]))),
    ];
    let server = MockHttpServer::start(move |req| match req.path.as_str() {
        "/v1/responses" if req.body.contains("\"stream\":true") => {
            MockResponse::sse(frames.clone())
        }
        "/v1/responses" => MockResponse::json(whole.clone()),
        _ => MockResponse::not_found(),
    })
    .await;
    let provider = cloud_at(&server.base_url);
    let req = request(vec![ChatMessage::user("go")], vec![closed_tool("search")]);

    let response = provider
        .chat_with_key("sk-test-not-a-key", req.clone())
        .await
        .expect("answers");
    assert_eq!(response.tool_calls[0].id, "call_1");

    let stream = provider
        .chat_streaming_with_key("sk-test-not-a-key", req)
        .await
        .expect("streams");
    let streamed = crate::streaming::collect_stream(stream, "gpt-6.1-sol".to_string())
        .await
        .expect("collects");
    assert_eq!(
        streamed.tool_calls[0].arguments,
        serde_json::json!({"q": "a"})
    );

    assert_eq!(server.hits("/v1/responses").await, 2);
    assert_eq!(server.hits("/v1/chat/completions").await, 0);
    for recorded in server.requests().await {
        let body: serde_json::Value = serde_json::from_str(&recorded.body).unwrap();
        assert_eq!(body["max_output_tokens"], 4096);
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"][0]["name"], "search");
        assert_eq!(body["tools"][0]["strict"], true);
        assert!(body.get("stream_options").is_none(), "{body}");
    }
}

/// A cloud request with no tools stays on Chat Completions: the narrowest
/// move the pages ask for.
#[tokio::test]
async fn a_cloud_request_without_tools_stays_on_chat_completions() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/chat/completions" => MockResponse::json(
            serde_json::json!({
                "model": "gpt-6.1-sol",
                "choices": [{"message": {"role": "assistant", "content": "pong"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })
            .to_string(),
        ),
        _ => MockResponse::not_found(),
    })
    .await;
    let response = cloud_at(&server.base_url)
        .chat_with_key(
            "sk-test-not-a-key",
            request(vec![ChatMessage::user("ping")], vec![]),
        )
        .await
        .expect("answers");
    assert_eq!(response.content, "pong");
    assert_eq!(server.hits("/v1/responses").await, 0);
}

/// A tool request as both characterizations send it.
fn tool_request() -> ChatRequest {
    let called = ChatResponse {
        content: "Looking.".to_string(),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "a"}),
        }],
        model: "local".to_string(),
        usage: Usage::default(),
        finish_reason: FinishReason::ToolUse,
        thinking: None,
        parts: None,
        echo: None,
    };
    let mut req = request(
        vec![
            ChatMessage::system("Be brief."),
            ChatMessage::user("go"),
            ChatMessage::assistant_with_tools(&called),
            ChatMessage::tool_result("call_1", "found"),
        ],
        vec![closed_tool("search"), open_tool("fetch")],
    );
    req.tool_choice = Some(ToolChoice::Auto);
    req
}

fn chat_answer() -> MockResponse {
    MockResponse::json(
        serde_json::json!({
            "model": "local",
            "choices": [{"message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })
        .to_string(),
    )
}

/// The body the parent sent for [`tool_request`] to an OpenAI-compatible
/// server, byte for byte (captured on `9d17a178` with C7b's inert
/// seam, which left the Chat Completions path as it was).
const COMPATIBLE_BODY: &str = r#"{"max_tokens":4096,"messages":[{"content":"Be brief.","role":"system"},{"content":"go","role":"user"},{"content":"Looking.","role":"assistant","tool_calls":[{"function":{"arguments":"{\"q\":\"a\"}","name":"search"},"id":"call_1","type":"function"}]},{"content":"found","role":"tool","tool_call_id":"call_1"}],"model":"local","tool_choice":"auto","tools":[{"function":{"description":"search things","name":"search","parameters":{"additionalProperties":false,"properties":{"q":{"type":"string"}},"required":["q"],"type":"object"},"strict":true},"type":"function"},{"function":{"description":"fetch things","name":"fetch","parameters":{"properties":{"n":{"type":"integer"},"q":{"type":"string"}},"required":["q"],"type":"object"}},"type":"function"}]}"#;

/// The body the parent sent for [`tool_request`] to Ollama, byte for byte
/// (captured on `9d17a178` with C7b's inert seam, which left the Chat
/// Completions path as it was).
const OLLAMA_BODY: &str = r#"{"max_tokens":4096,"messages":[{"content":"Be brief.","role":"system"},{"content":"go","role":"user"},{"content":"Looking.","role":"assistant","tool_calls":[{"function":{"arguments":"{\"q\":\"a\"}","name":"search"},"id":"call_1","type":"function"}]},{"content":"found","role":"tool","tool_call_id":"call_1"}],"model":"local","tool_choice":"auto","tools":[{"function":{"description":"search things","name":"search","parameters":{"additionalProperties":false,"properties":{"q":{"type":"string"}},"required":["q"],"type":"object"},"strict":true},"type":"function"},{"function":{"description":"fetch things","name":"fetch","parameters":{"properties":{"n":{"type":"integer"},"q":{"type":"string"}},"required":["q"],"type":"object"}},"type":"function"}]}"#;

/// An OpenAI-compatible server that is not OpenAI's own API keeps Chat
/// Completions, the body byte for byte as before (characterization).
#[tokio::test]
async fn a_compatible_server_keeps_chat_completions_byte_for_byte() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/chat/completions" => chat_answer(),
        _ => MockResponse::not_found(),
    })
    .await;
    let provider = OpenAiProvider::with_client(
        loopback_client(TIMEOUT),
        "sk-test-not-a-key".to_string(),
        Some("local".to_string()),
        Some(format!("{}/v1", server.base_url)),
        None,
    );
    provider
        .chat_with_key("sk-test-not-a-key", tool_request())
        .await
        .expect("answers");
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/chat/completions");
    assert_eq!(requests[0].body, COMPATIBLE_BODY);
}

/// Ollama keeps Chat Completions, the body byte for byte as before
/// (characterization).
#[tokio::test]
async fn ollama_keeps_chat_completions_byte_for_byte() {
    let server = MockHttpServer::start(|req| match req.path.as_str() {
        "/v1/chat/completions" => chat_answer(),
        _ => MockResponse::not_found(),
    })
    .await;
    let provider = crate::providers::ollama::OllamaProvider::new(
        "local".to_string(),
        Some(format!("{}/v1", server.base_url)),
        None,
    );
    provider
        .chat_with_key("", tool_request())
        .await
        .expect("answers");
    let requests = server.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/chat/completions");
    assert_eq!(requests[0].body, OLLAMA_BODY);
}
