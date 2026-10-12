# Stage 1 — live check report (Task S6.5)

**Date:** 2026-10-08 (08:11–08:14 local, 15:11–15:14 UTC). **Commit:** `00f057fe` on `feat/markdown-memory` (Stage 1, its final fix wave and residual pass; the stage gate S6.4 ran at the same commit). **Model:** local Ollama `qwen3.8:27b-mlx` for every turn and for the one lead (`lead_agent`'s Claude pin fell through the ladder to it; one WARN records it). **Surface:** the daemon's HTTP API, driven by the plan's Python driver (urllib) — attended turns (`"unattended": false`), every prompt read from `GET /v1/chat/confirmations` and recorded before it was answered — and the CLI (`openalpaca store list|rebase|purge`, `sessions`, `config set`, `daemon stop`).

**Verdicts:** (a) **PASS** · (b) **PASS** · (c) **PASS** · (d) **PASS** · (h) **PASS** · (e) **PASS** · (g) **PASS** · (f) **PASS**. 0 ERROR lines.

**The app's surfaces were not checked live:** this session had no screen access the owner granted (the owner was away). They are proven by their vitest suites, run green in S6.4 at `00f057fe` (119 files, 1,453 tests) — `ConnectionSection.test.tsx`, `ProjectsCard.test.tsx`, `RepointProjectDialog.test.tsx`, `DeleteProjectDataDialog.test.tsx`, `useWorkspaces.test.tsx`, `useProjectFollow.test.ts`, `workspaces.test.ts` and `MemoryView.test.tsx` among them. Steps 1 and 13 were skipped.

## Setup

- **Build:** one cargo process, `cargo build -p openalpacad -p openalpaca` from the repository root, exit 0, immediately before the launch (R-S5). `strings -a target/debug/openalpacad | grep -cF "take the crate's home-store guard"` → **0** (the stage gate's test-built binary had counted 1 and was never launched).
- **Scratch root:** `/private/tmp/stage1-live.Tku29E` (below, `<scratch>`; canonical — the plan's Step 2 now canonicalizes it, since the daemon records canonical paths and `/tmp` is `/private/tmp` on macOS): `store/`, `config/`, the scratch repository `repo/` (`git init`, one commit, `README.md` and `src/main.rs`), `share.txt`, the driver `driver.py`, `daemon.out` and `evidence/`. `~/.openalpaca` was never read, written, listed or named by any command; every command that started or talked to the daemon had `OPENALPACA_HOME_STORE=<scratch>/store` and `OPENALPACA_CONFIG_DIR=<scratch>/config`, and none ran from the repository.
- `ROOT` = `/private/tmp/stage1-live.Tku29E/repo`, `NAME` = `-private-tmp-stage1-live-Tku29E-repo`.
- **Daemon launch** (from `<scratch>/store`): `nohup sandbox-exec -f scripts/eval/memory/loopback-only.sb target/debug/openalpacad > <scratch>/daemon.out 2>&1 &`. `sandbox-exec` execs the daemon in place: `discovery.json`'s pid 49244 was `target/debug/openalpacad`, parent 1, port 50048. The token was read inside the driver and never printed.
- **Ollama on, model pinned:** `openalpaca config set ai.ollama.enabled true` and `config set ai.default_model qwen3.8:27b-mlx`, both exit 0 from `<scratch>`. `GET /v1/status` → `llm` = `{"default_model": "qwen3.8:27b-mlx", "default_model_routable": true, "effective_default_model": "qwen3.8:27b-mlx"}`.

## The profile proof (before the first model turn)

Run at 08:11:08 local, before the daemon's first launch, with the same profile:

| Request under `sandbox-exec -f scripts/eval/memory/loopback-only.sb` | Result |
|---|---|
| `curl -sS -m 5 http://localhost:11434/api/tags` | exit **0**, body `{"models":[{"name":"qwen3.8:27b-mlx","model":"qwen3.8:27b-mlx",…` |
| `curl -sS -m 5 https://example.com` | exit **6**, `curl: (6) Could not resolve host: example.com` |

The daemon ran under that profile from its first launch (08:11:13) to its stop (08:14:16).

## Onboarding (no project)

One attended turn (`onboarding`, 08:11:35): *Please save your identity and my profile now with update_persona …*. Two `update_persona` prompts (`orchestrator`, no task) — identity `{name: Alpaca, creature: assistant, vibe: Concise and warm, emoji: 🦙}` and user `{identity: {name: Tester, domain: software testing}, communication_style: Terse answers, …}` — both recorded, then approved. `config/orchestrator/` then held `IDENTITY.md`, `SOUL.md`, `USER.md`, `backups/`, `templates/` — `BOOTSTRAP.md` gone, deleted by the daemon. The store held `README.md`, `plugins/`, `sessions/`, `state/` — no `projects/` (no turn had named a project).

## (a) A turn in the repository records the project, and nothing lands in the repository: **PASS**

`a-first` (08:11:51, `x-workspace-path: $ROOT`): *In one short sentence: what is two plus two?* → *Two plus two is four.* Then `projects/$NAME/project.json`, pretty-printed, keys in order, no `previous_root`/`previous_name`:

```json
{
  "version": 1,
  "id": "761b08db-6b71-4cef-ba12-5f810af61580",
  "root": "/private/tmp/stage1-live.Tku29E/repo",
  "created_at": "2026-10-08T15:11:51Z"
}
```

`ls -la projects/$NAME` → `project.json` alone, `-rw-------` (the folder `drwx------`). `find $ROOT -name .openalpaca` → nothing; `git -C $ROOT status --porcelain --ignored` → nothing.

## (b) An artifact of that project lands under `projects/<name>/artifacts/`: **PASS**

`b-artifact` (08:12:04): *Start a workflow: write a one-line README.md as an artifact that says this repository is a scratch test.* — the main loop started task `4c8307d6-46a9-4751-800a-b5bc978692fb` ("Write scratch-test README.md"); its lead's one `artifact_write` prompt (`{content: "This repository is a scratch test.", kind: markdown, name: README.md}`) was recorded and approved; the run ended `completed` at 08:12:38. `GET /v1/artifacts` → `01-readme.md`, `project_root` = `/private/tmp/stage1-live.Tku29E/repo` (R15), `path` = `<scratch>/store/projects/-private-tmp-stage1-live-Tku29E-repo/artifacts/2026-10-08-write-scratch-test-readme-md-4c8307d6/01-readme.md`, which exists. The repository: `find` nothing, `git status --porcelain --ignored` nothing.

## (c) A note saved in the project lands under its folder and is recalled: **PASS**

`/remember --project The release step for this repository is make ship.` (deterministic tier, 53 ms) → *Saved as a note you wrote: "The release step for this repository is make ship." (737ab953, project).* One note file: `projects/$NAME/memory/owners/cc804f0e4d9bbd08fc554dbf3271a5a3/notes/737ab953-e5d4-4d8f-b32d-2f4cc09027ae--the-release-step-for-this-repository-is-make-shi.md`. `state/memory-control/` → `proj-cc804f0e4d9bbd08fc554dbf3271a5a3-8fa663b3e8da687f.json`, the 16 hex being the first of `sha256("761b08db-…")` (R18) — the only snapshot: no note had been written to the home partition, so the plan's "beside the home snapshot" had none to stand beside. `/memories release` → `737ab953  The release step for this repository is make ship.  [fact/project]  (matched: term)`. The recall turn (*What is the release step for this repository?*) answered *The release step is `make ship`. (per memory 737ab953)*, and its `done` carried `memory_used: [{"id": "737ab953-e5d4-4d8f-b32d-2f4cc09027ae", "reason": "recall", …}]`.

## (d) `GET /v1/workspaces` lists the project: **PASS**

`[{"id": "761b08db-6b71-4cef-ba12-5f810af61580", "root": "/private/tmp/stage1-live.Tku29E/repo", "name": "-private-tmp-stage1-live-Tku29E-repo", "missing": false}]`. `openalpaca store list` → `/private/tmp/stage1-live.Tku29E/repo` / `projects/-private-tmp-stage1-live-Tku29E-repo · id 761b08db-6b71-4cef-ba12-5f810af61580`.

## (h) setup — an upload of the project that a conversation with no project shows

`POST /v1/files/upload` of `share.txt` with the project's header → `200`, id `176563c8-adf0-4bce-9586-a869e4adf402`. `POST /v1/sessions {"inherit": false}` → `201`, session `623e55d0-cf67-4bd7-a0ef-cca8f2f06b68`, `workspace_id: null`. The turn `h-link` named that session (`activate: true`), sent no project header and attached the upload → *OK*. The database: `1` attachment link; the upload's row `project_root` = `/private/tmp/stage1-live.Tku29E/repo`, `storage_path` = `<scratch>/store/projects/-private-tmp-stage1-live-Tku29E-repo/uploads/2026-10-08/01-share.txt`; the session's `workspace_id` still null. (h) was exercised.

## (e) Move the folder, see it missing, re-point it, find everything under the new name: **PASS**

`mv repo repo-moved`. `GET /v1/workspaces` → the entry with `"missing": true` (ST-7). `openalpaca store rebase $ROOT $NEW --dry-run`:

```text
Would re-point /private/tmp/stage1-live.Tku29E/repo → /private/tmp/stage1-live.Tku29E/repo-moved
  2 artifacts, 1 conversations, 1 runs, 5 messages, 0 follow-ups
  its folder is gone; re-pointing is what re-attaches its data
  its folder projects/-private-tmp-stage1-live-Tku29E-repo would be renamed for the new path
```

The re-point: `Re-pointed … → /private/tmp/stage1-live.Tku29E/repo-moved`, the same counts, `its folder is now projects/-private-tmp-stage1-live-Tku29E-repo-moved`, and `if an app window still names /private/tmp/stage1-live.Tku29E/repo in Settings → Connection, choose /private/tmp/stage1-live.Tku29E/repo-moved there with Use`. Then `projects/` held `-private-tmp-stage1-live-Tku29E-repo-moved` only, its `project.json` with `root` = the new path, the **same id** and no marker keys; the list read `missing: false` at the new root; `openalpaca sessions --workspace $NEW` listed the conversation of Steps 7–9 (`f0e83a1a-…`, the onboarding conversation the first project turn bound, archived when Step 11's session was activated); both artifact rows (`01-readme.md` and the upload `share.txt`) name the new `project_root` and a path under `projects/-…-repo-moved/`; `GET /v1/memories?scope=project` and `…/search?q=release` for the new root list and find note `737ab953` (`index_state: ready`); the upload's record answers `200` with `storage_path` under `projects/-…-repo-moved/uploads/` (both prefixes rewritten, R16); and `state/memory-control/` is unchanged (`diff` empty — a re-point touches no snapshot, R18).

## (g) A path inside the project is refused, naming the root: **PASS**

`openalpaca store purge $NEW/src` → exit 1, *Error: WORKSPACE_NOT_A_ROOT: /private/tmp/stage1-live.Tku29E/repo-moved/src is inside the project rooted at /private/tmp/stage1-live.Tku29E/repo-moved; deleting a project's data takes its root itself. Delete the data of /private/tmp/stage1-live.Tku29E/repo-moved if that is what you meant (HTTP 422)*. `GET /v1/workspaces` and `ls -A projects/` before and after: both `diff`s empty.

## (f) Delete the project's data — plan first — and the repository is byte-identical: **PASS**

The dry run (`openalpaca store purge $NEW`, exit 0) printed the plan in exactly the shape `docs/CLI_Manual.md`'s `store` section quotes — the same nine entries in the same order, the same retention lines, `kept: 1 uploads and 0 by-products another conversation still uses, moved to the root store` and `Nothing was deleted. Re-run with -y to carry this out.` — and deleted nothing (the project still listed). Its counts: 1 conversation, 13 messages, 4 tool calls, 0 follow-ups; 1 run, 1 subagent span, 11 run events; 0 uploads handed to this project's conversations; 1 by-product; 1 memory note in 1 partition; keep 1 upload. `GET /v1/sessions?workspace_id=$NEW` → `f0e83a1a-b3ea-4024-8b05-17677e55179a`.

`openalpaca store purge $NEW -y` (exit 0) printed the same plan headed `Deleted the data of /private/tmp/stage1-live.Tku29E/repo-moved`, then:

```text
  deleted: 1 conversations, 13 messages, 4 tool calls, 0 follow-ups, 1 runs, 1 spans, 11 run events, 0 uploads, 1 by-products, 1 memory notes
  removed: 1 session log directories, 1 memory partitions, the folder projects/-private-tmp-stage1-live-Tku29E-repo-moved
  kept: 1 uploads and 0 by-products another conversation still uses, moved to the root store
```

Then: `sessions/` lost exactly `f0e83a1a-…` and kept `623e55d0-…` (Step 11's conversation of no project); `projects/` is empty and `GET /v1/workspaces` answers `[]`; `0` control snapshots carry `8fa663b3e8da687f`; the kept upload is one file, `<scratch>/store/uploads/kept/-private-tmp-stage1-live-Tku29E-repo-moved-761b08db-6b71-4cef-ba12-5f810af61580/2026-10-08/01-share.txt`, and its record answers `200` with `project_root: null`, `storage_path` that path, `status: ready`; the repository's file hashes (`.git/` included) are identical to those taken at the top of the step (`diff` empty) and `git status --porcelain --ignored` prints nothing.

## Every confirmation, in order (all recorded before they were answered; none denied)

| Time | Turn | `tool_name` | `agent_id` | `task_id` | Decision |
|---|---|---|---|---|---|
| 08:11:43 | onboarding | `update_persona` (identity) | `orchestrator` | — | approved |
| 08:11:43 | onboarding | `update_persona` (user) | `orchestrator` | — | approved |
| 08:12:32 | b-artifact | `artifact_write` | `lead_agent` | `4c8307d6-…` | approved |

## Every turn

| Time | Label | Project header | Answered by | Duration |
|---|---|---|---|---|
| 08:11:35 | onboarding | — | `qwen3.8:27b-mlx` | 10.1 s |
| 08:11:51 | a-first | `$ROOT` | `qwen3.8:27b-mlx` | 8.7 s |
| 08:12:04 | b-artifact | `$ROOT` | `qwen3.8:27b-mlx` (the run ended `completed` at 08:12:38) | 13.6 s |
| 08:12:48 | c-remember | `$ROOT` | the deterministic tier (no model) | 53 ms |
| 08:12:53 | c-list | `$ROOT` | the deterministic tier (no model) | 2 ms |
| 08:12:53 | c-recall | `$ROOT` | `qwen3.8:27b-mlx` | 9.8 s |
| 08:13:20 | h-link | — (session `623e55d0-…`) | `qwen3.8:27b-mlx` | 11.6 s |

## The log, and the stop

`daemon.out` (155 lines, colour codes stripped): **0** ERROR lines; one WARN — `loop.step.llm_call{agent_id="lead_agent" round=1 model="claude-sonnet-4-6" …}: Requested model is not available — answering with another` (the lead template's Claude pin substituted by the local model, as in Stage 0). `openalpaca daemon stop` (from `<scratch>`) → `Daemon stopped.`, exit 0; `discovery.json` removed (`test -e` exit 1); pid 49244 gone (`kill -0` exit 1).

## Left over

- Nothing was left on screen; no macOS permission or keychain dialog appeared.
- The scratch root `/private/tmp/stage1-live.Tku29E` is left in place (outside the repository, not committed).
- The app's surfaces were not checked live (no screen access); see the top.
