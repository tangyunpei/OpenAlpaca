# Lessons — OpenAlpaca

Rules written after corrections, so the same mistake is not made twice. Newest first.
Each entry: what went wrong → the rule that prevents it.

## 2026-09-02 — In a lifecycle spec, fix handle ownership as a table, not one rule at a time
**What happened:** Three consecutive review rounds on the extension-enable design each found one real S2 hole in the same family, and each fix was one rule: rev 10 added E-FAIL (a bring-up that fails *after* connect must tear down the handle it just built); rev 11 then added E-PRE (a verb that builds a handle must first tear down whatever handle the map still holds, because a Retry can win the mutex before the crash reaper). Both are the same question — *who owns the handle in each state, and which verb drops it* — answered one cell at a time.
**Rule:** When a design has a handle with a lifecycle (process, connection, task, lock), write the ownership matrix explicitly before declaring any invariant: rows = states (`Enabling`, `Enabled`, `Failed{Crashed}` pre-reaper, `Failed` post-reaper, `Disabled`…), columns = every verb and every failure exit that can create or drop the handle (`enable`, `reload`, `disable`, `deny`, reaper, watcher, E-FAIL…). Every cell must say "holds one", "holds none", or "tears down then builds". An invariant like "Failed owns no handle" that is not derived from that table will be false in the first race a critic constructs. Do this in the *first* pass; each missed cell costs a full review round.
**Validated the same day:** the matrix was drafted (in the scratchpad) while a critique was still running; it disagreed with the design text on exactly the two cells that critique then flagged as blocking (`Failed{Crashed}` × `deny`, × declaration-gone). The table found in ten minutes what four sequential one-rule fixes had not.

## 2026-09-02 — Scope a revise-loop's re-run to the artefact whose lens failed
**What happened:** A reconcile loop's readiness predicate required *all four* critics (three on the design, one on the plan/KB) to say ready. Round 1: the three design lenses passed rev 6; only the plan lens failed (a missing Phase 0 slot). The loop re-ran the *design* reviser anyway, producing rev 7 — which applied ten more lesson rows and introduced three fresh blockers into a document that had just passed. A plan-side gap cost a design regression and a full extra round.
**Rule:** In a revise-until-ready loop over several artefacts, key the re-run on *which* lens failed: re-edit only the artefact that failed, and re-run only its lenses. Never re-run an editor over an artefact whose own lenses passed. Also: an aggregation that counts "None found" prose as a finding is a bug — require empty arrays in the schema.

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

## 2026-09 — A binary that ignores its arguments will boot on `--help`
**What happened:** During a live acceptance run an agent ran `openalpacad --help` to read usage, without the isolation variables. The daemon never read its arguments, so it booted against the real `~/.openalpaca` and seeded a store on a machine where none was supposed to exist.
**Rule:** Never run the daemon binary, for any reason, without both `OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` pointing into a temp directory; put that sentence in every agent prompt that can reach the binary. To learn a binary's flags, read its source first. (The daemon now refuses arguments, `apps/openalpacad/src/args.rs`, but the rule stands for any binary whose argument handling has not been read.)

## 2026-09 — A fixer's green tests are not acceptance; start the live check from a blank slate
**What happened:** Round 3 made chat streaming real and every gate passed, yet on the live model every chat tool call lost its arguments (Ollama sends a whole tool call in one SSE frame; the parser's only test used OpenAI's split-frame shape). Separately, the first GUI session picked a chat model by hand before sending, which hid that a fresh window seeded an unroutable model and failed its first message.
**Rule:** Any change that moves a code path onto production for the first time gets a live end-to-end run before it is called done, and the live run starts exactly as a new user would: fresh store, fresh window, no manual setup step that the product does not require. When a provider's wire shape matters, capture the real frames and put them in the tests verbatim.

## 2026-09 — An unquoted heredoc executes the backticks inside it
**What happened:** I patched a spec file with `python3 - <<EOF … EOF` whose Python text contained Markdown backticks around `scripts/release/install.sh`. The shell ran that as a command substitution: the installer was executed (it exited at its argument check, so nothing was installed), and the substituted text vanished from the spec.
**Rule:** Any heredoc carrying code, Markdown or file paths is quoted — `<<'EOF'` — and values it needs are passed as arguments or environment variables, never interpolated. When a command prints output that does not belong to it, stop and check what actually ran before continuing.

## 2026-09 — "Facts the controller verified" must actually be verified
**What happened:** The brief I gave the documentation agents listed "there is no daily budget" and "$5 per workflow" as facts they could rely on. I had copied both from `CLAUDE.md`. The agents, told to trust only the code, found both incomplete: three background jobs carry daily ceilings, and the shipped `lead_agent` template overrides the $5 default with $3. The brief's own rule — another document is a map, not evidence — had not been applied to the brief.
**Rule:** Anything handed to subagents as a settled fact is checked against the code first, or labelled "from CLAUDE.md, verify". When an agent's finding contradicts the brief, correct the brief before the next agent reads it, and correct the document the wrong fact came from.

## 2026-09 — Cargo's colour codes and a `tail` make a failed gate read as green
**What happened:** In one session four gate reports were wrong in the same way. My own run kept only `tail -40` of `cargo test`, which was nothing but doc-test summaries reading "0 passed; 0 failed", and its clippy step grepped `^(warning|error)`, which matched nothing because every line began with an ANSI escape. Three implementation agents independently wrote "clippy: 0 warnings" into commit messages on the same colour-code miss, and one counted from a zsh `for` loop that never ran because zsh does not word-split a variable. None of it hid a real failure this time, which is luck, not method.
**Rule:** Run every cargo gate with `CARGO_TERM_COLOR=never`, redirect its **full** output to a file (never a pipe — `apps/openalpacad/src/managers/mcp/tests.rs`'s hang tests leave orphaned `sleep 3600` children that hold a pipe open), record that command's **own** exit code, and count `test result:` lines from the file. A gate whose output you did not see is not a gate you ran.

## 2026-09 — Carry the evidence grade when a finding becomes the reason for an owner decision
**What happened:** Recon reported that an open `/v1/events` WebSocket blocks graceful shutdown into `process::exit(1)`, and marked that half "documented semantics, not executed". I dropped the marking and put it to the owner as fact; it became half the case for decision D-E. The first implementer measured it before building: on axum 0.8.9 / hyper 1.11 an upgraded socket is untracked and `serve` resolved in under a millisecond. The real shutdown blocker was an open SSE chat stream. The fix landed correctly only because the implementer measured first.
**Rule:** When a finding is put to the owner, say how it is known — measured, read in our code, or inferred from a dependency's documentation — and never promote an inferred claim to a stated one. The first builder on an inferred premise measures it before writing code, and a false premise is corrected in the owner-facing record, not just in the commit.

## 2026-09 — Re-read external state before putting a decision to the owner
**What happened:** For a day the session's records said PR #32 was a draft with its merge paused, and I asked the owner to choose between "push and keep it a draft" and alternatives. The owner had merged it the evening before (`51e9560e`). The records were right when written; nothing re-checked GitHub before they became the premise of a question, so the owner answered a question about a PR that no longer existed.
**Rule:** Before asking the owner anything premised on external state — a PR's status, a branch's remote head, CI results, a running process — read it live (`gh pr view`, `git fetch`), even when a note says what it is. A note records what was true; only a read says what is.
