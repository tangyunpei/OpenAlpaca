# Storage and project work — design (rev 0.10, approved by the owner with ST-22, ST-23 taken; Stages 0 and 1 built)

**Status:** approved. The owner read rev 0.5 on 2026-10-05 ("everything in the design file is good") and asked for one
addition, an edit tool, taken as **ST-22**. Owner decisions **ST-1…ST-13** (2026-10-04) and **ST-14…ST-22** (2026-10-05)
are taken and are not reopened here. The lead's rulings are numbered **R1…R49** and are the lead's to change; the ones
the owner was shown on 2026-10-05 are listed in §0.3 and §0.5. **Stage 0 is built** (2026-10-06…07, `931460a4..a9dbed4e`,
from `tasks/storage-design/plan-stage0.md`, which the owner approved on 2026-10-06; one gate pass at `09b40624`; the
live check `stage0-live-report.md`, all four fixes PASS). **Stage 1 is built** (2026-10-07…08, `b8b768b6^..44d53783`,
from `tasks/storage-design/plan-stage1.md`, which the owner approved on 2026-10-07; one gate pass at `00f057fe`; the
live check `stage1-live-report.md`, `44d53783`). Stage 2 is not built, and is not built before the owner approves its
plan. Building Stage 1 raised two owner questions; the owner answered both on 2026-10-09 (§0.2): T36 keeps the
re-point as built, and **ST-23** makes a project whose recorded root no longer leads to itself read missing.

**Branch:** `feat/markdown-memory` at approval (2026-10-05), its code that of `032f6ae2`, the last commit before
the design's own. **Evidence:** two rounds of read-only
maps (`maps/critic.md`, `maps/r2-critic.md` are the corrected pictures) and a four-critic panel on rev 0.1 with a
sceptic behind each critic (`panel/`; 68 findings, 3 blocking, all answered in rev 0.2 — §19). Rev 0.3's code facts
were then checked by four readers (`panel/fact-check-rev03.json`: 47 claims, 34 confirmed, 13 corrected, 24 further findings —
§20). `maps/` and `panel/` are working evidence beside this file and are not tracked.

**How it is built:** three stages, each with its own plan and one gate pass.
**Stage 0** — four fixes that need no design. **Stage 1 (Part I)** — everything under `~/.openalpaca/`; a project
gets a folder, an id and a life cycle. **Stage 2 (Part II)** — agents work in the project's folder. **Part IV** says
what a beta waits for beyond the stages.

---

## 0. Decisions, questions, and what the owner is told

### 0.1 Taken (owner; ST-1…ST-13 on 2026-10-04, ST-14…ST-22 on 2026-10-05)

