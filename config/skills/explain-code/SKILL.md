---
id: explain-code
name: Explain Code
version: 0.1.0
description: Explain what a piece of code does in plain language
invoke:
  mode: auto
  slash: /explain-code
routing:
  intent:
    - "explain code"
    - "what does this do"
    - "how does this work"
    - "walk through"
  keywords:
    - "explain"
    - "understand"
    - "code"
  negative_keywords:
    - "write"
    - "create"
    - "review"
requires_capabilities:
  - file_read
permissions:
  level: readonly
tools:
  allow:
    - "file_read"
output:
  format: markdown
---

## Instructions

When explaining code:

1. Start with a **one-sentence summary** of what the code does overall
2. Identify the **key abstractions**: main types, functions, and their relationships
3. Walk through the **core logic step-by-step**, focusing on:
   - What each major block/function does and why
   - How data flows through the system
   - Key decision points and branching logic
4. Highlight any **non-obvious patterns, idioms, or design choices**
5. Note any **dependencies or side effects** that matter for understanding
6. Adjust detail level to the user's apparent expertise:
   - If they ask "what does this do?" -> high-level overview first
   - If they ask "how does this work?" -> more implementation detail

## Style

Use clear, plain language. Avoid jargon unless the user seems technical. Use analogies when explaining complex concepts. Structure the explanation from general to specific.
