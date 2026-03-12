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
    /// Clear old extended-thinking blocks, keeping N most recent.
    ClearThinking {
        keep_thinking_turns: usize,
    },
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
            ContextEdit::ClearThinking { keep_thinking_turns } => {
                serde_json::json!({
                    "type": "clear_thinking_20251015",
                    "keep": {
                        "type": "thinking_turns",
                        "value": keep_thinking_turns
                    }
                })
            }
        }
    }
}

impl ContextManagement {
    /// Build from budget manager parameters.
    pub fn from_budget(
        compaction_trigger: usize,
        keep_tool_uses: usize,
        keep_thinking_turns: usize,
    ) -> Self {
        Self {
            edits: vec![
                ContextEdit::ClearThinking { keep_thinking_turns },
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
            keep_thinking_turns: 2,
        };
        let val = edit.to_json();
        assert_eq!(val["type"], "clear_thinking_20251015");
        assert_eq!(val["keep"]["type"], "thinking_turns");
        assert_eq!(val["keep"]["value"], 2);
    }

    #[test]
    fn test_context_management_serialization() {
        let mgmt = ContextManagement {
            edits: vec![
                ContextEdit::ClearThinking { keep_thinking_turns: 2 },
                ContextEdit::ClearToolUses { trigger_tokens: 100_000, keep_tool_uses: 5 },
            ],
        };
        let val = mgmt.to_json();
        assert!(val["edits"].is_array());
        assert_eq!(val["edits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_from_budget_builds_correct_edits() {
        let mgmt = ContextManagement::from_budget(167_000, 5, 2);
        assert_eq!(mgmt.edits.len(), 2);
    }

    #[test]
    fn test_empty_context_management() {
        let mgmt = ContextManagement { edits: vec![] };
        let val = mgmt.to_json();
        assert!(val["edits"].as_array().unwrap().is_empty());
    }
}