| # | Decision |
|---|---|
| ST-1 | Nothing app-written inside a project folder. Per-project content lives under `~/.openalpaca/projects/<name>/{artifacts,uploads,memory}`. |
| ST-2 | The project concept stays: a project keeps its own memory notes and its own conversations. |
| ST-3 | `<name>` is the project's path, plain, Claude-style: `-Users-tangyunpei-dev-AlpacaLink`. |
| ST-4 | A short hash is appended only when the plain name is already taken by a different folder. |
| ST-5 | Letters and digits of every script are kept; every other character becomes `-`. |
| ST-6 | A project is the git repository's top folder, wherever the client runs from, or a folder the user chooses when there is no git; the choice is remembered at home; no marker in the folder. |
| ST-7 | A project whose folder is gone keeps its data and shows as missing; the user re-points it with one command or deletes its data. No automatic move detection. |
| ST-8 | Content with no project stays at the root: `artifacts/`, `uploads/`, `memory/`. |
| ST-9 | Old in-project `.openalpaca/` folders are never read or converted. |
| ST-10 | With a project, agents work in the project's folder: read, list, shell and `file_write` act on its real files, with approval prompts. With no project: a working folder under `~/.openalpaca/scratch/`, and `file_write` is an artifact. |
| ST-11 | In a project, what the job is asked to produce is written as real files with `file_write`; `artifact_write` is only for by-products (drafts, pictures, things handed between subagents), kept under `projects/<name>/artifacts/`. |
| ST-12 | A result file written into the project gets a pointer row: the real path, no copy, history left to git. |
| ST-13 | Each project has a permanent id, kept in a small file inside its folder under `projects/`, beside its exact path. |
| ST-14 | (Q1) The shell is not confined, and that is accepted for this version: a command starts in the project's folder, each one asks for approval, and the card names the folder it runs in. An approved command runs as the user, anywhere. The `system_agent` approval fix (F1) is therefore mandatory. Sandbox methods are researched for a later version. |
| ST-15 | (Q2) By-product writes stop asking: `artifact_write` and `workspace_write` write only under `~/.openalpaca` and raise no approval card. A write of a real file in the project and a shell command keep asking. |
| ST-16 | (Q3) Reading never asks. In a project an agent can read any file under the project's top folder, secrets such as `.env` included, and what it reads goes to the model that answers. The manual says so. |
| ST-17 | (Q4) Before `file_write` replaces an existing file, the old content is kept under `sessions/`, until that conversation's log is swept. Replacing a file larger than 10 MB is refused. |
| ST-18 | (Q5) Open on a result file works for the file types the artifact store keeps (documents, code, images, data). For any other file, such as `.sh`, `.command` or `Makefile`, the app offers Reveal in Finder only. |
| ST-19 | (Q6) "Delete its data" deletes everything OpenAlpaca holds for the project: conversations, runs, by-products, uploads, the notes of every owner, and its folder under `projects/`. It never touches the project's own folder, and a file another conversation still uses is kept. `store purge --all` is removed. |
| ST-20 | (Q7) Re-pointing a project onto a path that is already a project of its own is refused, and the message names both ways out. Merging two projects is later work. |
| ST-21 | (Q8; supersedes LQ5) A missing project's turns are still learned into its notes. Only a project whose data was deleted drops them. |
| ST-22 | An edit tool, `file_edit`: in a project, an agent changes part of a file — the exact text to find and the text to put in its place — instead of writing the whole file back. Same confinement, approval card, pre-edit copy and pointer row as `file_write`. (§13) |
| ST-23 | (2026-10-09, the owner's answer to Stage 1's question on ST-7's `missing`) A project whose recorded root no longer leads to its own folder — the folder renamed by case only, or moved with a link left at the old path — is marked missing, like one whose folder is gone, and the user re-points it. |

### 0.2 Questions

Q1 was answered on 2026-10-05 and is ST-14. Q2…Q8 were put together at the owner's request and all were taken as
recommended: ST-15…ST-21. Building Stage 1 raised two; the owner answered both on 2026-10-09 (rev 0.10):

- **T36 — no change.** `PATCH /v1/workspaces` with an `old_path` inside a project goes on re-pointing the whole
  project — the route resolves the path up to the project's root — while the delete refuses that shape with
  `422 WORKSPACE_NOT_A_ROOT` naming the root (the lead's ruling R-S8). §9's `PATCH` row stands. The lead had
  recommended the refusal; the owner kept the re-point as built.
- **ST-23 — a root that no longer leads to itself is missing.** A project whose recorded root no longer canonicalizes
  to itself — a folder renamed by case only, or moved with a link left at the old path — reads `missing`, like one
  whose folder is gone, and the user re-points it (the whole-stage review's R1; its route half is built, §3's
  *As built*). To be built in Stage 2, slice S0, before anything resolves a project's working folder: `missing` means
  "the recorded root is not a directory, or no longer canonicalizes to itself", and a recorded root is matched as
  written before it is resolved wherever the daemon looks a project up — which also closes §18's *review R1's deeper
  cross-wiring*.

### 0.3 Lead's rulings the owner was shown (2026-10-05, no objection)

| # | What |
|---|---|
| R7 | **The length rule.** A folder name longer than 200 bytes is cut and always gets the hash. This is ST-4's rule for a name that cannot be used whole. |
| R30 | **T34 is fixed as its own row said**: the shipped `system_agent` template loses its two confirmation lines. The rule that an explicit list wins stays. A store seeded earlier keeps its old template; the switch-over note says which two lines to delete. |
| R29 | **The blanket `../` text filter.** Today any tool argument containing `../` is refused, file content included, so a source file with a relative import cannot be written. Shown as "the filter goes"; narrowed in rev 0.4 — see §0.5. |
| F3, R23, T35 | **Three findings older than this design.** Every child process inherits the daemon's master key through the environment: fixed in Stage 0 (F3), though a shell command can still read the key file (ST-14). The `send` tool sends any file path it is given: confined in Stage 2 (R23). Extension tools never prompt, and an MCP tool that sends no hint is treated as harmless: recorded as pending decision **T35**, not built here. |
| R43 | **"Always allow" keeps its scope**: one tool, one subagent. A run-wide allow is not built. |

### 0.4 The owner's fix list (2026-10-04), and where each item lands

| Item | Where |
|---|---|
| The lead reads only 500 characters of a subagent's answer | F2, Stage 0 |
| The `system_agent` approval bypass (T34) | F1, Stage 0 |
| The file tools use the conversation's project (FW-2) | Stage 2, §10–§15; `file_edit` (ST-22) for a change inside a file |
| Reading an artifact back from the main chat was not proven live | R41 and R42 give the main chat `artifact_read` on every turn (Stage 2); Stage 2's live check does the round trip |
| A click on a stale approval card was not proven live | No design change. Stage 2's live check tries once more: a prompt is answered from the CLI while the app shows its card, then the card is clicked. If that cannot be provoked, it stays proven by the unit tests only, and the report says so. |

### 0.5 New in rev 0.4, for the owner's review

The fact check (§20) changed these after the owner's answers. Each is a lead's ruling or a consequence of a decision
already taken; none is built.

| # | What |
|---|---|
| R29, narrowed | The `../` filter stops applying to OpenAlpaca's own tools only. It stays for script, command, MCP and plugin tools: their arguments reach a shell line or another program, and one shipped script relies on it. |
| R46 (from ST-17) | Where no copy of the old content can be kept — inside a skill or a plugin agent — `file_write` creates a new file but refuses to replace an existing one, and says why. |
| R45 | A shell command, a command tool or a skill script that times out or is cancelled is killed. Today it keeps running after the tool reported it stopped. |
| ST-9, consequence | An old in-project `.openalpaca/` holds by-products, uploads and, on a store used with this branch's builds, that project's memory notes. None of it is read again. The switch-over note says so, and how to carry a note over by hand. |
| R25, consequence | The migration that drops the old in-project file rows also drops their links from any other conversation that was handed the same file. That message keeps its text and shows the file as unavailable. Only a store used with a project on this branch has such rows. |
| Command tools | ST-14's "every command asks" is `shell_execute`'s rule. A command tool the owner defines under `config/tools` asks only when its own file declares it destructive, as today; none is shipped. Recorded with T35. |
| F2, F4 widened | F2 also keeps a failed subagent's whole answer. F4 also covers the lead's own follow-ups, stored with no project today, and `/steer`. |
| ST-15, consequence | With no project, `file_write` saves a by-product and still asks, while `artifact_write` saving the same kind of file does not. The rule is per tool and is decided before the tool runs. |
| ST-19, consequence | A by-product that another conversation still uses is kept as its newest version only. Its older versions go with the project. |
| R44 | Every tool call carries its conversation's id, so a conversation with no project gets its own scratch folder inside a skill or a plugin agent too. A call with neither a project nor a conversation id is refused. |

---

# Stage 0 — four fixes that need no design

**Built** 2026-10-06…07 as `931460a4..a9dbed4e` (plan `tasks/storage-design/plan-stage0.md`): F1 `b79adc26`,
`9196d69a` (the switch-over note's step 7); F2 `bbfc676e^..7f16eba6` — `head_tail` moved from `tool_helpers.rs` to
`crate::utils::text`; F3 `0047cf90^..f3980cba`; F4 `0d9a9293^..85c9cacc`; the final review's fix wave `fe7df469`
(F2), `33feb142` (F3), `09b40624` (F4). Gate at `09b40624`: `cargo test --workspace --exclude openalpaca_gui` 4,517 passed, 0 failed, 16 ignored; clippy 80, the
baseline; `gen_api_docs.py --check` clean; `cargo check -p openalpaca_gui` clean; the bun gates not run (no file under
`apps/openalpaca-gui` changed). The live check (`stage0-live-report.md`, `a9dbed4e`) proved F4 as well as the three
checks §16 names. The table's Where column records the code as it was before Stage 0 (the helper at
`tool_helpers.rs:19`, for one, is `utils::text::head_tail` now).

| # | Fix | Where |
|---|---|---|
| F1 | T34: delete `require_confirmation_for` from the shipped `system_agent` template (its one entry, `file_delete`, names no tool); one test on the shipped template's effective set. | `config/agents/system_agent.md:22-23` |
| F2 | The lead reads more of each subagent's answer. The tracker keeps a failed subagent's whole answer as it keeps a completed one's; today the failed path stores a 500-character prefix inside the error text. `wait_for_subagents` gives the head and the tail of each answer with a notice of what was cut, sized in bytes so that a batch of eight stays under the loop's inline bound (`tool_result_inline_bytes`). `check_subagent_status` returns the whole answer; its description and the lead's prompt say so, and that a long one arrives as a stub to page with `read_result`. | `runner/lead_agent/tracker.rs:10`, `:108-123`; `tools.rs:926-934`, `:1085-1148`; `prompt.rs:157-158`; the helper at `agentic_loop/tool_helpers.rs:19` |
| F3 | The master key leaves the environment. At start the daemon takes the key as it does today (`KeyEncryptor::ensure_at`) and hands it to `openalpaca_llm` as a process-local value that `KeyEncryptor::from_env_opt` reads first. It no longer sets `OPENALPACA_MASTER_KEY` in its own environment, and removes one it inherited, so no child process receives the key. The environment read stays as the fallback: the CLI and the tests use it. One test: started with the variable set, a spawned `env` shows no key; the test fills no process-wide value. | `apps/openalpacad/src/main.rs:146-151`; `crates/openalpaca_llm/src/keys/key_encryption/mod.rs:27-71` |
| F4 | A follow-up and a steering message keep the turn's project. Every writer stores the resolved root from the call's context (`request_workspace_root`): `queue_followup` from the main loop (the path as sent today) and from the lead (nothing today, so a follow-up a workflow queued runs with no project), `steer_workflow`, and the `/steer` command. | `runner/lead_agent/tools.rs:1434-1491`; `tools/builtins/steer_workflow.rs:112-120`; `orchestrator/task_ops.rs:392-400` |

---

# Part I — Everything under `~/.openalpaca/` (Stage 1)

**What you will see after Stage 1.** No `.openalpaca/` appears in a project. Each project you work in has a folder
under `~/.openalpaca/projects/`. Settings lists your projects and marks a missing one. Agents still cannot touch a
project's own files from the app; a file an agent writes is saved as a by-product in the project's folder at home.

## 1. The target tree

```text
~/.openalpaca/
  README.md, .layout
  config/                              yours: llm.toml, daemon.toml, mcp.toml, agents/, skills/, tools/, orchestrator/
  plugins/                             yours
  projects/                            NEW — one folder per project
    -Users-tangyunpei-dev-AlpacaLink/
      project.json                     the project's id and exact path (ST-13)
      artifacts/                       <date>-<title>-<task id 8>/NN-name.ext, .versions/, loose/<date>/
      uploads/<date>/NN-name.ext
      memory/owners/<owner-key>/       notes/, .history/, MEMORY.md
  artifacts/  uploads/  memory/        work with no project (ST-8), same shape as today
  sessions/<session-id>/               every conversation's log, any project (unchanged)
  scratch/<session-id>/                NEW in Stage 2 — the working folder of a conversation with no project
  state/                               the machine's: database, lock, keys, repl_history, logs/, backups/,
                                       memory-control/, cache/
```

- **R1.** "`state/` is the machine's; everything else at the root is yours" stays. `projects/` is yours.
- **R2.** The reserved content kinds `skills` and `cache` leave `ContentKind` and the seeded README; `config/skills/`
  and `state/cache/` are the live ones (`store/mod.rs:393-429`, `:836-844`). `scratch` stays reserved until Stage 2.
  The README's `state/` row gains `repl_history`. An existing home keeps its old README (it is written only when
  absent); the switch-over note says to delete it.
- **R3.** A folder under `projects/` holds `project.json` and content, nothing else: no `README.md`, no `.gitignore`,
  no `.layout`.

## 2. A project's folder name and its file

- **R4.** The name is computed from the project's **canonical root path** — symlinks resolved, on-disk case, the
  string the rows hold — normalised to NFC (`unicode-normalization`, already a dependency of the storage crate).
- **R5 (ST-5).** A character that is a Unicode letter, digit or combining mark is kept; every other character becomes
  one `-`; runs are not collapsed. Marks are kept because in Thai and Devanagari a word is not whole without them.

  | Project folder | Name |
  |---|---|
  | `/Users/tangyunpei/dev/AlpacaLink` | `-Users-tangyunpei-dev-AlpacaLink` |
  | `/Users/tangyunpei/dev/my_app` | `-Users-tangyunpei-dev-my-app` |
  | `/Users/tangyunpei/文档/笔记` | `-Users-tangyunpei-文档-笔记` |

- **R6 (ST-4).** The plain name is used unless a folder of that name exists that is not this project's: its
  `project.json` names a different root, or it has no readable `project.json`. Then the name is
  `<plain>-<8 hex of sha256(the canonical path)>`. Whether two names are the same is whatever the home volume says.
  The file inside decides whose folder it is, never the name.
- **R7.** A plain name longer than 200 bytes is cut at the last character boundary at or below 200 bytes and
  always gets the hash suffix.
- **R8.** A name is never parsed back into a path.

`projects/<name>/project.json`, mode 0600, written atomically:

```json
{ "version": 1, "id": "<uuid v4>", "root": "<canonical path>", "created_at": "<RFC 3339>" }
```

A re-point in flight adds `"previous_root"` and `"previous_name"` (§6). Nothing else writes those keys.

- **R9.** The file is the authority for the project's identity. There is no project table (R14).

## 3. Finding a project, and registering one

**No registry object.** `openalpaca_storage::store::projects` (new) is functions over files under the current home
root, read on demand, so the daemon, the CLI and the app shell agree and a test's home override holds:

- `lookup(root)` reads `projects/<plain name>/project.json` and, when that names another root,
  `projects/<plain>-<hash>/project.json`. Two small reads.
- `list()` scans `projects/*/project.json`. A project is **missing** when its root is not a directory.
- `register(root)` creates the folder and the file, create-if-absent, under one process-wide lock.

**Resolve — pure, creates nothing.** Input: the path a client sent. Canonicalise it; walk up; at each folder:

1. it is the root of a registered project → that project;
2. it holds `.git` (a directory or a file, so a worktree is its own project, as today) → that folder is the root.

The nearest match wins. No match → no project. A path that cannot be canonicalised (its folder is gone) is matched as
written against the registered roots, so a missing project still resolves to itself.

| Where the client is | Project |
|---|---|
| `/Users/tangyunpei/dev/AlpacaLink/src` (a git repository) | `/Users/tangyunpei/dev/AlpacaLink` |
| `/Users/tangyunpei/notes`, no git, never chosen | none |
| `/Users/tangyunpei/notes` after `Use` | `/Users/tangyunpei/notes` |
| `/Users/tangyunpei/notes/2026` after that | `/Users/tangyunpei/notes` |
| `/Users/tangyunpei` | none (R13) |

**Register — only the daemon, only where it accepts work for a root:** a chat turn, an upload, a memory write through
a route, and choosing a folder. A read never registers: the app's status poll and the Memory view resolve and create
nothing.

- **R10.** Choosing a folder (the app's `Use`, `openalpaca chat --project <dir>`) resolves it like any path. Inside a
  git repository that is the repository (ST-6). Only when nothing is found does the folder itself become a project.
- **R11.** Two folders carrying the same id (a copied folder), or a folder with no readable `project.json`: never
  opened, never deleted, reported once at WARN.
- **R12.** A root that resolves but is not registered — a git repository with no turn yet — has no store. Readers
  check registration before they build a project scope and treat such a root as an empty project: the Memory view,
  the doctor and `doctor --repair` answer from the home notes, as they do for a refused path today
  (`service/mod.rs:785-797`; `doctor.rs:133`; `commands/memory.rs:1721`). `store_root` for it is a typed error that
  creates nothing. A background job holding a stored root that is no longer registered drops its work by an explicit
  check, not by that error (§5, ST-21).
- **R13.** A root that is the home store, lies inside it, or contains it is never a project, so `$HOME` stays "no
  project" even with a `~/.git`. This replaces `resolves_to_the_home_store` (`memory/scope_context.rs:43-58`).
- `.openalpaca` is no longer a marker, and the walk's process-lifetime cache is removed (`memory/workspace.rs:11-39`,
  `:64-77`): a resolution is a few `stat`s and two small reads, and chat and the Memory view can no longer disagree.
- A stored root that is a registered project's root is used as given. Anything else is resolved as a fresh path.
- A project that only an earlier build's rows name has no folder under `projects/`. Its next turn registers it, and
  it appears in the list from then.
- In test builds, `register` under a home root that was not set through `OPENALPACA_HOME_STORE` fails loudly, so no
  test can write into a developer's real store.

**As built (rev 0.9).** `register` is called at five sites, not four: a chat turn, an upload and a memory write
through a route (`MemoryScopeContext::for_turn`, `memory/scope_context.rs:102`), choosing a folder
(`POST /v1/workspaces`, `projects::choose`), and the dispatch — a `rerun`, `start` or `resume` has no turn of its own,
so it records the run's root before anything of the run exists (`dispatcher/lead_agent.rs:239-241`; the plan's
conflict C4). A turn whose client sent no path has no project: nothing is resolved from the daemon's working
directory (the lead's ruling R-S13). The walk is the one above — the nearest folder at or above the path that is a
recorded project's root or holds `.git`, R13 ending it first — and R13 also judges a home store that is itself a
link by the folder holding the link (the whole-stage review's finding R4 — *review R4*; its numbers are not this design's rulings). A route sent a root a project records matches it
as written before anything is resolved (`projects::by_recorded_root`), so a project whose root now leads elsewhere is
still the one its row names (review R1); `lookup`, `store_root`, the dispatch's `register` and memory still take such a
root by its canonical form (§18). A folder that vanishes while it is read is no project, and `list` and `by_id` pass
it over (review R2). The test-build guard is a Cargo feature, `test-home-guard`, armed through `[dev-dependencies]` in the
five crates that reach `register`; its panic is a backstop — the rule is that every test that resolves, registers
or lists projects holds its crate's home-store guard, and no test reads `~/.openalpaca`, not even its metadata.

## 4. Where content goes, and how rows address it

- **R14 (identity in rows: path).** Rows keep the project's canonical path: `task.workspace_id`,
  `session.workspace_id`, `conversation_messages.workspace_root`, `lane_followups.workspace_path` (after F4),
  `file_assets.project_root`, and the absolute `file_assets.storage_path`. The id lives in the folder file and keys
  memory (§5). Not taken: ids in rows — a five-table migration, about 28 repository functions and about 60
  statements, to make a re-point one row where it is one transaction.
- `store_root(StoreScope::Project(root))` is `<home>/projects/<name>` by `lookup`; `content_dir` keeps its shape.
- **R15.** `file_assets.project_root` comes from the turn's project, not from where the bytes landed.
  `project_root_at`, which under ST-1 would name `~/.openalpaca/projects` for every project, goes, with the fix
  plan's R24 symlinked-store handling (`store/mod.rs:590-601`; `artifacts/mod.rs:906-915`; `uploads/mod.rs:220-225`).
  `artifacts_root_for` resolves through `lookup` and creates nothing (`artifacts/mod.rs:2823-2837`).
- **R16.** `storage_path` stays stored and absolute; a re-point rewrites its prefix (§6).
- **R17.** Upload dedup is scoped to the turn's project (one predicate, `uploads/mod.rs:352-356`). Today it matches
  across every store, so one project's turn can be handed a row that belongs to another.
- The CLI's `--file` upload sends the project header like its turn (`apps/openalpaca/src/client.rs:205-232`).
- `revealable_path`: a store file is one under the home root; the `.openalpaca`-ancestor rule goes
  (`store/reveal.rs:77-89`).
- In Stage 1, `OPENALPACA_HOME_STORE` isolates project content too: a daemon on a scratch home writes nothing into a
  real project folder. (Stage 2's `file_write` writes there by design.)
- **Between the stages.** `file_write` keeps Round G's rule, so a project turn's file is a by-product under
  `projects/<name>/artifacts/`, and its read-back path is now named for project turns too. The file tools' root is
  still the home store, so `file_read` can open anything under it but `state/`, other projects' folders included —
  as it can open the home `memory/` and `sessions/` today. Stage 2 ends that.

## 5. Memory

- A project's notes: `projects/<name>/memory/owners/<owner-key>/{notes/,.history/,MEMORY.md}`. The scope rules are
  unchanged.
- **R18 (the key).** The partition key is `proj-<owner-key>-<16 hex of sha256(project id)>`, so `is_partition_key`
  and `snapshot_path_for` hold as they are (`memory_files/mod.rs:201-211`; `partition.rs:576-586`). The control
  snapshot gains an optional `project_id`; a project partition opens only when the id in `project.json` beside its
  `memory/` equals the id in its snapshot. A project snapshot's `root` is no longer checked. The home partition is
  unchanged. The three places that build a fresh snapshot stamp the id — the first write, `repair` and `adopt`
  (`partition.rs:1053`, `:2593`, `:2974`). `repair` matters most: a project snapshot with no id is never opened
  again (R22).
- **R19 (the snapshot file format does not break).** `ControlSnapshot` refuses unknown fields and every snapshot
  written so far carries `"moved_from": null` (`snapshot.rs:157-178`). The field stays, parsed and never set;
  `project_id` reads as absent when missing and is written only when set, never as `null`, so a home snapshot stays
  readable to an earlier build; the schema number stays 1. One test: a snapshot as today's build writes it
  loads and binds.
- A re-point touches no snapshot and no index row. `begin_project_move`, `finish_project_move`,
  `classify_move_snapshots` and the doctor's stale / abandoned / orphaned states go (`partition.rs:2706-2845`).
- No code reads a project snapshot's `root` any more. Besides the `nth(4)` sites below, `adopt`'s read of its parent
  (`partition.rs:2913`) and the three `is_dir()` tests of it (`partition.rs:2712`; `service/mod.rs:895`;
  `doctor.rs:145`) take the project from `list()`, by id.
- `MemoryService::quiesce` stays: it is the lock the re-point and the delete take. Its slot is named by the
  project's id, not its path (`service/mod.rs:817-832`), so one slot holds under the old path and the new. It refuses
  rather than waits, and the 30 s reconciliation and an index rebuild take the same slot, so the two routes try for
  a few seconds before they answer `WORKSPACE_BUSY`.
- **R20 (`adopt`).** It finds each project partition's folder by the id in its snapshot through `lookup`/`list`,
  builds the new key from that id, and skips a snapshot that carries no id. Tests: adopt after a re-point; adopt
  beside an earlier build's snapshot.
- `memory/.gitignore` and its re-assertion go (`partition.rs:753-755`, `:997-1016`). "Known projects" come from
  `list()`; the four `ancestors().nth(4)` sites go (`partition.rs:2749`, `:2788`; `service/mod.rs:896`, `:1243`;
  `doctor.rs:150`).
- **R21.** The memory index's schema number goes to 6: every project partition's key changes, so an old cache is
  rebuilt from the notes.
- **R22 (earlier builds' snapshots).** A `proj-…` snapshot with no project id is never opened and never adopted. The
  doctor lists it. Nothing deletes it.
- **ST-21 (supersedes LQ5).** `root_gone` becomes "the root is no longer a registered project", checked before the
  model call at each site (`queue.rs:1365-1379`, `:2187-2196`; `extractor.rs:913`; `episode.rs:325`). It has to be an
  explicit check: a store error holds the cursor and retries, a model call each time (`extractor.rs:992-994`). Task
  extraction gains the same check before its call (`task_extraction/mod.rs:398-404`). A missing project's turns are
  learned into its notes; a deleted project's are dropped.
  *As built (rev 0.9):* the check is `extractor::check_project`, taken under `projects::lock()`: a root a folder
  records is learned, a missing project's included; a root an unfinished re-point names as its old or its new one,
  or whose project seen before the call is still listed under its id, is **held**, and so is every root while
  `projects/` cannot be listed; a root nothing listed names is dropped — a deleted project's, and also one never
  registered, the rows of an earlier build (the switch-over note says how to keep them). A memory write for a
  project re-pointed or deleted mid-request is refused, never filed in the home partition (§9's *As built*).

## 6. A project's life

| Action | App | CLI | Route |
|---|---|---|---|
| Choose a folder | Settings → Connection → `Use` | `chat --project <dir>` | `POST /v1/workspaces {path}` |
| List projects | Settings → Connection → Projects | `store list` | `GET /v1/workspaces` (no `path`) |
| Re-point | `Re-point…` on a project | `store rebase <old> <new>` | `PATCH /v1/workspaces` |
| Delete its data | `Delete data…` on a project | `store purge <project>` | `POST /v1/workspaces/purge` |

**Missing (ST-7).** A project whose root is not a directory is listed as missing, and its data is kept. Its
conversations still open and continue. (In Stage 2 its file tools and shell are refused with one sentence naming the
two actions.)

**Re-point** — example: `openalpaca store rebase /Users/tangyunpei/dev/AlpacaLink /Users/tangyunpei/work/AlpacaLink`
renames `projects/-Users-tangyunpei-dev-AlpacaLink` to `projects/-Users-tangyunpei-work-AlpacaLink`.

Refused: while the project has a run in flight (`WORKSPACE_BUSY`); when the new path is not a directory (new: today
a mistyped path is accepted and created); when the new path is already a registered project or any of the five path
columns names it (`WORKSPACE_EXISTS` — ST-20; wider than today's check). Then, holding `quiesce`:

1. `project.json` is rewritten in place with `root` = the new path, `previous_root` and `previous_name`.
2. One transaction rewrites the old path to the new one in the five path columns — the two the re-base misses today
   included — and both `storage_path` prefixes, each scoped by `project_root`: `projects/<previous_name>/` and, for
   Stage 2's pointer rows, the old project path. A failure here clears the marker and reports; nothing else changed.
3. `projects/<previous_name>` is renamed to the new name. The target is free by construction (R6).
4. `project.json` is rewritten without the two keys.

- **R24 (recovery).** At boot, before any resolution, a `project.json` carrying `previous_root` is a re-point that did
  not finish: steps 2–4 run again, each idempotent. A recovery that fails on I/O is reported and tried again at the
  next boot. The re-base's crash-injection hook is re-cut to these four steps.

**Delete its data (ST-19).** Refused while the project has a run in flight. A plan is shown first and nothing is deleted
without confirmation (`dry_run` stays the default): conversations, runs, by-products, uploads, and the notes of every
owner. Then, holding `quiesce`:

1. A file of this project that a message of a surviving conversation still links is kept, an upload or a by-product:
   its file is moved to the root store under a fresh address and its row re-addressed with no project. A kept
   by-product's older versions go with the project. The plan counts these.
2. One transaction deletes the project's rows: every row that names the project in one of the five path columns, and
   the rows that hang off its sessions and runs, `learning_cursor` included. A queued follow-up that names the
   project goes even when its session is another project's; today's statements, keyed by session and run, would leave
   it to start later (`artifacts/mod.rs:1878-1926`).
3. The sessions' directories go from `sessions/`; each of the project's partitions is dropped from the memory index
   (`MemoryIndex::remove_partition`) and its control snapshot removed from `state/memory-control/`; a deleted
   conversation's summary that lies in another partition is forgotten, as today; `projects/<name>/` is removed as a
   directory.

The delete never iterates `storage_path`. Outside `projects/<name>/`, `sessions/`, `state/memory-control/` and the
index it does two things only: it places the files step 1 keeps in the root store, and it forgets the summaries of
step 3 through the memory service. `store purge --all` and the route's `all` form are removed.

## 7. What Stage 1 removes (each with its last caller)

`store/project_move.rs`; the project half of `ensure_store` and `ensure_layout`; `PROJECT_README`; `GITIGNORE_TEXT`;
`PROJECT_GITIGNORE_TEXT` and `assert_gitignore`; `recorded_project_root` / `set_recorded_project_root`;
`project_root_at`; `project_root_of` (no caller); `resolves_to_the_home_store`; `unknown_entries` for a project
store; the `.openalpaca` marker; the walk's cache and the uncached resolver pair; the memory move functions,
`MemoryService::begin_project_move` / `finish_project_move` (`known_project_roots` stays — *As built* below), the doctor report's
`move_snapshots` and the CLI code that prints it; `MoveStep` and `rebase_workspace_with` (replaced by §6); the purge's
upload-unlink loop and `upload_paths`; the app's `useMovedProject` and `MovedProjectOffer`; the messages that tell a
user to `mkdir .openalpaca`; memory fixtures 03, 04 and 17 are rewritten. `is_the_home_root` stays: `ensure_store` calls it
(`store/mod.rs:464`), and Round G's `file_write` rule calls it until Stage 2.

**As built (rev 0.9).** Stage 1 removed the list above with these differences, each a ruling of the plan's:
`MemoryService::known_project_roots` **stays**, read from `projects::list()` — each listed project's own folder —
because the 30 s reconciliation calls it (`reconcile_owner`; the lead's ruling R-S3); `project_root_of` stays — R15
gives it two callers, the artifact and the upload writers (the plan's conflict C3); `unknown_entries` went whole,
with `ContentKind::ALL`, `GITIGNORE_FILE` and `VERSIONS_DIR`, its last users, because its one caller was the purge
plan; R2 removed `ContentKind::Skills` and `ContentKind::Cache`; `is_the_home_root` stays for Round G's `file_write`
rule alone (`file_ops.rs:335`; `ensure_store` seeds the home store only and no longer calls it); and fixtures 01,
02, 07, 08, 09 and 24 were rewritten too (they named `.openalpaca` as a marker or as the project's folder), beside
03, 04 and 17.

## 8. Existing data, the database, reset

- **ST-9.** No code reads an in-project `.openalpaca/`. The switch-over note says what such a folder holds —
  by-products, uploads and, on a store used with this branch's builds, the project's memory notes — that none of it
  is read again, and that it can be deleted by hand. A note can be carried over by hand: a note file placed in the
  new partition's `notes/` is recorded by `openalpaca memory doctor --repair` as an outside edit (origin `external`);
  its origin and its forget history are not carried.
- **R25 (migration `003_no_project_store_rows.sql`).** Registers 3 and deletes the `file_assets` rows whose bytes lie
  inside a project's own folder: `project_root IS NOT NULL` and `storage_path` begins with `project_root` followed by
  a path separator, `/` or `\`, compared with `substr`, not `LIKE`. That holds whatever the store directory was
  called (a store reached through a link is recorded under the link's target), and no later row matches until Stage
  2's pointer rows, which are written after the migration has run. Their version rows and every attachment link go
  with them by cascade, a link from another conversation included (the dedup of R17 could hand one project's row to
  another's turn); that message keeps its text and shows the file as unavailable. Such rows exist only in a store
  that was used with a project on this branch. Left in place they would share the new project's address space.
- A factory reset leaves `projects/` in place, as it leaves `artifacts/` and `memory/`, and the preview says so (a
  Rust string with tests). Project ids survive it, so `openalpaca memory adopt` re-attaches a project's notes.

## 9. Surfaces and wire (Stage 1)

| Route | Request | Answer | Refusals |
|---|---|---|---|
| `GET /v1/workspaces` (new; `400 MISSING_PATH` today) | — | `[{id, root, name, missing}]` | — |
| `GET /v1/workspaces?path=` | a path | `{path, root, id, name, missing, rows, active_tasks, queued_tasks}`, with `root: null` when there is no project; `store_present`, `recorded_root`, `moved` are gone | — |
| `POST /v1/workspaces` (new) | `{path}` | the project | `WORKSPACE_IS_HOME` (R13), not a directory |
| `PATCH /v1/workspaces` | `{old_path, new_path}`, as today | the project; `store_moved` is gone | `WORKSPACE_BUSY`, `WORKSPACE_EXISTS`, not a directory (new); `WORKSPACE_MOVE_BLOCKED` and `WORKSPACE_NOT_A_ROOT` are gone |
| `POST /v1/workspaces/purge` | `{path, dry_run}` | the plan, or what was deleted; `entries`, `kept`, `removed` change shape | `WORKSPACE_BUSY` |

- **CLI.** `store list`, `store rebase`, `store purge <project>`; `chat --project <dir>`. `store rebase --dry-run`
  keeps its row and run counts, read from the `?path=` answer.
- **App.** `Use` makes a folder with no git a project; the card shows the resolved root, not the typed text; a
  Projects list with a missing badge, `Re-point…` and `Delete data…`; the Memory view's copy stops naming
  `.git or .openalpaca`.
- **Text.** The home README, four manuals, `API_MAP.md`, `DESIGN_SPEC.md`, `CLAUDE.md`, the factory-reset preview,
  and `artifact_write`'s description, which names `<project>/.openalpaca/artifacts` today.

**As built (rev 0.9).** The wire shipped as the table says, with these additions. Two new codes,
`422 WORKSPACE_NOT_A_DIRECTORY` and `500 WORKSPACE_REGISTER_FAILED`; a registration failure answers the same on every
route that registers (the lead's ruling R-S7), though only the workspace routes can answer `409 WORKSPACE_IS_HOME` —
an upload or a memory write from the home store is no project and goes home. The purge keeps
`422 WORKSPACE_NOT_A_ROOT` for a path inside a project and takes the root exactly (R-S8; T36 in §0.2 asks whether
`PATCH` should too); `PATCH` and the purge answer `404 WORKSPACE_NOT_FOUND` for a root no folder records or a project
holding another owner's rows (the fix plan's R40); a re-point onto the project's own path is `400 INVALID_PATH`. `rows` also counts
`messages` and `followups`; `PATCH` answers `{old_path, new_path, moved, project}`; the memory slot is tried every
100 ms for 3 s. The whole-stage review's fix wave and its residual pass added (*review R…* and X…): a root a project records, sent as
written, is matched before anything is resolved and answered as sent (review R1); `409 WORKSPACE_BUSY` for a project whose
re-point has not finished, naming the next start (review R11), for a project re-pointed or deleted while the route waited
for its slot (review R15), and for a run queued, running or paused under the root, counted again inside the re-point's
transaction as the delete already did (`project_life::RunsInFlight`, review R10); a re-point onto either path of an
unfinished re-point is `409 WORKSPACE_EXISTS` naming that start (`ProjectError::RepointUnfinished`, X7);
`500 IO_ERROR` on `GET ?path=`, `PATCH` and the purge while `projects/` cannot be listed (X1); `memory_store`,
`/remember` and `POST /v1/memories` for a project re-pointed or deleted mid-request are refused — the route's
`500 MEMORY_INTERNAL` — rather than saved in global memory (review R19); and the handler is handed the root the gateway
recorded, not the raw path (review R7). The app keeps `{root, id}` in `oa-project` and follows its project by id to a root a
re-point made elsewhere (review R22); an upload into a conversation bound to a project is filed under that project, in the
app and the CLI (review R6); a re-point or a delete refreshes the follow-up queue too (review R24); a content write creates the
folders below a project's folder one level at a time and never re-creates the folder itself (`store::create_below`,
review R16); and the delete removes the folder straight after the rows, `project.json` last (review R3).

---

# Part II — Agents work in the project (Stage 2)

**What you will see after Stage 2.** In a project conversation an agent can list, read, write and edit the project's
files and run commands there. Each write and each command asks first. A file it writes for you is in your project, with a
card in the chat and a row in the Library that point at it. Drafts and hand-offs stay under `~/.openalpaca`.

## 10. The working folder of a turn

- With a project: its root. With none: `~/.openalpaca/scratch/<session-id>/`, created on first use and removed with
  the conversation, so one conversation never sees another's files.
- **R26.** The tools read the folder from the call's context (`ToolContext.request_workspace_root`), which every
  sandboxed call carries (`tools/registry/mod.rs:76-87`). `file_read` and `shell_execute` gain
  `execute_with_context`. The shell, which today runs wherever the daemon was started, is spawned in that folder. The
  file tools' boot-root constructor parameter goes, with `file_write`'s use of `is_the_home_root`
  (`services/tools.rs:52`; `shell_execute.rs:29`; `platform.rs:8-17`). A call with no context is refused. Command
  tools get the folder too: the dispatch hands the context to `execute_command` (`tools/registry/mod.rs:753-784`,
  `:1368-1403`). Skill scripts keep the skill's own directory.
- **R44.** Every path that runs a sandboxed tool puts the turn's session id on the context. The skill tier, nested
  skills, plugin skills and plugin agents leave it unset today (`agentic_loop/mod.rs:1095-1103`;
  `skill/invocation.rs:763-781`, `:1261-1274`), and without it a call with no project has no scratch folder to name.
  A call with neither a project nor a session id is refused.
- Work that carries no project works in a scratch folder: a connector lane, a scheduled skill, and a run parked by
  `POST /v1/tasks` with no project header (`scheduled_skills.rs:208`; `routes/tasks.rs:132`).
- The daemon's own directory stays where the launcher put it. MCP servers and the CLI model fallback cannot follow a
  turn and keep it (`stdio.rs:69-71`; `cli_backend/mod.rs:93-95`).
- **R27.** A missing project is refused before any tool runs, with the sentence of §6.
- **R28.** The model is told its working folder: one line in the turn's context, on the main loop, the lead, the
  subagent spawn and the skill tier (`prompt_ctx/sources/workspace.rs:25-32` is a stub today, and its gate admits
  only the lead).

## 11. Confinement, and what it cannot do

**File tools — real.** Relative paths only, no `..`, the resolved path under the root, as today. Added:

- **R29.** The sanitizer's `../` rule stops applying to the builtin tools. Today it refuses any string of any tool —
  file content, a workflow goal and a subagent objective included (`security/sanitizer/mod.rs:94-122`;
  `sandbox/mod.rs:252-263`). A builtin's path arguments have their own refusal of `..` (`helpers/mod.rs:11-24`) and
  its other arguments are content. The rule stays for script, command, MCP and plugin tools: their arguments reach a
  shell line or another program, and one shipped script relies on it
  (`config/skills/create-skill/scripts/write_skill_md.sh:79-90` builds a path from `--id` with no check of its own).
- **R31.** The write resolver resolves the last path component like every other, as the read resolver already does,
  so a `file_write` through a link that leaves the root is refused (`helpers/mod.rs:44-57`, `:80-133`; the test at
  `file_ops.rs:935-956` flips). Today such a write replaces the outside file, and no copy is kept.
- **R32.** In a project, a path with a component named `.git` or `.openalpaca`, compared without case on the resolved
  path, is refused for writing; `.openalpaca` also for reading (ST-9). This binds the file tools only.
- **R33.** The five rules written for the home store — `state/`, `.history/`, `MEMORY.md`, `memory/.gitignore`, the
  persona file names — go from the file tools: from a project or a scratch folder nothing they protect is reachable
  (`helpers/mod.rs:176-210`; `file_ops.rs:292-313`). `is_store_owned_path` stays for `workspace_write`'s key check
  (`tools/builtins/mod.rs:183`).
- **R23.** `send` with a file takes a path resolved like `file_read`'s (relative, under the working folder) or an
  artifact id; the card names the resolved path (`tools/builtins/send.rs:58-101`).

**The shell — a working directory, not a confinement (ST-14).** A command starts in the project. An absolute path, `~`
or `cd /` reaches anywhere. An approved command runs as the user: it can read `state/discovery.json` and answer the
prompts after it, edit `daemon.toml` to switch prompting off, and read the key file. Only an operating-system sandbox
changes that. The owner accepted this for now (ST-14): every `shell_execute` command asks for approval, the card
names the folder it runs in (R34), and sandbox methods are researched for a later version (§18).

- **R45.** A shell command, a command tool or a skill script that times out, or whose run is cancelled, is killed,
  with the processes it started where the platform allows (`shell_execute.rs:28-32`; `registry/mod.rs:1402-1404`;
  `builtins/mod.rs:561-564`). Today it keeps running after the tool reported it stopped — after Stage 2, in the
  user's project.
- A command tool the owner defines under `config/tools` asks only when its own file declares it destructive, and it
  cannot name its folder on a card (`tools/config/mod.rs:149`; `registry/mod.rs:667-676`). That is as today; none is
  shipped (`config/tools/` holds `example.toml`). It is recorded with T35.

## 12. Approvals

- **R30.** F1 is the fix for T34. The rule that an explicit `require_confirmation_for` wins stays
  (`sandbox/mod.rs:754-771`).
- **ST-15.** `artifact_write` and `workspace_write` stop prompting: one line each in `annotations_for_builtin`
  (`tools/builtins/mod.rs:602-605`; the test `annotations_for_builtin_destructive_tools_tagged` flips for the two).
  `file_write` and `shell_execute` ask on every call, wherever the file lands: with no project `file_write` saves a
  by-product and still asks, as today. The rule is by tool name and is decided before the tool runs
  (`sandbox/mod.rs:280`).
- **R34.** The card names the folder a write or a command acts in and, for `file_write`, whether the file lands in
  the project or as a by-product, through the preview hook (`tools/registry/mod.rs:295-306`).
- **R43.** "Always allow" keeps today's scope: one tool, one subagent.
- **ST-16.** `file_read`, `file_list` and `artifact_read` are read-only and never prompt.
- An unattended turn refuses a prompting tool at once and says so in the report, as today.

## 13. Results and by-products (ST-11, ST-12)

**In a project.** `file_write` writes the real file under the project root and records a pointer row.
`artifact_write` saves a by-product under `projects/<name>/artifacts/`. **With no project**, both save an artifact at
the root, as Round G built.

**The pointer row** — `file_assets` with a new column `in_project INTEGER NOT NULL DEFAULT 0` (migration
`004_project_files.sql`, Stage 2), and the unique address index rebuilt as
`(COALESCE(project_root,''), in_project, rel_path)` with `DROP INDEX IF EXISTS`:

| Column | Pointer row |
|---|---|
| `origin` | `produced`, set on insert and on update — never the column default `upload`, which the orphan sweep deletes |
| `status` | `ready`, so the text extractor never copies the file into the database |
| `in_project` | `1` |
| `project_root`, `rel_path` | the project root, and the path relative to it — an exact suffix of `storage_path` |
| `storage_path`, `filename` | the real file's absolute path; the project-relative path |
| versions | none: no `artifact_versions` row, nothing under `.versions/` |

- **R35.** Written by a new `ArtifactStore` method keyed on `(project_root, in_project = 1, rel_path)`: it inserts
  or updates that row and announces `artifact_written`. It never calls `put`, whose protocol would move the user's
  file into `.versions/` and remove an empty file at the target name (`artifacts/mod.rs:2573-2590`, `:2746-2762`).
  `put`'s identity scan, `dir_rows` and `verify` filter on `in_project = 0`.
- **R36.** If the file is written and the row cannot be, the tool reports the write as done and says the app could
  not list it. The file is the result; the row is a view of it.
- **R37.** Every reader of a pointer row goes through the resolver of R31, under the row's `project_root`. The
  read-time check refreshes hash and size and writes no version; `missing_since` is cleared when a read finds the
  file again; History and Diff are not offered.
- **ST-17.** Pre-edit copies for an overwrite: kept, under `sessions/<session>/snapshots/`. Today the tier judges the
  target against the boot root and skips anything under the home store, so it is re-pointed at the call's working
  folder (`file_ops.rs:152-237`). Its two refusals stand: replacing a file larger than
  `[orchestrator.sessions] snapshot_max_bytes` (10 MiB by default) is refused, and so is a write whose copy fails.
- **R46.** A copy needs the loop's session log. Where a loop has none — the skill tier, nested skills, plugin skills
  and plugin agents today — `file_write` creates a new file but refuses to replace an existing one, and says why.
- **R38.** A run's count, its completion chips and connector delivery count by-products only; result files are
  listed as "written in the project". A connector is never sent a project's files automatically.
- **ST-18.** Open on a pointer row works for the extensions the artifact store keeps
  (`kind_allowed_extensions`, `store/artifact.rs:412-434`). Anything else gets Reveal only: `.sh`, `.command`,
  `.svg`, `.exe`, and every name with no extension, such as `Makefile`, `Dockerfile` or a dotfile.
- **Surfaces.** The FILE WRITTEN card, the Library (a `project` badge and the relative path), Open and Reveal work on
  a pointer row. `revealable_path` also admits a file under a registered project's root, read from `list()`. A run
  that writes forty files draws forty rows; the Library's default view shows by-products, with a filter for results.

**The tests that must exist (R39).** The fixture row is written by R35's method, never a hand `INSERT`; each test
asserts the user's bytes and modification time.

| Test | Fails when |
|---|---|
| The orphan sweep leaves a pointer row's file | the pointer insert omits `origin` |
| Deleting a project's data leaves its real files | the delete iterates `storage_path` |
| A second `file_write` of a path updates only its own row | the lookup ignores `in_project` |
| `put` never adopts a pointer row at an artifact-shaped path | `put`'s scan ignores `in_project` |
| A pointer path equal to an upload's address leaves the upload row alone | the index or the lookup ignores `in_project` |
| A user's edit of a result file records no version | `verify` or the read check ignores `in_project` |
| A re-point rewrites a pointer row's path and leaves the file | step 2 misses the second prefix |

**Editing (ST-22).** `file_edit` changes part of one file in the project.

- **R47 (the tool).** Arguments `path`, `old_string`, `new_string` and `replace_all` (default false), named as Claude
  Code names them. The path is resolved like `file_write`'s (relative, under the working folder, R31, R32). The file
  must exist and be UTF-8 text; `old_string` must occur exactly once, or `replace_all` must be set; `old_string` and
  `new_string` must differ. Each failure is a refusal naming the cause, and a missing file's names `file_write`,
  which creates one. The rest of the file is kept byte for byte, line endings included. The write goes the way
  `file_write`'s does: the pre-edit copy first (ST-17, its two refusals, and R46 where no copy can be kept), then the
  file, then the pointer row through R35's method. The result names the file, the number of replacements and the
  line of the first.
- **R48 (where, and who).** In a project only; with no project it is refused with one sentence naming `file_write`.
  It comes with the `file_write` capability, so the four shipped templates that write files hold it with no template
  edit and a template that denies `file_write` denies it too. The main chat is offered it beside `file_write` on a
  project turn, by the same keywords; the lead does not hold it (§15).
- **R49 (the card).** It asks like `file_write` (destructive annotation). The preview names the folder, the file and
  the line, and shows `old_string` and `new_string`, each cut to a budget with the cut notice, hidden characters
  escaped as every approval surface escapes them (Gotchas).
- **Tests (with R39).** `old_string` found twice without `replace_all` → refused and the file unchanged; a missing
  file → refused, naming `file_write`; the copy exists before the change; a second edit updates the same pointer
  row; a U+202E in either string is shown escaped on the card; a CRLF file keeps its line endings.

## 14. Reading

- **R40 `file_list`** (new, read-only): lists a folder under the working folder. Today no tool lists a directory.
- **R41 `artifact_read`** (new, read-only, by id, modelled on `read_result`): reads a by-product of the turn's
  project or of no project, for the turn's owner; it refuses a pointer row, which is `file_read`'s. `artifact_write`'s
  result, a workspace entry of type `artifact` and `task_status` name the id. Round G's `read_back_path` and its
  pinned test go.
- **R42 (who gets them, ST-16).** `file_list` comes with the `file_read` capability and `artifact_read` with
  `read_result`, which all nine shipped templates name, so a seeded install gets both with no template edit and a
  plugin's template only by naming them. The lead gets `file_list`, `file_read` and `artifact_read` on its
  hand-built surface in code. The main chat gets `artifact_read` on every turn — a globally registered tool whose
  definition is put on the main loop's surface per request, as `read_result`'s is — and `file_read` and `file_list`
  on a project turn.

## 15. Who holds which tool

- **The main chat** writes and runs commands by keyword as today (`intent/tool_suggest.rs`; under
  `tool_selection = "full"` it holds every tool on every turn). The relay rules and
  `start_workflow`'s description stop saying chat cannot write a file (`main_loop.rs:111-114`;
  `start_workflow.rs:178-181`).
- **The lead** holds no file-writing or editing tool and delegates result files. Its `workspace_write` and, under the shipped
  template, `artifact_write` write by-products and stop asking (ST-15).
- **Subagents** keep their templates' capabilities. `writing_agent` gains `file_read` in the shipped template; the
  switch-over note has the line for a seeded store.
- The tool descriptions and result strings that ST-11 makes wrong are rewritten: `artifact_write` is for by-products,
  and "this file is the deliverable" is said only with no project.

---

# Part III — Building it

## 16. Order

**Rules.** A removal lands with its last caller. Every slice that changes a route regenerates `docs/api` in its own
commit. Every slice that changes a type the evaluation harness uses runs
`cargo check -p openalpaca_core --features live-eval --tests` — compile only; the harness is never run as a gate.

**Stage 0.** F1–F4. One gate pass; a live check on the rebuilt daemon through its HTTP API (the app runs the same
daemon), isolated and sandboxed as below: a delegated
`file_write`'s report names the artifact's path (F2), `system_agent`'s shell command raises a card (F1), and a spawned
shell shows no master key (F3).

**Stage 1.** (1) `store::projects`, additive: the name, `project.json`, `lookup`, `list`, `register`; nothing calls
it. (2) The resolver and registration (§3), content still in the project. (3) The flip, in one commit set: `store_root`,
R15, R17, the `nth(4)` sites, `revealable_path`, the purge guard, migration `003`, every fixture, and an audit that
every test passing a project path holds the home-store guard. (4) Memory: the id key and binding, R19–R22, the re-point
that replaces the move machinery. (5) Life cycle: the routes and the CLI, then the app. (6) Text and records. One gate
pass; a live check on the rebuilt daemon through its HTTP API and the CLI (isolated home, the daemon under
`scripts/eval/memory/loopback-only.sb` from its first launch); the app's surfaces by their vitest suites, the session
having no screen access.

**Stage 2.** Guards before the root moves: (1) confinement, the filter and the kill (R29–R33, R23, R45); (2) approvals (ST-15, R34);
(3) the working folder (R26–R28, R44) — until (4) lands, `file_write` on a project turn stays a by-product; (4) pointer
rows, migration `004`, R39, the pre-edit copy (ST-17, R46) and `file_edit` (ST-22, R47–R49); (5) `file_list`, `artifact_read`, R42; (6) surfaces and wording. One gate pass; a live
check in a project folder made for the test, which includes reading a by-product back from the main chat (R41) and
one more try at a click on a stale approval card (§0.4).

## 17. Records

`tasks/api-fix-plan.md` §0 (ST-1…ST-22, T34 and FW-2 moved to taken, T35 added as pending),
`CLAUDE.md`, `tasks/memory-redesign/03-design.md` (MD-5's marker half and MD-7 are superseded),
`tasks/memory-redesign/09-learning-design.md` §10 (LQ5 is superseded by ST-21), the vault (ADR-047),
the manuals and the switch-over note. `.gitignore` whitelists `tasks/storage-design/*.md`; `maps/` and `panel/` stay
untracked evidence.

## 18. Recorded, not built

Research on sandbox methods for shell commands, for a later version (ST-14): what confines a command to the
project on each platform, and what it costs. Merging two projects on a re-point (ST-20). **T35**: an extension
tool that does not declare itself harmless prompts — three options (no change; MCP only; MCP and plugins), with the
consequence that an unattended turn using such a tool is refused. A run-wide "Always allow" (R43). Stage 1's two
owner questions are answered (§0.2): **T36** keeps the re-point as built, and **ST-23** — a root that no longer leads to
itself reads missing — is built in Stage 2's S0.

From Stage 0, each found by its review or its live check and left as it is:

- A lead run with no session log cuts a long completed subagent answer head-only, although the lead's prompt and
  `check_subagent_status`'s description promise a stub to page with `read_result`. Rare: every run started from a
  chat or a follow-up has a session log; the gap is a run parked by `POST /v1/tasks` on a lane with no session. For
  Stage 2's wording pass.
- `request_workspace_root`'s list of readers in `ToolContext`'s doc omits the per-turn memory path
  (`prompt_ctx/sources/memory.rs`). The collapse of the three workspace fields recorded at
  `tools/registry/mod.rs:27-28` owns the reader lists.
- Since F4 a follow-up the lead queues carries the run's project root, and the row is pinned to the lane's active
  session when it is queued (`repository/followup/mod.rs:111-118`): when it runs, a session bound to another project
  is archived and a new one opened (R48), and an unbound one becomes bound to the run's project. Before F4 (NULL) it
  joined the pinned session as it was. The remedy, for later: pin a lead-queued follow-up to `task.session_id`, as
  `recover_unprocessed_steering` already does (`:265-268`). Recorded beside T18.
- `wait_for_subagents` can lose a wakeup: the loop checks `all_done()` (`runner/lead_agent/tools.rs:1205`) before it
  creates the `notified()` future it waits on (`:1232`), and `notify_waiters` keeps no permit, so a last completion
  between the two leaves the lead idle until a steering message or the 600 s cap. Older than Stage 0; offered to the
  owner as its own task. **Fixed** in `32292608` (2026-10-07): the `Notified` future is created and enabled before the
  `all_done()` re-check.

From Stage 1, each found while planning, building, reviewing or checking it, and left as it is. The source is the
plan's ruling (R-S…), the lead's ruling on the final review (R-F…), a slice's review (S1…S5b), the whole-stage
review's finding (written *review R1…R33*, apart from this design's own rulings R1…R49) or the residual pass's (X1…X10, B1…B4, A-N2).

*Registry and resolution*

- **A control snapshot whose project id names no registered folder** — a project's folder under `projects/` removed
  by hand, or one of R11's two folders with one id, which `list()` leaves out — is skipped by the rebuild and the
  doctor and not listed by the doctor (`IdlessSnapshot` lists only id-less snapshots); `adopt` skips it with a WARN.
  Recorded, not built (the lead's ruling R-S11).
- **R11 is kept by `list` and `by_id`, not by `lookup`.** `lookup` reads two folder names, so it cannot see a copy of
  a project's folder elsewhere under `projects/` and still answers for the original; two folders recording one root
  under *different* ids (a copy whose id was edited) read as the plain-name one by `lookup` and as both by `list`.
  `still_the_project`'s `by_id` finds nothing for an id two folders carry, so a re-point or a delete of such a project
  answers that it was "re-pointed or deleted" while it waited.
- **A stranded `projects/.<name>.<pid>-<n>.tmp/`** after a crash in the middle of `register` is hidden, passed over by
  `list` and never swept (a few hundred bytes; a sweep would be a deletion path). Recorded, not built (the lead's
  ruling R-S14).
- **Case-insensitive home volumes** keep R6 by construction (`name_for` and `lookup` read the folder through the
  volume) but no test covers a case-sensitive project volume beside a case-insensitive home.
- **Review R1's deeper cross-wiring** (R-F7): only the routes match a recorded root as written; `lookup`, `store_root`, the
  dispatch's `register` and memory's resolution still take a root that now leads elsewhere (a case-only rename, a
  link left at the old path) to the project its canonical form names, so that project's follow-ups write into the
  other's folder and partitions. `missing` stays "the root is not a directory". **Taken by ST-23 (2026-10-09):** a recorded root is matched as written
  before it is resolved, and one that no longer canonicalizes to itself reads missing — to be built in Stage 2's S0.
- **Review R4's other variant** (R-F7): with `OPENALPACA_HOME_STORE` outside `$HOME`, a `~/.git` makes `$HOME` a project.
  Older than Stage 1.
- **A root that is not valid UTF-8** is held through `to_string_lossy` and can alias another (S1 M4; Linux; the shape
  of every path column).
- **An absolute path whose missing tail ends in `..`** is refused by `for_turn` rather than resolved to no project
  (S2 M1).
- **The test guard is a backstop** (the plan's interfaces §7.4): an unguarded test that calls `register` while
  another test of its crate holds that crate's guard passes the check and writes into that test's temp store,
  unreported; a `~/.openalpaca` that is itself a link, with the variable set to the link's target, is not recognised
  as the real store. The guard audit, not the panic, is what makes a test guarded.
- **While `projects/` cannot be listed**, every `GET /v1/workspaces?path=`, purge and `PATCH` answers `500 IO_ERROR`,
  for a path that is no recorded root too (X1, failing closed); a turn's own resolution is unchanged. The route's
  error arm for that listing has no route-level test — only a transient failure reaches it, and its storage twin is
  pinned (B4).
- **A project whose recorded root no longer opens** is listed, but the 30 s reconciliation skips its partition
  (logged at debug, X6; A-N2).
- **`openalpaca memory --project`** reads the home store's registry itself for a missing absolute root (X9), so a CLI
  and a daemon started with different `OPENALPACA_HOME_STORE` can disagree, as the local verbs already can.

*The re-point*

- **After a live re-point's rename fails past step 2** (S3's re-review): `lookup` of both roots answers `None` until
  the next boot, and a turn at the old root can still register a fresh project there, whose rows recovery's step 2
  would then merge; learning holds the whole session's cursor until that boot (S4).
- **Recovery before step 3 chooses the new folder name again** (`name_for`, `project_life.rs:292-296`) rather than
  reading the name step 2 wrote into the `storage_path` prefixes; if the plain name was taken in between, the two
  differ (S3 M1).
- **`store rebase --dry-run`** advises "delete that project's data first" for a target that is an unfinished
  re-point's root, where the daemon answers `RepointUnfinished`'s `409` naming the restart (B1).
- **A turn's file writes during a re-point** (review R13's file half): `file_write` and `artifact_write` on the old root are
  refused "… is not a project OpenAlpaca knows", which a model may route around through the shell.

*Memory and learning*

- **A write for a root no folder records** is refused before it takes a slot (review R19); the slot
  `unregistered:<canonical root>` is left only to keep two such writes apart, and a lookup that fails takes it with a
  WARN, so that write does not wait for a re-point's or a delete's hold (`memory/service/mod.rs:828-856`, S4 M1).
- **`root_gone` reads a failed `lookup` as "not gone"**, so while `projects/` is unreadable each learning retry spends
  a model call before the write's own error holds the cursor (S4).
- **Task extraction across a re-point or a delete** (R-F4's remainder): a run re-pointed before its pre-call check is
  `Skipped(root_gone)`; items re-pointed during its model call are refused, counted failed and lost — a run has no
  cursor; and the delete half's verify-then-resolve window stays. Closing it needs the extraction descheduled across
  the delete, or the dispatch's project id threaded through the dispatcher.
- **Review R7's narrower window**: the handler resolves the root the gateway recorded once more, so a re-point committing
  between the two leaves the turn with no project and no stale root, and its unscoped notes land home as a no-project
  turn's do.
- **The stale-root refusal's shape**: `POST /v1/memories` answers it `500 MEMORY_INTERNAL`, not a typed `409` (that
  would be a wire change), and the model reads it with the `io:` class prefix.
- **A deleted conversation's summary in a third project's partition** stays until forgotten by hand: the delete
  forgets its conversations' summaries in the home partition only, and the project's own go with it (R-F1, SC-1).
- **The delete publishes no `memory_changed`** for the partitions it drops (the event names one note): the window
  that deleted invalidates its own Memory queries (S5b); an open Memory view in another window keeps showing the
  deleted notes until a later `memory_changed` or a fresh read — the 30 s reconciliation scans the home partition
  and the registered projects' only, so it announces nothing for a deleted project, and the view's queries do not
  poll. Recorded, not built (the lead's ruling R-S14, whose "refreshes at the next reconciliation" the code does not
  bear out).

*The delete*

- **The delete's edges** (S5a): a chat turn — not a run — in one of the project's conversations is not refused, so
  its write between the transaction and the folder's removal is lost with the folder, and its answer leaves an
  orphan message row (review R17; closing it needs a per-lane in-flight-turn signal); a project only an earlier build's
  rows name cannot be deleted until a turn registers it (`404`); the delete and the re-point do their database and
  file work inside the async handler, and registration runs inline on the runtime at the chat turn and the upload
  (S2).
- **Review R16's remaining window** (R-F7): a put whose whole transaction lands between the delete's commit and the
  folder's removal leaves a row of the deleted project, answered `410`. **Review R3's**: an entry created in the folder after
  its listing fails `remove_dir` once `project.json` went, leaving a folder no list shows (the hidden-rename variant
  was not taken).
- **Kept files** (S5a): empty `kept/<folder>-<id>/` folders after a failed attempt; a target that NFD or case folding
  changes is refused (`kept_address`'s `target != joined`, `project_life/delete.rs:279`); a `written: None` row can
  adopt a kept copy a crash cut short; and `copy_fresh`'s refusal of an address a committed row names is permanent
  until resolved by hand (`500 DB_ERROR` naming the address).
- **No daemon field tells "the folder was kept" from "it was already gone"** (R-F2): the toast and the CLI line cover
  both, and when it was already gone the toast's "run Delete data… again" has nothing to press.

*The app and the CLI*

- **The CLI's counts print "1 partitions", "1 uploads"**: the plural is not chosen by count.
- **The app's Projects list** (S5b): no event announces a project a turn registered, so the list re-reads on mount
  and after the app's own writes; a project root is drawn unescaped there, in the two dialogs and in the CLI (the
  owner's own path, not text a tool, model or extension controls — S5b's recorded choice, review R25); `Delete data…` takes
  no typed word (the lead's R-S10; T33 stays the owner's question).
- **The dialogs** (S5b): `windowFollows` diverges in a subfolder after a cancel; `Use`'s own per-call `onSuccess` is
  dropped on a view switch; a refusal after the dialog closed is silent; a reopened dialog keeps the old project; the
  card header lags briefly after a re-point.
- **Following a project** (review R22, review R6): an `oa-project` an earlier build wrote has no id and cannot follow a re-point
  made elsewhere until `Use`; after a delete elsewhere the Project card's note says the folder "may have moved"; a
  pinned conversation's project re-syncs from the sidebar's loaded page only.

## 19. What the panel changed (rev 0.1 → 0.2)

- **Blocking, fixed.** Removing `moved_from` would have made every existing snapshot unreadable (R19). Deleting a
  project's folder would have taken files another conversation still shows (§6 step 1, R17).
- **Mechanism replaced.** The in-memory registry became functions over files with a pure resolver and a separate
  `register` (§3). `adopt` uses the project id (R20). The re-point runs its transaction before the rename and records
  the old name (§6). Pointer rows got their own migration and index (§13).
- **Owner decisions taken out of the rulings.** "An explicit list extends the defaults" was dropped (R30). Pre-edit
  copies, Open, reading without a prompt and who gets the new tools, and the re-point conflict became questions to
  the owner (now ST-17, ST-18, ST-16, ST-20). The run-wide allow was dropped (R43). Extension tools became T35.
- **Corrected to the owner.** The shell: an approved command can answer later prompts and switch prompting off (now ST-14).
  An "owner check" on the answer route closes nothing and was withdrawn.
- **Scope.** `store add`, the `chosen` field and the list's counts were cut. A fix round (Stage 0) was split out.

## 20. What the fact check changed (rev 0.3 → 0.4)

Four readers checked 47 code facts of rev 0.3 (`panel/fact-check-rev03.json`): 34 confirmed, 13 corrected in part, none
refuted outright, and 24 further findings (9 major).

- **Stage 0.** F2: the failed path's cut is made where the error text is built, and a failed answer is not kept at
  all. F3: an inherited variable must be removed too. F4: the lead's follow-up stores nothing, and `/steer` is a
  third writer.
- **Stage 1.** R12: an unregistered root must read as empty, not as an error. R18: three sites build a snapshot. The
  snapshot's `root` has more readers than the `nth(4)` sites. `quiesce` is keyed by id. ST-21 is an explicit check.
  The delete takes rows by all five path columns and keeps a linked by-product as well as a linked upload. R25's
  test is on the project folder, not on the name `.openalpaca`. The wire table matches today's field names.
- **Stage 2.** R29 is narrowed to the builtin tools. R44 (a session id on every context), R45 (the kill) and R46
  (no copy, no overwrite) are new. ST-17 states the tier's two refusals and its re-pointing. The lead does hold two
  by-product writers. A command tool's prompting is recorded with T35.

---

# Part IV — The beta

## 21. What a beta waits for

On 2026-10-05 the owner asked whether the app is ready for a beta. It is not. Stage 0's fixes are built
(`931460a4..a9dbed4e`, 2026-10-06…07), and so is Stage 1 (`b8b768b6^..44d53783`, 2026-10-07…08): nothing is written
in a repository any more. Agents not in the project (Stage 2) and B1–B3 below remain. The gaps, and where each is
fixed:

| Gap | Fix | Stage |
|---|---|---|
| The shipped `system_agent` ran shell commands and wrote files with no approval card | F1, built | 0 |
| Every program the daemon started inherited the master key | F3, built | 0 |
| A workflow's report could name the wrong path for a file it wrote (the 500-character cut) | F2, built | 0 |
| A CLI chat inside any git repository creates `<repo>/.openalpaca/` in the tester's repository | Part I, built | 1 |
| From the app, agents cannot read, write or edit the project's files, and the shell runs in the daemon's folder | Part II, with `file_edit` (ST-22) | 2 |

Three things a beta needs that no stage covers:

- **B1. The branch reaches `main`.** The Round G commits and the stages are pushed, PR #34 leaves draft and is merged,
  and the beta build is made from `main`, never from a working tree. The owner pushes and merges.
- **B2. A fresh-install check of the packaged app**, after the last stage the beta includes, the way 08 §4.1 did it
  at `8fde2925`: the bundle from `scripts/release/package-macos.sh`, started on a clean home
  (`OPENALPACA_HOME_STORE` and `OPENALPACA_CONFIG_DIR` on a scratch directory; the daemon under
  `scripts/eval/memory/loopback-only.sb` from its first launch; the local model), walking the path a tester walks:
  the first-run card; Ollama switched on, or a key added, from Settings; a streamed chat; an upload; a workflow that
  writes a file, and the report naming its path; the memory round trip; an approval card answered; Stop daemon and
  Start daemon; a second start on the same store. The evidence is archived beside 08's, and the result goes in the
  PR. The Windows and Linux packages are not checked by this; the Windows scripts have their Pester tests only.
- **B3. The manuals say what the beta can and cannot do.** One list in `docs/Installation_Manual.md`: the shell is
  not confined to the project (ST-14); extension tools never ask (T35); learning is below its own targets
  (`tasks/memory-redesign/10-learning-evaluation.md`); a store from before 2026-09-28 is refused and the way through
  is the factory reset; and, until Stage 2 ships, that agents do not touch the project's files.

Which stages the beta waits for is the owner's call. Stage 0 with B1–B3 is enough for a few testers who know the
limits; Stage 1 is what keeps every tester's repository clean; Stage 2 is what a tester who expects help with their
own project needs.

## Revision log

- **rev 0.10 (2026-10-09).** The owner answered Stage 1's two questions: **T36 — no change** (the re-point of an
  `old_path` inside a project stays as built; §9's `PATCH` row stands), and **ST-23** (§0.1) — a project whose recorded
  root no longer leads to its own folder (a case-only rename, a link left at the old path) reads missing and is
  re-pointed by the user. §0.2 records both; §18's first paragraph and its *review R1's deeper cross-wiring* bullet
  point at Stage 2's S0, where ST-23 is built. No code changed.
- **rev 0.9 (2026-10-08).** **Stage 1 built**, from `tasks/storage-design/plan-stage1.md` (approved by the owner on
  2026-10-07), as `b8b768b6^..44d53783` (131 commits): slices S1–S6, each reviewed with one fix round; the whole-stage
  review (eight reviewers by dimension; 33 findings, 1 Critical, 9 Important, cited as *review R1…R33*) and its fix wave `b923c81e..a9ea4864`
  with a residual pass `a9ea4864..909e724c`; one gate pass at `00f057fe`; the live check `stage1-live-report.md`
  (`44d53783`; (a)–(h) PASS, 0 ERROR lines; the app's surfaces by their vitest suites, no screen access). The status
  line and §21 say so (§21's Stage 1 row reads built); §0.2 names the two owner questions it raised, T36 and
  `missing` for a root that no longer canonicalizes to itself; §3, §5 (ST-21) and §9 gain *As built* paragraphs (the
  dispatch as a fifth registration site, R-S13, the recorded root matched as written, the hold, the wire's added
  codes and refusals); §7's list marks `known_project_roots` kept (R-S3), and an *As built* paragraph records how the
  removals were built (`known_project_roots` read from `projects::list()`; `project_root_of` kept; `unknown_entries`
  whole; R2; `is_the_home_root`'s one caller; six more fixtures); §16's Stage 1 clause says the app's surfaces were
  proven by their tests; §18 marks Stage 0's `wait_for_subagents` residual fixed in `32292608` and gains Stage 1's
  residue. The lead's rulings that shaped the build, one line each:
  - R-S1: S6 — text, gate, live check, records — drafted with the plan.
  - R-S2: the tests that only resolve a path take their crate's home-store guard in the commit that switches
    resolution; no test reads `~/.openalpaca`, even read-only.
  - R-S3: `known_project_roots` stays, over `projects::list()`, for the 30 s reconciliation.
  - R-S4: the guard is not called inside `repoint_with` or `remove_project_folder`; the guard audit covers their
    tests.
  - R-S5: the guard is proved compiled into a daemon `cargo test` builds and absent from one `cargo build` builds;
    every live check rebuilds first.
  - R-S6: `qk.statusAll()` joins the app's invalidation lists.
  - R-S7 (amended): a registration failure answers the same on every route — `409 WORKSPACE_IS_HOME`,
    `500 WORKSPACE_REGISTER_FAILED`.
  - R-S8: the delete takes the project exactly; a path inside it is `422 WORKSPACE_NOT_A_ROOT` naming the root, on
    the purge only (T36 asks about `PATCH`).
  - R-S9: the two daemon tests that wait a real 3 s stay.
  - R-S10: `Delete data…` confirms with the plan and a danger button, no typed word (T33 stays the owner's).
  - R-S11: a snapshot whose project id names no registered folder is not listed by the doctor (§18).
  - R-S12: the `wait_for_subagents` fix stays as built, with its test-only hook.
  - R-S13: the daemon-CWD fallback goes — a turn whose client sent no path has no project.
  - R-S14: each slice's recorded spec conflicts stand as drafted; the stranded `.tmp` folder and the delete's missing
    `memory_changed` go to §18.
  - R-S15: the four tests S2.5's verifier added are guarded under R-S2's rule.
  - R-S16: the tests that need a `.git` above the test process's directory stay.
  - R-S17: the app's copy about a relative path is swept with the code text.
  - R-S18: `ToolContext.workspace_id` and `request_workspace_root` stay two fields (equal since R-S13).
  - R-F1: SC-1 not built — the delete forgets its conversations' summaries in the home partition only (§18).
  - R-F2: the kept-folder toast is softened; a daemon field telling "kept" from "already gone" is residue (§18).
  - R-F3: a content write creates below a project's folder one level at a time and never re-creates it — built
    (`store::create_below`, review R16).
  - R-F4: task extraction across a delete or a re-point is not built (§18); its re-point half was later closed by
    review R19's refusal.
  - R-F5: `list()` and `by_id` pass over a vanished folder, and `known_project_roots` reads each project's own
    folder — built.
  - R-F6: the gate, the live check and the records run outside the slice workflow; the app half of the live check is
    proven by vitest, the session having no screen access.
  - R-F7: R-F1, R-F4's remainder, review R4's store-outside-`$HOME` variant, review R1's deeper cross-wiring,
    review R16's remaining window and review R17 go to §18.
  - R-F8: the review's R21 and R30–R33 are defects in the plan's own text, fixed by the lead at S6.5 and S6.6.
- **rev 0.8 (2026-10-07).** Records corrected after the Stage 0 records check. The Stage 0 lead-in writes each
  fix's range with its first commit (`bbfc676e^..7f16eba6`, `0047cf90^..f3980cba`, `0d9a9293^..85c9cacc` — in
  git `A..B` leaves `A` out) and says once that the table's Where column is the code before Stage 0; the branch line
  is dated at approval; §21 says Stage 0's fixes are built and that Stage 1, Stage 2 and B1–B3 remain.
- **rev 0.7 (2026-10-07).** **Stage 0 built**, from `tasks/storage-design/plan-stage0.md` (approved by the owner on
  2026-10-06), as `931460a4..a9dbed4e`: the commits per fix above the Stage 0 table, one gate pass at `09b40624`, and
  the live check `stage0-live-report.md` (F1–F4 PASS; the daemon under `loopback-only.sb` from its first launch, driven
  through its HTTP API). The status line, the Stage 0 table's lead-in and §16's clause ("the rebuilt app" → "the
  rebuilt daemon through its HTTP API") say so; §17's ST range reads ST-22; §18 gains Stage 0's four recorded
  residuals. T34 is recorded as taken and FW-2's row as taken by ST-10…ST-12 in `tasks/api-fix-plan.md` rev 3.21,
  with T35 pending.
- **rev 0.6 (2026-10-05).** The owner approved rev 0.5 with one addition: **ST-22**, the edit tool `file_edit`
  (§13, R47–R49). Status: approved; the plan comes next.
- **rev 0.5 (2026-10-05).** Part IV, §21: what a beta waits for — the gaps mapped to F1–F3 and the stages, and
  B1–B3 (the merge, the packaged fresh-install check, the limits in the manual), at the owner's request.
- **rev 0.4 (2026-10-05).** Q2…Q8 taken as recommended: **ST-15…ST-21**. The told items are cited by their ruling
  numbers (T1–T5 collided with the plan's T-series). The fix list is mapped (§0.4). A fact check of rev 0.3's code
  facts: §20, and §0.5 for what the owner has not seen. Rulings R44–R46. §17–§20 were renumbered §16–§19.
- **rev 0.3 (2026-10-05).** Q1 taken as **ST-14**: the shell's limit is accepted for this version, F1 is mandatory,
  sandbox methods are researched later. Q2–Q8 go to the owner together, at the owner's request.
- **rev 0.2 (2026-10-04).** After the panel: see §19 (then §20). Questions Q1–Q8, told items T1–T5, rulings R1–R42 (R23 moved
  to §11).
- **rev 0.1 (2026-10-04).** First draft from ST-1…ST-13 and the two rounds of maps.
