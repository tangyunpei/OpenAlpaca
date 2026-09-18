//! The live half of one chat turn (S1).
//!
//! A turn's answer used to reach the client only after the whole turn was
//! done: the chat service took the finished string and re-cut it into word
//! chunks with a sleep between them, which looked like streaming and was not.
//! This is the seam that makes it real — the turn carries a sink, and the
//! agentic loop hands it every text delta the provider produces, as it
//! arrives.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where a turn's model output goes **while the provider is still producing
/// it**.
///
/// The chat service's [`StreamSink`](super::StreamSink) is the only
/// implementation in production. Both methods are called from inside the
/// provider's stream, once per delta, so neither may block: a broadcast
/// `send` is the shape they are built for.
pub trait TurnSink: Send + Sync {
    /// One text delta, verbatim as the provider produced it.
    ///
    /// Never the authoritative transcript — the turn's `done` carries that —
    /// so a dropped delta costs a flicker, not the answer.
    fn text_delta(&self, text: &str);

    /// One reasoning delta (S2).
    ///
    /// Surfaced and then forgotten: reasoning is not part of the turn's
    /// content, is never persisted, and never reaches the model's own history
    /// — the client shows it while the model thinks and drops it after.
    fn reasoning_delta(&self, text: &str);
}

/// A cloneable handle to a [`TurnSink`] that remembers whether any text ever
/// reached it.
///
/// It rides on [`GatewayRequest`](crate::gateway::GatewayRequest) and
/// [`HandleRequest`](crate::gateway::HandleRequest) down to the main loop's
/// `LoopConfig.stream_callback`, the same way `unattended` rides down to the
/// sandbox policy. Afterwards the creator asks [`saw_text`](Self::saw_text):
/// a turn that streamed nothing — a deterministic tier, a provider with no
/// streaming, a failed stream that fell back — still owes the client its one
/// delta, and a turn that streamed must not repeat itself.
#[derive(Clone)]
pub struct TurnSinkHandle {
    sink: Arc<dyn TurnSink>,
    saw_text: Arc<AtomicBool>,
}

impl TurnSinkHandle {
    pub fn new(sink: Arc<dyn TurnSink>) -> Self {
        Self {
            sink,
            saw_text: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Forward one text delta and remember that the client has seen text.
    pub fn text_delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.saw_text.store(true, Ordering::Relaxed);
        self.sink.text_delta(text);
    }

    /// Forward one reasoning delta. It does not count as text: a turn whose
    /// whole output was reasoning still owes the client its answer.
    pub fn reasoning_delta(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.sink.reasoning_delta(text);
    }

    /// Whether any non-empty text delta was forwarded through this handle.
    pub fn saw_text(&self) -> bool {
        self.saw_text.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for TurnSinkHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnSinkHandle")
            .field("saw_text", &self.saw_text())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        text: Mutex<Vec<String>>,
        reasoning: Mutex<Vec<String>>,
    }

    impl TurnSink for Recorder {
        fn text_delta(&self, text: &str) {
            self.text.lock().unwrap().push(text.to_string());
        }

        fn reasoning_delta(&self, text: &str) {
            self.reasoning.lock().unwrap().push(text.to_string());
        }
    }

    #[test]
    fn an_empty_delta_is_neither_forwarded_nor_counted() {
        let recorder = Arc::new(Recorder::default());
        let handle = TurnSinkHandle::new(recorder.clone());

        handle.text_delta("");
        assert!(!handle.saw_text(), "an empty delta is not text");
        assert!(recorder.text.lock().unwrap().is_empty());

        handle.text_delta("hi");
        assert!(handle.saw_text());
        assert_eq!(recorder.text.lock().unwrap().as_slice(), ["hi"]);
    }

    /// **S2.** Reasoning is forwarded but is not the answer: a turn that only
    /// thought out loud still owes the client its text.
    #[test]
    fn reasoning_is_forwarded_without_counting_as_text() {
        let recorder = Arc::new(Recorder::default());
        let handle = TurnSinkHandle::new(recorder.clone());

        handle.reasoning_delta("let me think");
        handle.reasoning_delta("");

        assert_eq!(
            recorder.reasoning.lock().unwrap().as_slice(),
            ["let me think"]
        );
        assert!(recorder.text.lock().unwrap().is_empty());
        assert!(
            !handle.saw_text(),
            "reasoning must not suppress the fallback delta"
        );
    }

    /// The flag is shared by every clone — the loop forwards through one
    /// clone and the chat service reads the answer off another.
    #[test]
    fn clones_share_the_flag() {
        let handle = TurnSinkHandle::new(Arc::new(Recorder::default()));
        let clone = handle.clone();
        clone.text_delta("a");
        assert!(handle.saw_text());
    }
}
