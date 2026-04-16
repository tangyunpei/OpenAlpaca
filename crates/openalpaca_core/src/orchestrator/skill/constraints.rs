use std::collections::HashSet;
use std::fmt;

/// Maximum skill invocation depth before we suspect a cycle.
pub const MAX_SKILL_STACK_DEPTH: usize = 16;

/// The effective tool set a skill is allowed to use.
#[derive(Clone, Debug, Default)]
pub struct EffectiveToolSet {
    /// `None` means all tools are allowed; `Some(set)` restricts to those tools.
    pub allowed: Option<HashSet<String>>,
    /// Tools explicitly denied regardless of the allow list.
    pub denied: HashSet<String>,
    /// Chain of skill IDs from root to the current skill, for diagnostics.
    pub source_chain: Vec<String>,
}

/// Why a tool constraint was violated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictReason {
    /// The tool is in an ancestor's deny list.
    DeniedByAncestor,
    /// The tool is not present in an ancestor's allow list.
    NotInAncestorAllowList,
}

/// A constraint conflict produced when a child skill requires a tool that its
/// ancestors have forbidden or omitted from their allow lists.
#[derive(Debug, Clone)]
pub struct ConstraintConflict {
    pub skill_id: String,
    pub tool: String,
    pub reason: ConflictReason,
    pub denied_by: String,
}

impl fmt::Display for ConstraintConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            ConflictReason::DeniedByAncestor => {
                write!(
                    f,
                    "skill '{}' requires tool '{}' but denied by '{}'",
                    self.skill_id, self.tool, self.denied_by
                )
            }
            ConflictReason::NotInAncestorAllowList => {
                write!(
                    f,
                    "skill '{}' requires tool '{}' but not in allow list of '{}'",
                    self.skill_id, self.tool, self.denied_by
                )
            }
        }
    }
}

impl std::error::Error for ConstraintConflict {}

/// Compose parent + child constraints. Returns an error if the child requires a
/// tool that is denied or not in the effective allow list.
///
/// Logic:
/// 1. Start with parent's denied set, union child's denies.
/// 2. Allowed: intersect parent + child (`None` = all tools).
/// 3. Validate `child_requires`: each must not be denied AND must be in allowed
///    (if `Some`).
/// 4. Build `source_chain`: parent's chain + `child_skill_id`.
pub fn compose_constraints(
    parent: &EffectiveToolSet,
    child_allow: Option<&[String]>,
    child_deny: &[String],
    child_requires: &[String],
    child_skill_id: &str,
) -> Result<EffectiveToolSet, ConstraintConflict> {
    // 1. Denied = parent denied ∪ child denied
    let mut denied = parent.denied.clone();
    for d in child_deny {
        denied.insert(d.clone());
    }

    // 2. Allowed = intersect(parent.allowed, child_allow)
    let allowed = match (&parent.allowed, child_allow) {
        (None, None) => None,
        (Some(parent_set), None) => Some(parent_set.clone()),
        (None, Some(child_list)) => {
            Some(child_list.iter().cloned().collect::<HashSet<String>>())
        }
        (Some(parent_set), Some(child_list)) => {
            let child_set: HashSet<String> = child_list.iter().cloned().collect();
            Some(parent_set.intersection(&child_set).cloned().collect())
        }
    };

    // 3. Validate child_requires
    for req in child_requires {
        // Check deny list first
        if denied.contains(req) {
            // Determine which ancestor originally denied this tool.
            let denied_by = if parent.denied.contains(req) {
                parent
                    .source_chain
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "root".to_string())
            } else {
                child_skill_id.to_string()
            };
            return Err(ConstraintConflict {
                skill_id: child_skill_id.to_string(),
                tool: req.clone(),
                reason: ConflictReason::DeniedByAncestor,
                denied_by,
            });
        }
        // Check allow list
        if let Some(ref allow_set) = allowed {
            if !allow_set.contains(req) {
                let denied_by = parent
                    .source_chain
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "root".to_string());
                return Err(ConstraintConflict {
                    skill_id: child_skill_id.to_string(),
                    tool: req.clone(),
                    reason: ConflictReason::NotInAncestorAllowList,
                    denied_by,
                });
            }
        }
    }

    // 4. Build source chain
    let mut source_chain = parent.source_chain.clone();
    source_chain.push(child_skill_id.to_string());

    Ok(EffectiveToolSet {
        allowed,
        denied,
        source_chain,
    })
}

