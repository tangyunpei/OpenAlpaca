use crate::bus::EventBus;
use crate::orchestrator::skill::catalog::SkillCatalog;
use crate::runner::{LoopConfig, run_agentic_loop_routed};
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
    ) -> Self {
        Self {
            catalog,
            tool_registry,
            router,
            bus,
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

        // Build per-invocation registry clone with script tools and nested invoke_skill backends
        let needs_clone = !skill_doc.frontmatter.scripts.is_empty()
            || !skill_doc.frontmatter.depends_on.is_empty();
        let registry = if needs_clone {
            let cloned = (*self.tool_registry).clone();

            // Register script tools
            for cfg in &skill_doc.frontmatter.scripts {
                let tool = ScriptToolBuiltIn::new(&entry.skill_dir, cfg)?;
                cloned.register(RegisteredTool {
                    definition: ScriptToolBuiltIn::tool_definition(&cfg.name),
                    backend: ToolBackend::BuiltIn(Arc::new(tool)),
                    provides_capabilities: vec![],
                    exempt_from_timeout: false,
                });
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
                        });
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

        let mut config = LoopConfig {
            max_rounds: 10,
            ..LoopConfig::default()
        };
        config.event_bus = Some(self.bus.clone());

        // Build per-invocation sandbox so nested skills can execute tools
        let sandbox = SandboxManager::with_defaults(registry, self.bus.clone());
        let tool_ctx = ToolContext::default();

        let result = run_agentic_loop_routed(
            self.router.as_ref(),
            messages,
            tool_defs,
            &config,
            Some(&sandbox),
            &format!("skill:{}", skill_id),
            None,  // sandbox_policy
            None,  // task_id
            None,  // context_budget
            self.cancel_token.clone(),
            Some(&tool_ctx),
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
