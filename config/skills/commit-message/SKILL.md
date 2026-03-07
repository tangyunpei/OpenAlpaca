---
id: commit-message
name: Commit Message
version: 0.1.0
description: Generate conventional commit messages from code changes
invoke:
  mode: auto
  slash: /commit
routing:
  intent:
    - "commit message"
    - "write commit"
    - "generate commit"
    - "git commit"
  keywords:
    - "commit"
    - "git"
    - "message"
  negative_keywords:
    - "review"
    - "explain"
permissions:
  level: readwrite
tools:
  allow:
    - "shell_execute"
output:
  format: markdown
---

## Instructions

When generating a commit message:

1. Run `git diff --staged` to see staged changes
2. If no staged changes, run `git diff` to see unstaged changes and inform the user
3. Analyze the changes and classify the commit type:
   - **feat**: A new feature or capability
   - **fix**: A bug fix
   - **refactor**: Code restructuring without behavior change
   - **docs**: Documentation changes only
   - **test**: Adding or updating tests
   - **chore**: Build, CI, dependency updates, tooling
   - **style**: Formatting, whitespace, naming (no logic change)
   - **perf**: Performance improvements
4. Determine the scope (affected module, component, or area)
5. Write a conventional commit message:
   - Subject: `type(scope): concise description` (under 72 chars)
   - Body (if needed): blank line, then explain *why* not *what*
6. Present the message to the user for approval before committing

## Style

Follow the Conventional Commits specification. Keep the subject line under 72 characters. Focus on the "why" in the body. Use imperative mood ("add" not "added").
