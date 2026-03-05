use crate::error::LlmError;
use crate::types::*;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;

/// Maximum time to wait between SSE chunks before treating the stream as stalled.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Wraps a `ChatStream` and holds a concurrency semaphore permit.
/// The permit is released when the stream is fully consumed or dropped,
/// ensuring the concurrency limiter accurately tracks in-flight API calls.
pub(crate) struct PermitStream {
    inner: ChatStream,
    _permit: OwnedSemaphorePermit,
}

// SAFETY: PermitStream's fields are both Unpin (ChatStream is Box<Pin<…>>,
// OwnedSemaphorePermit is Unpin). Explicit impl guards against future field
// additions that might not be Unpin, since poll_next uses get_mut().
impl Unpin for PermitStream {}

impl futures_util::Stream for PermitStream {
    type Item = Result<StreamEvent, LlmError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}

impl PermitStream {
    pub(crate) fn new(inner: ChatStream, permit: OwnedSemaphorePermit) -> Self {
        Self { inner, _permit: permit }
    }
}

/// Collect a streaming response into a complete ChatResponse.
pub async fn collect_stream(
    mut stream: ChatStream,
    model: String,
) -> Result<ChatResponse, LlmError> {
    let mut content = String::new();
    let mut thinking = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut partial_json: HashMap<usize, String> = HashMap::new();
    let mut tool_meta: HashMap<usize, (String, String)> = HashMap::new();
    let mut usage = Usage::default();
    let mut finish_reason = FinishReason::Stop;

    while let Some(event) = tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| LlmError::Stream("SSE stream idle timeout (90s)".into()))?
    {
        match event? {
            StreamEvent::TextDelta { text } => content.push_str(&text),
            StreamEvent::ThinkingDelta { thinking: t } => {
                thinking.get_or_insert_with(String::new).push_str(&t);
            }
            StreamEvent::ToolUseStart { index, id, name } => {
                tool_meta.insert(index, (id, name));
                partial_json.entry(index).or_default();
            }
            StreamEvent::InputJsonDelta {
                index,
                partial_json: pj,
            } => {
                partial_json.entry(index).or_default().push_str(&pj);
            }
            StreamEvent::Usage(u) => {
                usage.input_tokens += u.input_tokens;
                usage.output_tokens += u.output_tokens;
                usage.cache_creation_input_tokens += u.cache_creation_input_tokens;
                usage.cache_read_input_tokens += u.cache_read_input_tokens;
            }
            StreamEvent::Done { finish_reason: fr } => finish_reason = fr,
            StreamEvent::Error { message } => {
                return Err(LlmError::Stream(message));
            }
        }
    }

    // Assemble tool calls from accumulated JSON
    let mut indices: Vec<usize> = partial_json.keys().copied().collect();
    indices.sort();
    for index in indices {
        if let Some((id, name)) = tool_meta.remove(&index) {
            let json_str = partial_json.remove(&index).unwrap_or_default();
            let arguments: serde_json::Value = serde_json::from_str(&json_str)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        model,
        usage,
        finish_reason,
        thinking,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collect_stream_text_only() {
        let events = vec![
            Ok(StreamEvent::TextDelta {
                text: "Hello ".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "world!".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            })),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.content, "Hello world!");
        assert!(response.tool_calls.is_empty());
        assert!(response.thinking.is_none());
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.usage.output_tokens, 5);
        assert_eq!(response.finish_reason, FinishReason::Stop);
    }

    #[tokio::test]
    async fn test_collect_stream_with_tools() {
        let events = vec![
            Ok(StreamEvent::ToolUseStart {
                index: 0,
                id: "tc1".to_string(),
                name: "search".to_string(),
            }),
            Ok(StreamEvent::InputJsonDelta {
                index: 0,
                partial_json: r#"{"query":"#.to_string(),
            }),
            Ok(StreamEvent::InputJsonDelta {
                index: 0,
                partial_json: r#""rust"}"#.to_string(),
            }),
            Ok(StreamEvent::Usage(Usage::default())),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::ToolUse,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].name, "search");
        assert_eq!(response.tool_calls[0].arguments["query"], "rust");
        assert_eq!(response.finish_reason, FinishReason::ToolUse);
    }

    #[tokio::test]
    async fn test_collect_stream_with_thinking() {
        let events = vec![
            Ok(StreamEvent::ThinkingDelta {
                thinking: "Let me think...".to_string(),
            }),
            Ok(StreamEvent::TextDelta {
                text: "Answer.".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage::default())),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.thinking.as_deref(), Some("Let me think..."));
        assert_eq!(response.content, "Answer.");
    }

    #[tokio::test]
    async fn test_collect_stream_error() {
        let events: Vec<Result<StreamEvent, LlmError>> = vec![
            Ok(StreamEvent::TextDelta {
                text: "partial".to_string(),
            }),
            Ok(StreamEvent::Error {
                message: "connection reset".to_string(),
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let result = collect_stream(stream, "test-model".to_string()).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LlmError::Stream(ref s) if s.contains("connection reset")));
    }

    #[tokio::test(start_paused = true)]
    async fn test_collect_stream_idle_timeout() {
        // A stream that never yields — with start_paused, tokio auto-advances
        // time when all tasks are blocked, so the 90s timeout fires instantly.
        let stream: ChatStream = Box::pin(futures_util::stream::pending());
        let result = collect_stream(stream, "mock".into()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("idle timeout"));
    }

    /// M3 test: A slow stream that sends chunks within the idle timeout (90s)
    /// but exceeds a wall-clock deadline should be cancelled by the outer timeout.
    /// This mirrors the `tokio::time::timeout(max_stream_duration, collect_stream(...))`
    /// pattern used in `LlmBackend::complete()`.
    #[tokio::test(start_paused = true)]
    async fn test_wall_clock_timeout_cancels_slow_stream() {
        use std::time::Duration;

        // Stream that emits a TextDelta every 30s — well within the 90s idle
        // timeout, so collect_stream itself would never time out.
        let slow_stream: ChatStream = Box::pin(futures_util::stream::unfold(0u32, |count| async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Some((
                Ok(StreamEvent::TextDelta { text: ".".to_string() }),
                count + 1,
            ))
        }));

        // Apply a wall-clock deadline of 2 minutes (shorter than "infinite" but
        // longer than idle timeout). The stream keeps sending so idle timeout
        // never fires, but the wall-clock timeout should cancel it.
        let wall_clock = Duration::from_secs(120);
        let result =
            tokio::time::timeout(wall_clock, collect_stream(slow_stream, "m".into())).await;

        // The outer timeout should fire (Err = elapsed), proving wall-clock
        // cancellation works even when the stream is actively producing chunks.
        assert!(result.is_err(), "Expected wall-clock timeout to fire");
    }

    /// M4 test: PermitStream holds the semaphore permit during stream collection
    /// and releases it when the stream is dropped/consumed.
    #[tokio::test]
    async fn test_permit_stream_holds_and_releases_permit() {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(1));

        // Acquire an owned permit and wrap a stream with it.
        let permit = sem.clone().acquire_owned().await.unwrap();
        assert_eq!(sem.available_permits(), 0, "permit should be held");

        let events: Vec<Result<StreamEvent, LlmError>> = vec![
            Ok(StreamEvent::TextDelta {
                text: "hi".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage::default())),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let inner: ChatStream = Box::pin(futures_util::stream::iter(events));
        let permit_stream: ChatStream = Box::pin(PermitStream::new(inner, permit));

        // Permit is still held during collection.
        assert_eq!(sem.available_permits(), 0);

        let response = collect_stream(permit_stream, "m".into()).await.unwrap();
        assert_eq!(response.content, "hi");

        // After the stream is fully consumed and dropped, the permit is released.
        assert_eq!(sem.available_permits(), 1, "permit should be released after drop");
    }

    #[tokio::test]
    async fn test_collect_stream_usage_accumulates() {
        let events = vec![
            Ok(StreamEvent::Usage(Usage {
                input_tokens: 100,
                output_tokens: 0,
                cache_creation_input_tokens: 50,
                cache_read_input_tokens: 20,
            })),
            Ok(StreamEvent::TextDelta {
                text: "Hello".to_string(),
            }),
            Ok(StreamEvent::Usage(Usage {
                input_tokens: 0,
                output_tokens: 30,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            })),
            Ok(StreamEvent::Done {
                finish_reason: FinishReason::Stop,
            }),
        ];
        let stream = Box::pin(futures_util::stream::iter(events));
        let response = collect_stream(stream, "test-model".to_string())
            .await
            .unwrap();
        assert_eq!(response.usage.input_tokens, 100);
        assert_eq!(response.usage.output_tokens, 30);
        assert_eq!(response.usage.cache_creation_input_tokens, 50);
        assert_eq!(response.usage.cache_read_input_tokens, 20);
    }
}
