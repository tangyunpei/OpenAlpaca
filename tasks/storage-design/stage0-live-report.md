# Stage 0 — live check report (Task L)

**Date:** 2026-10-07 (01:16–01:30 local, 08:16–08:30 UTC). **Commit:** `09b40624` on `feat/markdown-memory` (F1–F4 plus the final review's fix wave). **Model:** local Ollama `qwen3.8:27b-mlx` for every turn, every lead and every subagent (each shipped template's Claude pin fell through the ladder to it, which the log records). **Surface:** the daemon's HTTP API, driven by a Python script (urllib) as the brief ruled — attended turns (`"unattended": false`), each prompt read from `GET /v1/chat/confirmations` and recorded before it was answered.

**Verdicts:** F3 **PASS** · F1 **PASS** · F2 **PASS** · F4 **PASS**.

## Setup

- **Build:** one cargo process, `cargo build -p openalpacad -p openalpaca` from the repository root — `Finished dev profile … in 59.79s`, exit 0. No other cargo command ran.
- **Scratch root:** `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/1f42058f-d65d-4389-92dd-57d86d113731/scratchpad/stage0-live` (below, `<scratch>`): `store/`, `config/`, `repo/` (`git init`), the driver `driver.py`, the daemon's output `daemon.out` and `evidence/`. `~/.openalpaca` was never read, written, listed or named by any command; every command that started or talked to the daemon had `OPENALPACA_HOME_STORE=<scratch>/store` and `OPENALPACA_CONFIG_DIR=<scratch>/config`, and none ran from the repository.
- **Daemon launch** (from `<scratch>/store`, the key masked here):
  `OPENALPACA_HOME_STORE=<scratch>/store OPENALPACA_CONFIG_DIR=<scratch>/config OPENALPACA_MASTER_KEY=<throwaway, 64 × '1'> nohup sandbox-exec -f scripts/eval/memory/loopback-only.sb target/debug/openalpacad > <scratch>/daemon.out 2>&1 &` — the variable on that one command line only, never exported; the calling shell's `env | grep -c OPENALPACA_MASTER_KEY` was `0`. `sandbox-exec` execs the daemon in place: `discovery.json`'s pid 30029 was `target/debug/openalpacad`, parent 1, port 65462 on 127.0.0.1. The token was read inside the driver and never printed.
- **The daemon carried the variable:** `ps -E -ww -p 30029` counted 1 `OPENALPACA_MASTER_KEY=` in its exec-time environment (count only; that is what F3 must keep from a child).
- **Ollama on, model pinned:** `openalpaca config set ai.ollama.enabled true` → `Config set: ai.ollama.enabled = true`; `openalpaca config set ai.default_model qwen3.8:27b-mlx` → `Config set: ai.default_model = qwen3.8:27b-mlx` (both exit 0, cwd `<scratch>`). `GET /v1/status` → `llm` = `{"default_model": "qwen3.8:27b-mlx", "default_model_routable": true, "effective_default_model": "qwen3.8:27b-mlx"}`.

## The profile proof (before the first model turn)

Run at 01:21:05 local, before the daemon's first launch, with the same profile:

| Request under `sandbox-exec -f scripts/eval/memory/loopback-only.sb` | Result |
|---|---|
| `curl -sS -m 5 http://localhost:11434/api/tags` | exit **0**, body `{"models":[{"name":"qwen3.8:27b-mlx",…` |
| `curl -sS -m 5 https://example.com` | exit **6**, `curl: (6) Could not resolve host: example.com`, 0 bytes received |
| `curl -sS -m 5 https://93.184.215.14` (no DNS) | exit **7**, `Failed to connect to 93.184.215.14 port 443 after 0 ms` |

The daemon ran under that profile from its first launch to its stop.

## What the brief did not foresee: a fresh store starts in persona onboarding

A fresh store seeds `BOOTSTRAP.md`, and while it exists a turn is onboarding (`Orchestrator::is_bootstrapping`, `orchestrator/mod.rs:698`): the model gets the intent-suggested tools plus `update_persona` and none of the main loop's per-request set, so there is no `start_workflow`. The first F1 turn (F1-a1, below) met exactly that — the model answered that its only tool was `update_persona`. This was not the model declining, and it is not counted as an attempt. The plan's Step 4 would have met the same wall; **the next live check's brief should say that a fresh store's first turns are onboarding.**

Onboarding was completed through the product path, one attended turn (`onboarding-a1`) asking for `update_persona` on both documents. Both calls prompted and were approved (identity: `name Alpaca, creature "helpful llama-like assistant", vibe "concise and warm", emoji 🦙`; user: `Tester`, terse answers, software testing). The daemon logged `Bootstrap onboarding complete! Identity and user profile populated.` and `Deleted BOOTSTRAP.md: …/config/orchestrator/BOOTSTRAP.md` at 08:24:42Z, and the file was gone. F3 was then re-run as a true main-loop turn (F3-a2).

## F3 — no child sees the master key: **PASS**

