use crate::bus::EventBus;
use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::orchestrator::skill::constraints::{compose_constraints, filter_tools_by_constraints, MAX_SKILL_STACK_DEPTH};
use crate::runner::{LoopConfig, LoopCostAccumulator, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::tools::builtins::ScriptToolBuiltIn;
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext, ToolRegistry};
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
    pub bus: EventBus,
    pub call_stack: Vec<String>,
    pub max_depth: usize,
    pub cancel_token: Option<CancellationToken>,
    pub cost_accumulator: Option<LoopCostAccumulator>,
    pub parent_tool_context: Option<ToolContext>,
    pub parent_max_cost: f64,
}

impl SkillInvocationToolExecutor {
    pub fn new(
        catalog: Arc<SkillCatalog>,
        tool_registry: Arc<ToolRegistry>,
        router: Arc<LlmRouter>,
        bus: EventBus,
        call_stack: Vec<String>,
        max_depth: usize,
        cancel_token: Option<CancellationToken>,
        cost_accumulator: Option<LoopCostAccumulator>,
        parent_tool_context: Option<ToolContext>,
        parent_max_cost: f64,
    ) -> Self {
        Self {
            catalog,
            tool_registry,
            router,
            bus,
            call_stack,
            max_depth,
            cancel_token,
            cost_accumulator,
            parent_tool_context,
            parent_max_cost,
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

        // Skill stack depth guard (from parent ToolContext)
        let skill_stack = self
            .parent_tool_context
            .as_ref()
            .map(|c| c.skill_stack.clone())
            .unwrap_or_default();
        if skill_stack.len() >= MAX_SKILL_STACK_DEPTH {
            return Err(format!(
                "skill invocation depth {} exceeds max {} — possible cycle: {}",
                skill_stack.len(),
                MAX_SKILL_STACK_DEPTH,
                skill_stack.join(" -> "),
            ));
        }

        // Extract query
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "invoke_skill requires a 'query' parameter".to_string())?;

        // Load skill
        let entry = self
            .catalog
            .get(skill_id)
            .ok_or_else(|| format!("Skill '{}' not found in catalog", skill_id))?;
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

        // Add invoke_skill:* synthetic tool definitions for nested skill's own depends_on
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

        // Compose tool constraints: parent's constraints bound the child
        let parent_constraints = self
            .parent_tool_context
            .as_ref()
            .and_then(|c| c.effective_constraints.clone())
            .unwrap_or_default();

        let child_requires: Vec<String> = skill_doc.frontmatter.requires_capabilities.clone();
        let child_allow = if skill_doc.frontmatter.tools.allow.is_empty() {
            None
        } else {
            Some(skill_doc.frontmatter.tools.allow.as_slice())
        };
        let child_constraints = compose_constraints(
            &parent_constraints,
            child_allow,
            &skill_doc.frontmatter.tools.deny,
            &child_requires,
            skill_id,
        )
        .map_err(|e| {
            let stack_str = skill_stack.join(" -> ");
            tracing::warn!(
                skill_id = skill_id,
                tool = %e.tool,
                denied_by = %e.denied_by,
                skill_stack = %stack_str,
                "Constraint conflict in nested skill invocation"
            );
            format!("{} (skill stack: {})", e, stack_str)
        })?;

        // Filter tool definitions by composed constraints
        tool_defs = filter_tools_by_constraints(&tool_defs, &child_constraints);

        // Build per-invocation registry clone with script tools and nested invoke_skill backends
        let needs_clone = !skill_doc.frontmatter.scripts.is_empty()
            || !skill_doc.frontmatter.depends_on.is_empty();
        let registry = if needs_clone {
            let cloned = (*self.tool_registry).clone();

            // Register script tools (file-based skills only)
            if let Some(ref skill_dir) = entry.skill_dir {
                for cfg in &skill_doc.frontmatter.scripts {
                    let tool = ScriptToolBuiltIn::new(skill_dir, cfg)?;
                    cloned.register(RegisteredTool {
                        definition: ScriptToolBuiltIn::tool_definition(&cfg.name),
                        backend: ToolBackend::BuiltIn(Arc::new(tool)),
                        provides_capabilities: vec![],
                        exempt_from_timeout: false,
                    })?;
                }
            }

            // Register invoke_skill:* backends for nested dependencies
            if !skill_doc.frontmatter.depends_on.is_empty() {
                let mut child_stack = self.call_stack.clone();
                child_stack.push(skill_id.to_string());
                let child_executor = Arc::new(SkillInvocationToolExecutor::new(
                    self.catalog.clone(),
                    self.tool_registry.clone(),
                    self.router.clone(),
                    self.bus.clone(),
                    child_stack,
                    self.max_depth,
                    self.cancel_token.clone(),
                    self.cost_accumulator.clone(),
                    self.parent_tool_context.clone(),
                    self.parent_max_cost,
                ));
                for dep_id in &skill_doc.frontmatter.depends_on {
                    if self.catalog.get(dep_id).is_some() {
                        let invoke_name = format!("invoke_skill:{}", dep_id);
                        cloned.register(RegisteredTool {
                            definition: ToolDefinition {
                                name: invoke_name,
                                description: format!("Invoke the '{}' skill", dep_id),
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
                            },
                            backend: ToolBackend::BuiltIn(Arc::new(SkillInvocationBuiltInAdapter {
                                executor: child_executor.clone(),
                                skill_id: dep_id.clone(),
                            })),
                            provides_capabilities: vec![],
                            exempt_from_timeout: true,
                        })?;
                    }
                }
            }

            Arc::new(cloned)
        } else {
            self.tool_registry.clone()
        };

        // Build system prompt from skill instructions
        let system_prompt = format!(
            "You are executing the '{}' skill.\n\n{}",
            skill_doc.frontmatter.name, skill_doc.body
        );

        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(query),
        ];

