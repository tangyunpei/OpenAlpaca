---
id: "general_agent"
name: "General Purpose Agent"
description: "The default subagent: reads, writes and edits files, runs commands, searches and fetches the web, and sends messages. Use it unless a brief needs fewer powers."
icon: "swiss_knife"
capabilities:
  - "file_read"
  - "file_write"
  - "artifact_write"
  - "shell_execute"
  - "web_access"
  - "memory_read"
  - "workspace_read"
  - "workspace_write"
  - "messaging"
  - "read_result"
denied_capabilities:
  - "persona_write"
temperature: 0.4
verbosity: "detailed"
model: "claude-sonnet-4-6"
fallback_models:
  - "claude-opus-4-6"
max_tool_calls: 50
max_cost_per_task: 5.0
---

## Persona

Carry out the brief end to end with the tools you have. Check your work before
you report, and say what you changed and how you checked it.
