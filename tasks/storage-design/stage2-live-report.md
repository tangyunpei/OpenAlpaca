# Stage 2 — live check report (Task S6.5)

**Date:** 2026-10-10 (18:32–18:40 local, 2026-10-11 01:32–01:40 UTC). **Commit:** `3810d9d8` on `feat/markdown-memory` (Stage 2, its final fix wave, two residual passes and the lead's two last fixes; the stage gate S6.4 ran at the same commit: 5,080 passed, 0 failed, 16 ignored). **Model:** local Ollama `qwen3.8:27b-mlx` for every turn and for the one lead (`lead_agent`'s Claude pin fell through the ladder to it; one WARN records it). **Surface:** the daemon's HTTP API, driven by the plan's Python driver (urllib) — attended turns (`"unattended": false`), every prompt read from `GET /v1/chat/confirmations` and recorded, with its `preview`, before it was answered — and the CLI (`openalpaca chat` on a pseudo-terminal, `tasks confirmations list|approve`, `store list|rebase`, `config set`, `daemon tail|stop`).

**Verdicts:** (a) **PASS** · (b) **PASS** · (b2) **PASS** — the CLI keeps the preview's line breaks · (c) **PASS** · (d) **PASS** · (f) **PASS** · (g) **PASS** · (e) **PASS** · (j) **PASS** · (h) **PASS** · (i) *not exercised live* (no screen access). The repository changed by the one approved file only. 0 ERROR lines.

**The app's surfaces were not checked live:** this session had no screen access the owner granted. Step (i)'s stale approval card is proven by `views/chat/approval-card-returns.test.tsx`, and the app's result-file surfaces by S6a's and the fix wave's suites — `WrittenArtifactCard`, `LibraryRow`, `LibraryDetailHeader`, `LibraryView`, `FilePanelSlot`, `OutputSection`, `RunDetail`, `preview`, `MediaPreview` among them — all run green in S6.4 at `3810d9d8` (121 files, 1,551 tests). Steps 1 and 18 were skipped. `Open` was not called live: it would have launched an application on the owner's desktop.

## Setup

