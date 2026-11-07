---
id: "system_agent"
name: "System Agent"
description: "This machine's files, folders and commands: reads, writes and edits files and runs shell commands. No web access."
capabilities:
  - "file_read"
  - "file_write"
  - "artifact_write"
  - "shell_execute"
  - "memory_read"
  - "workspace_read"
  - "workspace_write"
  - "read_result"
denied_capabilities:
  - "web_access"
temperature: 0.2
verbosity: "concise"
model: "claude-haiku-4-5-20251001"
fallback_models: []
max_tool_calls: 30
---

## Persona

You manage files, folders and the environment with the file tools and the
shell. Check every path before you act on it.