        // Build child tool context: inherit parent's identity, push skill stack
        let mut child_tool_ctx = self
            .parent_tool_context
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .with_skill_pushed(skill_id);
        child_tool_ctx.effective_constraints = Some(child_constraints);

        tracing::info!(
            parent_skill = ?self.call_stack.last(),
            child_skill = skill_id,
            remaining_budget = %(self.parent_max_cost - self.cost_accumulator.as_ref().map_or(0.0, |a| a.total_usd())),
            stack_depth = child_tool_ctx.skill_stack.len(),
            "Nested skill invocation"
        );

        // Cost budget: pass remaining as child's max_cost
        let cost_acc = self.cost_accumulator.clone().unwrap_or_default();
        let remaining = (self.parent_max_cost - cost_acc.total_usd()).max(0.0);
        if remaining <= 0.0 {
            return Err(format!(
                "budget exhausted (${:.4}/${:.2} spent) — cannot invoke skill '{}'",
                cost_acc.total_usd(),
                self.parent_max_cost,
                skill_id
            ));
        }

        let mut config = LoopConfig {
            max_rounds: 10,
            max_cost: remaining,
            ..LoopConfig::default()
        };
        config.event_bus = Some(self.bus.clone());

        // Build per-invocation sandbox so nested skills can execute tools
        let sandbox = SandboxManager::with_defaults(registry, self.bus.clone());

        let result = run_agentic_loop_routed(
            self.router.as_ref(),
            messages,
            tool_defs,
            &config,
            Some(&sandbox),
            &format!("skill:{}", skill_id),
            None, // sandbox_policy
            self.parent_tool_context
                .as_ref()
                .and_then(|c| c.task_id.as_deref()), // propagate task_id
            None, // context_budget
            self.cancel_token.clone(),
            Some(&child_tool_ctx),
            Some(cost_acc.clone()), // shared accumulator
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