- **Build:** one cargo process, `cargo build -p openalpacad -p openalpaca` from the repository root, exit 0, immediately before the launch (R-S5). `strings -a target/debug/openalpacad | grep -cF "take the crate's home-store guard"` → **0** (the stage gate's test-built binary had counted 1 and was never launched).
- **Scratch root:** `/private/tmp/claude-501/-Users-tangyunpei-dev-OpenAlpaca/<session>/scratchpad/s2live.dsOA7z` (below, `<scratch>`; the session's scratchpad, canonical as made — `realpath` changed nothing): `store/`, `config/`, the scratch repository `repo/` (`git init`, one commit: `README.md`, `src/main.rs`, `.env` holding `LIVE_CHECK_TOKEN=not-a-secret`, and the link `escape -> <scratch>/outside`), `outside/target.txt`, the driver `driver.py`, `results.py`, `pty_chat.py`, `daemon.out` and `evidence/`. `~/.openalpaca` was never read, written, listed or named by any command; every command that started or talked to the daemon had `OPENALPACA_HOME_STORE=<scratch>/store` and `OPENALPACA_CONFIG_DIR=<scratch>/config`, and none ran from the repository.
- `ROOT` = `<scratch>/repo`, `NAME` its dashed form (123 characters, all ASCII: R5's plain name). The repository sits beside the store, never inside it.
- **Daemon launch** (from `<scratch>/store`): `nohup sandbox-exec -f scripts/eval/memory/loopback-only.sb target/debug/openalpacad > <scratch>/daemon.out 2>&1 &`. `sandbox-exec` execs the daemon in place: `discovery.json`'s pid 29667 was `target/debug/openalpacad`, port 53680. The token was read inside the driver and never printed.
- **Ollama on, model pinned:** `openalpaca config set ai.ollama.enabled true` and `config set ai.default_model qwen3.8:27b-mlx`, both exit 0. `GET /v1/status` → `llm` = `{"default_model": "qwen3.8:27b-mlx", "default_model_routable": true, "effective_default_model": "qwen3.8:27b-mlx"}`. `openalpaca daemon tail` ran for the whole check into `evidence/tail.log`.
- **Driver fix:** the plan's `results.py` passed `kind` to `driver.record` twice (`TypeError`); the scratch copy names the field `record_type`. Not committed (scratch).

## The profile proof (before the first model turn)

Run at 18:32:39 local, before the daemon's first launch, with the same profile:

| Request under `sandbox-exec -f scripts/eval/memory/loopback-only.sb` | Result |
|---|---|
| `curl -sS -m 5 http://localhost:11434/api/tags` | exit **0**, body `{"models":[{"name":"qwen3.8:27b-mlx","model":"qwen3.8:27b-mlx",…` |
| `curl -sS -m 5 https://example.com` | exit **6**, `curl: (6) Could not resolve host: example.com` |

The daemon ran under that profile from its first launch (18:32:45) to its stop (18:40).

## Onboarding (no project)

One attended turn (`onboarding`, 18:33:10). Three `update_persona` prompts (`orchestrator`, no task, no preview) — identity (its first call carried an unknown field `create` and was refused by the tool after approval, then sent again without it) and user — all recorded, then approved. `config/orchestrator/` then held `IDENTITY.md`, `SOUL.md`, `USER.md`, `backups/`, `templates/` — `BOOTSTRAP.md` gone. The store held `README.md`, `plugins/`, `sessions/`, `state/` — no `projects/` and no `scratch/` (no tool had worked in a folder).

## (a) A `file_write` in the project writes the real file, asks with a card naming the folder, and gets a pointer row: **PASS**

Turn `a-write` (18:33:28, `x-workspace-path: <scratch>/repo`). One confirmation, `file_write`, arguments `{"path": "notes/plan.md", "content": "Plan for the live check\nstep one: draft\n"}`, **preview** `Writes notes/plan.md in the project folder <scratch>/repo.` — approved. `notes/plan.md` held the two lines. `GET /v1/artifacts?in_project=true` → one row: `name` `notes/plan.md`, `in_project: true`, `openable: true`, `kind` `markdown`, `path` `<scratch>/repo/notes/plan.md`, `project_root` `<scratch>/repo`; `in_project=false` → none. The database row: `1|produced|ready|notes/plan.md|<ROOT>/notes/plan.md|<ROOT>`. The `artifact_written` event's `detail` carried `"in_project": true, "openable": true`. The tail printed `📄 artifact notes/plan.md [in the project] <scratch>/repo/notes/plan.md`. `projects/<NAME>/` held `project.json` only — no `artifacts/`. The tool's result: `Wrote notes/plan.md in the project (40 bytes), at <scratch>/repo/notes/plan.md. It is listed in the Library with the project's results. …`

## (b) A `file_edit` shows the file, the line and the two texts, keeps the old content, and updates the same row: **PASS**

Turn `b-edit` (18:33:58). One confirmation, `file_edit`, arguments `{"path": "notes/plan.md", "old_string": "step one: draft", "new_string": "step one: done"}` — approved — whose **preview**, line for line:

```
Edits notes/plan.md in the project folder <scratch>/repo: 1 replacement, the first at line 2. Its current content is kept in this conversation's log.
Replace:
step one: draft
With:
step one: done
```

The file's second line became `step one: done`, its first unchanged. `sessions/<PSID>/snapshots/000024-notes_plan_md` (40 bytes) is byte-identical to the pre-edit copy taken before the turn, and the log holds `file_snapshot {"path": "notes/plan.md", "seq": 24, "sha256": "63fdae82…", "size": 40, "snapshot_ref": "file:snapshots/000024-notes_plan_md"}`. One row for `notes/plan.md`: `sha256` `7e22c3bb…ce97b1d`, 39 bytes — the file's new hash (`shasum` agrees).

## (b2) `openalpaca chat`'s inline prompt prints the edit preview with its line breaks: **PASS**

`openalpaca chat --message 'Edit notes/plan.md with file_edit: replace the text "step one: done" with "step one: checked".'` from `<scratch>/repo` on a pseudo-terminal (`pty_chat.py`; `answered=1 exit=0`). What it printed, colour codes stripped:

```
From OpenAlpaca:
  Edits notes/plan.md in the project folder <scratch>/repo: 1 replacement, the first at line 2. Its current c…
  Replace:
  step one: done
  With:
  step one: checked
Arguments: {
  "new_string": "step one: checked",
  "old_string": "step one: done",
  "path": "notes/plan.md"
}
Allow execution? [y/N] n
Denied
```

(The first preview line is cut here by the report, not by the CLI.) **The answer to interfaces §11's open item:** the CLI keeps the preview's line breaks, each line indented under `From OpenAlpaca:`. The prompt was denied, and the file still read `step one: done`.

## (c) A shell command's card names the folder; the CLI lists it with its preview and answers it; the command runs there: **PASS**

Turn `c-shell` (18:34:27) with `--hold shell_execute`. One held prompt, `shell_execute` `{"command": "pwd"}`, **preview** `Runs in the project folder <scratch>/repo. The command itself can reach files outside it.` `openalpaca tasks confirmations list` printed the table (`895f264e-… shell_execute - 2026-10-11T01:34:38`) and, under it:

```
895f264e-325a-4193-a35c-069dc23ede75 — from OpenAlpaca:
  Runs in the project folder <scratch>/repo. The command itself can reach files outside it.
```

`--format json` carried the same `preview`. `openalpaca tasks confirmations approve 895f264e-…` printed `✓ Approved — the tool call is allowed to run. The run continues.`, exit 0; the driver's record shows the prompt as `confirmation_held` and no `confirmation_answered` for it. The `shell_execute` result: `True`, `<scratch>/repo\n` — the command started in the project.

## (d) Listing and reading never prompt — `.env` included: **PASS**

Turn `d-read` (18:34:47). **0** confirmations. `file_list` `True`: `. (<scratch>/repo): 6 entries` — `.env  30 bytes`, `.git/`, `README.md  21 bytes`, `escape@`, `notes/`, `src/` (dotfiles listed, the link marked `@`). `file_read` `True`: `LIVE_CHECK_TOKEN=not-a-secret\n`.

## (f) A path through a link that leaves the project is refused — writing and reading — with no card: **PASS**

Turn `f-write` (18:35:07): **0** confirmations; `file_write` `False` — `[tool_error] Path 'escape/owned.txt' resolves outside the workspace boundary`. Turn `f-read` (18:35:16): the model made **no** call and answered from the write's refusal ("Same as before"); asked once more naming the tool (`f-read2`, 18:35:49): **0** confirmations, `file_read` `False` — `[tool_error] Path 'escape/target.txt' resolves outside the workspace boundary`. `<scratch>/outside` held `target.txt` only.

## (g) A write into `.git` is refused, with no card: **PASS**

Turn `g-git` (18:36:01): the model made **no** call and answered with an invented refusal sentence; asked once more naming the tool (`g-git2`, 18:36:12): **0** confirmations, `file_write` `False` — `[tool_error] Writing '.git/live-check.txt' is refused: it is inside the project's .git folder, which git keeps. Run git through shell_execute instead.`; `.git/live-check.txt` does not exist.

## (e) A workflow's lead saves a by-product with no card; the main chat reads it back with `artifact_read`: **PASS**

Turn `e-workflow` (18:36:30) started run `af37d0c3-…` (`delegation` on `done`), which ended `completed` at 18:37:03 with **no** confirmation raised — no `artifact_write` or `workspace_write` card (ST-15). `GET /v1/artifacts?in_project=false` listed `01-handoff.md` (`cc80fe90-…`), `project_root` `<scratch>/repo`, `path` under `<scratch>/store/projects/<NAME>/artifacts/`; the tail printed it as `[markdown v1]`. The run's `session_id` is the project conversation's. Turn `e-read` (18:37:15): **0** confirmations; `artifact_read` `True` — `Hand-off for the live check\nThis is a short hand-off note saved as a by-product artifact.\n\n\n[artifact_read: bytes 0–90 of 90 from 01-handoff.md, version 1]`.

## The repository, after the project steps

`diff repo-before.sha repo-after.sha` printed exactly one added line, `7e22c3bb…ce97b1d  ./notes/plan.md` (Step 8's content; Step 9's edit was denied) — every other file, `.git/` included, byte-identical. `find <ROOT> -name .openalpaca` printed nothing; `git status --porcelain` printed `?? notes/` only.

## (j) A project whose folder moved with a link left at its old path reads missing, keeps its turn, and is re-pointed (ST-23): **PASS**

A second scratch repository `<scratch>/second` (`git init`, `README.md`), chosen through `POST /v1/workspaces` → `200`, `root` `<scratch>/second`, `missing: false`. Then `mv second second-moved && ln -s second-moved second`.

- Before the turn, `GET /v1/workspaces` listed 2 projects, `[('<scratch>/second', True)]` — missing, nothing at `<scratch>/second-moved`. `openalpaca store list` printed `<scratch>/second  missing — its folder is gone or has moved: re-point it, or delete its data`.
- Turn `j-old` (18:37:50, `x-workspace-path: <scratch>/second`): **0** confirmations; `file_read` `False` — `[tool_error] The project folder <scratch>/second is missing — its folder is gone or has moved: re-point the project to where its folder is now (Settings → Connection → Projects → Re-point…, o…`. `GET /v1/status` with the old path answered `project_root` `<scratch>/second` — the missing project, never the folder the link reaches. The conversation was filed under `<scratch>/second` (a new session; the lane's previous one was bound to `<scratch>/repo`), and **0** sessions under `<scratch>/second-moved`.
- An upload at the old path (no model): its row's `project_root` `<scratch>/second`, `storage_path` `<scratch>/store/projects/<second's name>/uploads/2026-10-11/01-j-upload.txt` — the missing project's own folder, never `second-moved` (R-T19). The list after the turn was the list before it — nothing registered at the link's target.
- `openalpaca store rebase <scratch>/second <scratch>/second-moved` printed `Re-pointed <scratch>/second → <scratch>/second-moved`, `1 artifacts, 1 conversations, 0 runs, 1 messages, 0 follow-ups`, `its folder is now projects/<second-moved's name>`, and the app-window note. (Its exit code was not captured: the check's shell is zsh, which has no `PIPESTATUS`; the output and every later read show it finished.) The list then held `[('<scratch>/second-moved', False)]` and nothing at `<scratch>/second`; the session rows named `<scratch>/second-moved` only (1); the upload's row named `<scratch>/second-moved` with its `storage_path` under the renamed folder. The link at `<scratch>/second` was left as it was (`test -L` → 0).

## (h) A conversation with no project works in its own scratch folder, which goes with it: **PASS**

`POST /v1/sessions {"inherit": false}` → session `cf1d33aa-…` (`SID`); every turn below carried `session_id` and no project header.

- `h-pwd`: card `shell_execute` `{"command": "pwd"}`, **preview** `Runs in this conversation's scratch folder <scratch>/store/scratch/<SID>. The command itself can reach files outside it.` — approved; result `<scratch>/store/scratch/<SID>\n`.
- `h-note`: card `shell_execute` `{"command": "printf scratch-check > note.txt"}`, the same preview — approved. `scratch/` was `drwx------` and held `<SID>`, which held `note.txt` (13 bytes).
- `h-read` (`Call the file_read tool with path note.txt …`): the model answered that it holds no `file_read` — on a turn with no project the main loop is offered `file_read` only when the message says `read file`, `open file`, `show file` or `cat ` (R42/R-T6, the keyword rule; the working-folder line names only the tools held, R-T22), and this wording carried none. Re-sent with the plan's own words (`h-read2`, `Read file note.txt with file_read …`): no card, `file_read` `True` — `scratch-check`.
- `h-write`: card `file_write` `{"path": "summary.md", "content": "scratch summary\n"}`, **preview** `Saves summary.md as a by-product in <scratch>/store/artifacts, OpenAlpaca's artifact store — not in a folder of yours.` — approved; result `Saved: summary.md is the artifact "01-summary.md", version 1, at <scratch>/store/artifacts/loose/2026-10-11/01-summary.md. This conversation has no project folder, …`; the row has no `project_root`.
- `h-readback`: no card; `artifact_read` `True` — `scratch summary\n\n\n[artifact_read: bytes 0–16 of 16 from 01-summary.md, version 1]`.
- The confirmations of the `h-*` turns: exactly three cards (two `shell_execute`, one `file_write`).
- `DELETE /v1/sessions/<SID>` → `204`; `<scratch>/store/scratch/<SID>` no longer exists and `scratch/` is empty.

## (i) The stale approval card, and the app's result-file surfaces: *not exercised live*

No screen access was granted to this session. Proven by `views/chat/approval-card-returns.test.tsx` and the suites named above (S6.4 Step 9, 121 files, 1,551 tests).

## Every confirmation, in order (all recorded, with their previews, before they were answered)

| Time | Turn | Tool | Agent | Arguments (short) | Decision |
|---|---|---|---|---|---|
| 18:33:19 | onboarding | `update_persona` | orchestrator | identity, with an unknown field `create` | approved (the tool refused it) |
| 18:33:19 | onboarding | `update_persona` | orchestrator | user: Tester, terse, software testing | approved |
| 18:33:22 | onboarding | `update_persona` | orchestrator | identity: Alpaca, 🦙, concise and warm | approved |
| 18:33:41 | a-write | `file_write` | orchestrator | `notes/plan.md`, two lines | approved |
| 18:34:01 | b-edit | `file_edit` | orchestrator | `step one: draft` → `step one: done` | approved |
| — | b2 (CLI) | `file_edit` | orchestrator | `step one: done` → `step one: checked` | **denied** at the CLI's `[y/N]` |
| 18:34:39 | c-shell | `shell_execute` | orchestrator | `pwd` | held; approved by `openalpaca tasks confirmations approve` |
| 18:38:28 | h-pwd | `shell_execute` | orchestrator | `pwd` | approved |
| 18:38:40 | h-note | `shell_execute` | orchestrator | `printf scratch-check > note.txt` | approved |
| 18:39:14 | h-write | `file_write` | orchestrator | `summary.md`, `scratch summary` | approved |

No other prompt was raised: none in (d), (f), (g), (e) — the lead's whole run included — (j), and none for any reader.

## Every turn

`onboarding` 18:33:10 · `a-write` 18:33:28 · `b-edit` 18:33:58 · b2 (CLI, pseudo-terminal) · `c-shell` 18:34:27 · `d-read` 18:34:47 · `f-write` 18:35:07 · `f-read` 18:35:16 (no call made) · `f-read2` 18:35:49 · `g-git` 18:36:01 (no call made) · `g-git2` 18:36:12 · `e-workflow` 18:36:30 (run `af37d0c3-…`, completed 18:37:03) · `e-read` 18:37:15 · `j-old` 18:37:50 · `h-pwd` 18:38:16 · `h-note` 18:38:30 · `h-read` 18:38:48 (no `file_read` offered: no keyword) · `h-write` 18:39:02 · `h-read2` 18:39:27 · `h-readback` 18:39:30. Every project turn but (j)'s ran in one conversation (`4ce2e9e3-…`, the onboarding conversation, bound to `<scratch>/repo` by the first project turn).

## The log, and the stop

`daemon.out` (colour codes stripped): **0** ERROR lines. One WARN: `loop.step.llm_call{agent_id="lead_agent" round=1 model="claude-sonnet-4-6" …}: Requested model is not available — answering with another model.` — the lead template's pinned Claude model answered by the local one, as in Stages 0 and 1. `openalpaca daemon stop` → `✅ Daemon stopped.`, exit 0; `state/discovery.json` removed; pid 29667 gone; no `sleep 30` left. The tail ended with `? unknown event` (the `daemon_shutting_down` frame, which `daemon tail` does not name — recorded residue since the daemon-lifecycle wave) and `→ Connection closed by server`.

## Observations (none a FAIL of Stage 2)

- **The model answered from memory twice** (f-read, g-git): it quoted a refusal without making the call — once inventing the sentence. Each was asked once more naming the tool, and the second ask made the call. The daemon's refusal sentences are the ones quoted above, from the session log.
- **A turn with no project offers `file_read` by keyword only** (h-read): `Call the file_read tool with path note.txt` carried none of the four phrases, so the main loop held no reader, and the working-folder line, which names only the tools held, did not name it either. This is the surface as designed (R42, R-T6, R-T22); whether a scratch turn should be offered the readers as a project turn is, is a question for the records (design §18).
- **`openalpaca daemon tail` prints `? unknown event` for `daemon_shutting_down`** — recorded residue, unchanged by Stage 2.

## Left over

The scratch root stays in the session's scratchpad (not in the repository). Nothing was left on screen; no macOS dialog appeared.
