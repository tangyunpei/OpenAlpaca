//! A call's arguments before its backend runs (N-TOOL-5, audit Z1-14): every
//! problem is reported in one result, arguments a provider sent that could not
//! be read never run a tool, and a JSON string is refused rather than read as
//! the object it holds — the sanitizer has already passed its outer text.

use super::*;
use std::sync::Mutex;

/// A builtin that keeps the arguments of every run; `reaches_a_program`
/// answers [`BuiltInTool::arguments_reach_a_program`], so the sanitizer's
/// `../` rule binds it as it binds a skill script.
#[derive(Default)]
struct Kept {
    runs: Mutex<Vec<serde_json::Value>>,
    reaches_a_program: bool,
}

#[async_trait]
impl BuiltInTool for Kept {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.runs.lock().unwrap().push(arguments.clone());
        Ok("ran".to_string())
    }

    fn arguments_reach_a_program(&self) -> bool {
        self.reaches_a_program
    }
}

fn registry_with(name: &str, parameters: serde_json::Value, tool: Arc<Kept>) -> ToolRegistry {
    let registry = ToolRegistry::default();
    register(&registry, name, parameters, tool);
    registry
}

fn register(registry: &ToolRegistry, name: &str, parameters: serde_json::Value, tool: Arc<Kept>) {
    registry
        .register(RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: name.to_string(),
                parameters,
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::BuiltIn(tool),
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "test".into(),
            author: "test".into(),
            created_at: chrono::Utc::now(),
        })
        .unwrap();
}

fn search_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": {"type": "string"},
            "scope": {"type": "string"},
            "limit": {"type": "integer"},
            "exact": {"type": "boolean"},
            "mode": {"type": "string", "enum": ["fast", "deep"]}
        },
        "required": ["query", "scope"]
    })
}

/// The gate's third test: a call with two schema violations hears both.
#[tokio::test]
async fn a_call_with_two_problems_is_told_both() {
    let tool = Arc::new(Kept::default());
    let registry = registry_with("search", search_schema(), tool.clone());

    let err = registry
        .execute_with_context(
            "search",
            &serde_json::json!({"scope": "notes", "limit": "ten"}),
            &ToolContext::default(),
        )
        .await
        .expect_err("refused");

    assert!(err.contains("missing required parameter 'query'"), "{err}");
    assert!(
        err.contains("parameter 'limit' should be integer, got string"),
        "{err}"
    );
    assert!(tool.runs.lock().unwrap().is_empty());
}

/// Every missing parameter, then every wrong type or value, in one result —
/// and a single problem keeps the sentence it always had.
#[tokio::test]
async fn every_problem_is_named_in_one_result() {
    let tool = Arc::new(Kept::default());
    let registry = registry_with("search", search_schema(), tool.clone());
    let ctx = ToolContext::default();

    let err = registry
        .execute_with_context(
            "search",
            &serde_json::json!({"exact": "yes", "limit": 2.5, "mode": "slow"}),
            &ctx,
        )
        .await
        .expect_err("refused");
    assert_eq!(
        err,
        "Tool 'search': the arguments have 5 problems: \
         missing required parameter 'query'; \
         missing required parameter 'scope'; \
         parameter 'exact' should be boolean, got string; \
         parameter 'limit' should be integer, got number; \
         parameter 'mode' value \"slow\" not in allowed values"
    );

    let one = registry
        .execute_with_context("search", &serde_json::json!({"query": "q"}), &ctx)
        .await
        .expect_err("refused");
    assert_eq!(one, "Tool 'search': missing required parameter 'scope'");
    assert!(tool.runs.lock().unwrap().is_empty());
}

