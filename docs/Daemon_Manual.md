# OpenAlpaca Daemon Manual (`openalpacad`)

`openalpacad` is the local control plane for OpenAlpaca, serving HTTP APIs, streaming events, orchestration runtime, connectors, and storage.

Related docs:
- [API Docs index](api/README.md) (generated from source by `python3 scripts/gen_api_docs.py`)
- [CLI Manual](CLI_Manual.md)
- [GUI Manual](GUI_Manual.md)
- [Agent Loop reference](agent-loop.md) — how one turn or one agent runs: rounds, exits, guards, compaction, steering
- [Installation Manual](Installation_Manual.md) — packages, the data-directory move, local models step by step

## Run

From repository root:

```bash
cargo run -p openalpacad
```

Release:

```bash
cargo build -p openalpacad --release
./target/release/openalpacad
```

The daemon takes **no arguments**. It is configured by files and by the
environment variables below, and `openalpaca daemon start|stop|status` is the
usual way to run it.

| Command line | Result |
|---|---|
| *(nothing)* | Runs the daemon in the foreground. |
| `--help`, `-h` | Prints the usage text and exits 0. |
| `--version`, `-V` | Prints `openalpacad <version>` and exits 0. |
| anything else | Names the argument on stderr, prints the usage text, exits 2. |

The command line is read before anything else happens, so asking for usage
never creates a store, a lock, a master key or a database. Only the first
argument is looked at.

