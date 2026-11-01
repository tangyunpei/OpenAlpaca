# Where OpenAlpaca stores things today (2026-10-04, at `032f6ae2`)

Input to the storage design discussion. The owner's proposal: no `.openalpaca/` inside project
directories; everything under `~/.openalpaca/`. Nothing is decided and nothing is built.
Read from the code by five readers and one critic; the full maps with `path:line` evidence are in
`maps/` beside this file (`critic.md` is the corrected picture). The owner's real store was not read.

## 1. The short answer

- **Tasks, subtasks and workflows are not stored in any project directory.** A task (a run, a
  workflow) is one row of the `task` table in `~/.openalpaca/state/openalpaca.db`; a subagent is a
  `subagent_span` row; steps, events, tool calls, costs, follow-ups and the completion report are rows
  there too. The transcript is `~/.openalpaca/sessions/<session-id>/`. A task has no directory of its
  own anywhere.
- **A project's `.openalpaca/` holds three kinds of content only**: `artifacts/`, `uploads/` and
  `memory/` notes, plus `README.md`, `.gitignore`, `.layout`.
- **It is one per project root, not one per directory.** The project root is the nearest folder at or
  above the path the client sent that holds `.openalpaca` or `.git`. The store is created on the first
  write, never earlier.
- **The usual cause is the CLI inside a git repository.** `openalpaca chat` sends its current
  directory on every turn, and a `.git` alone makes that repository a project. The app sends a project
  only when a path is typed in Settings → Connection, or when it continues a conversation the CLI
  bound to a repository.

## 2. `~/.openalpaca/` today

```text
~/.openalpaca/                       OPENALPACA_HOME_STORE overrides the root
  README.md, .layout                 seeded when absent
  state/                             THE MACHINE'S — never user-edited
    openalpaca.db (+ -wal, -shm)     every row: tasks, subagent spans, sessions, messages, events,
                                     tool calls, costs, follow-ups, the index of every file
    discovery.json, openalpacad.lock, .master_key, repl_history
    logs/daemon.log (+ rotations)
    backups/                         rotated copies of hand-edited config
    memory-control/<partition>.json  one per memory partition — home AND project partitions
    cache/memory-index-v1.sqlite     the memory search index (rebuildable)
  config/                            YOURS — llm.toml, daemon.toml, mcp.toml
    agents/ (+ .archived/), skills/, tools/
    orchestrator/                    SOUL.md, USER.md, IDENTITY.md, BOOTSTRAP.md, backups/
  plugins/                           plugin directories you dropped in
    .permissions.toml, .config/, .data/, .staging/, .trash/
  artifacts/                         files produced by turns with NO project
    <date>-<title>-<task id 8>/NN-name.ext      one directory per run that wrote a file
      .versions/<stem>/vN.ext                   older versions
    loose/<date>/NN-name.ext                    files written from chat, outside a run
  uploads/<date>/NN-name.ext         uploads with NO project
  sessions/<session-id>/             ALL conversations' logs, any project
    log.jsonl (+ rotated), results/, snapshots/
  memory/owners/<owner-key>/         global notes
    notes/<uuid>--<slug>.md, .history/, MEMORY.md
  skills/, scratch/, cache/          reserved names — nothing creates them
```

Three things about this tree:

- **The root is also the daemon's working directory.** Both launchers start the daemon there, and
  `shell_execute`, script tools and MCP servers with no `cwd` inherit it. A relative write from any of
  them lands at `~/.openalpaca/<path>`, owned by no content kind. `file_read` and the lister read from
  the same root.
- **Two names appear twice**: `state/cache/` (live) and root `cache/` (reserved); `config/skills/`
  (live) and root `skills/` (reserved).
- **`config/` need not be here**: `OPENALPACA_CONFIG_DIR`, or a development run's `./config`.

## 3. `<project root>/.openalpaca/` today

```text
<project root>/.openalpaca/
  README.md, .gitignore, .layout     seeded on the first write; .layout holds project_root=<path>
  artifacts/                         same grammar as home; heads committable, .versions/ git-ignored
  uploads/<date>/                    git-ignored
  memory/.gitignore ("*")            notes never travel with the repository
  memory/owners/<owner-key>/         the project's notes: notes/, .history/, MEMORY.md
```

It is not self-contained. In the home store, keyed by the project's absolute path: the rows that index
its files, its memory control snapshot, and its search-index rows.

**What creates it** (each on a turn or run whose path resolved to a project):

| Trigger | Writes |
|---|---|
| `artifact_write`, `file_write`, a run's workspace spill | `artifacts/…` |
| An upload from the app with a project set | `uploads/<date>/…` |
| `/remember`, a model `memory_store`, `POST /v1/memories`, notes learned from a run | `memory/…` (every note kind except `preference` defaults to the project) |
| The conversation summary at a close (4 or more user turns) | `memory/…` |
| Background learning of items that are not facts about the user | `memory/…` |