/// A JSON string is refused at dispatch, as it always was, and not read as
/// the object its text holds — by dispatch or by the precheck before a card.
/// The model's strings are read upstream, where a provider's answer is parsed;
/// anything else reaches the registry after the sandbox's sanitizer has
/// checked only the string's outer text.
#[tokio::test]
async fn a_json_string_is_refused_not_read_as_the_object_it_holds() {
    let tool = Arc::new(Kept::default());
    let registry = registry_with("search", search_schema(), tool.clone());
    let ctx = ToolContext::default();
    let text = serde_json::json!(r#"{"query":"q","scope":"notes"}"#);

    let err = registry
        .execute_with_context("search", &text, &ctx)
        .await
        .expect_err("a string root is refused");

    assert_eq!(
        err,
        "Tool 'search': arguments must be a JSON object, got string"
    );
    assert_eq!(registry.precheck("search", &text, Some(&ctx)), Err(err));
    assert!(tool.runs.lock().unwrap().is_empty());
}

fn sandbox_policy(confirm: &[&str], auto_approve: bool) -> crate::security::sandbox::SandboxPolicy {
    crate::security::sandbox::SandboxPolicy {
        agent_id: "agent-1".to_string(),
        allowed_capabilities: crate::security::capabilities::Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: confirm.iter().map(|t| t.to_string()).collect(),
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve,
        unattended: false,
    }
}

/// Found by review: what the sanitizer refuses in an object — a backtick and
/// a newline in a shell command, `../` in an argument that reaches a program,
/// a NUL anywhere — must not reach the tool sent as a JSON string, whose outer
/// text carries them only as escapes (`\u0060`, `\n`, `.\u002e/`,
/// `\u0000`) the sanitizer cannot see. Through the sandbox, with no card to
/// stop it and under `auto_approve` on the confirm list.
#[tokio::test]
async fn a_json_string_cannot_carry_past_the_sanitizer_what_it_refuses_in_an_object() {
    use crate::security::sandbox::SandboxManager;

    let shell = Arc::new(Kept::default());
    let script = Arc::new(Kept {
        reaches_a_program: true,
        ..Kept::default()
    });
    let note = Arc::new(Kept::default());
    let registry = ToolRegistry::default();
    let one_string = |name: &str| {
        serde_json::json!({
            "type": "object",
            "properties": {name: {"type": "string"}},
            "required": [name]
        })
    };
    register(
        &registry,
        "shell_execute",
        one_string("command"),
        shell.clone(),
    );
    register(&registry, "run_script", one_string("path"), script.clone());
    register(&registry, "write_note", one_string("note"), note.clone());
    let sandbox =
        SandboxManager::with_defaults(Arc::new(registry), crate::bus::EventBus::default());
    let ctx = ToolContext::default();

    let cases = [
        (
            "shell_execute",
            r#"{"command":"echo \u0060id\u0060\nrm x"}"#,
            "Potential command injection: backtick command substitution",
            sandbox_policy(&[], false),
        ),
        (
            "shell_execute",
            r#"{"command":"echo \u0060id\u0060\nrm x"}"#,
            "Potential command injection: backtick command substitution",
            sandbox_policy(&["shell_execute"], true),
        ),
        (
            "run_script",
            r#"{"path":".\u002e/.\u002e/etc/passwd"}"#,
            "Path traversal pattern detected",
            sandbox_policy(&[], false),
        ),
        (
            "write_note",
            r#"{"note":"a\u0000b"}"#,
            "Argument contains null bytes",
            sandbox_policy(&[], false),
        ),
    ];
    let mut refused_as_strings = Vec::new();
    for (tool, text, sanitizer_says, policy) in cases {
        for hidden in ["`", "\n", "../", "\0"] {
            assert!(
                !text.contains(hidden),
                "{tool}: the outer text hides {hidden:?}"
            );
        }
        let object: serde_json::Value = serde_json::from_str(text).expect("an object");
        let call = |arguments| openalpaca_llm::ToolCall {
            id: "call_1".to_string(),
            name: tool.to_string(),
            arguments,
        };

        let as_object = sandbox
            .execute_tool(&call(object), &policy, &ctx)
            .await
            .expect_err("the sanitizer refuses the object");
        assert!(as_object.contains(sanitizer_says), "{tool}: {as_object}");

        let as_string = sandbox
            .execute_tool(&call(serde_json::json!(text)), &policy, &ctx)
            .await;
        refused_as_strings.push((tool, as_string));
    }

    let expected: Vec<_> = ["shell_execute", "shell_execute", "run_script", "write_note"]
        .into_iter()
        .map(|tool| {
            (
                tool,
                Err(format!(
                    "Tool '{tool}': arguments must be a JSON object, got string"
                )),
            )
        })
        .collect();
    assert_eq!(refused_as_strings, expected);
    let runs = |tool: &Kept| tool.runs.lock().unwrap().clone();
    assert_eq!(
        [runs(&shell), runs(&script), runs(&note)],
        [Vec::<serde_json::Value>::new(), vec![], vec![]],
        "no tool ran"
    );
}

/// The call the loop gets when a provider streams `text` as `tool`'s
/// arguments — read by the provider layer itself.
async fn streamed_call(tool: &str, text: &str) -> openalpaca_llm::ToolCall {
    use futures_util::stream;
    use openalpaca_llm::StreamEvent;

    let events: Vec<Result<StreamEvent, openalpaca_llm::LlmError>> = vec![
        Ok(StreamEvent::ToolUseStart {
            index: 0,
            id: "call_1".to_string(),
            name: tool.to_string(),
        }),
        Ok(StreamEvent::InputJsonDelta {
            index: 0,
            partial_json: text.to_string(),
        }),
    ];
    let mut response =
        openalpaca_llm::collect_stream(Box::pin(stream::iter(events)), "m".to_string())
            .await
            .expect("collects");
    response.tool_calls.remove(0)
}

/// Arguments a provider streamed that do not parse are never run as `{}` —
/// not even by a tool whose schema `{}` satisfies — and the refusal names the
/// problem and the length; the precheck before a card gives the same refusal.
#[tokio::test]
async fn arguments_that_could_not_be_read_never_run_the_tool() {
    let tool = Arc::new(Kept::default());
    let registry = registry_with("ping", serde_json::json!({"type": "object"}), tool.clone());
    let cut = r#"{"target":"#;
    let call = &streamed_call("ping", cut).await;
    let ctx = ToolContext::default();

    let err = registry
        .execute_with_context("ping", &call.arguments, &ctx)
        .await
        .expect_err("a call whose arguments could not be read does not run");

    assert!(tool.runs.lock().unwrap().is_empty(), "the tool never ran");
    assert!(err.starts_with("Tool 'ping': "), "{err}");
    assert!(err.contains("could not be read"), "{err}");
    assert!(err.contains("not valid JSON"), "{err}");
    assert!(err.contains(&format!("{} bytes", cut.len())), "{err}");
    assert!(err.contains("did not run"), "{err}");
    assert_eq!(
        registry.precheck("ping", &call.arguments, Some(&ctx)),
        Err(err)
    );
}

/// Through the sandbox, a shell tool's unreadable arguments are refused for
/// what they are: the stand-in passes the sanitizer, whose shell rules refuse
/// a backtick — and serde says "expected `:`" — so the model hears why its
/// call did not run, not a command-injection refusal.
#[tokio::test]
async fn a_shell_call_whose_arguments_could_not_be_read_hears_why() {
    use crate::security::capabilities::Allowlist;
    use crate::security::sandbox::{SandboxManager, SandboxPolicy};

    let tool = Arc::new(Kept::default());
    let registry = registry_with(
        "shell_execute",
        serde_json::json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }),
        tool.clone(),
    );
    let sandbox =
        SandboxManager::with_defaults(Arc::new(registry), crate::bus::EventBus::default());
    let policy = SandboxPolicy {
        agent_id: "agent-1".to_string(),
        allowed_capabilities: Allowlist::Unrestricted,
        denied_capabilities: vec![],
        require_confirmation_for: vec![],
        max_tool_calls: None,
        max_tool_runtime_secs: 60,
        stream_id: None,
        lane_key: None,
        confirmation_timeout_secs: None,
        auto_approve: false,
        unattended: false,
    };
    let call = streamed_call("shell_execute", r#"{"command" "ls"}"#).await;

    let err = sandbox
        .execute_tool(&call, &policy, &ToolContext::default())
        .await
        .expect_err("refused");

    assert!(err.contains("could not be read"), "{err}");
    assert!(err.contains("expected ':'"), "{err}");
    assert!(!err.contains("injection"), "{err}");
    assert!(tool.runs.lock().unwrap().is_empty());
}