| Environment variable | Meaning |
|---|---|
| `OPENALPACA_HOME_STORE` | Absolute path of the store root (default `~/.openalpaca`). |
| `OPENALPACA_CONFIG_DIR` | Directory holding `daemon.toml`, `llm.toml`, `mcp.toml` and the agent, skill and persona files. A path that does not exist is warned about and ignored. |
| `RUST_LOG` | Log filter (default `info`). See [Logging and Operations](#logging-and-operations). |

## macOS Package Install (No Cargo on target machine)

Build package (builder machine):

```bash
./scripts/release/package-macos.sh
```

Install package (target machine):

```bash
./scripts/release/install.sh --file ./dist/openalpaca-macos-<target>-v<version>.tar.gz
```

`install.sh` also supports `--url <https://...>` (instead of `--file`), `--prefix <dir>` (default `~/.local/openalpaca`), `--app-dir <dir>` (macOS app bundle location, default `~/Applications`), and `--yes` (non-interactive). Linux (`package-linux.sh`) and Windows (`package-windows.ps1` / `install-windows.ps1`) equivalents exist alongside the macOS scripts.

Runtime paths after installer-based startup. Everything lives under one root,
`~/.openalpaca` (`OPENALPACA_HOME_STORE` overrides it; absolute paths only, an
empty or relative value is rejected and the daemon refuses to start). The
organising rule is that `state/` is the machine's and everything else at the
root is the human's:
- config base dir: `~/.openalpaca/config`
- discovery: `~/.openalpaca/state/discovery.json`
- database: `~/.openalpaca/state/openalpaca.db` (plus its `-wal` / `-shm`)
- single-instance lock: `~/.openalpaca/state/openalpacad.lock`
- master key: `~/.openalpaca/state/.master_key`
- rotated copies of hand-edited config: `~/.openalpaca/state/backups/`
- plugins: `~/.openalpaca/plugins/` — one directory per plugin, plus `.permissions.toml` (approvals and the ENABLE bit for all plugins), `.config/<name>.toml` (per-plugin config), `.data/<name>/` (per-plugin durable state, kept across an update) and `.trash/` (where an uninstalled plugin's directory is moved)
- self-description: `~/.openalpaca/README.md` (what every entry is, with a retention class) and `.layout` (layout version and this install's id), both seeded when the store is created. `README.md` is written only when absent: an existing one, edited or not, is never rewritten — delete it and the next start seeds the current text
- content, home scope: one directory per content kind under the root, created on first use — `artifacts/`, `uploads/`, `sessions/`, `memory/`, `skills/`, `scratch/`, `cache/`
- memory notes: `~/.openalpaca/memory/owners/<owner-key>/notes/<id>--<slug>.md`,
  one Markdown file per note, with `.history/` (the last five revisions the
  daemon wrote) and a rebuildable `MEMORY.md` index beside them; a project's
  are under `<project>/.openalpaca/memory/owners/<owner-key>/`, where
  `memory/.gitignore` (`*`, re-asserted on every open) keeps them out of the
  repository. `<owner-key>` is the first 32 hex characters of the SHA-256 of
  the owner id. Directories are created `0700` and files `0600`
- memory control snapshots: `~/.openalpaca/state/memory-control/<partition-key>.json`,
  one per partition — the revision the daemon last wrote for each note, each
  note's origin, and the tombstones of forgotten notes. Never swept: losing one
  quarantines its partition until `openalpaca memory doctor --repair`
- memory search index: `~/.openalpaca/state/cache/memory-index-v1.sqlite` — a
  disposable SQLite full-text index over the notes. Deleting it costs one
  rebuild from the notes at the next start; nothing in it is authoritative.
  Nothing downloads a model: there is no embedding cache any more
- a project's own store: `<project>/.openalpaca/` — the same content shape, so an artifact of a project lives beside the project
- daemon log (launcher-managed startup): `~/.openalpaca/state/logs/daemon.log` —
  the daemon's stdout and stderr, appended across restarts and rotated by
  whichever launcher starts it when it is past 16 MB (`daemon.log.1` … `.3`,
  oldest dropped), so it costs at most four files. Both launchers write it:
  `openalpaca daemon start` and the GUI app's sidecar. `GET /v1/status`
  reports the path for a daemon either of them launched; a daemon started by
  hand (a bare `cargo run`, or `openalpacad` in a terminal) writes to its
  terminal instead and reports `null`, even when an older `daemon.log` is still
  there. When `openalpaca gui start`
  launches the GUI from a source checkout (`bun run tauri dev`), its output
  goes beside it as `gui.log`.

## Startup and Lifecycle

1. Parse the command line (none expected; `--help`/`--version` print and exit here), then initialize tracing/logging.
2. Seed the `~/.openalpaca` home store (`store::ensure_store`).
3. Acquire single-instance lock (`openalpacad.lock`).
4. Resolve config directory, seed missing default configs, and ensure master key.
5. Install signal handlers.
6. Bind to `127.0.0.1:0` (OS-selected port).
7. Write discovery metadata (`discovery.json`).
8. Open SQLite database and apply migrations. Still in this step, before
   anything can create new work: every run the previous process left in flight
   is marked `interrupted` (a terminal state), and any steering message it never
   delivered is recovered from the session log and filed for the lane's next
   turn. `POST /v1/tasks/{id}/rerun` restarts such a run under a new id;
   resuming it under its own id exists behind `[orchestrator.routing]
   resume_enabled`, which is **off** by default.
9. Bootstrap persona documents (SOUL/USER/IDENTITY/BOOTSTRAP) if missing.
10. Open the memory service — its search index at `state/cache/memory-index-v1.sqlite` is opened without waiting for anything: one that is missing or of another version is rebuilt from the note files in the background once the daemon runs (and one found damaged in use, by the next reconciliation), recall and search answering from the note files meanwhile — then start the orchestrator, the learning queue, wake manager, plugin manager, MCP clients, connectors, hot reload, background workers (the 30 s memory reconciliation and the 15 s learning sweeper among them) and the HTTP router. Thirty seconds after start, the learning queue's boot catch-up looks for conversations with turns not yet learned, and for conversations with at least four of your turns no episode note covers yet (see [Memory](#memory)).

Shutdown can be initiated by a signal (SIGINT/SIGTERM) or by `POST
/v1/command {"command":"shutdown"}`; both converge on one cancellation. The
daemon then stops accepting connections and waits for in-flight responses to
finish, after which it runs its shutdown tail in this order: learn what is
pending and write the episode notes due, for at most 3 seconds (the learning
drain — what it does not finish, the next start learns); stop the background jobs — the working state's turn-end refresh, a
finished run's task extraction and learning — within 3 seconds; flush the cost
tracker to the database; flush every session log; close every MCP connection
and kill every plugin child; stop the connectors; stop the wake scheduler and
file watchers; remove `discovery.json`.

A watchdog bounds all of it at **10 seconds** from the cancellation. The
session-log flush and the extension sweep each take only what is left of that
window. If the window runs out, the process exits with code 1 and skips
whatever of the tail had not run — possibly unflushed spend, dropped
session-log records, orphaned MCP and plugin children, connectors that never
deregistered, and a stale `discovery.json` (the next boot overwrites it, and the
CLI checks that the pid it names is a live daemon before trusting it).

Open connections no longer run that window out. An open
`GET /v1/chat/stream/{id}` body used to hold the wait for in-flight responses
until the watchdog fired; it now ends with a farewell `error` frame (see
[SSE Chat Stream](#sse-chat-stream)). An open `/v1/events` socket never held
the shutdown — the HTTP server stops tracking a connection once it upgrades —
but it stayed open and silent until the process died under it; it now gets a
`daemon_shutting_down` frame and a 1001 close (see
[WebSocket Events](#websocket-events)). Work that is still running — a chat
turn, a workflow, a follow-up, a scheduled skill — is not drained: it stops
when the process does, and a run left in flight is marked `interrupted` on the
next boot (step 8).

## Config Resolution

Config base directory precedence:

1. `OPENALPACA_CONFIG_DIR` env override (if path exists)
2. Upward search from current executable for `config/llm.toml`
3. Upward search from current working directory for `config/llm.toml`
4. Fallback: `<cwd>/config`

Important runtime files:

- `config/llm.toml`
- `config/daemon.toml`
- `config/mcp.toml` (MCP server declarations)
- `config/agents/*.md` (Markdown with YAML frontmatter; legacy `.toml` agent files still load with a deprecation warning)
- `config/skills/*/SKILL.md`
- `config/tools/*.toml`
- orchestrator persona docs under `config/orchestrator/`

## Secrets and First Run

- On first startup the daemon seeds missing `llm.toml`, `daemon.toml` and `mcp.toml` from templates embedded in the binary (sourced from `scripts/release/templates/config/`).
- The same step seeds the **content** the daemon also carries in its binary: `config/agents/` (the nine templates), `config/skills/` (helper scripts included, written executable) and `config/tools/`. The rule is per directory:
  - a directory that already exists is skipped whole, including one the owner emptied;
  - inside a directory being filled, an existing file is never overwritten;
  - one `INFO` line per directory names the count and the path, and a per-file failure is a `WARN` that does not stop the rest.
- A workflow is led by an agent template: one with the `orchestration` capability when there is one, otherwise any template that can be spawned. When no agent templates are loaded at all, the workflow request fails with "No agent templates are installed…" and names the `config/agents` directory. That is a different message from "All agents are busy", which clears by itself.
- The AES-256-GCM master key lives at `~/.openalpaca/state/.master_key` (`store::master_key_dir()`). The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; startup fails hard if the key cannot be ensured.
- Persona documents (`SOUL.md`, `USER.md`, `IDENTITY.md`, and conditionally `BOOTSTRAP.md`) are written into `<config>/orchestrator/` from templates if absent.

## Hot Reload

A file watcher reloads configuration without restart:

- `config/orchestrator/SOUL.md`, `USER.md`, `IDENTITY.md`, `BOOTSTRAP.md` (parse failures keep the last valid version)
- `config/llm.toml` and `config/daemon.toml`. An `llm.toml` edit reloads the runtime config first and **then** registers every provider the file enables that the router does not already hold — the same registration, discovery included, that the toggle route performs — so turning a provider on by hand needs no restart. Nothing is unloaded on this path: a disable arrives through the toggle. The daemon's own writes are swallowed by a hash ring.
- the `config/skills/` and `config/agents/` directories
- `config/mcp.toml` — that file **is** the MCP declaration and toggle store, so a hand edit is authoritative: the supervisor diffs desired against actual on presence, the `enabled` bit and a config fingerprint, and loads or unloads only what changed. An unparseable save keeps the last good set rather than tearing servers down, and the daemon's own writes are swallowed by a hash ring so a toggle does not reconcile twice.

## LLM Providers and Local Models

Providers are declared in `config/llm.toml` under `[providers.<name>]` and each
carries its own ENABLE bit. Three ways write it and they all write the same
field: `PUT /v1/settings/llm/providers/{provider}/enabled` (the GUI's switch),
`openalpaca config set ai.<provider>.enabled true` (the config schema's
`ai.*` key, backend `llm.toml`), and a hand edit the watcher picks up. When the
daemon writes `llm.toml` it edits the file in place: comments, key order and
keys it does not know are kept.

Every provider in the seeded `llm.toml` starts with `enabled = false`, and
adding a key does not turn an existing provider section on. A cloud provider
therefore takes two steps — enable it, then add its key:

```bash
openalpaca config set ai.anthropic.enabled true   # or ai.openai.enabled
openalpaca llm keys add --provider anthropic      # prompts for the key
```

An API key in the environment (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) is not
picked up as a provider key by itself: a key entry in `llm.toml` has to name
the variable in `secret_env`. A local Ollama needs the first step only.

Two facts shape the rest of this section: **a provider may need no API key**
(Ollama is the one that does not), and **a model the router cannot reach is
substituted, never silently**.

### Keyless providers

`LlmProvider::requires_key()` is the fact. For a provider that answers `false`,
an empty key pool is not an error: both router paths — the non-streaming retry
ladder and the streaming one — issue the call with no key, using an internal
rate-limiter slot that is never written to config and never appears in key
health. `GET /v1/settings/llm` carries `requires_key` per provider so the CLI
and the GUI can say "no key needed" instead of marking the provider broken. A
keyed provider with an empty pool is still refused, and a keyless provider that
*does* have keys configured (an authenticating proxy in front of it) still uses
them.

### Model discovery

For Ollama the daemon uses Ollama's **own** API, not the OpenAI-compatible one:
`GET {root}/api/tags` for the installed tags (root = `base_url` without the
`/v1` suffix), then `POST {root}/api/show` per tag for its context length
(`model_info.*.context_length`) and its capabilities. Each chat model is
registered with:

- input and output price `0`
- the context window `/api/show` reported, or 8192 when it reports none
- `supports_image` from the `vision` capability, tool support from `tools`
- `discovered = true`

A model whose capabilities omit `completion` — embedding-only — is not
registered. A tag that disappears from `/api/tags` is withdrawn at the next
refresh; a row the owner declared in `[models]` only stops being offered —
the declaration itself is left alone, so a later `ollama pull` of that tag
brings it back with the owner's own fields. Discovery needs no key, is not gated on
the key pool, and runs whenever the provider is registered (boot, the enable
toggle, a hot reload) and on `POST /v1/models/refresh`. An Ollama that cannot
be reached leaves the provider registered with zero models, one `WARN`, and the
reason on its discovery status — never a boot failure. `PUT
/v1/settings/llm/providers/{provider}/enabled` answers with
`discovered_models` and `discovery_error` so a zero always has a stated cause.

### The effective model

A requested model that is not routable (unknown id, provider not loaded,
provider disabled) walks a ladder instead of failing: the request's
`fallback_models` → the model's own chain → `[orchestrator] fallback_models` →
the **effective default** — the configured default when it is routable,
otherwise the default model of the first loaded provider that has a routable
one. Providers are walked in the built-in order (anthropic, openai, ollama,
then anything else by name); `llm.toml` is parsed into a map, so its own
declaration order is not recoverable. For a provider whose `default_model` is
empty or names a tag that is not installed, "its default" is the first
discovered model, preferring one that can use tools and then the lowest id, so
the answer does not move between runs. Every substitution is announced — one `WARN`
per distinct (requested → effective) pair, and the call log carries the model
actually used. `GET /v1/status` serves the pair:

```jsonc
"llm": {
  "default_model": "claude-haiku-4-5-20251001",
  "default_model_routable": false,
  "effective_default_model": "qwen3:8b"   // null = nothing is routable
}
```

`llm: null` means this daemon has no LLM router at all. When nothing is
routable, the next request fails with one error naming the fix (enable a
provider in Settings → Models, `openalpaca llm status`, `ollama pull`) rather
than "Unknown model".

**What is reported is what answered.** Every place a turn names its model — the
SSE `done` frame, the usage and call-log rows, the conversation row, the
session log — carries the model the router actually called, on the streaming
path as much as the non-streaming one; a substitution is visible rather than
hidden behind the requested id, and the turn is priced against the model that
ran. `POST /v1/chat`'s `model_used` is the one prediction in the set, because
it answers before the turn runs: it echoes the model named in the body, or the
effective default for a turn that names none, and `null` when nothing is
routable.

### Cost

The cost tracker prices from the router's live registry — compiled defaults
plus `[models]` rows plus discovered models — so a discovered local model costs
`0`, a declared price is honoured, and a registry reload reaches the tracker.
An unknown model is free only where every model on the daemon is local;
otherwise the conservative cloud fallback still applies.

| Cap | Default | Key in `daemon.toml` |
|---|---|---|
| One chat turn, or one subagent | $1 | `[execution.agent_defaults] max_cost` |
| One workflow's lead agent | $5 | `[execution.lead_agent_defaults] max_cost` |
| Learning from conversations, per UTC day | $0.25 | `[orchestrator.costs] extract_max_daily_cost_usd` |
| Learning from conversations, calls per UTC day | 200 (`0` = no cap) | `[orchestrator.costs] extract_max_calls_per_day` |
| The conversation's working state — compaction, turn end and close — per day | $0.50 | `[orchestrator.costs] summary_max_daily_cost_usd` |
| Task-output extraction, per UTC day | $0.50 | `[orchestrator.costs] task_extract_max_daily_cost_usd` |

An agent template's `max_cost_per_task` replaces the default for that agent.
Seven of the nine shipped templates set one: `lead_agent` sets `3.0`, so a
stock install caps a workflow's lead at $3, not $5, and the subagent templates
range from `0.25` (`explore_agent`) to `5.0` (`general_agent`). `system_agent`
and `writing_agent` set none and run at the $1 default.

There is no overall daily budget for turns or workflows. Only the background
jobs in the table carry a daily ceiling. The two memory jobs read today's calls
and spend from the usage table (`llm_usage_daily`) for the **UTC** date, so
their day starts at 00:00 UTC whatever the daemon's uptime. The call cap is
what bounds a local model, where the cost cap never binds at price `0`. A
capped learning window is not dropped: it waits, with one `INFO` per
conversation per day, and is learned once the cap no longer binds. The working
state's ceiling is still a running total kept in memory from the daemon's
start, seeded from today's usage rows. The two memory jobs are bounded by count as
well: `[orchestrator.costs] extract_max_items_per_call` (default 12, fewer when
the extraction model's output budget is small) notes from one learning call and
`task_extract_max_items_per_run` (default 10) candidates from one run; the
items past the count are dropped and counted, never stored.

An agent template's `max_cost_per_task` overrides the first two rows (the
shipped `lead_agent` template sets `3.0`). At price `0` none of the caps bite.

### Timeouts and output ceiling

| Key | Default | Scope |
|---|---|---|
| `[timeouts] llm_request_timeout_secs` | 120 | Wall clock for one **non-streaming** LLM call, any provider that sets no override. Clamped to 1…86400 s with a `WARN`. |
| `[providers.<name>] request_timeout_secs` | unset | Per-provider override of the above. The seeded template sets **600** for Ollama. |
| `[providers.<name>] default_max_tokens` | 4096 (8192 for the seeded Ollama) | Output ceiling for one answer; a request-level `max_tokens` still wins. |

The HTTP client carries **no total deadline**: it bounds the connect (30 s,
never longer than the budget) and the idle gap between reads, so a healthy long
stream is never cut by a wall clock. The non-streaming total is applied per
request instead. Both provider-construction paths — the boot builder and the
registration a toggle or a hot reload performs — resolve the budget through the
same function, so they cannot disagree.

Three bounds sit over a **streamed** turn, and it is worth knowing which one
fires:

| Bound | Value | Where | What happens |
|---|---|---|---|
| Idle between SSE chunks | **90 s**, not configurable | `STREAM_IDLE_TIMEOUT`, `crates/openalpaca_llm/src/streaming.rs` | The stream errors, the loop logs it and **retries the turn without streaming**. |
| Streaming wall clock | 600 s, not configurable | `LoopConfig::max_stream_duration` | Same fallback: the collection is cancelled and the turn is retried non-streaming. |
| HTTP read gap | `request_timeout_secs` (600 s for the seeded Ollama, else `[timeouts] llm_request_timeout_secs`) | `build_http_client` | The request fails at the transport. |

The first is by far the tightest, so on a local model it is the one that fires,
and the symptom is a turn that stalls and then answers un-streamed rather than
one that fails. The case that reaches it in practice is a model still being
loaded into memory: warm the tag once before a long run. The read timeout at
600 s is ten minutes of silence and is effectively unreachable while the loop
is the consumer.

Streaming for Ollama is real (the provider forwards to the OpenAI-compatible
streaming client), and every OpenAI-compatible base is asked for
`stream_options.include_usage`, so a streamed local turn records real token
counts.

## MCP Servers

Servers declared in `config/mcp.toml` are connected at boot; per-server failures are logged, never fatal. Each remote tool registers in the tool registry as `<server>__<tool>` and provides a capability equal to that namespaced name. Installed MCP and plugin tools are available by default to the assistant's main conversational loop (under both `tool_selection` modes) and to the lead agent orchestrating background workflows. Subagents remain template-scoped — they see only the capabilities their template declares. To expose an MCP tool to an agent, list the namespaced name in the agent template's `capabilities` frontmatter (for skills: `requires_capabilities`). MCP resources and prompts are not implemented, and serving MCP is a non-goal.

There is **no per-tool switch**: a whole server (or plugin) is turned off with `openalpaca ext disable mcp <server>` — see Extensions below. A disabled server's tools leave every surface and the gate refuses them.

## Extensions (MCP servers + plugins)

The ENABLE axis is one toggle per install unit: per MCP server (`config/mcp.toml`'s `enabled`) and per plugin (`enabled` in the plugins root's `.permissions.toml`). Builtins are never toggled, and there is no per-tool toggle or deny list. Disabled means unloaded: the plugin child is killed, the MCP connection dropped, and no reconnect is attempted. A tool whose extension is disabled is refused at the gate, with a warning in the log and — where a surface asked for it — in the run.

Verbs (`openalpaca ext …`, and `POST /v1/extensions/{kind}/{id}/{verb}`):

| Verb | Effect |
|---|---|
| `enable` | Writes the bit, then loads. A plugin that has not been approved records the bit and stays `Unapproved` — consent pre-empts the switch. |
| `disable` | Writes the bit, drains in-flight calls (`[extensions] drain_timeout_secs`, default 10 s), then unloads. |
| `reload` | Re-applies an edited declaration or a rotated credential: unload, then load from what is on disk. |
| `approve` / `deny` | Plugins only — records consent, or refuses and unloads. `approve` alone does not turn a plugin on. |
| `remove` (`DELETE`) | Drops the permissions entry of an orphaned plugin (directory gone). Untouched by the flag below: the bare `DELETE` never touches a directory. |

Install, update and uninstall are their own routes rather than verbs:

| Route | Effect |
|---|---|
| `POST /v1/extensions/plugin` | Copies the directory at `{"source":"path","path":"/abs/dir"}` into `~/.openalpaca/plugins/`. It grants nothing — the row lands unapproved and `approve` is the one action that starts it. `source: "url"` is declined by name. |
| `POST /v1/extensions/mcp` | Writes a `[servers.<name>]` block into `config/mcp.toml`. A declaration handing the daemon a literal secret is refused `422` (`secret_literal_refused`) — use the `env_from` / `bearer_env` / `extra_headers_from` indirections. |
| `POST /v1/extensions/plugin/validate` | Reads a candidate directory's `plugin.toml` and answers what it would install. Copies nothing. |
| `PUT /v1/extensions/plugin/{id}` | Replaces an installed plugin's tree with the one at a given path, through the same drain-and-unload a disable uses. |
| `DELETE /v1/extensions/{kind}/{id}?uninstall=true` | The real removal. A plugin's directory is **moved to `~/.openalpaca/plugins/.trash/`**, never deleted, and the response says where; `&keep_data=false` sends its `.data/<name>/` to the trash too. An MCP server must be `Disabled` first, and its `[servers.<name>]` block is what goes. Nothing auto-purges `.trash/`. |

`openalpaca ext list` / `info` render the same rows the GUI's Extensions view uses; `openalpaca ext list --include-orphaned` adds plugins whose directory has disappeared. `openalpaca ext install|update|uninstall` and `openalpaca ext mcp add|remove` cover the routes above. `openalpaca plugin …` remains as the plugin-shaped shortcut over the same routes, including `plugin config get|set` (values stored as secret references read back `<redacted>`). `GET /v1/tools` lists the tool catalog read-only — each row carries its `origin` (`kind`, `id`, `enabled`, `state`) for extension tools and `null` for builtins; there is no per-tool write.

## Discovery and Auth Model

Daemon writes discovery object including:

- instance id
- process id
- listen host/port
- auth token with expiry
- build metadata

The token is generated at every start and its `expires_at` is 24 hours later.
The expiry is checked by the clients that read `discovery.json` (the CLI and
the GUI refuse an expired file); the daemon's own middleware compares the token
and nothing else. The file is removed on a clean shutdown.

Auth behavior:

- Public: `/`, `/v1/health`
- Bearer token: most `/v1/*` endpoints
- Query token: `/v1/events`, `/v1/chat/stream/{stream_id}`
- Either, checked inside the handler: the three content routes
  (`/v1/files/{id}/content`, `/v1/artifacts/{id}/content`,
  `/v1/artifacts/{id}/versions/{n}/content`) accept `?token=` **or** the bearer
  header, because a webview `<img src>` cannot send one. Only authentication
  moved off the middleware — the owner check is unchanged, and the metadata and
  `/open` routes stay header-only.

Those three content routes also send `Content-Security-Policy: sandbox` on
anything that could run script on the daemon's own origin, where the bearer sits
in `?token=`: `text/html`, `image/svg+xml`, and **every XML essence** —
`text/xml`, `application/xml` and any `*/*+xml` suffix, matched on the essence
so a `;charset=` parameter or odd casing cannot slip past. An XML
document whose root is XHTML or SVG, or one carrying an `xml-stylesheet` XSLT
that produces either, is a script-bearing navigable document however its type is
spelled. Bytes that are not one of those — an image, a PDF, plain text — are
served without the header.

## API Route Groups

The route table's source of truth is `apps/openalpacad/src/router.rs`. Request
and response shapes are in the generated [API docs](api/README.md). Every route
below needs the bearer token unless its row says otherwise (see
[Discovery and Auth Model](#discovery-and-auth-model)).

Errors are answered as `{"error": {"code": "...", "message": "..."}}`. The
extension routes differ by design: they answer a flat `{"error": "<word>"}`.

### Core

| Method and path | Purpose |
|---|---|
| `GET /` | Name and version. Public. |
| `GET /v1/health` | Liveness: status, version, pid, instance id. Public. |
| `GET /v1/status` | Where the daemon keeps things and how it is doing: store root, state dir, database path, project root, start time and uptime, schema version, `log_path` (only when a launcher — `openalpaca daemon start` or the GUI app — manages the log), upload and artifact bytes, the session-log limits and the last boot sweep, `routing.resume_enabled`, the `llm` block described under [The effective model](#the-effective-model), and `busy` — what the daemon has in flight right now, as three counts and never contents: `running_tasks` (runs queued, running or paused, which a stop would leave `interrupted`), `pending_confirmations` (approval prompts nobody has answered) and `connected_clients` (open `/v1/events` sockets, the caller's own included). `busy` is `null` on a daemon with no confirmation broker, rather than a block with a guessed zero in it. |
| `GET /v1/me` | The local user id, the default lane key, and the sources this user has conversations under. |
| `POST /v1/command` | Daemon commands: `echo`, `process` (runs a full turn), `link_generate`, `link_consume`, `shutdown`. Takes `unattended`. |
| `GET /v1/events/history` | Persisted events. Filters: `task_id`, `agent_id`, `event_type`. Paged with `before` (an event id, exclusive) and `limit`; always answers `{events, next_before}`. |
| `GET /v1/events` | The WebSocket event stream. Query token (`?token=`). |

### Chat

| Method and path | Purpose |
|---|---|
| `POST /v1/chat` | Start a turn. Answers `{stream_id, lane_key, model_used}` at once; the answer arrives on the stream. See [SSE Chat Stream](#sse-chat-stream). |
| `GET /v1/chat/stream/{stream_id}` | The turn's SSE stream. Query token. |
| `GET /v1/chat/history` | One conversation's messages (`lane_key`, `session_id`, `limit`, `offset`; defaults to the default lane's active session). Each message carries its `artifacts` links. |
| `DELETE /v1/chat/history` | Empties one conversation and its summary, and forgets the episode notes that summarise it. The session itself survives — `DELETE /v1/sessions/{id}` removes one. |
| `PUT\|GET\|DELETE /v1/chat/messages/{message_id}/feedback` | Message feedback (`positive` or `negative`, with an optional comment). |
| `GET /v1/chat/confirmations` | The tool-approval prompts still waiting for an answer. |
| `POST /v1/chat/confirmations/{request_id}` | Answer one prompt. |

`POST /v1/chat` body: `content`, plus optional `attachments` (`[{file_id,
caption?}]`), `session_id`, `activate`, `model` and `unattended`. It refuses, in
this order and before it changes anything: more attachments than
`[upload] max_files_per_message` (`400 TOO_MANY_ATTACHMENTS`), a `model` that is
not in the registry (`400 UNKNOWN_MODEL`), an attachment id that does not exist
or is not the caller's (`404 ATTACHMENT_NOT_FOUND`, `403
ATTACHMENT_ACCESS_DENIED`), then the session checks (`404 SESSION_NOT_FOUND`,
`409 SESSION_ARCHIVED` unless `activate` is true).

Confirmations are covered under
[Tool confirmations](#tool-confirmations).

### Sessions

| Method and path | Purpose |
|---|---|
| `GET\|POST /v1/sessions` | List or create conversations. `POST` takes `{source?, workspace_path?, title?, inherit?}`; `inherit: false` starts the conversation fresh — it is never given the previous one's working state ([Conversation continuity](#conversation-continuity)). |
| `GET\|PATCH\|DELETE /v1/sessions/{id}` | Read one, change its title or bind it to a project, or delete it — and with it the episode notes that summarise it. |
| `GET /v1/sessions/{id}/messages` | Its transcript. |
| `GET /v1/sessions/{id}/events` | Its session log (JSONL records), paged by `seq`. |
| `POST /v1/sessions/{id}/activate` | Make it the lane's active conversation. |
| `POST /v1/sessions/{id}/archive` | Archive it. |

A lane holds many sessions with exactly one `active`. Each is served with its
**working state** — `working_state`, the JSON object
[Conversation continuity](#conversation-continuity) describes, or `null` while
it has none, and `working_state_updated_at` — and without the writer's own
bookkeeping. `DELETE /v1/sessions/{id}` takes the transcript with the rows:

- One transaction deletes the messages and the session.
- Everything that only *pointed* at it survives with its session link cleared:
  its runs, its queued follow-ups and its tool-call audit rows.
- The session's log directory, `~/.openalpaca/sessions/<id>/`, is removed.
- It answers `204`. A run still writing into the session is `409
  SESSION_HAS_ACTIVE_WORKFLOWS` (cancel it first). Another owner's session is
  `404`.

### Tasks and follow-ups

| Method and path | Purpose |
|---|---|
| `GET /v1/tasks` | List runs. A row carries `subagent_count`, not the agents themselves. |
| `POST /v1/tasks` | Create a run. Takes `unattended`. |
| `GET /v1/tasks/{id}` | One run. |
| `GET /v1/tasks/{id}/timeline` | One lane per spawned subagent (label, template, state, start and end). This is where a run's agents are reported. |
| `POST /v1/tasks/{id}/action` | `cancel`, `pause`, `resume` or `start`. Takes an optional `unattended`. |
| `POST /v1/tasks/{id}/steer` | Push a message into the run's steering inbox. `404` on a run that is not yours. |
| `POST /v1/tasks/{id}/rerun` | Answers `201` with a **new** id, copied from a finished run's goal. Takes an optional `unattended`. |
| `GET\|POST /v1/lanes/{lane_key}/followups` | Read or add to a lane's follow-up queue. The `POST` takes `unattended`. |
| `DELETE /v1/lanes/{lane_key}/followups/{id}` | Cancel a queued follow-up. A cancel that lost the race to autostart answers `409`. |

`POST /v1/tasks` records the caller as the run's owner whatever the body
claims, and refuses a `source_lane` the caller does not own with `404
LANE_NOT_FOUND` — never `403`. `start`, `rerun` and `resume` re-check the run's
owner **and** its lane before dispatching, so a run parked on somebody else's
lane never posts its completion report or its confirmation prompts there.

### Agents

| Method and path | Purpose |
|---|---|
| `GET\|POST /v1/agents` | List or create agents. |
| `POST /v1/agents/from-toml` | Create an agent from a TOML definition. |
| `GET\|DELETE /v1/agents/{id}` | Read or delete one. |
| `GET\|PUT /v1/agents/{id}/config` | Read or update its config. |
| `POST /v1/agents/{id}/action` | `pause` or `resume`. |
| `GET\|POST /v1/agent-templates` | List or create agent templates. |
| `GET\|PUT\|DELETE /v1/agent-templates/{id}` | Read, update or delete one template. |
| `GET /v1/agent-instances` | Read-only list of running agent instances. |

### Files, artifacts and workspaces

| Method and path | Purpose |
|---|---|
| `POST /v1/files/upload` | Multipart upload (body limit 100 MiB). Answers `{id, filename, mime_type, size_bytes, status}`; the `id` is what a chat turn names in `attachments`. |
| `GET /v1/files/{id}` | Upload metadata. |
| `GET /v1/files/{id}/content` | The bytes. Query token or bearer header. |
| `POST /v1/files/{id}/open` | Open the file with the OS default application. |
| `GET /v1/artifacts` | List produced artifacts (filters and paging). |
| `GET /v1/artifacts/{id}` | One artifact. |
| `GET /v1/artifacts/{id}/versions` | Its versions. |
| `GET /v1/artifacts/{id}/diff?from=&to=` | A diff between two versions. |
| `PUT /v1/artifacts/{id}/pin` | Pin or unpin it. |
| `GET /v1/artifacts/{id}/content` | The head version's bytes. Query token or bearer header. |
| `GET /v1/artifacts/{id}/versions/{n}/content` | One version's bytes. Query token or bearer header. |
| `GET\|PATCH /v1/workspaces` | Describe a project root; re-base everything addressed under it after the project moved. |
| `POST /v1/workspaces/purge` | Delete a project's conversations, runs and uploads, and answer the plan it followed. `dry_run` defaults to **true**. It removes the session-log directory of every session it purges. The project's memory notes stay, except the episode notes summarising the conversations it deletes. |

An upload is refused with the reason in the error code: `NO_FILE`,
`FILE_TOO_LARGE`, `STORAGE_QUOTA_EXCEEDED`, `UNSUPPORTED_MIME`,
`MIME_MISMATCH`, `MIME_UNDETECTABLE`, `UPLOAD_VALIDATION_FAILED`. The limits are
`[upload]` in `daemon.toml` (`max_file_size_bytes`, `max_total_storage_bytes`,
`allowed_mime_prefixes`).

### Connectors

| Method and path | Purpose |
|---|---|
| `GET /v1/connectors` | List connectors and their status. |
| `POST /v1/connectors/{id}/action` | `enable`, `disable` or `delete`. |
| `POST /v1/connectors/{id}/config` | Update connector configuration. |
| `GET\|PUT /v1/connectors/{id}/settings` | Read or update one connector's settings. |
| `POST /v1/auth/link` | Generate a link token for the local user — the short code a chat-platform account is linked with. |

### LLM settings, models and usage

| Method and path | Purpose |
|---|---|
| `GET /v1/settings/llm` | The provider configuration, with keys masked. Each provider carries `requires_key`. |
| `PUT /v1/settings/llm` | Add or update a key. It does not enable a provider that `llm.toml` already declares — use the ENABLE route below. |
| `DELETE /v1/settings/llm/keys/{provider}/{key_id}` | Remove a key. |
| `PUT /v1/settings/llm/keys/reorder` | Reorder keys and set the primary. |
| `PUT /v1/settings/llm/keys/priority` | Set one key's priority. |
| `POST /v1/settings/llm/validate` | Test a key. |
| `GET /v1/settings/llm/status` | Live key health. |
| `GET /v1/settings/llm/credentials` | Credentials discovered on this machine. |
| `POST /v1/settings/llm/credentials/rescan` | Rescan for them. |
| `GET /v1/settings/llm/cli-backends` | Status of the CLI fallback backends. |
| `GET /v1/settings/llm/providers/usage` | Per-provider usage summaries. |
| `PUT /v1/settings/llm/providers/{provider}/enabled` | The provider ENABLE bit. Its `200` carries `discovered_models` and `discovery_error`. |
| `GET /v1/models` | The model catalogue. Rows carry `supports_tools`. |
| `POST /v1/models/refresh` | Re-read the catalogue from every loaded provider, keyless ones included. |
| `GET /v1/llm/usage` | The LLM call log. |
| `GET /v1/llm/usage/daily` | Daily usage aggregates. |
| `GET /v1/usage/summary` | Today's spend (UTC day), its per-provider breakdown, and the two caps that bound a run (per workflow and per turn). There is no overall daily budget, so it reports none; see [Cost](#cost). |

### Orchestrator and daemon config

| Method and path | Purpose |
|---|---|
| `GET\|PUT /v1/orchestrator/config` | Read or set the configured default model and its `fallback_models`. The `GET` also reports the active agent and run counts and today's spend. |
| `GET /v1/orchestrator/latency` | Orchestrator stage latencies. |
| `GET /v1/orchestrator/latency/aggregate` | P50/P95/P99 by routing mode. |
| `GET /v1/orchestrator/decisions` | Dispatch decision history. |
| `GET /v1/daemon/config/providers` | The web-search provider configuration. |
| `PUT /v1/daemon/config/providers/web-search` | Update it (written to `llm.toml`). |

### Memory

**How memory works.** A note is one Markdown file with a short frontmatter
(`id`, `title`, `kind` — `fact`, `preference`, `decision`, `procedure`,
`reference` or `episode` — `status`, timestamps, and optional `aliases`, `tags`,
`fact_key`, `sources`, validity bounds and `supersedes`). Notes live in
**partitions**: one global partition per owner in the home store, and one per
owner in each project, the project being the nearest `.openalpaca`/`.git` root
above the request's workspace (`x-workspace-path`) — never the daemon's own
working directory. A turn with no project reads and writes the global partition
alone; a turn in a project reads both, and a `preference` defaults to global
while the other kinds default to the project. Only a real user owns memory: a
`/v1/command process` turn (owner `system`), the workflows it starts and an
unlinked connector identity have no partition, recall nothing and write nothing.

Every turn of the main loop and of a file-based skill gets an automatic recall
— pinned notes, then the notes a keyword search of the message finds, each
re-read and re-hashed before it is sent, all within 8 KiB and delivered as
quoted reference data rather than instructions — and the turn's `done` frame
names what it delivered (`memory_used`, below). `[orchestrator.memory]
recall_mode` says how the matches are delivered. `index` (the default) sends
the top 2 as excerpts and lists up to 8 more one line each — `<id> <date>
<kind> “<title>” — <reason>`, the date and the reason as a `memory_search`
index row writes them — under the line `More notes that may be relevant
(read with memory_read):`, so the model opens only the ones it needs; a
listed note is reported with reason `index`, and a match too large to
excerpt within the budget is listed instead. `excerpts` sends up to 8 notes,
pinned ones included, each as an excerpt. A file-based skill's turn is recalled
in `excerpts` whatever the key says unless the skill can call `memory_read`
(its `requires_capabilities` or, without them, its `tools.allow` names it,
and its `tools.deny` does not) —
none of the shipped skills can, and a list it cannot open would only take the
room of excerpts. The key is read on every turn, so a reloaded `daemon.toml`
applies to the next one. A workflow's lead gets the same
recall, in the same mode, once at its start. Search is literal: a term is
matched as a whole word or as the beginning of one (`prefer` finds
`prefers`), and inside a word only in a note's title, aliases and tags, or in
Chinese, Japanese, Korean, Thai, Hindi, Tamil and similar scripts — `presso` does
not find a body's `espresso`; a one- or two-character query (a short Chinese
word) goes through a bounded scan, and nothing asks a model. A reply waits at most 1.5 s for its recall
(and, apart, for the suggestions it is told about): past that the turn goes
on without them, as it does when memory fails, and the daemon logs one
warning.

**Learning from conversations** happens in the background and never delays a
reply. Every turn is queued, and a conversation's turns not yet learned are
extracted together:

- when it has been idle about two minutes (`[orchestrator.learning]
  idle_secs`, default 120; `0` turns this trigger off);
- when 10 of your turns have accumulated (`turns_per_flush`, default 10);
- when you switch to another conversation — "New chat", reopening an older
  one, or a turn in another project;
- before the daemon stops, for at most 3 seconds; what that does not finish is
  learned 30 seconds after the next start, so nothing is lost.

A learning call starts only after 20 seconds with no chat turn in flight, one
at a time on the whole daemon. On a local model a chat turn that starts cancels
it — the model serves your reply alone, and learning retries later — and on a
cloud model with a key it starts only when half of that key's rate budget would
remain for chat.

Learning reads the stored conversation: the messages **as typed** — never an
attachment's extracted text — and the answers, in chunks sized to the
extraction model's context window (a long message is no longer cut to 800
characters; one over 8 KiB keeps its first 6 KiB and last 2 KiB). Each message
learns into the project its own turn named, or into the global partition when
it named none, whatever project the conversation was later bound to. What you
said in your own words becomes an active note learned from you (origin
`extracted_user`, listed as *recently learned* with a one-step undo). A learned
note carries `aliases` — other words you might search it by, the other language
included — a `fact_key` naming the fact it answers for (reusing a key your notes
already use when one fits; a key only the assistant's own `memory_store` ever
used is not offered to it — the note itself still is — while a key learning or
you used, even on a note since replaced, is, and in every partition learning
reads together), and `observed_at`, when you said it. A fact about you (a key
starting `user.`) that replaces no note is saved in the **global** partition,
whichever project you said it in; a replacement is saved in the partition of
the note it replaces. When it
changes a note that was itself learned, it replaces that note at once (an
explicit `supersedes`; undo brings the old one back). When it changes a note you
saved, edited by hand or imported, it is written as a **suggestion** instead: a
candidate naming the note it would replace, while your note stays in force and
in recall until you *Replace* or *Keep mine* (`GET /v1/memories/suggestions`
below). What was inferred from an answer, from a follow-up the assistant
queued for itself, from a message over 8 KiB (which may quote other text — a
piped email, a pasted page) or from a run's output becomes a **candidate**,
kept out of recall until you confirm it; one that would change a note is
listed with the suggestions, marked as inferred. Every learned note cites the
messages it came from, and a message you forgot a note from is never learned
from again. Slash commands and their answers, extension notices, workflow
updates and completion reports are not read.

At most `[orchestrator.costs] extract_max_items_per_call` (12) notes per
learning call and `task_extract_max_items_per_run` (10) candidates per run are
written, on top of the daily ceilings in [Cost](#cost); an item the model rates
under `[orchestrator.memory] memory_confidence_threshold` (0.65) is dropped. A
failed or cancelled run produces nothing. Learning waits, and loses nothing,
while no model can answer (one `WARN` when that starts, one `INFO` when it
ends, with retries backing off from 1 minute to 1 hour), while a memory
partition is quarantined, and while a daily cap binds; an answer the model
cannot give in the expected shape three times running is skipped, with one
`WARN` naming the conversation and the messages. An answer that carries its
JSON after the model's reasoning (behind a `</think>`), before a stray
`</think>` that ends it, inside prose or as a bare list of items is read for
the items it carries, each checked as in a well-formed answer — a draft in the
reasoning, or a list or object in the prose that is not the items (no entry
with a `content`), is not taken, and `[]` means nothing to keep; an answer
whose `<think>` never closes, or cut at its output ceiling, is not read — the
latter counts as cut. Deleting or clearing a
conversation drops its turns not yet learned, and so does deleting the project
directory a turn named — even while the model is answering: learning, and an
episode note, skip those turns rather than re-create the directory and its
`.openalpaca/` store. `[orchestrator.learning] enabled
= false` switches learning off.

**Episode notes** record conversations the way learning records facts: one
note of kind `episode` per stretch of conversation — what was discussed, what
was decided, what was left open. The learning queue writes one when a
conversation has been quiet for **30 minutes**, when you switch to another
conversation, and before the daemon stops (its durable half, as for
learning, is the boot catch-up), and only once **at least 4 of your turns**
have accumulated since that conversation's last episode note; a
conversation's facts are learned first. No model call writes the note: it
is projected from the conversation's working state, which the queue brings
up to date first — one call on `[orchestrator.costs] summary_model`,
only when a turn of yours lies past what the state covers, under
`summary_max_daily_cost_usd` and learning's quiet start and yielding to chat.
The note does not cite a message you forgot a note from, nor its answer.

The note is titled `Conversation on <YYYY-MM-DD>: <topic>`, the date its last
turn's; its `## Knowledge` holds at most 1 KiB of prose — `Discussed: …`,
`Decided: …`, `Left open: …` — in the conversation's language; `observed_at`
is its last turn's time, and it cites the conversation's messages (all of
them up to eight, else eight spread from the first to the last). It is
written with no `fact_key` and replaces nothing. It is the assistant's own
wording, so its origin is `extracted_other`, but it is **active**: `memory_search`,
`memory_timeline`, `memory_list`, the routes below and the Memory view find
it. **Automatic recall never delivers one** — not as an excerpt, not in the
index block, not to a workflow's lead — and learning never shows one to the
extractor: it reaches a model only when the model or you look for it (a note
you pinned is delivered, as every pinned note is). An episode you saved
yourself, or one learned from what you said, is an ordinary note and is
recalled as any other. The assistant's own edit or archive of an episode note
(`memory_update`) leaves it an episode note, and a `fact_key` that edit adds
counts for nothing in learning: the key is not offered to the extractor, and
something you say later under it is learned as usual rather than proposed as a
replacement for the episode note. An edit you make makes it your note,
recalled as any other.

Forgetting an episode note — the Memory view, `/forget`, `memory_forget`,
`DELETE /v1/memories/{id}` — forgets that note and nothing else: unlike any
other forget, it suppresses none of the messages it cites (a pending
suggestion's or candidate's forget spares the messages and runs that a note you
saved, or the note it would replace, cites), so what those messages taught stays
learned and recallable. Deleting or clearing a
conversation (`DELETE /v1/sessions/{id}`, `DELETE /v1/chat/history`) or
purging its project forgets the episode notes whose cited messages all
belong to it.

Nothing is pruned by age and nothing is replaced because it looks similar: a
note is replaced only by an explicit `supersedes`, and forgotten only by a
forget. `USER.md` is not written by any of this — see [Persona](#persona).

The model's tools are the same service with less authority. `memory_search`,
`memory_read`, `memory_list` and `memory_timeline` are registered for every
agent granted `memory_read` — all nine shipped agent templates name it, and a
built-in template that neither names nor denies it is granted it anyway, so an
install whose `config/agents/` predates that needs no edit (a plugin's agent
template gets it only by naming it, since its results go to the plugin's own
process); a workflow's lead has `memory_search`, `memory_read` and
`memory_timeline` on its own surface. `memory_store`, `memory_update` and
`memory_forget` are offered to the main conversation alone.

**The read tools are layered**: filter first, fetch later. `memory_search`
answers an **index** by default — one row per note, `{id, date, kind, scope,
status, title, reason, size, conflict}`, no body — where `date` is when the
note's evidence was observed, else when it was first saved, `reason` is how it
matched, and `size` is its approximate token count (body bytes ÷ 4);
`detail: "snippets"` gives the cited excerpts instead, each with the note's
`valid_from` and `valid_to` (RFC 3339, or null). It also takes
`date_from` and `date_to` (inclusive, on that date; RFC 3339, or `YYYY-MM-DD`
as that whole UTC day) and `order_by` (`relevance`, the default, `newest` or
`oldest`), and `limit` is 1–50, default 10. `memory_read` takes `ids`, 1–10 in
one call, and answers every note in the order asked, each with its revision
and provenance; the bodies share the one 32 KiB read budget — a short one is
never cut to make room, a cut one says `truncated` — and an id no note of
yours has (another owner's note reads the same) answers `{id, error:
"not_found"}` in its place. `expected_revision` and `section` go with a single
id. `memory_timeline` shows what surrounds a note in time: around exactly one
of `anchor` (a note id), `query` (its first hit, superseded and archived notes
included) or `date` (an instant), the nearest `before` and `after` notes
(default 5, at most 20 each), in date order, as index rows whose `reason` says
`before`, `anchor` or `after`. Unlike a search it lists notes that have left
recall — superseded, archived or with an ended validity window — marked by
`status`; a candidate (a suggestion included) is never listed, and a timeline
is not drawn around one. The main conversation's system prompt says the same
in one line. Search hits, timeline rows and read notes are all reported as
`memory_used` with reason `tool`. While the search index is being rebuilt,
`memory_timeline` answers `index_rebuilding` at once — the model can tell you,
and search and read answer meanwhile — rather than wait for the rebuild. A tool call records origin `extracted_user` at most,
never `user`; it cannot confirm a candidate, it cannot archive a note you
wrote, and it cannot edit a suggestion once written (the text you are asked
about is the one its writer — learning or the assistant — wrote). **The
assistant's replacement of a note you saved, edited by hand or imported is a
suggestion** — whether it names the note in `supersedes`, saves under that
note's `fact_key` (in the partition the note would land in), names another
note that shares your note's key, or edits your note: a candidate naming your
note, which stays in force and in recall until you *Replace* or *Keep mine*,
and the tool's result tells the model so (its `superseded` is empty; `replaces`
names your note). A change that only bounds your note in time
(`valid_from`, `valid_to`) is a suggestion too. With no `scope` it is stored in
your note's partition. It cites the message you typed on that turn — an edit's
suggestion, a copy of your note's text, cites what your note cites first and
that message when there is room (at most eight), so forgetting a message your
note learned from takes the copy out too, while forgetting the pending
suggestion itself leaves your note in force: the forget of any pending
suggestion or candidate suppresses none of the messages or runs the note it
would replace cites, whoever saved that note, nor any a note you saved cites —
such as a suggestion or candidate from the same turn or run that you have since
confirmed. Because it cites the message you typed, the assistant may raise it
with `memory_confirm` on a later attended turn; the Memory view,
`openalpaca memory suggestions`, the in-chat card and the `<memory_suggestions>`
line say the assistant suggested it, and the first three give no reason of its
text's (the text may copy your note's, whose reason is that note's) and say
what else it changes — your note's key, aliases, tags or validity.
Saving exactly what your note
already says, with the same validity, returns your note, not a suggestion. The
assistant may not edit a note of yours that a forget took out of recall (it
cites a message you asked to forget): that is `authority_conflict`, and the edit
is yours, in the Memory view or with `openalpaca memory edit`. An edit goes
through `memory_update`'s ordinary
card first: approving it gives you a suggestion to *Replace* or *Keep mine*,
not the edit. Before this, a replacement of a note you saved was refused and
one of a note edited by hand or imported was applied at once. Still direct, as
before: archiving a note edited by hand or imported, and an edit of one that
changes only `review_after` — after which it counts as learned and a
replacement of it stands. Its replacement of a note learned from you replaces
it at once, as learning's does. No surface
archives a suggestion — it is replaced or kept — and forgetting a note
withdraws every suggestion that would replace it, which could never be
confirmed any more. A forget takes the pending suggestions it names first —
every one of them, even after one of them fails — then its other targets in
the order given, and answers them in the order given; the withdrawals run once
every note it names has had its turn, so a suggestion named with its note is
forgotten as asked, in either order. A target that fails ends the forget with
its refusal, once every suggestion it names has had its turn and after the
withdrawals the forgets that landed owe. `memory_update` and `memory_forget` raise the ordinary
confirmation card, and an unattended turn (a scheduled skill, a piped
`openalpaca chat`) is refused them outright. `memory_update` with `action:
confirm` is refused **before** any card, since nothing the model does confirms
a candidate.

**Confirming a suggestion in chat** (`memory_confirm`). On an attended turn of
the app's or the CLI's chat — never a connector's, never an unattended one —
the main conversation also has `memory_confirm {id, expected_revision,
target_revision}`, and that turn's memory message ends with a
`<memory_suggestions>` block: up to three pending suggestions — learning's,
from your own words, and the assistant's own, each line saying which — newest
first, one line each with both ids and both revisions, read from
the index (no note file is parsed for it), leaving out a suggestion already
raised on that lane since the daemon started, and any other suggestion for the
note it would replace — the lane is `{owner}:gui`, which every app and CLI chat
conversation of that owner shares. The call raises the ordinary card with a
preview the **daemon** composed from the notes, never from the call's
arguments — which note, your text, the new text, what else it changes (a
`Changes:` line naming every one of the key, aliases, tags or validity it
changes, the validity always whole), why (none
for the assistant's own), and when — the date of the newest message you typed
that it cites ("from what you said on …"), or for one the assistant wrote the
day it saved it ("suggested by the assistant on …"), with every hidden character of what it quotes left out and, when any
was, a last line saying how many (`This preview leaves out 2 hidden
characters; the Memory view shows them as <U+…>.`) — and `per_call: true`. Only a person's **Approve** on that card, for
that call, confirms it: the confirm is then a user-directed write (origin
`user`, `memory_changed.surface` `"user"`), bound to the replaced note's
revision, and it dismisses the other suggestions for the same note. The model
calling the tool without that answer gets `approval_required` and nothing is
written; `[security] auto_approve_confirmations` and an earlier *Always allow*
do not answer this card, and it is never cached. **Deny** withdraws the
suggestion for good (as `POST /v1/memories/{id}/undo` does); a timeout leaves
it waiting in the Memory view. A suggestion inferred from the conversation
rather than your words, or citing no message you typed, is `not_a_suggestion`
here — it is confirmed only through the routes below.

**Chat memory commands** (`/remember`, `/forget`, `/memories`). The
orchestrator answers these itself, in its deterministic tier, with **no model
involved** — the turn's `IntentClassified` names them `memory_command`. Typed
by a person on an attended turn of the app's or the CLI's chat (source `gui`
or `cli`, not `unattended`), such a command is a **user-directed** surface like
the routes below: it builds its `UserSurface` context from the turn — the
turn's owner, and the project root the turn's own workspace resolved to — so
`/remember` records origin `user`. G2 is untouched: no model ever sees the
command, and the text written is exactly what was typed.

- `/remember [--kind <k>] [--global | --project] <text>` stores a note: the
  title is the first line cut at 80 characters, the body is the whole text,
  the kind is `fact` unless named, and the scope follows the kind against the
  turn's project (a `preference` global, the other kinds the project, as
  above) unless `--global` or `--project` says otherwise;
  `--project` on a turn with no project root is refused `invalid_input`. An
  exact duplicate answers the existing note. The answer is `Saved as a note you
  wrote: “<title>” (<id8>, <scope>). It is used from your next message.`, or
  the refusal's own sentence after `Nothing was saved:`.
- `/forget <id-or-prefix>` resolves a prefix of at least 8 hex characters
  (case and hyphens ignored) among every note the turn can read, any status,
  and forgets a unique match at the revision just read — tombstone first, as
  every forget — answering `Forgot “<title>” (<id8>).` and the service's
  retained-copies sentence. A prefix several notes share lists them by full id
  and forgets nothing.
- `/forget <text>` searches (5 active notes) and forgets nothing: it lists the
  matches and `Send /forget <id> to forget one.`
- `/memories` lists the 10 most recently updated active notes and the number of
  pending suggestions; `/memories <query>` searches (8 active notes) with each
  hit's match reason. A line is `<id8>  <title>  [<kind>/<scope>]`, at most 160
  characters; no body is ever printed.

Every write is the service's own, so it is announced as one `memory_changed`
(`surface: "user"`) like any other; an exact duplicate writes nothing and
announces nothing. A connector turn (Telegram, Discord, iMessage), an
unattended one (a piped `openalpaca chat`), a scheduled skill, a follow-up
and a `/v1/command process` turn are answered ``Memory commands work only in
the OpenAlpaca app's chat, or in `openalpaca chat` run at a terminal — not with
its input or output piped.``; a turn with no owner (`Principal::System`, an
unlinked identity) or a daemon with no memory service, `Memory is not
available here.` — and nothing is read or written. The turn's attachments are
reported skipped. Learning never reads these turns: it leaves out every
`/`-line and its answer.

The routes below reach the memory notes (Markdown files under the home store's
`memory/` and a project's `.openalpaca/memory/`) through the one memory service
the model's tools and the automatic per-turn recall also use. These are the **user-directed**
surface: a write here records origin `user`, the highest authority, which a
model's tool call can never edit, archive or replace. `x-workspace-path` selects
the project partition exactly as on `POST /v1/chat` (the nearest
`.openalpaca`/`.git` root above it); absent, the global partition alone is
addressed. Bodies and queries are closed objects — an unknown key is a `400`.

| Method and path | Purpose |
|---|---|
| `GET /v1/memories` | List notes: `kind`, `status` (default `active`), `scope`, `include_history`, `include_candidates`, `limit` (1–100, default 50) and `cursor`. Answers `{items, next_cursor}`, each item with `supersedes` (the ids it names as replaced); a cursor is valid for the filter set that issued it, and one from another filter set is `400 MEMORY_INVALID_INPUT`. |
| `GET /v1/memories/search` | Literal search: `q` (1–512 chars), `kind`, `scope`, `limit` (1–50, default 10), `historical`, `as_of`, and the model tool's `date_from`, `date_to` (inclusive, on the note's date — observed, else first saved; RFC 3339 or `YYYY-MM-DD`) and `order_by` (`relevance`, `newest`, `oldest`). `detail` defaults to `snippets` here: cited results with `freshness`, `conflict`, `same_key_loser`, `sources_status`, `valid_from` and `valid_to`; `detail=index` answers the tool's index rows. Either way with `count`, `partial` and `index_state`. Candidates are never returned. |
| `GET /v1/memories/timeline` | `memory_timeline`: around exactly one of `anchor` (a note id), `q` (its first hit) or `date` (RFC 3339 or `YYYY-MM-DD`), the nearest `before` and `after` notes (0–20, default 5), in date order. Answers `{anchor, results, count, index_state}` — `anchor` is `{id}`, `{date}` or null when `q` matched nothing — with index rows whose `reason` is `before`, `anchor` or `after`, superseded, archived and ended notes marked by `status`, candidates never listed. An unknown anchor is `404`; no anchor, two, or a side over 20 is `400`; while the search index is being rebuilt it answers `503 MEMORY_INDEX_REBUILDING` at once. |
| `GET /v1/memories/recently-learned` | The extractor's `extracted_user` notes that recall would use, newest first, each with `supersedes` (what it replaced) — `since` (RFC 3339, or a window like `7d`, `24h`, `30m`; default 7 days) and `limit`. A note that is not in force, loses its `fact_key` to another note, or cites a message a forget suppressed is not listed. An edit through this surface re-records a note as `user` and it leaves the list. |
| `GET /v1/memories/suggestions` | The pending suggestions, newest first — `limit` (1–100, default 20). Each item: `id`, `revision`, `title`, `knowledge` (the suggested text, cut at 4 KiB), `knowledge_left_out` (how many characters that cut left out, 0 when whole), `reason` (none for a suggestion the assistant saved in chat, whose text may copy the note it names), `changes` (what else it changes of that note: `[{field, from, to}]` for each of `fact_key`, `aliases`, `tags`, `valid_from` and `valid_to` whose values differ, the values as lists, empty for none), `created_at`, `observed_at` (the newest cited message's time, as learning records it; `null` for a suggestion the assistant saved in chat, which is how the app and the CLI tell it apart), `origin`, `scope`, `sources`, `older_count`, and `target: {id, revision, title, excerpt, excerpt_left_out, origin, scope}` — the note it would replace, as read now, its text cut at 600 bytes. Where several name one note only the newest is listed, and `older_count` counts the rest. A suggestion whose note is gone or no longer in force is not listed (undo still dismisses it). Records nothing. |
| `GET /v1/memories/recently-used` | The in-memory ring of notes delivered to a model since the daemon started (`limit`, default 50, max 200): `{id, revision, reason, delivered_at, lane_key, task_id}`, `reason` as in `memory_used` — a note the recall only listed is recorded `index`, not as delivered. Cleared at restart. |
| `GET /v1/memories/doctor` | The read-only report: every partition with its health, counts, malformed files, duplicate ids and conflicts, other owners' partitions as orphans, and stale/abandoned/orphaned move snapshots. Changes nothing. |
| `POST /v1/memories` | Save a note: `title`, `body`, `kind`, and optionally `scope`, `aliases`, `tags`, `fact_key`, `sources` (`[{type: message\|task, id}]`, verified against the database), `valid_from`, `valid_to`, and `supersedes: {id, expected_revision}` to replace a note. `201` with the `memory_store` result and `ETag`; an exact duplicate is `200` with the existing id and `dedup: existing`; a moved `supersedes` target is `409`. |
| `POST /v1/memories/forget` | Forget several: `{targets: [{id, expected_revision}], reason?}` (1–20), per-id status in the order given, plus what is retained. The pending suggestions it names are forgotten first — every one of them, even after one of them fails — then the other targets in the order given. A forgotten note's pending suggestions are withdrawn once every target has had its turn, so a suggestion named with its note is forgotten as asked, in either order; a target that fails ends the request with its refusal, once every suggestion it names has had its turn and after the withdrawals the forgets that landed owe. `openalpaca memory forget` sends its ids here in requests of up to 20, the pending suggestions first. |
| `POST /v1/memories/sync` | One reconciliation scan now: `{scanned, changed, index_state, changes}`. |
| `GET /v1/memories/{id}` | One note with its frontmatter, body (≤ 32 KiB), provenance and `ETag: "<revision>"`. `?section=<heading>` for one section; `?revision=<rev>` for one of the last five daemon-written revisions. |
| `GET /v1/memories/{id}/history` | Those revisions, newest first: `{revisions: [{revision, written_at}]}`. |
| `POST /v1/memories/{id}` | Update under `If-Match`: `{action: edit\|archive\|confirm, patch?, target_revision?, reason?}`. `edit` takes a `patch` of `title`, `body`, `aliases`, `tags`, `fact_key`, `valid_from`, `valid_to`, `review_after` (an explicit `null` clears a nullable one); `confirm` makes a candidate active. Confirming a **suggestion** also retires the note it names, so the body carries `target_revision`, that note's revision as you last read it: absent is `400`; a note edited since is `409 MEMORY_REVISION_CONFLICT` with *that* note's revision as `ETag`; a note gone is `404`; a note already replaced or archived another way is `400` (nothing is left to replace — undo the suggestion); a candidate naming several notes is `400`. `target_revision` with any other action is `400`, and `archive` of a suggestion is `400`. Answers `{id, revision, status, superseded, dismissed}` and the new revision as `ETag`: `superseded` is the note a confirmed suggestion replaced (kept as history), `dismissed` the other suggestions for that note, each withdrawn. |
| `POST /v1/memories/{id}/undo` | Take back what learning did, under `If-Match`: a note learned from your words (origin `extracted_user`) or any candidate — for a suggestion, *Keep mine*. It is a **withdrawal**, not a forget: the one note goes, no message it cited is suppressed (whatever else that message taught stays learned), and what it had replaced is in force again unless another note still replaces it. Answers `{id, status: "withdrawn", restored}`, `restored` naming the notes that came back (empty for a candidate). Any other note is `409 MEMORY_NOT_UNDOABLE`. |
| `DELETE /v1/memories/{id}` | Forget one note under `If-Match`: `{id, status: "forgotten", retained}`. An episode note's forget suppresses none of the messages it cites (see [Memory](#memory)). |

**A revision travels in `If-Match` and nowhere else.** The three single-note
mutations take `If-Match: "<revision>"` — the `ETag` a read answered. Absent is
`428 MEMORY_IF_MATCH_REQUIRED`; a revision that is not the note's current one is
`409 MEMORY_REVISION_CONFLICT`, with the current revision in the message and as
`ETag`, and nothing written; `If-Match: *` names no revision and is `400`; a body
`expected_revision` on an update or a create is `400 MEMORY_INVALID_INPUT`, so
two revisions can never disagree silently. (`POST /v1/memories/forget` carries
its targets' revisions in the body by contract, one per id, and a create's
`supersedes.expected_revision` is the *target's*.) The other codes: `404
MEMORY_NOT_FOUND` (an unknown id, or a revision the history does not keep), `409
MEMORY_QUARANTINED` (the partition's control snapshot is missing or invalid —
`openalpaca memory doctor --repair`, with the daemon stopped, is the only way
out), `400 MEMORY_INVALID_INPUT` (a limit, a grammar, a shape, an unknown key, a
cursor from another filter set). Three more the envelope can carry: `409
MEMORY_BUSY` (a memory write on that partition is in flight — retry), `503
MEMORY_INDEX_REBUILDING` (the timeline, while the search index is being
rebuilt — retry in a minute) and `500 MEMORY_INTERNAL` (the store or the
database refused; the cause is in the daemon log). `409
MEMORY_AUTHORITY_CONFLICT` and `409 MEMORY_CONFIRM_REFUSED` are the model's
tool-surface refusals — a tool call archiving a `user` note, editing a
suggestion or a note a forget took out of recall, a confirm from a tool call —
and this user-directed surface never produces them.

**A note is at most 32 KiB, so a read is never cut.** The limit covers the whole
file, frontmatter included, and it is no larger than what a read serves, so
every note a route can reach comes back whole, with `truncated: false`. A note
file made longer by hand does not parse: it is malformed, left on disk and
excluded, so the daemon answers for its id exactly as for an unknown one — a
read, an `edit` and a `DELETE` are `404 MEMORY_NOT_FOUND`, and nothing is
written — while `GET /v1/memories/doctor` lists the file under `malformed` with
the reason `oversized`. Shortened by hand, it is a note again at the next
scan. That is why an edit can never send back a body cut short (a `patch`
body is capped at 32 KiB as well), and why no separate refusal exists for one.
A client that ever reads `truncated: true` must not offer that body for
editing; `openalpaca memory edit` and the app both refuse.

Every write through these routes — like every write the model's tools and the
extractor make — is announced as one `memory_changed` frame on `/v1/events`
(see [Event Taxonomy](#event-taxonomy)), and so is every file the reconciliation
scan finds edited behind the daemon's back (`change: external`, no `surface`).
The daemon scans every 30 s on its own and before every read; `sync` is for a
client that wants the answer now. `openalpaca memory …` is the CLI over these
routes; `doctor --repair` and `adopt` are local verbs with no route.

### Persona

`USER.md` (`<config>/orchestrator/USER.md`) is what every conversation is told
about you, and it is yours to write: the background learner no longer adds to
it — what it learns is a memory note — so the onboarding conversation's
`update_persona` and these two routes are the only writers.

| Method and path | Purpose |
|---|---|
| `GET /v1/persona/user` | The file as raw Markdown, `{content}`, with `ETag: "<sha256>"` — the lowercase hex SHA-256 of its bytes. |
| `PUT /v1/persona/user` | Replace it: `{content}` plus `If-Match: "<sha256>"` (the header only; `*` means "whatever is there now"). Answers `{etag}` and the same `ETag`. |

Absent `If-Match` is `428 PERSONA_IF_MATCH_REQUIRED`; a hash that is not the
file's current one is `409 PERSONA_REVISION_CONFLICT`, naming the current
revision and carrying it as `ETag`, and nothing is written — a concurrent
onboarding `update_persona` changes the hash, so a stale editor gets the `409`
instead of overwriting it. A body `update_persona` would refuse is
`400 PERSONA_INVALID`. The write takes `update_persona`'s own path (a timestamped
backup, then an atomic replace) and the daemon swaps its in-memory copy itself,
so a `GET` after a `PUT` already reads the new text. `openalpaca persona show|edit
user` and the app's Settings → Persona are the clients.

### Extensions, tools and skills

| Method and path | Purpose |
|---|---|
| `GET /v1/extensions` | Every MCP server and plugin with its state. There is no per-extension `GET`. |
| `POST /v1/extensions/{kind}/{id}/{verb}` | `enable`, `disable`, `reload`, `approve`, `deny`. |
| `GET\|POST /v1/extensions/{kind}/{id}/config` | Plugin config (plugins only; the `GET` redacts secret references). |
| `POST /v1/extensions/{kind}` | Install a plugin or declare an MCP server. |
| `POST /v1/extensions/plugin/validate` | Dry run of a plugin install. |
| `PUT /v1/extensions/{kind}/{id}` | Replace an installed plugin's tree. `mcp` is refused `409 unsupported_for_kind`. |
| `DELETE /v1/extensions/{kind}/{id}` | Remove an orphaned row; `?uninstall=true` is the real removal. |
| `GET /v1/tools` | Read-only tool catalog. No per-tool toggle. |
| `GET /v1/skills` | Read-only skill catalog. |
| `GET /v1/skills/health` | Skill health. |

See [Extensions](#extensions-mcp-servers--plugins) for what the verbs do.
Plugins are loaded from `~/.openalpaca/plugins`; the plugin system is
early-stage. There are no `/v1/plugins*` routes.

## Message Routing (Orchestrator)

Every chat message (any source: GUI, CLI, connectors) is routed by the orchestrator in tiers:

1. **Deterministic commands** (no LLM call):
   - `/status` / `/tasks` — task summary; `/status <task_id>` for one task.
   - `/cancel`, `/pause`, `/resume` — task control. Bare forms (no id) resolve against the lane's active workflows; an explicit id (`/cancel <id>`) targets that task.
   - `/steer <text>` — inject a steering message into the lane's running workflow (guaranteed delivery, bypasses the model; requires `orchestrator.routing.steering_enabled`, default on).
   - `/remember`, `/forget`, `/memories` — the chat memory commands (see [Memory](#memory), *Chat memory commands*): matched before any skill — a slash command or the router's pick — and before the task commands above, which the three words cannot be confused with; the command word in any case and followed by a space or nothing (`/remembering` is not one). Only on an attended turn of the app's or the CLI's chat; every other turn is answered with a fixed sentence and changes nothing. Their names are reserved: a skill claiming one as its slash command, an alias or its id is not reached by that slash, and the skill loader logs one `WARN` naming it.
   - `/<skill> [args]` — invoke a skill by slash command, alias, or skill ID (directory name); skills can also be selected by the weighted skill router.
2. **Social fast path** — trivial acknowledgements ("ok", "thanks") answered with an ultra-light prompt.
3. **Main loop** — everything else, including messages sent while workflows run. The model answers directly or calls routing tools: `start_workflow` (background workflow), `steer_workflow` / `queue_followup` (offered while the lane has active workflows), `task_status`, and memory tools. Workflows run in the background under a lead agent that can spawn subagents; concurrency is capped per lane (`max_workflows_per_lane`, default 3). On completion the workflow posts a model-authored completion report to the lane, and queued follow-ups auto-start (`followup_autostart`, default on).

Tunables live under `[orchestrator.routing]` in `config/daemon.toml` (steering, per-lane workflow cap, follow-up autostart, main-loop round/tool budgets, tool-surface selection, scheduled-skills kill switch).

### Conversation continuity

Each conversation keeps a **working state** on its session row: where its work
stands, as one fixed-shape JSON object — `topic`, `goal`, `done`, `asked`
(the questions you asked and had answered), `decisions`, `open`, `next` and
`refs` (task ids and artifact paths), in the user's language. A turn's model gets it as the first message of its history, as
quoted reference data, never trimmed, at most `[orchestrator.memory]
summary_max_chars` (4,000) characters, its heading saying when it was
written. It stands in for the turns the model no longer sees: the recent
window holds `prompt_recent_messages` rows (40 in the shipped `daemon.toml`).

One writer keeps it, with one prompt, one model and one daily ceiling, at
three moments:

- **Compaction.** When a turn's context nears the model's window, the
  compactor's `LlmSummary` tier folds the older messages into the state with
  one call, and the state reaches the session row at turn end with no other
  call. The next turn's window then starts after the rows the state covers,
  keeping `[execution.context] min_recent_messages` (4) rows verbatim.
- **Turn end.** Otherwise, after a turn, the state is refreshed only when
  `[orchestrator.memory] summary_min_new_older_messages` (12) rows have
  scrolled out of the recent window past what it covers.
- **Close.** When a conversation has been quiet for 30 minutes, when you
  switch conversations, and in the stop drain and the boot catch-up, the
  learning queue refreshes the state — only when a turn of yours lies past
  what it covers — and then projects the conversation's episode note from it
  (see [Memory](#memory)). `[orchestrator.learning] enabled = false` turns the
  close off with the rest of learning.

Every call is made on `[orchestrator.costs] summary_model` (unset: the
router's default model), asks for no reasoning, answers at most 1,536 tokens,
is recorded under the label `orchestrator_summary` and is checked first
against `summary_max_daily_cost_usd` — the compaction's call included, so it
no longer counts against the turn's $1 or the workflow's cap. An answer that
is not a state leaves the row as it was; at a compaction the turn goes on over
the heuristic summary. A history clear (`DELETE /v1/chat/history`) empties the
state.

**A new conversation carries the previous one.** On the same lane and
project, a new conversation is given the previous conversation — the lane's
last closed one with the same project, never an older one — for its first
`[orchestrator.memory] carry_over_turns` (default 3, `0` = never) user turns,
until it has a state of its own: its state and its last exchange, under a
heading naming the episode note that records it (for the model to
`memory_read`) or, before its close, the date `memory_timeline` will list it
under. It is read live on each of those turns. A conversation created with
`POST /v1/sessions {"inherit": false}` — the GUI's **Start fresh** — is never
given it. The turn's `done` frame names the conversation it was given
(`continued_from`), and a workflow's lead is given the chat's state after its
objective.

`[execution.context] compaction_model` and `[orchestrator.learning]
episode_max_per_day` are gone (the compactor uses `summary_model`; an episode
note costs no call). A hand-edited `daemon.toml` still carrying either is
read as if it did not.

### Scheduled skills

Skills whose frontmatter sets `invoke.cron` (see the Skill Template Reference) are registered with the wake-module cron scheduler at boot and re-synced on skill hot-reload. Each fire injects the skill's slash command as a fresh turn — through the same gateway as user messages — on the local user's `<user>:scheduled` lane, running as that user so memory and preferences apply and results appear in chat history. `scheduled_skills_enabled = false` under `[orchestrator.routing]` disables all skill cron jobs (hot-reloadable).

## Streaming Surfaces

### WebSocket Events

- Endpoint: `GET /v1/events?token=...`
- Payload: `openalpaca_api::events::ServerEvent`
- Includes operational, task, agent, security, and orchestration events.

**Shutdown.** When the daemon begins shutting down, every open socket receives
one `daemon_shutting_down` frame — `{"type", "grace_secs", "ts",
"instance_id"}`, where `grace_secs` is the force-exit window (10) — and is then
closed with WebSocket code **1001** ("going away"), reason
`daemon_shutting_down`. The frame is composed **per socket** rather than
broadcast, because the two race: a socket taking its shutdown arm in the same
tick would never forward a broadcast frame, and a client that learns of a stop
only by watching its socket die cannot tell a deliberate stop from a crash.
Before this frame existed the socket simply stayed open and silent until the
process exited under it. The frame is not persisted to `event_log`, and
**nothing is replayed** — the socket is best-effort by design, so a shutdown
mid-stream is one more gap and the usual resync-and-refetch covers it. A client
that does not know the frame (`openalpaca daemon tail` prints it as an unknown
event) still sees the 1001 close.

### SSE Chat Stream

1. `POST /v1/chat` starts the turn and answers `{stream_id, lane_key,
   model_used}` immediately.
2. `GET /v1/chat/stream/{stream_id}?token=...` delivers the turn.

The stream stays available for 5 seconds after its terminal frame, for a late
subscriber; after that the id answers `404 STREAM_NOT_FOUND`. Keep-alive
comments are sent every `[server] sse_keep_alive_secs` (default 15).

| SSE event | Data | Meaning |
|---|---|---|
| `thinking` | `{}` | The turn started. Sent once, whether or not the model reasons. |
| `reasoning` | `{"text": "<chunk>"}` | The model thinking out loud. Live only. |
| `delta` | `{"content": "<chunk>"}` | A piece of the answer, as the provider produces it. |
| `confirmation_requested` | `{"request_id", "tool_name", "tool_arguments"}` | A tool is waiting for approval. Does not end the stream. |
| `confirmation_resolved` | `{"request_id", "outcome"}` | That prompt is no longer pending. Does not end the stream. |
| `done` | see below | The turn finished. Terminal. |
| `error` | `{"message": "..."}` | The turn failed, or the daemon is shutting down. Terminal. |

The `done` payload:

| Field | Always present | Meaning |
|---|---|---|
| `content` | yes | The whole answer. Authoritative. |
| `model` | yes | The model that answered — the one the router actually called, not the one requested. `"default"` for a turn that reached no model. |
| `tokens_in`, `tokens_out` | yes | Token counts for the turn (`0` when no model ran). |
| `duration_ms` | yes | Wall-clock time of the turn. |
| `attachments_used` | no | Ids of the turn's files that the model's request really carried. |
| `attachments_skipped` | no | `[{"id", "reason"}]` — the turn's files that did not reach the model, each with a sentence saying why. |
| `memory_used` | no | `[{"id", "revision", "reason"}]` — the memory notes the turn delivered to the model: `reason` is `pinned` or `recall` for the automatic per-turn recall, `index` for a note it only listed in its index block (`recall_mode = "index"`: the line reached the model, the body did not), `tool` for a note the model fetched itself through `memory_search`, `memory_timeline` or `memory_read` (an index row counts: its title reached the model). Live only: nothing persists it, and `GET /v1/chat/history` does not serve it. |
| `delegation` | no | `{"task_id", "title"}` when the turn started a background workflow, so a client can track the run without parsing prose. |
| `working_state_updated` | yes | `true` when the turn compacted, so the conversation's working state changed: its session is worth re-reading. |
| `continued_from` | yes | `{"session_id", "topic"}` — the previous conversation the turn was given ([Conversation continuity](#conversation-continuity)), the topic `""` when it left no state — or `null`. Live only. |

The optional fields are omitted when they are empty.

**The deltas are real.** A `delta` carries the provider's own tokens as they
arrive. A **file-based skill** (a `/slash` or router-selected skill) streams the
same way as the main loop. A turn that streams nothing sends exactly one
`delta` with the finished answer. That covers:

- a tier that answers without a model (task commands, `/steer`, a memory
  command, a withdrawn skill's notice);
- a **plugin-contributed** skill — the plugin protocol hands back a finished
  answer, so there is nothing to stream;
- a provider without streaming;
- a stream that failed and was answered by the non-streaming fallback.

The keys that used to pace a simulated stream,
`server.chat_streams.stream_chunk_words` and `stream_chunk_delay_ms`, no longer
exist. A `daemon.toml` still carrying one gets a boot `WARN` and the key is
ignored.

**`done.content` is authoritative, and the deltas need not add up to it.** A
multi-round turn streams the text the model wrote before a tool call, and a
broken stream is followed by the whole answer in `done`. A client renders the
accumulated deltas as a live preview and replaces them on `done`. The CLI
reconciles the difference so a terminal ends on the answer exactly once, and
writes nothing into a pipe before `done`.

**Reasoning is shown, never kept.** `reasoning` carries Anthropic's extended
thinking, and an OpenAI-compatible provider's `reasoning` /
`reasoning_content` delta — which is what a local thinking model on Ollama
emits. The field is `text`, not `content`, deliberately: it is not part of the
answer, nothing persists it, and `GET /v1/chat/history` never replays it. A
client that wants to show it must show it live. The GUI puts it in the thinking
indicator; the CLI prints it dim on a terminal and not at all into a pipe.

**A turn never ends with nothing.** A turn that finishes without answer text —
it ran out of tool rounds, hit its cost cap, was truncated, wrote nothing, or
errored — answers with one runtime-authored line naming the reason and, where
there is one, the last tool error. A cancelled turn is the exception. For every
exit but an error, that line is the turn's ordinary answer: stored as the
assistant message, sent as the one fallback `delta`, carried on `done.content`.

**A failed turn still fails.** When the turn errored before it wrote anything,
the same line is the message, but the error channel is kept: the stream's
terminal frame is `error` rather than `done`, and no assistant message is
stored.

**A shutdown ends the stream.** When the daemon begins shutting down — `POST
/v1/command {"command":"shutdown"}`, SIGINT or SIGTERM — an open stream that
has not yet sent its `done` or `error` gets one last `error` frame and ends:

> The daemon is shutting down, so this stream ended before the turn finished.
> If the turn had started a workflow, that run comes back as `interrupted` when
> the daemon starts again — rerun it to pick the work back up.

Anything already buffered is delivered first, so a turn whose `done` was in the
buffer is told it finished, not that it was cut off; a stream that has already
sent its terminal frame just ends. Nothing the turn produces after the farewell
is relayed. Until this existed, an open stream held the shutdown open until the
10-second watchdog force-exited the daemon (see
[Startup and Lifecycle](#startup-and-lifecycle)).

**A turn never claims a run it did not start.** Before a main-loop answer is
returned, the daemon checks any task id the answer states. If no
`start_workflow` call succeeded in this turn and the id matches no run of this
user, the model gets one corrective round. If the id is still there afterwards,
the daemon **appends** one line beneath the model's answer: *"Note from
OpenAlpaca: no workflow was started in this turn, and no task with id `<id>`
exists. Ask again to start one."* The answer itself is never removed. Because
the first answer's deltas were already sent, this is one more reason a client
must end on `done.content`. Details: [Agent Loop](agent-loop.md#the-run-claim-guard).

### Attachments

A file reaches a turn in two steps: `POST /v1/files/upload` answers an `id`,
and `POST /v1/chat` names it in `attachments`. What happens next depends on the
model that will **actually answer** — resolved through the same ladder as
[the effective model](#the-effective-model), not the configured default.

| File | The answering model… | What the model receives |
|---|---|---|
| image | takes images | the image |
| image | does not | a placeholder; the file is reported skipped |
| document | takes documents natively | the document |
| document | does not, and text was extracted | a labelled, fenced text block, cut at `[upload.governance] max_extracted_text_chars` with the cut named inside it |
| document | does not, and no text was extracted | a placeholder; skipped |
| audio | takes audio | the clip |
| audio | does not, and a transcript was extracted | the transcript as text |
| audio | does not, and there is no transcript | a placeholder; skipped |

When nothing is routable, the parts are left untouched and the router's own
"no routable model" error is what the user sees.

Every file of the turn ends up in exactly one of `attachments_used` and
`attachments_skipped`. "Used" means the model's request carried it. A turn
answered without its files says so, with a reason per file:

- task commands, `/steer`, the memory commands, the social fast path and other
  paths that never hand the files to a model;
- a **plugin-contributed** skill, whose protocol carries a plain query and no
  files. A file-based skill does take the turn's attachments.

Each skip also produces one `WARN` in the daemon log. Nothing persists a skip:
it is on `done` and nowhere else.

### Tool confirmations

A tool on the confirm list pauses until somebody answers.

1. The daemon emits `tool_confirmation_requested` on the WebSocket and, when
   the prompt belongs to a chat stream, `confirmation_requested` on that SSE
   stream.
2. A client answers with `POST /v1/chat/confirmations/{request_id}` and the body
   `{"approved": true|false, "approval_scope": "these_args"|"entire_tool"}`.
   `approval_scope` is optional and defaults to `these_args`. An id that is no
   longer pending answers `404`.
3. The daemon emits `tool_confirmation_resolved` (WebSocket) and
   `confirmation_resolved` (SSE) with the outcome.

| Outcome | Meaning | Tool ran? |
|---|---|---|
| `approved` | Somebody said yes. | yes |
| `denied` | Somebody said no. | no |
| `timed_out` | Nobody answered within the timeout. | no |
| `cancelled` | The request was withdrawn without an answer. | no |

Every way a prompt stops being pending is announced — a timeout included — so
a client can always take its prompt down. The timeout defaults to 300 s
(`execution.agent_defaults.confirmation_timeout_secs` in `daemon.toml`) and is
fail-closed.

An approval is remembered for the rest of the loop that asked — one chat turn,
or one agent's run inside a workflow — and no further: `these_args` covers
later calls of the same tool with the same arguments, `entire_tool` covers
every later call of that tool. `[security] auto_approve_confirmations = true` skips
the prompts altogether and writes a `tool_auto_approved` audit row per call. It
is meant for development and testing, and is off by default.

**Per-call prompts.** A request may carry `per_call: true` — only
`memory_confirm` does (see [Memory](#memory) above): its approval covers that
one call. The daemon always asks for it, whatever `auto_approve_confirmations`
or an earlier approval says, never records its approval, and ignores an
`approval_scope`. A request may also carry `preview`: text the daemon composed
from the tool's own read, never from the call's arguments, which a client shows above the
literal `tool_arguments` (`null` for an ordinary prompt). A tool can refuse a
call **before** its card is raised — `memory_confirm` does for a suggestion
that moved, and `memory_update` for any `confirm` — and then no prompt is
emitted at all.

`GET /v1/chat/confirmations` lists the prompts **still waiting**, oldest first:
`{request_id, tool_name, tool_arguments, task_id, agent_id, lane_key, preview,
per_call, raised_at}` (RFC 3339). It answers an empty list, not an error, when the daemon
has no confirmation broker. It is a snapshot — an entry can be answered a
moment later — and it is not filtered by owner. It carries each prompt's
`tool_arguments` and `preview` — for `memory_confirm`, note titles and text —
which the persisted `tool_confirmation_requested` event does not: its
`event_log` row keeps ids only, so a note forgotten later leaves no copy there. Whether the list should be owner-scoped or redact those arguments is an
open owner decision (T22 in [`tasks/api-fix-plan.md`](../tasks/api-fix-plan.md)
§0). `openalpaca tasks confirmations list|watch|approve|deny` is the CLI over
these two routes.

**A client that cannot be asked says so.** A client that will not be around to
answer declares `unattended`:

| Route | Field |
|---|---|
| `POST /v1/chat`, `POST /v1/command`, `POST /v1/tasks`, `POST /v1/lanes/{lane_key}/followups` | `unattended: bool`, default `false` |
| `POST /v1/tasks/{id}/action`, `POST /v1/tasks/{id}/rerun` | `unattended: bool`, optional — absent means the run's own stored declaration, not `false` |

A turn or run that declared it has a confirm-listed tool **refused at once**,
with a message saying where it can be approved, instead of waiting out the
timeout. It is a declaration, never an approval: nothing is pre-allowed by it.

- The CLI declares it when stdin or stdout is not a terminal.
- A scheduled skill is unattended by definition.
- A queued follow-up inherits the declaration of the turn that queued it.
- A workflow's completion report ends with one runtime-authored line naming the
  tools that were refused this way.

Confirmation prompts also reach connector channels. When the originating lane
belongs to Telegram, iMessage or Discord, the connector sends the prompt into
the conversation and the user replies `/yes` (or `/y`) to approve, `/no` (or
`/n`) to deny. (A connector's `/yes` is keyed by chat, not by sender, so it is
not a user-directed answer: `memory_confirm` is never offered on a connector's
lane, and a suggestion is never raised there.) Multiple pending confirmations in one conversation are answered
in FIFO order: a prompt sent while others wait says its place and that a reply
answers the oldest first (`2 of 2 pending; each /yes or /no answers the oldest
first`), and the acknowledgement names the tool it approved or denied
(`Approved: shell_exec. Tool execution will proceed.`), or no tool when the
request had already timed out, been cancelled or been answered elsewhere (in
the GUI or with `openalpaca tasks confirmations`).

Every hidden character in the prompt's tool name and arguments (a bidi override,
a zero-width or control character, a variation selector, any space but a plain
one, an object replacement character — U+FFFC, which Apple devices draw as blank
space — a code point Unicode has not assigned or leaves to private use, which
has no agreed appearance and some of which Apple devices draw as blank space or
as nothing, and in the name also a line break or a tab) is shown as `<U+XXXX>`,
and a line under the arguments counts them (`Note: this request contains 2
hidden characters, shown as <U+…>.`). Those are Unicode's classes — control,
format, default-ignorable, unassigned and private-use characters, and every
blank but the plain space — not what a device's font draws: a letter that a
particular font happens to draw as an empty cell is sent as it is and not
counted (a Mac can draw the CJK compatibility ideographs U+FA2E and U+FA2F so,
through a font it downloads). The arguments are cut after 500 bytes, and
the cut says how much it left out (`... (31 more characters not shown)`).
Discord receives the prompt with its Markdown characters backslash-escaped, so
it reads as written, and neither Telegram nor Discord draws a link preview under
the prompt or the acknowledgement.

## Event Taxonomy

The wire names below are the `type` tag of `ServerEvent`
(`crates/openalpaca_api/src/events/mod.rs`, `rename_all = "snake_case"`); the
list is the whole union. Every frame also carries `ts` and `instance_id`.

- `heartbeat`, `command_received`, `wake`
- `daemon_shutting_down` (`grace_secs`) — sent **per socket** by
  `/v1/events`'s shutdown arm, immediately before a `Close(1001,
  "daemon_shutting_down")`, never through the broadcaster and never written to
  `event_log`
- `task_status`, `agent_status`, `agent_config_changed`
- `workflow_started`, `workflow_progress`, `workflow_steered`
- `subagent_span` — one subagent lane of a run opening or closing: the
  `subagent_span` row itself, so the socket and `GET /v1/tasks/{id}/timeline`
  cannot disagree. `state` is never `blocked` (a blocked lane is derived at read
  time from the confirmation broker)
- `followup_queued`, `followup_cancelled`
- `session_changed` (a session's lifecycle transition, or the `interrupted`
  frame, which names the run it belongs to)
- `artifact_written` (`artifact_id`, `task_id`, `agent_id`, `name`, `kind`,
  `version`, `path`)
- `connector_status`, `key_status_changed`
- `chat_stream_started`, `chat_stream_ended`
- `orchestrator_config_changed`, `daemon_config_changed`
- `security_violation`, `circuit_breaker_tripped`, `tool_executed`
- `tool_confirmation_requested` and its twin `tool_confirmation_resolved`
  (`request_id`, `agent_id`, `tool_name`, `outcome`, `stream_id`, `lane_key`,
  `task_id`) — sent for every way a prompt stops being pending, with `outcome`
  one of `approved`, `denied`, `timed_out`, `cancelled`. The persisted row keeps
  the outcome in its `result` field. The request also carries `preview` (the
  daemon-authored card text, or `null`) and `per_call`; neither is persisted
- `llm_call_completed`, `skill_catalog_updated`, `soul_updated`
- `skill_invocation_started`, `skill_completed`, `skill_failed`
- `extension_state_changed` (an MCP server or plugin changed state), `extension_capability_withheld` (a surface asked for a tool a disabled extension owns), `extension_capability_withdrawn` (a disable/crash took tools away, with the affected templates, skills and cron skills)
- `memory_changed` (`partition_key`, `note_id`, `change` — `created`,
  `updated`, `archived`, `confirmed`, `forgotten` (also sent for an undo and
  for each suggestion a confirm dismisses) or `external` —, `revision`
  (`null` for a forget), `origin`, and `surface` — `user`, `model_tool` or
  `extraction`, **absent** when the daemon's reconciliation scan found the change
  on disk): one frame per write the memory service makes, whichever surface
  asked, and per external change it detects. A client that shows memory
  invalidates on it instead of polling. Never written to `event_log`

`dag_node_status` was **deleted**: `subagent_span` describes the same spawns,
and emitting both meant two frames for one transition. Rows an older daemon
already wrote keep their `event_type` and still list from
`GET /v1/events/history`, so a stored log stays readable — nothing new carries
that name.

Persisted rows use the same words with two exceptions: `agent_status` is
logged as `agent_status_change`, and none of `heartbeat`,
`daemon_shutting_down` and `memory_changed` is persisted at all
(`apps/openalpacad/src/events/persistence.rs`).

## Background Tasks

The daemon runs periodic workers, all cancelled together on shutdown (intervals are read from `daemon.toml` and hot-reloadable unless noted):

- heartbeat event emitter
- memory reconciliation (fixed 30 s interval): a stat scan of every memory partition the owner's control snapshots know; a note file edited, added or removed behind the daemon's back is re-recorded as `external`, the search index follows, and each finding is one `memory_changed` frame. It first runs any rebuild of the search index still owed, and once the index has had no write for a whole interval it merges the index and truncates its write-ahead log — rebuilding it from a new file when it has grown past twice its size after its last rebuild.
- memory index rebuild (once, at start, only when the search index is missing, unreadable or of another version): rebuilds it from the note files on a blocking worker while the daemon already serves, each partition under its writer slot, so a write meanwhile is never indexed with stale derived facts; recall and search — also when a rebuild starts under them — the suggestions list and a deleted conversation's episode notes answer from the note files until it is done, the timeline answers `MEMORY_INDEX_REBUILDING`, and only background learning waits for it; and a rebuild that fails is retried by the next reconciliation
- learning sweeper (fixed 15 s interval): a conversation with turns not yet learned becomes due once it has been idle `[orchestrator.learning] idle_secs` or is no longer its lane's active conversation, and a due one is learned once no chat turn has been in flight for 20 s; a conversation with new turns becomes due for its episode note once it has been quiet 30 minutes or is no longer its lane's active conversation, and the note is written after its facts, when it has at least 4 of your turns (see [Memory](#memory))
- learning boot catch-up (once, 30 s after start): every conversation with turns not yet learned is due, and so is the episode note of every conversation with at least 4 of your turns past its episode cursor
- file-processing worker and asset cleanup (upload governance)
- telemetry cleanup (fixed daily interval)
- chat-stream cleanup (stale SSE streams)

## Storage Model

- SQLite location is resolved by `openalpaca_storage::store::database_path()` — `~/.openalpaca/state/openalpaca.db`. Every path in the layout comes from that module; no crate joins a literal directory name onto a store root.
- Migrations are embedded and applied from `openalpaca_storage::migrations::MIGRATIONS`, and a migration's file number is its schema version. The unreleased development chain was re-baselined on 2026-09-28 into one file, `crates/openalpaca_storage/src/migrations/001_baseline.sql`, which initializes schema version **1** directly and needs nothing beyond SQLite itself — no extension is loaded. It carries the whole schema, the turn's id and project root on each stored message (`conversation_messages.request_id`, `workspace_root`) and the `learning_cursor` table included — how far learning, and episode notes, have read each conversation; no cursor row is written in advance — a conversation without one reads as read by neither yet. It is the only migration: a fresh database opens at 1, and the next migration will be `002_<name>.sql`, registering version 2. A build from before a migration refuses a database that has it, so keep a copy of `state/openalpaca.db` before the first start of a build that migrates it if you may go back. `GET /v1/status` reports the version the open database is actually at.
- Memory is **not** in the database. The notes are Markdown files under `memory/` (see the runtime paths at the top of this manual), each partition's revisions, origins and tombstones are one JSON control snapshot under `state/memory-control/`, and the search index is a separate, disposable SQLite file, `state/cache/memory-index-v1.sqlite`. A backup of your memory is the `memory/` directories **and** `state/memory-control/`; the index can always be rebuilt. A memory write whose index update fails still succeeds — the note is written and announced, the daemon logs a warning, and the next search or the next reconciliation (within 30 s) re-indexes what changed. See [Memory](#memory).
- **If your database is refused.** No migration runs — the schema and its rows are left exactly as they were — and the message names the absolute path of the file. There is one refusal, for a database at a schema version above the newest this build knows:
  - ``Database `<path>` is at schema version <n>, which this build does not know (it creates schema version 1 and knows versions up to 1). Nothing was changed.`` The message goes on to name both builds that could have written the file, because it cannot tell them apart. **A newer OpenAlpaca:** that database is fine and this build is the wrong one — use the build that created it, and do not reset. **A build from before the schema was re-baselined at version 1** (2026-09-28): 42, the complete, working database of the build before memory moved to files, or 43 to 45 from an earlier build of the Markdown memory. Such a database cannot be read or upgraded, and it is not converted: the message names `openalpaca config reset --factory`, which deletes it and `llm.toml`'s keys. The whole procedure — keeping a copy first, reconnecting a model, reviewing `USER.md` — is [Installation Manual → Switching to the Markdown Memory Build](Installation_Manual.md#switching-to-the-markdown-memory-build).

  `openalpaca config reset --factory` deletes that file together with its `-wal` and `-shm` siblings and clears `llm.toml` and `daemon.toml`, after which the next start builds a fresh database at version 1; the artifact, upload and session *files* stay on disk unreferenced, and the memory notes and `state/memory-control/` are not touched (the new owner id leaves them orphaned until `openalpaca memory adopt`). The verb reaches a database in this state precisely because it opens none — it deletes files and asks the schema nothing — and it is the only form of `openalpaca config` that clears one (`set`, `get` and a keyed `reset` on an `ai.*` or `daemon.*` key open no database either, and work meanwhile, but leave it refused); it refuses while a daemon is running, prints the absolute store root, and requires you to type `factory-reset` at a terminal.
- Session logs live at `~/.openalpaca/sessions/<id>/` and are bounded by **size only**:
  - `[orchestrator.sessions] log_max_session_bytes` (default 256 MiB) per session. On exceed the writer drops whole oldest segments, never the live one, and records the `seq` range that went.
  - `log_max_total_bytes` (default 2 GiB) across all of them, swept once at boot, oldest-touched archived session first, never an active one.
  - `log_retention_days` is **reserved and does nothing**. The key is parsed, clamped and reported by `GET /v1/status`, and the daemon warns at boot when it is set to anything but its default (`0`) — but no age-based sweep exists, so setting it to `90` expires nothing. Whether one is built is an open owner decision (T12 in [`tasks/api-fix-plan.md`](../tasks/api-fix-plan.md) §0). Until then a log leaves only by byte cap, by `DELETE /v1/sessions/{id}`, or by a purge.
- A tool result larger than `[orchestrator.sessions] tool_result_inline_bytes` (default 32 KiB) is written whole to the session's `results/` directory, and the model is handed the first 2 KB plus a reference it can page with the `read_result` tool. A loop with no session log cuts the result inline at the same threshold instead. See [Agent Loop](agent-loop.md#tool-results-and-the-spill).
- The `event_log` table is **never pruned**. Every event but `heartbeat`, `daemon_shutting_down` and `memory_changed` is persisted, plus the sandbox's audit rows (`security_violation`, `tool_auto_approved`, and the refusal an unattended run records). The daily telemetry cleanup removes `skill_execution_log` rows older than 90 days and `tool_execution_log` rows older than 7 days, and nothing else. `event_log` rows go only with a factory reset or with the runs a project purge deletes.

## Logging and Operations

Control logging with `RUST_LOG`, for example:

```bash
RUST_LOG=info cargo run -p openalpacad
RUST_LOG=openalpacad=debug cargo run -p openalpacad
```

Graceful shutdown endpoint:

```http
POST /v1/command
{"command":"shutdown"}
```

The answer — `200 {"request_id", "status": "shutting_down"}` — is an
**acceptance, not a completion.** The daemon replies first and raises the
shutdown about 100 ms later, so that the reply can leave; the process is gone
only once the shutdown tail has run, which can take up to the 10-second window
described in [Startup and Lifecycle](#startup-and-lifecycle). Nothing in the
answer says the daemon has stopped. A client that needs to know waits on the
daemon's **process**, then on its **single-instance lock** — never on the port,
which closes first while the lock is still held — as `openalpaca daemon stop`,
`openalpaca daemon restart` and the GUI app's `Stop daemon` all do (15 s,
`openalpaca_storage::daemon_lifecycle`). Starting a new daemon before the lock
is free makes the new one exit at once. The request is behind the bearer token
like every other `/v1/*` route, and it drains nothing: see what a stop
interrupts under [Startup and Lifecycle](#startup-and-lifecycle).

**Where the daemon's output goes.** A daemon started by `openalpaca daemon
start` or by the GUI app writes its stdout and stderr — every `tracing` line,
and every fatal start-up refusal — to `state/logs/daemon.log` under the store
root, rotated at 16 MB with three older generations kept. The GUI sidecar used
to discard that output, so a daemon that refused to start told a bundle-only
user nothing but "did not become ready"; it now shares the CLI's treatment, and
when a start times out the app quotes the end of what that start wrote. Lines
carry ANSI colour codes when read raw (`tracing` writes them even to a file);
the app strips them for display and changes nothing else.

## Troubleshooting

- Daemon already running: check lock/discovery and stop existing instance cleanly.
- The GUI app says the daemon did not start within 5 seconds: the rest of that message is the daemon's own reason, quoted from the end of `state/logs/daemon.log` — the same text the CLI launcher would have written there. Settings → Connection → `Show daemon log` reads more of it. "…and wrote nothing to its log" means the daemon binary never ran: check it is installed beside the app.
- `openalpaca daemon restart` stops the daemon but starts nothing, and exits with status 2: the old daemon was not completely gone 15 s after it was asked to stop, so starting a new one would have lost the single-instance lock. The message says which of two things happened. "`The daemon (PID …) is still running 15s after it was asked to stop, so nothing was restarted.`" — read the daemon log it names (`state/logs/daemon.log` under the store root) to see what it is doing, stop it with the `kill -9 <pid>` it prints, then `openalpaca daemon start`. "`The daemon (PID …) exited, but something still holds the single-instance lock 15s after it was asked to stop, so nothing was restarted.`" — another daemon is probably already running on this store; check `openalpaca daemon status` before starting one. `openalpaca daemon stop` reports the same two cases and exits 2 as well.
- Discovery expired: restart daemon to rotate token and rewrite discovery.
- DB lock/contention: ensure single daemon instance and avoid conflicting external writers.
- Config not loading: verify resolved config directory and presence of expected files.
- GUI/CLI auth failures: ensure they read the current discovery token and instance.
- "No model is available: no enabled provider offers one": nothing is routable. Turn a provider on (`openalpaca config set ai.<provider>.enabled true`, or Settings → Models in the GUI), then check `openalpaca llm status`. A cloud provider also needs a key (`openalpaca llm keys add`), and adding a key alone does not enable it. A local Ollama needs no API key, only `ollama pull <model>`. `GET /v1/status` shows the same fact as `llm.effective_default_model: null`.
- A local-model turn stalls, then answers all at once: the stream sat idle for 90 s (usually the model still loading into memory) and the turn was retried without streaming. Warm the model once before a long run. See [Timeouts and output ceiling](#timeouts-and-output-ceiling).
- A tool call "timed out after 300s": nobody answered its approval prompt. Answer it from the GUI or with `openalpaca tasks confirmations list|approve|deny`; an API client that cannot answer should send `unattended: true`, so the call is refused at once instead (the CLI does this by itself when it is piped). See [Tool confirmations](#tool-confirmations).
