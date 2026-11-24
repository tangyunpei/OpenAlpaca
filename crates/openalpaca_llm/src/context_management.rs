/// Claude API context_management configuration for server-side context editing.
///
/// Reference: Anthropic API docs — context_management beta feature.
#[derive(Debug, Clone)]
pub struct ContextManagement {
    pub edits: Vec<ContextEdit>,
}

/// A single context edit instruction.
#[derive(Debug, Clone)]
pub enum ContextEdit {
    /// Clear old tool-use blocks when input tokens exceed trigger.
    ClearToolUses {
        trigger_tokens: usize,
        keep_tool_uses: usize,
    },
    /// Clear old extended-thinking blocks, keeping what `keep` says.
    ClearThinking { keep: ThinkingKept },
}

/// Which assistant turns `clear_thinking_20251015` leaves their thinking
/// (the context-editing page, read 2026-11-14: `{type: "thinking_turns",
/// value: N}` "to keep the last N turns, or `"all"` to keep all thinking
/// blocks"; N counts the turn in progress).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingKept {
    /// The last N assistant turns, N > 0.
    Turns(usize),
    /// Every turn's.
    All,
}

impl ContextEdit {
    /// Serialize to the Anthropic API JSON format.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ContextEdit::ClearToolUses { trigger_tokens, keep_tool_uses } => {
                serde_json::json!({
                    "type": "clear_tool_uses_20250919",
                    "trigger": {
                        "type": "input_tokens",
                        "value": trigger_tokens
                    },
                    "keep": {
                        "type": "tool_uses",
                        "value": keep_tool_uses
                    }
                })
            }
            ContextEdit::ClearThinking { keep } => {
                let keep = match keep {
                    ThinkingKept::Turns(turns) => {
                        serde_json::json!({ "type": "thinking_turns", "value": turns })
                    }
                    ThinkingKept::All => serde_json::json!("all"),
                };
                serde_json::json!({
                    "type": "clear_thinking_20251015",
                    "keep": keep
                })
            }
        }
    }
}

impl ContextManagement {
    /// Build from budget manager parameters: old tool results cleared past
    /// `compaction_trigger`, every turn's thinking kept (ANTHROPIC-THINKING:
    /// the loop sends every kept block back, and `keep` `"all"` has the
    /// server leave Claude all of them; a request with this strategy and no
    /// `keep` gets one turn on every model, the context-editing page, read
    /// 2026-11-14).
    pub fn from_budget(compaction_trigger: usize, keep_tool_uses: usize) -> Self {
        Self {
            edits: vec![
                ContextEdit::ClearThinking {
                    keep: ThinkingKept::All,
                },
                ContextEdit::ClearToolUses {
                    trigger_tokens: compaction_trigger,
                    keep_tool_uses,
                },
            ],
        }
    }

    /// Serialize to the Anthropic API JSON format.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "edits": self.edits.iter().map(|e| e.to_json()).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clear_tool_uses_serialization() {
        let edit = ContextEdit::ClearToolUses {
            trigger_tokens: 100_000,
            keep_tool_uses: 5,
        };
        let val = edit.to_json();
        assert_eq!(val["type"], "clear_tool_uses_20250919");
        assert_eq!(val["trigger"]["type"], "input_tokens");
        assert_eq!(val["trigger"]["value"], 100_000);
        assert_eq!(val["keep"]["type"], "tool_uses");
        assert_eq!(val["keep"]["value"], 5);
    }

    #[test]
    fn test_clear_thinking_serialization() {
        let edit = ContextEdit::ClearThinking {
            keep: ThinkingKept::Turns(2),
        };
        let val = edit.to_json();
        assert_eq!(val["type"], "clear_thinking_20251015");
        assert_eq!(val["keep"]["type"], "thinking_turns");
        assert_eq!(val["keep"]["value"], 2);
    }

    /// `"all"`, the page's form for keeping every thinking block.
    #[test]
    fn keeping_all_thinking_is_sent_as_the_pages_all() {
        let edit = ContextEdit::ClearThinking {
            keep: ThinkingKept::All,
        };
        assert_eq!(
            edit.to_json(),
            serde_json::json!({"type": "clear_thinking_20251015", "keep": "all"})
        );
    }

    #[test]
    fn test_context_management_serialization() {
        let mgmt = ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking {
                    keep: ThinkingKept::Turns(2),
                },
                ContextEdit::ClearToolUses {
                    trigger_tokens: 100_000,
                    keep_tool_uses: 5,
                },
            ],
        };
        let val = mgmt.to_json();
        assert!(val["edits"].is_array());
        assert_eq!(val["edits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_from_budget_builds_correct_edits() {
        let mgmt = ContextManagement::from_budget(167_000, 5);
        assert_eq!(mgmt.edits.len(), 2);
        std::assert_matches!(
            mgmt.edits[0],
            ContextEdit::ClearThinking {
                keep: ThinkingKept::All
            }
        );
    }

    #[test]
    fn test_empty_context_management() {
        let mgmt = ContextManagement { edits: vec![] };
        let val = mgmt.to_json();
        assert!(val["edits"].as_array().unwrap().is_empty());
    }
}