A chat turn, a session, a task row, or choosing a project in the app create nothing on disk.

**Who decides the project**

| Client | Project of a turn |
|---|---|
| App | one typed path in Settings → Connection (default: none); a selected conversation's own project overrides it |
| `openalpaca chat` | always the current directory |
| `openalpaca memory` | the current directory, or `--global` / `--project` |
| `openalpaca tasks create`, `--file` uploads | none |
| Connectors, scheduled skills | none |
| Follow-ups, steering, relaunches, learning | the stored path of the turn they came from |

## 4. Outside both stores

- The app's chosen project path, pins and pane widths: the WebView's local storage.
- *Closed by N-STORE-1 (plan-rev3 §5.4, 2026-10-17): Open's copies now live under
  `~/.openalpaca/state/cache/open/<file id>/` (swept at boot, removed with their rows by a
  project's delete) and the CLI's editor buffers under `~/.openalpaca/state/editor/`; the next
  bullet is the picture as it was read.*
- OS temp: a copy made when an uploaded file is opened (`openalpaca-open/…`, never removed); CLI
  editor scratch files.
- OS keychain entries for keys stored by reference.
- The installed programs (`~/.local/openalpaca`, `~/.local/bin/openalpaca`, `~/Applications`).

## 5. What "a project" is

There is no project table and no project id. A project is its **canonical absolute path string**,
held in six columns of five tables (`task.workspace_id`, `session.workspace_id`,
`conversation_messages.workspace_root`, `lane_followups.workspace_path`, `file_assets.project_root`,
`file_assets.storage_path`). A re-base after a move (`PATCH /v1/workspaces`, `openalpaca store rebase`)
rewrites four of them and renames the store; two keep the old path.

What a project changes on a turn, wherever the bytes live:

- **Conversation**: a turn whose project differs from the lane's active conversation opens a new one;
  carry-over continues only from the previous conversation on the same lane and project.
- **Memory**: recall reads the global notes plus the project's; most new notes go to the project's.
- **Files**: artifacts and uploads go to the project's store.
- **Runs**: the task row, the lead's recall, subagents and follow-ups carry the project.

What it does not change: the file tools (rooted at the daemon's directory — FW-2), the prompt, config,
persona, session logs.

## 6. Why the store was put inside the project (decisions of record)

- The fix plan's first directive and its appendix A: outputs "sit beside its source", and artifact
  heads are committable because git "is strictly better history than `.versions/`".
- Since then: session logs were made home-only (never committable); project memory was git-ignored
  (MD-7, 2026-09-23); FW-1 (2026-10-02) recorded the owner asking for "a centralized location for the
  artifacts like ~/.openalpaca/artifacts". Only artifact heads remain committable.

## 7. What the in-project directory does besides hold files

A move under the home store has to replace each of these:

1. **It is a project marker.** `.openalpaca` is checked before `.git`; it is the only thing that makes
   a folder without `.git` a project, and several messages tell the user to create one.
2. **It detects a moved project.** `.layout`'s `project_root=` travels with the folder; the app's
   re-base offer reads it.
3. **The artifact address is derived from where the bytes are.** Bytes under the home root address as
   "no project", so the per-project filter, purge and re-base of artifacts rely on the location.
4. **The memory partition key is a hash of the in-project path.**
5. **Files travel with the folder** when it is moved or copied, and die with it when it is deleted
   (the home-side rows stay behind).
6. **Artifact heads can be committed** with the repository.

Also in the way today: `OPENALPACA_HOME_STORE` does not isolate project writes (an isolated daemon
still writes into the real project directory); a factory reset names only the home root and leaves
every project store orphaned; nothing deletes a project store.

## 8. Prior art: Claude Code

- Per-project data lives at home: `~/.claude/projects/<the project path with separators turned into
  dashes>/` holds each session's transcript, tool results, subagent transcripts and a `memory/`
  directory. No registry; the encoded path is the key.
- Inside the project stays only what a person writes and a team shares: `CLAUDE.md`,
  `.claude/settings.json`, commands, skills.
- A renamed project is not reconciled: the new path starts empty.
- It has no store for produced files, so it gives no precedent for artifacts.

## 9. Questions the design has to answer

1. Does the project **concept** stay (project memory, a conversation per project), with only its files
   moving home — or does the scope go too?
2. On disk at home: grouped per project (`projects/<key>/{artifacts,uploads,memory}`) or by kind with
   the project as a label on the rows (`artifacts/`, `uploads/`, `memory/projects/<key>/`)?
3. What identifies a project: the path as today, a readable encoded path, or a registered id?
4. What makes a folder a project when no `.openalpaca` is in it: `.git` only, any folder the user
   names, or an explicit list?
5. What happens when a project folder is moved, renamed or deleted?
6. Are committable artifact heads given up?
7. Where do agents work? FW-2 wants the file tools to read and edit the project's own files; the
   daemon's working directory is the home root today.
8. Existing data: convert, or refuse and reset as with the schema restart?
