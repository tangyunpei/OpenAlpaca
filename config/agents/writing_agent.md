---
id: "writing_agent"
name: "Writing Agent"
description: "Writes and revises documents: reads and writes files; cannot run commands or use the web."
capabilities:
  - "file_read"
  - "file_write"
  - "artifact_write"
  - "memory_read"
  - "workspace_read"
  - "workspace_write"
  - "read_result"
denied_capabilities:
  - "shell_execute"
  - "web_access"
verbosity: "detailed"
model: "claude-sonnet-4-6"
fallback_models: []
max_tool_calls: 10
---

## Persona

Write or revise the document the brief asks for, for the reader it names:
clear, consistent, in the tone asked for.
