//! How a tool call's arguments are read from what a provider sent
//! (N-TOOL-5, audit Z1-14).
//!
//! Two sites used to turn arguments they could not parse into `{}`, silently
//! — the stream collector and the OpenAI-compatible response parser — so a
//! call the model garbled ran with no arguments, or failed on "missing
//! required parameter" with no word of why. Both now read through
//! [`read_tool_arguments`], and what cannot be read is said once in the log
//! and carried to the tool registry, which refuses the call with a sentence
//! the model can act on.

use serde_json::{Map, Value};

/// Arguments a provider sent for a tool call that could not be read as a JSON
/// object: not valid JSON, or valid JSON of another kind.
///
/// The call itself is kept — dropped, its `tool_result` would have nothing to
/// answer and the next request would be refused — but it never runs with
/// `{}`. Its arguments become [`Self::to_arguments`], one object under
/// [`Self::KEY`] that every provider accepts back in history, and the tool
/// registry refuses any call whose arguments are exactly that object
/// ([`Self::of`]), naming [`Self::problem`] and [`Self::bytes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableArguments {
    /// The length of what the provider sent, in bytes.
    pub bytes: usize,
    /// What was wrong with it, in words that quote none of it: "not valid
    /// JSON (EOF while parsing a string at line 1 column 24)", "a JSON array,
    /// not an object".
    pub problem: String,
}

impl UnreadableArguments {
    /// The one key of the object that stands in for unreadable arguments.
    pub const KEY: &'static str = "__openalpaca_unreadable_arguments";

    /// The arguments a call carries in place of the ones that could not be
    /// read.
    pub fn to_arguments(&self) -> Value {
        serde_json::json!({ Self::KEY: { "bytes": self.bytes, "problem": self.problem } })
    }

    /// The unreadable arguments these stand for, when they are exactly the
    /// object [`Self::to_arguments`] makes — and nothing else.
    pub fn of(arguments: &Value) -> Option<Self> {
        let object = arguments.as_object().filter(|object| object.len() == 1)?;
        let inner = object.get(Self::KEY)?;
        Some(Self {
            bytes: usize::try_from(inner.get("bytes")?.as_u64()?).ok()?,
            problem: inner.get("problem")?.as_str()?.to_string(),
        })
    }
}

/// Read a tool call's arguments text — the one reading both sites use.
///
/// * Nothing, whitespace or `null`: no arguments, `{}`, and nothing is
///   logged.
/// * A JSON object: itself.
/// * A JSON string whose text is a JSON object — arguments some models encode
///   twice: that object.
/// * Anything else — text that is not JSON, or JSON of another kind — is
///   [`UnreadableArguments`]: logged once at `warn` with the tool's name, the
///   byte length and what was wrong, never the text itself, and carried as
///   [`UnreadableArguments::to_arguments`] so the call is refused, not run.
///
/// One leading byte-order mark is dropped first. Nothing is repaired: a cut
/// answer is not closed, a stray comma is not removed.
pub(crate) fn read_tool_arguments(tool: &str, text: &str) -> Value {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.trim().is_empty() {
        return Value::Object(Map::new());
    }
    let read = match serde_json::from_str::<Value>(text) {
        Ok(value) => as_arguments(value),
        Err(error) => Err(format!("not valid JSON ({})", plain(&error.to_string()))),
    };
    settle(tool, text.len(), read)
}

