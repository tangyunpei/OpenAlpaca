use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::runner::{LoopConfig, run_agentic_loop_routed};
use crate::tools::registry::{BuiltInTool, ToolRegistry};
use async_trait::async_trait;
use openalpaca_llm::LlmRouter;
use openalpaca_llm::{ChatMessage, ToolDefinition};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Executor for `invoke_skill:*` synthetic tool calls.
/// Created per skill invocation, not shared. Call stack grows with nesting.
pub struct SkillInvocationToolExecutor {
    pub catalog: Arc<SkillCatalog>,
    pub tool_registry: Arc<ToolRegistry>,
    pub router: Arc<LlmRouter>,
    pub call_stack: Vec<String>,
    pub max_depth: usize,
    pub cancel_token: Option<CancellationToken>,
}

impl SkillInvocationToolExecutor {
    pub fn new(
        catalog: Arc<SkillCatalog>,
        tool_registry: Arc<ToolRegistry>,
        router: Arc<LlmRouter>,
        call_stack: Vec<String>,
        max_depth: usize,
        cancel_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            catalog,
            tool_registry,
            router,
            call_stack,
            max_depth,
            cancel_token,
        }
    }

    /// Check if a tool name is an invoke_skill call.
    pub fn is_skill_invocation(tool_name: &str) -> bool {
        tool_name.starts_with("invoke_skill:")
    }

    /// Execute a skill invocation tool call.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let skill_id = tool_name
            .strip_prefix("invoke_skill:")
            .ok_or_else(|| format!("Invalid invoke_skill tool name: {}", tool_name))?;

        // Depth check
        if self.call_stack.len() >= self.max_depth {
            return Err(format!(
                "Max skill nesting depth ({}) exceeded. Call stack: {:?}",
                self.max_depth, self.call_stack
            ));
        }

        // Cycle check
        if self.call_stack.contains(&skill_id.to_string()) {
            return Err(format!(
                "Circular skill invocation detected: '{}' already in call stack {:?}",
                skill_id, self.call_stack
            ));
        }

        // Extract query
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "invoke_skill requires a 'query' parameter".to_string())?;

        // Load skill
        let skill_doc = self
            .catalog
            .load_full(skill_id)
            .map_err(|e| format!("Failed to load skill '{}': {}", skill_id, e))?;

        // Resolve tools for nested skill
        let mut tool_defs: Vec<ToolDefinition> =
            if !skill_doc.frontmatter.requires_capabilities.is_empty() {
                let deny = &skill_doc.frontmatter.tools.deny;
                let mut defs = self
                    .tool_registry
                    .tools_for_capabilities(&skill_doc.frontmatter.requires_capabilities);
                defs.retain(|t| !deny.contains(&t.name));
                defs
            } else if !skill_doc.frontmatter.tools.allow.is_empty() {
                skill_doc
                    .frontmatter
                    .tools
                    .allow
                    .iter()
                    .filter_map(|name| {
                        self.tool_registry.get(name).map(|t| t.definition.clone())
                    })
                    .collect()
            } else {
                vec![]
            };

        // Add invoke_skill:* synthetic tools for nested skill's own depends_on
        for dep_id in &skill_doc.frontmatter.depends_on {
            if let Some(dep_entry) = self.catalog.get(dep_id) {
                tool_defs.push(ToolDefinition {
                    name: format!("invoke_skill:{}", dep_id),
                    description: format!(
                        "Invoke the '{}' skill: {}",
                        dep_entry.frontmatter.name, dep_entry.frontmatter.description
                    ),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The input/query to pass to the skill"
                            }
                        },
                        "required": ["query"]
                    }),
                    strict: None,
                    input_examples: None,
                });
            }
        }

        // Build system prompt from skill instructions
        let system_prompt = format!(
            "You are executing the '{}' skill.\n\n{}",
            skill_doc.frontmatter.name, skill_doc.body
        );

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(query),
        ];

        let config = LoopConfig {
            max_rounds: 10,
            ..LoopConfig::default()
        };

        // Run nested agentic loop.
        // Note: sandbox is None — nested skills cannot execute tools yet.
        // This is a known limitation; full wiring requires building a SandboxManager
        // per invocation (same pattern as invocation.rs).
        let result = run_agentic_loop_routed(
            self.router.as_ref(),
            messages,
            tool_defs,
            &config,
            None, // sandbox (TODO: wire SandboxManager for tool execution)
            &format!("skill:{}", skill_id),
            None,  // sandbox_policy
            None,  // task_id
            None,  // context_budget
            self.cancel_token.clone(),
        )
        .await;

        Ok(result.final_content)
    }
}

/// Adapter to make SkillInvocationToolExecutor usable as a BuiltInTool.
pub struct SkillInvocationBuiltInAdapter {
    pub executor: Arc<SkillInvocationToolExecutor>,
    pub skill_id: String,
}

#[async_trait]
impl BuiltInTool for SkillInvocationBuiltInAdapter {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        self.executor
            .execute(&format!("invoke_skill:{}", self.skill_id), arguments)
            .await
    }
}
