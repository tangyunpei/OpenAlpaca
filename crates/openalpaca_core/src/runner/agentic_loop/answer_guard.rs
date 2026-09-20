//! H3 — the loop's last look at an answer before it becomes final.
//!
//! The steering completion guard (`mod.rs`) already proves the shape: at the
//! point where the loop would return `Complete`, something else gets a say and
//! may spend one more round. This is the same hook for a different question —
//! *is the answer true about this runtime?* — and, unlike steering, it can also
//! refuse to ship what it read.
//!
//! The loop owns the policy (at most **one** corrective round per turn, then
//! the replacement); an implementation owns the judgement. The only production
//! implementation is the main loop's fabricated-run guard
//! (`orchestrator::query_handler::run_claim_guard`), and a loop with no guard
//! configured — every non-main-loop caller — behaves exactly as it did.

/// What a guard wants done about an answer it rejects.
///
/// Both strings are supplied by the guard and neither is composed by the loop:
/// the note is what the model is told, the replacement is what the user reads
/// if the model says it again anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    /// Appended to the conversation as a user message for the one corrective
    /// round the loop grants.
    pub note: String,
    /// The turn's content when the corrective round produced the same problem
    /// — the runtime speaking in place of the model, rather than shipping it.
    pub replacement: String,
}

/// Reviews the answer the loop is about to return as `Complete`.
///
/// Called at most twice per turn: once on the model's answer, and once more on
/// the answer that followed a [`Correction::note`]. `None` means ship it.
pub trait AnswerGuard: Send + Sync {
    fn review(&self, answer: &str) -> Option<Correction>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A guard that objects to whatever it is told to object to, and records
    /// every answer it saw — the shape the loop tests drive.
    pub struct ScriptedGuard {
        pub reject: &'static str,
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    impl AnswerGuard for ScriptedGuard {
        fn review(&self, answer: &str) -> Option<Correction> {
            self.seen
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(answer.to_string());
            answer.contains(self.reject).then(|| Correction {
                note: "note".to_string(),
                replacement: "replacement".to_string(),
            })
        }
    }

    #[test]
    fn a_guard_that_finds_nothing_ships_the_answer() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let guard = ScriptedGuard {
            reject: "lie",
            seen: seen.clone(),
        };
        assert_eq!(guard.review("the truth"), None);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_correction_carries_both_halves() {
        let guard = ScriptedGuard {
            reject: "lie",
            seen: Arc::new(Mutex::new(Vec::new())),
        };
        let correction = guard.review("a lie").expect("rejected");
        assert_eq!(correction.note, "note");
        assert_eq!(correction.replacement, "replacement");
    }
}
