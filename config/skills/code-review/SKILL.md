---
id: code-review
name: Code Review
version: 0.1.0
description: Review code for bugs, style issues, security concerns, and improvements
invoke:
  mode: auto
  slash: /review
routing:
  intent:
    - "review code"
    - "code review"
    - "check for bugs"
    - "review file"
  keywords:
    - "review"
    - "bugs"
    - "style"
    - "security"
  negative_keywords:
    - "write"
    - "create"
  score:
    base: 0.2
    intent_weight: 0.45
    keyword_weight: 0.35
    recency_weight: 0.2
permissions:
  level: readonly
tools:
  allow:
    - "file_read"
output:
  format: markdown
  required_sections:
    - "Critical"
    - "Summary"
---

## Instructions

When performing a code review, follow these steps:

1. Read the file(s) specified by the user using the file_read tool
2. Analyze the code systematically for:
   - **Bugs and logic errors**: incorrect conditions, off-by-one, null/None handling
   - **Performance issues**: unnecessary allocations, O(n^2) where O(n) suffices
   - **Style inconsistencies**: naming conventions, formatting, idiomatic patterns
   - **Missing error handling**: unwrapped Results, unchecked edge cases
   - **Security concerns**: injection risks, unsafe operations, exposed secrets
3. Organize feedback by severity:
   - **Critical**: Bugs that will cause incorrect behavior or crashes
   - **Warning**: Issues that may cause problems under certain conditions
   - **Suggestion**: Improvements for readability, performance, or maintainability
4. Include specific line references and concrete fix suggestions
5. End with a brief summary assessment (overall quality, key areas to address)

## Style

Be constructive and specific. Reference line numbers. Suggest concrete fixes rather than vague improvements. Acknowledge what the code does well before diving into issues.