Message `run command: env | grep -c OPENALPACA_MASTER_KEY || echo ABSENT`, approved at the prompt. The proof is the spawned command's own output, taken from the session log's `tool_result` (`<scratch>/store/sessions/d7d036a7-…/log.jsonl`), not the model's paraphrase:

| Run | Turn kind | Prompt (`tool_name`, `agent_id`, `task_id`) | Raw `shell_execute` result |
|---|---|---|---|
| F3-a1, 08:22:00Z | onboarding (`ModelOnly`; the surface 890 tool tokens) | `shell_execute`, `orchestrator`, none | `"0\nABSENT\n"` |
| F3-a2, 08:25:01Z | main loop (the surface 4,170 tool tokens) | `shell_execute`, `orchestrator`, none | `"0\nABSENT\n"` |

`grep -c` counted **0** occurrences in the child's environment although the daemon's own exec-time environment held the variable. The model's answers: a1 "`OPENALPACA_MASTER_KEY` is not set in the environment. `grep` found 0 matches, so the fallback printed `ABSENT`."; a2's answer was about the onboarding it had just done ("All saved: … Welcome aboard, Tester.") and did not mention the command — the raw result above is the evidence, not the answer. (In a2 the model also stored three memory notes with `memory_store`, which raises no prompt by design.)

Observation, not part of the verdict: the daemon created `state/.master_key` at start, and it does not hold the throwaway value (compared in process; neither value printed).

## F1 — `system_agent`'s shell asks first: **PASS**

Message (as the brief gives it): `Start a workflow: delegate to the system agent (template system_agent) one shell command, uname -a, and report its output verbatim.`

- **F1-a1** (08:22:23Z) — onboarding mode, no `start_workflow` on the surface; the model answered "I don't have a shell, a sub-process runner, or a "system_agent" delegate. My only available tool is `update_persona`." Not an attempt (above).
- **F1-a2** (08:25:26Z) — the first attempt after onboarding, on the same lane. The main loop called `start_workflow` (task `268ae384-b40c-44e0-b97d-2bc7bac815d5`, title "uname -a"); the lead spawned `system_agent` (`system_agent::ca8b62a5`). The pending confirmation the driver recorded **before** answering it:
  `request_id df0ac726-41d2-47a8-b704-c6ba09ffad37 · tool_name shell_execute · agent_id system_agent · task_id 268ae384-… · raised_at 2026-10-07T08:26:05.043440Z · tool_arguments {"command": "uname -a"}` → approved.
  `event_log` for the run, in order: `tool_confirmation_requested` 08:26:05.043627 (`system_agent`) → `tool_confirmation_resolved` 08:26:05.082977 `{"outcome":"approved"}` → `tool_executed` 08:26:05.094102 (`system_agent`, 12 ms). The session log shows that one `shell_execute` as `system_agent`'s only tool call in the run. Completion report (08:26:10Z): "Output of `uname -a`: ``` Darwin Junpeis-MacBook-Pro.local 27.0.0 Darwin Kernel Version 27.0.0: … RELEASE_ARM64_T6050 arm64 ```".

## F2 — the report names the artifact's path: **PASS**

Message: `Start a workflow: a writing agent writes notes/tea.md with three facts about green tea, then report exactly where the file is saved.` One attempt (F2-a1, 08:26:45Z; task `b02ce066-4679-485c-9144-6c16ab42cf73`).

- The lead spawned `writing_agent` (`writing_agent::4b407542`). Prompt recorded, then approved: `request_id 614fbf53-e20f-4fd7-9f42-55d723a0e3eb · file_write · writing_agent · task b02ce066-… · path "notes/tea.md"`.
- `file_write`'s result: "Saved: notes/tea.md is the artifact "01-notes-tea.md", version 1, at `<scratch>/store/artifacts/2026-10-07-write-tea-md-b02ce066/01-notes-tea.md`. … This file is the deliverable: tell the user it is saved at that absolute path …". No shell call followed.
- The subagent's answer was 1,128 characters with the path at characters 603–784 — past the old 500-character cut that lost it in Round G — and `wait_for_subagents` handed the lead the whole answer (1,219 characters).
- Completion report (08:27:42Z), quoted: "Done. File written with exactly three facts about green tea. **Saved location (verbatim):** - **Absolute path:** `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/1f42058f-d65d-4389-92dd-57d86d113731/scratchpad/stage0-live/store/artifacts/2026-10-07-write-tea-md-b02ce066/01-notes-tea.md` - **Artifact ref:** `01-notes-tea.md` (v1, in Library) - **Read-back path:** `artifacts/2026-10-07-write-tea-md-b02ce066/01-notes-tea.md` …". The file exists at that path; the run's outcome lists one artifact (`writing_agent`, `01-notes-tea.md`) and the report row links it.

## F4 — a follow-up the lead queues keeps its project: **PASS**

