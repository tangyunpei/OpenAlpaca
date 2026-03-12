/// Known section names for validation against `denied_sections`.
pub const KNOWN_SECTIONS: &[&str] = &[
    "conversation_summary",
    "relevant_memories",
    "user_context",
    "workspace_artifacts",
];

/// A minimum-exposure context package for sub-agent prompt assembly.
#[derive(Debug, Clone)]
pub struct ContextPackage {
    pub task_description: String,
    pub conversation_summary: Option<String>,
    pub relevant_memories: Vec<String>,
    pub user_context: Option<String>,
    pub workspace_artifacts: Vec<String>,
}

impl ContextPackage {
    /// Format the package as a prompt string for injection into the sub-agent's messages.
    pub fn format_for_prompt(&self) -> String {
        let mut parts = Vec::new();

        parts.push(format!("<assignment>\n{}\n</assignment>", self.task_description));

        if let Some(ref summary) = self.conversation_summary {
            parts.push(format!("<conversation-context>\n{}\n</conversation-context>", summary));
        }

        if !self.relevant_memories.is_empty() {
            let mem_block = self.relevant_memories.join("\n- ");
            parts.push(format!("<relevant-memories>\n- {}\n</relevant-memories>", mem_block));
        }

        if let Some(ref ctx) = self.user_context {
            parts.push(format!("<user-context>\n{}\n</user-context>", ctx));
        }

        if !self.workspace_artifacts.is_empty() {
            for (i, artifact) in self.workspace_artifacts.iter().enumerate() {
                parts.push(format!(
                    "<workspace-artifact index=\"{}\">\n{}\n</workspace-artifact>",
                    i, artifact
                ));
            }
        }

        parts.join("\n\n")
    }

    /// List which sections are present (for telemetry).
    pub fn sections_included(&self) -> Vec<&'static str> {
        let mut sections = vec!["task_description"];
        if self.conversation_summary.is_some() {
            sections.push("conversation_summary");
        }
        if !self.relevant_memories.is_empty() {
            sections.push("relevant_memories");
        }
        if self.user_context.is_some() {
            sections.push("user_context");
        }
        if !self.workspace_artifacts.is_empty() {
            sections.push("workspace_artifacts");
        }
        sections
    }

    /// Estimate total token count (bytes / 4 heuristic).
    pub fn estimated_tokens(&self) -> usize {
        self.format_for_prompt().len() / 4
    }
}

/// Builder for `ContextPackage` with `denied_sections` enforcement.
pub struct ContextPackageBuilder {
    task_description: String,
    conversation_summary: Option<String>,
    relevant_memories: Vec<String>,
    user_context: Option<String>,
    workspace_artifacts: Vec<String>,
    denied_sections: Vec<String>,
}

impl ContextPackageBuilder {
    pub fn new(task_description: String) -> Self {
        Self {
            task_description,
            conversation_summary: None,
            relevant_memories: Vec::new(),
            user_context: None,
            workspace_artifacts: Vec::new(),
            denied_sections: Vec::new(),
        }
    }

    pub fn conversation_summary(mut self, summary: String) -> Self {
        self.conversation_summary = Some(summary);
        self
    }

    pub fn relevant_memory(mut self, memory: String) -> Self {
        self.relevant_memories.push(memory);
        self
    }

    pub fn user_context(mut self, ctx: String) -> Self {
        self.user_context = Some(ctx);
        self
    }

    pub fn workspace_artifact(mut self, artifact: String) -> Self {
        self.workspace_artifacts.push(artifact);
        self
    }

    pub fn denied_sections(mut self, denied: &[String]) -> Self {
        self.denied_sections = denied.iter().map(|s| s.to_lowercase()).collect();
        self
    }

    pub fn build(self) -> ContextPackage {
        let is_denied = |section: &str| self.denied_sections.contains(&section.to_lowercase());

        ContextPackage {
            task_description: self.task_description,
            conversation_summary: if is_denied("conversation_summary") {
                None
            } else {
                self.conversation_summary
            },
            relevant_memories: if is_denied("relevant_memories") {
                Vec::new()
            } else {
                self.relevant_memories
            },
            user_context: if is_denied("user_context") {
                None
            } else {
                self.user_context
            },
            workspace_artifacts: if is_denied("workspace_artifacts") {
                Vec::new()
            } else {
                self.workspace_artifacts
            },
        }
    }
}