/// Filter tool definitions by constraints (deny list + allow list).
///
/// A tool is kept if:
/// - It is not in the deny list, AND
/// - Either `allowed` is `None` (all tools permitted) or the tool name is in
///   the allow set.
pub fn filter_tools_by_constraints(
    tools: &[openalpaca_llm::ToolDefinition],
    constraints: &EffectiveToolSet,
) -> Vec<openalpaca_llm::ToolDefinition> {
    tools
        .iter()
        .filter(|t| {
            if constraints.denied.contains(&t.name) {
                return false;
            }
            if let Some(ref allow_set) = constraints.allowed {
                return allow_set.contains(&t.name);
            }
            true
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a minimal ToolDefinition for testing.
    fn tool_def(name: &str) -> openalpaca_llm::ToolDefinition {
        openalpaca_llm::ToolDefinition {
            name: name.to_string(),
            description: format!("{name} tool"),
            parameters: serde_json::json!({}),
            strict: None,
            input_examples: None,
        }
    }

    #[test]
    fn test_compose_child_restricts() {
        let parent = EffectiveToolSet::default();
        let result = compose_constraints(
            &parent,
            None,
            &["dangerous_tool".to_string()],
            &[],
            "child_skill",
        )
        .unwrap();

        assert!(result.denied.contains("dangerous_tool"));
        assert_eq!(result.source_chain, vec!["child_skill"]);
    }

    #[test]
    fn test_compose_parent_denies_child_requires() {
        let parent = EffectiveToolSet {
            denied: HashSet::from(["exec".to_string()]),
            source_chain: vec!["parent_skill".to_string()],
            ..Default::default()
        };

        let err = compose_constraints(
            &parent,
            None,
            &[],
            &["exec".to_string()],
            "child_skill",
        )
        .unwrap_err();

        assert_eq!(err.skill_id, "child_skill");
        assert_eq!(err.tool, "exec");
        assert_eq!(err.reason, ConflictReason::DeniedByAncestor);
        assert_eq!(err.denied_by, "parent_skill");
    }

    #[test]
    fn test_compose_parent_allowlist_excludes() {
        let parent = EffectiveToolSet {
            allowed: Some(HashSet::from(["read".to_string(), "write".to_string()])),
            source_chain: vec!["parent_skill".to_string()],
            ..Default::default()
        };

        let err = compose_constraints(
            &parent,
            None,
            &[],
            &["exec".to_string()],
            "child_skill",
        )
        .unwrap_err();

        assert_eq!(err.skill_id, "child_skill");
        assert_eq!(err.tool, "exec");
        assert_eq!(err.reason, ConflictReason::NotInAncestorAllowList);
        assert_eq!(err.denied_by, "parent_skill");
    }

    #[test]
    fn test_compose_both_restrict() {
        let parent = EffectiveToolSet {
            denied: HashSet::from(["tool_a".to_string()]),
            source_chain: vec!["parent".to_string()],
            ..Default::default()
        };

        let result = compose_constraints(
            &parent,
            None,
            &["tool_b".to_string()],
            &[],
            "child",
        )
        .unwrap();

        assert!(result.denied.contains("tool_a"));
        assert!(result.denied.contains("tool_b"));
        assert_eq!(result.denied.len(), 2);
    }

    #[test]
    fn test_compose_chain_three_levels() {
        // grandparent → parent → child
        let grandparent = EffectiveToolSet::default();

        let parent = compose_constraints(
            &grandparent,
            None,
            &["gp_denied".to_string()],
            &[],
            "grandparent_skill",
        )
        .unwrap();

        let child = compose_constraints(
            &parent,
            None,
            &["parent_denied".to_string()],
            &[],
            "parent_skill",
        )
        .unwrap();

        let leaf = compose_constraints(
            &child,
            None,
            &["child_denied".to_string()],
            &[],
            "child_skill",
        )
        .unwrap();

        assert_eq!(
            leaf.source_chain,
            vec!["grandparent_skill", "parent_skill", "child_skill"]
        );
        assert!(leaf.denied.contains("gp_denied"));
        assert!(leaf.denied.contains("parent_denied"));
        assert!(leaf.denied.contains("child_denied"));
    }

    #[test]
    fn test_compose_allowlist_intersection() {
        let parent = EffectiveToolSet {
            allowed: Some(HashSet::from([
                "read".to_string(),
                "write".to_string(),
                "search".to_string(),
            ])),
            source_chain: vec!["parent".to_string()],
            ..Default::default()
        };

        let result = compose_constraints(
            &parent,
            Some(&["read".to_string(), "exec".to_string()]),
            &[],
            &[],
            "child",
        )
        .unwrap();

        let allowed = result.allowed.unwrap();
        assert_eq!(allowed.len(), 1);
        assert!(allowed.contains("read"));
        // "exec" not in parent's allow list → excluded by intersection
        assert!(!allowed.contains("exec"));
        // "write" not in child's allow list → excluded by intersection
        assert!(!allowed.contains("write"));
    }

    #[test]
    fn test_skill_stack_depth_limit() {
        let mut current = EffectiveToolSet::default();
        for i in 0..MAX_SKILL_STACK_DEPTH {
            current = compose_constraints(
                &current,
                None,
                &[],
                &[],
                &format!("skill_{i}"),
            )
            .unwrap();
        }
        assert_eq!(current.source_chain.len(), MAX_SKILL_STACK_DEPTH);
    }

    #[test]
    fn test_filter_tools_by_constraints() {
        let tools = vec![
            tool_def("read"),
            tool_def("write"),
            tool_def("exec"),
            tool_def("search"),
        ];

        let constraints = EffectiveToolSet {
            allowed: Some(HashSet::from([
                "read".to_string(),
                "write".to_string(),
                "exec".to_string(),
            ])),
            denied: HashSet::from(["exec".to_string()]),
            ..Default::default()
        };

        let filtered = filter_tools_by_constraints(&tools, &constraints);
        let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"read"));
        assert!(names.contains(&"write"));
        // "exec" denied, "search" not in allow list
        assert!(!names.contains(&"exec"));
        assert!(!names.contains(&"search"));
    }

    #[test]
    fn test_constraint_conflict_display() {
        let conflict = ConstraintConflict {
            skill_id: "code_runner".to_string(),
            tool: "shell_exec".to_string(),
            reason: ConflictReason::DeniedByAncestor,
            denied_by: "sandbox_skill".to_string(),
        };
        let msg = conflict.to_string();
        assert_eq!(
            msg,
            "skill 'code_runner' requires tool 'shell_exec' but denied by 'sandbox_skill'"
        );

        let conflict2 = ConstraintConflict {
            skill_id: "analyzer".to_string(),
            tool: "network".to_string(),
            reason: ConflictReason::NotInAncestorAllowList,
            denied_by: "restricted_parent".to_string(),
        };
        let msg2 = conflict2.to_string();
        assert_eq!(
            msg2,
            "skill 'analyzer' requires tool 'network' but not in allow list of 'restricted_parent'"
        );
    }
}