/// The parser's message with its backticks as plain quotes ("expected `:`" →
/// "expected ':'") and any control character as a space. The stand-in
/// reaches the sandbox before the registry refuses it, and the sanitizer
/// refuses a backtick or a line break in any argument of a shell tool as an
/// injection — which would hide the real problem behind that refusal.
/// serde's syntax messages quote no part of the text that failed to parse.
fn plain(message: &str) -> String {
    message
        .chars()
        .map(|c| match c {
            '`' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect()
}

/// Read the `arguments` field of an OpenAI-compatible tool call as the wire
/// carried it: normally a string of JSON, which [`read_tool_arguments`] reads;
/// some servers send the object itself, which is kept; absent or `null` is no
/// arguments. Only the OpenAI-compatible parser reads a field.
#[cfg(any(feature = "openai", test))]
pub(crate) fn read_tool_arguments_field(tool: &str, field: &Value) -> Value {
    match field {
        Value::String(text) => read_tool_arguments(tool, text),
        other => settle(tool, other.to_string().len(), as_arguments(other.clone())),
    }
}

/// A parsed value as a call's arguments, or what is wrong with it.
fn as_arguments(value: Value) -> Result<Value, String> {
    match value {
        Value::Null => Ok(Value::Object(Map::new())),
        Value::Object(_) => Ok(value),
        Value::String(text) => match serde_json::from_str::<Value>(&text) {
            Ok(object @ Value::Object(_)) => Ok(object),
            _ => Err("a JSON string that does not hold a JSON object".to_string()),
        },
        Value::Array(_) => Err("a JSON array, not an object".to_string()),
        Value::Number(_) => Err("a JSON number, not an object".to_string()),
        Value::Bool(_) => Err("a JSON boolean, not an object".to_string()),
    }
}

/// The arguments, or the stand-in for unreadable ones with its one warning.
fn settle(tool: &str, bytes: usize, read: Result<Value, String>) -> Value {
    match read {
        Ok(arguments) => arguments,
        Err(problem) => {
            tracing::warn!(
                tool,
                bytes,
                problem = %problem,
                "a tool call's arguments could not be read as a JSON object; \
                 the call is refused, not run with empty arguments"
            );
            UnreadableArguments { bytes, problem }.to_arguments()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_object_is_itself_and_a_string_holding_one_is_that_object() {
        assert_eq!(read_tool_arguments("t", r#"{"a":1}"#), json!({"a": 1}));
        assert_eq!(read_tool_arguments("t", r#""{\"a\":1}""#), json!({"a": 1}));
        assert_eq!(
            read_tool_arguments("t", "\u{feff}{\"a\":1}"),
            json!({"a": 1})
        );
    }

    /// serde writes "expected `:`"; a backtick in a shell tool's argument is
    /// refused by the sanitizer as command substitution, so the problem says
    /// it with plain quotes.
    #[test]
    fn the_problem_holds_no_backtick_or_control_character() {
        let unreadable = UnreadableArguments::of(&read_tool_arguments("t", r#"{"command" "ls"}"#))
            .expect("the stand-in");
        assert!(
            unreadable.problem.contains("expected ':'"),
            "{}",
            unreadable.problem
        );
        assert!(!unreadable.problem.contains('`'), "{}", unreadable.problem);
        assert!(!unreadable.problem.chars().any(char::is_control));
        assert_eq!(plain("a`b\nc\u{0}d"), "a'b c d");
    }

    #[test]
    fn nothing_is_no_arguments() {
        for text in ["", "   ", "null", "\u{feff}"] {
            assert_eq!(read_tool_arguments("t", text), json!({}), "{text:?}");
        }
        assert_eq!(read_tool_arguments_field("t", &Value::Null), json!({}));
    }

    #[test]
    fn what_cannot_be_read_stands_in_as_its_length_and_problem() {
        let cut = r#"{"a":"#;
        let arguments = read_tool_arguments("t", cut);
        let unreadable = UnreadableArguments::of(&arguments).expect("the stand-in");
        assert_eq!(unreadable.bytes, cut.len());
        assert!(
            unreadable.problem.starts_with("not valid JSON ("),
            "{}",
            unreadable.problem
        );
        assert!(
            !unreadable.problem.contains("\"a\""),
            "the text is never quoted"
        );

        for (text, problem) in [
            ("[1,2]", "a JSON array, not an object"),
            ("42", "a JSON number, not an object"),
            ("true", "a JSON boolean, not an object"),
            (
                r#""plain words""#,
                "a JSON string that does not hold a JSON object",
            ),
            (r#""[1]""#, "a JSON string that does not hold a JSON object"),
        ] {
            let unreadable =
                UnreadableArguments::of(&read_tool_arguments("t", text)).expect("the stand-in");
            assert_eq!(unreadable.problem, problem, "{text}");
            assert_eq!(unreadable.bytes, text.len(), "{text}");
        }
    }

    #[test]
    fn a_field_sent_as_an_object_is_kept_and_any_other_kind_stands_in() {
        assert_eq!(
            read_tool_arguments_field("t", &json!({"a": 1})),
            json!({"a": 1})
        );
        assert_eq!(
            read_tool_arguments_field("t", &json!(r#"{"a":1}"#)),
            json!({"a": 1})
        );
        let unreadable = UnreadableArguments::of(&read_tool_arguments_field("t", &json!([1])))
            .expect("the stand-in");
        assert_eq!(unreadable.problem, "a JSON array, not an object");
        assert_eq!(unreadable.bytes, 3);
    }

    #[test]
    fn only_the_stand_in_itself_reads_as_unreadable() {
        let unreadable = UnreadableArguments {
            bytes: 7,
            problem: "a JSON array, not an object".to_string(),
        };
        assert_eq!(
            UnreadableArguments::of(&unreadable.to_arguments()),
            Some(unreadable.clone())
        );

        let mut with_more = unreadable.to_arguments();
        with_more["path"] = json!("a");
        assert_eq!(
            UnreadableArguments::of(&with_more),
            None,
            "another key: not ours"
        );
        assert_eq!(
            UnreadableArguments::of(&json!({UnreadableArguments::KEY: "x"})),
            None
        );
        assert_eq!(UnreadableArguments::of(&json!({"path": "a"})), None);
    }
}