Message: `Start a workflow: write a one-line README.md as an artifact, and use queue_followup to queue a follow-up that will later summarise what was written.`, sent with `x-workspace-path: <scratch>/repo`. One attempt (F4-a1, 08:28:06Z; task `eb8636bf-909c-4375-8928-66378e17f08e`, whose `workspace_id` is the repo's path).

- The lead itself wrote the artifact; prompt recorded, then approved: `request_id 406c9239-2a21-472a-bd8a-bf18b32ec91b · artifact_write · lead_agent · task eb8636bf-… · name "README.md"`. It landed in the project store, `<scratch>/repo/.openalpaca/artifacts/2026-10-07-readme-follow-up-eb8636bf/01-readme.md`.
- The lead then called `queue_followup` ("Follow-up queued (id: 1). It will run as a new task after this workflow completes."). Read-only, after the run finished:

  ```
  sqlite3 -readonly <scratch>/store/state/openalpaca.db "select id, kind, workspace_path, source_task_id from lane_followups order by id desc limit 5"
  1|followup|/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/1f42058f-d65d-4389-92dd-57d86d113731/scratchpad/stage0-live/repo|eb8636bf-909c-4375-8928-66378e17f08e
  ```

  `realpath <scratch>/repo` is the same `/private/tmp/…/stage0-live/repo`. Before F4 that column held NULL.
- The follow-up auto-started at the run's end and finished (status `done`, 08:28:51Z); its user row in `conversation_messages` carries `workspace_root` = the repo path, so the follow-up ran in the project it was queued from. Completion report: "Done. - Wrote `README.md` (artifact `14b3aed1`, v1): "Alpaca is a helpful multi-agent orchestration assistant …" - Queued follow-up (id `1`) to summarize that content."

## Every confirmation, in order (all recorded before they were answered; none denied)

| # | Raised (UTC) | Turn | `tool_name` | `agent_id` | `task_id` | Answer |
|---|---|---|---|---|---|---|
| 1 | 08:22:00 | F3-a1 | `shell_execute` | `orchestrator` | — | approved |
| 2 | 08:24:40 | onboarding-a1 | `update_persona` (identity) | `orchestrator` | — | approved |
| 3 | 08:24:40 | onboarding-a1 | `update_persona` (user) | `orchestrator` | — | approved |
| 4 | 08:25:00 | F3-a2 | `shell_execute` | `orchestrator` | — | approved |
| 5 | 08:26:05 | F1-a2 | `shell_execute` | `system_agent` | `268ae384-…` | approved |
| 6 | 08:27:22 | F2-a1 | `file_write` | `writing_agent` | `b02ce066-…` | approved |
| 7 | 08:28:27 | F4-a1 | `artifact_write` | `lead_agent` | `eb8636bf-…` | approved |

The driver approved only what matched the check's allowlist (a tool name, and for `shell_execute` the expected command); everything else would have been denied and recorded. Nothing needed it. No `GET /v1/chat/confirmations` entry was left pending at the end.

## Every turn

| Turn | Sent (UTC) | Result |
|---|---|---|
| F3-a1 | 08:21:46 | PASS evidence (onboarding turn) |
| F1-a1 | 08:22:23 | not an attempt: onboarding mode, no `start_workflow` |
| onboarding-a1 | 08:24:34 | onboarding complete, `BOOTSTRAP.md` deleted by the daemon |
| F3-a2 | 08:24:49 | PASS (main-loop turn) |
| F1-a2 | 08:25:26 | PASS (run 30 s) |
| F2-a1 | 08:26:45 | PASS (run 40 s) |
| F4-a1 | 08:28:06 | PASS (run 15 s, follow-up 19 s) |

## The log, and the stop

- A bare `openalpacad` writes its log to stderr, so the log is `<scratch>/daemon.out` (243 lines, ANSI colour stripped into `evidence/daemon.plain.log` for the counts); `state/logs/daemon.log` is written only by `openalpaca daemon start` and the app's sidecar and did not exist.
- **ERROR lines: 0.** No panic.
- **WARN lines: 2**, both the router's announced substitution of a template's model, once per distinct pair (cut to 200 characters):
  - `WARN loop.step.llm_call{agent_id="lead_agent" round=1 model="claude-sonnet-4-6" messages_count=2}: openalpaca_llm::routing::router: Requested model is not available — answering with another model.` (`effective="qwen3.8:27b-mlx"`)
  - `WARN loop.step.llm_call{agent_id="system_agent::ca8b62a5" round=1 model="claude-haiku-4-5-20251001" messages_count=3}: openalpaca_llm::routing::router: Requested model is not available — answering`
- **Stop:** `openalpaca daemon stop` (both variables set, cwd `<scratch>`) → "Stopping Daemon (PID: 30029)… Daemon stopped.", exit 0, **3.2 s**; pid 30029 gone, `discovery.json` removed, the log ends `INFO openalpacad: Daemon stopped`.

## Left over

Nothing is running: the daemon is stopped, every driver finished (each wrote its `end` row; the six with a recorded exit code exited 0), and no run or follow-up was in flight at the stop. No macOS permission or keychain dialog surfaced in any command's output and none was answered; this session had no screen access and did not inspect the screen. The scratch root (store, config, repo, `driver.py`, `evidence/` with one `.jsonl` and `.log` per turn, `profile-proof.txt`, `F4-lane_followups.txt`, `daemon-stop.txt`) is in the session scratchpad and untracked.
