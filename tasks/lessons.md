# Lessons — OpenAlpaca

Rules written after corrections, so the same mistake is not made twice. Newest first.
Each entry: what went wrong → the rule that prevents it.

## 2026-09-01 — A "default" that adds an enforcement point is a decision
**What happened:** N4 settled the cost cap as *relabel-only* (per-workflow $5, per-turn $1, no new enforcement). A research lesson proposed "charge subagent spend to the lead's accumulator so per-workflow means what it says", and I listed it to the owner under "defaults I'll apply unless you say otherwise". A critic caught it: folding subagent dollars into the lead's cap is **new enforcement** — workflows die sooner, and each subagent dollar counts against its own $1 turn cap *and* the lead's $5. Same family: a "watcher auto-reloads an enabled server on config change" lesson widened a component's job without an owner row.
**Rule:** Before calling anything a default, ask: does it add an enforcement point, widen a component's job, or reverse a row in an accepted design? If yes, it is an owner decision — present it with a recommendation, never as a default. "Corroborated, not contested" applies to the *semantics* of a decision, not to a proposal that changes its *mechanism*.

## 2026-09-01 — Design against prior art before designing from scratch
**What happened:** The tool allow/enable design was drafted from first principles (ADR-029) and superseded the same day; its successor's warning path did not reach the default lane. Claude Code — which runs on this machine — had already solved both: per-server/per-plugin toggles, needs-auth as a first-class actionable state, failure caching with a retry window, and operational state rendered *into the agent's context* at turn boundaries instead of emitted as events.
**Rule:** For any subsystem Claude Code also has (MCP lifecycle, plugins, permissions, sessions/home layout, tool-result spill, context budget, cost), read Claude Code's docs and inspect `~/.claude/` (shape-only, no values) **before** proposing a mechanism. Cite it. Write "what not to copy" too. Research of record: `tasks/research/claude-code-design-lessons.md`.

## 2026-09-01 — "By construction" requires every call site
**What happened:** I told the owner `global_tool_deny` was "extension-only by construction", having verified one call site (`extension_tool_defs`, `registry/mod.rs:627`). The skill path (`orchestrator/skill/invocation.rs:201`) applies the same key as a flat `retain` over tools that include builtins. The claim went into the KB and the plan before a critic caught it.
**Rule:** Before describing a config key or function as having a property "by construction", grep **every** reader of it and check the property at each one. If a claim is going into a decision record, the grep output is part of the evidence, not optional.

## 2026-09-01 — Separate the axes before choosing a store
**What happened:** ADR-029 asked "where does per-tool enable state live?" and answered with a flat deny list spanning builtins. The owner's model was two axes — *allow* (per-agent, via agent config) and *enable* (per-extension toggles) — and builtins were never in the toggle's domain. The right question was "what is being toggled?", not "where is the bit stored?".
**Rule:** When a design question is phrased as "where should X live", first pin down X's granularity and scope (per what? governing what?) with the owner. Storage follows scope; choosing storage first bakes in the wrong scope.

## 2026-08-31 — Screenshots race lazy-loaded views
**What happened:** Batched screenshots of the new GUI showed blank views; I reported broken rendering. The views were `React.lazy` chunks still loading — DOM inspection proved they rendered.
**Rule:** Never conclude "blank/broken" from a screenshot of a lazy-loaded or streaming surface. Confirm with `read_page`/DOM state or wait for a load signal first, then screenshot.

## 2026-08-30 — A fix is "in the PR" only if it is an ancestor of the PR head
**What happened:** Twice, fixes were pushed to a branch that had already auto-deleted after merge, so they were silently not in the PR. The owner caught the first.
**Rule:** After pushing a follow-up to an open PR, verify with `git merge-base --is-ancestor <sha> origin/<pr-branch>` (or `gh pr view --json commits`). If the branch is gone, re-land on a fresh branch and say so.

## 2026-08 — New `.md` files are invisible to git by default
**What happened:** `.gitignore` has a `*.md` blanket ignore (line ~92). New docs under `tasks/` were untracked until explicitly whitelisted.
**Rule:** After creating any `.md` outside a whitelisted path, run `git check-ignore -q <file>` — exit 0 means add a `!path` line next to the existing whitelist block. Do this before claiming the doc is committed.

## 2026-08 — Verify a link check's own transform
**What happened:** A KB link checker used `tr -d '](.)'`, which also stripped the `.` in `.md`, producing false "MISSING" results.
**Rule:** When a check reports many failures, test the checker on one known-good input before trusting the report.
