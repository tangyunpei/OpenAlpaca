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
- self-description: `~/.openalpaca/README.md` (what every entry is, with a retention class) and `.layout` (layout version and this install's id), both seeded when the store is created
- content, home scope: one directory per content kind under the root, created on first use — `artifacts/`, `uploads/`, `sessions/`, `memory/`, `skills/`, `scratch/`, `cache/`
- embedding model cache: `~/.openalpaca/state/cache/fastembed` — where the
  local embedding backend puts the ~1 GB model it downloads on first use
  (`store::embedding_cache_dir()`, created on demand). Regenerable: deleting it
  costs one re-download. A store that cannot be created falls back to the
  library default with a `WARN` naming it, rather than dropping a gigabyte
  under the daemon's working directory
- a project's own store: `<project>/.openalpaca/` — the same content shape, so an artifact of a project lives beside the project
- daemon log (CLI-managed startup): `~/.openalpaca/state/logs/daemon.log` —
  appended across restarts and rotated by `openalpaca daemon start` when it is
  past 16 MB (`daemon.log.1` … `.3`, oldest dropped), so it costs at most four
  files. `GET /v1/status` reports the path only for a daemon that
  `openalpaca daemon start` launched. A daemon started any other way
  (`cargo run`, the GUI sidecar) writes no log file and reports `null`, even
  when an older `daemon.log` is still there. When `openalpaca gui start`
  launches the GUI from a source checkout (`bun run tauri dev`), its output
  goes beside it as `gui.log`.

## Startup and Lifecycle

1. Parse the command line (none expected; `--help`/`--version` print and exit here), then initialize tracing/logging.
2. Seed the `~/.openalpaca` home store, then move a legacy app directory into it
   — once, before the lock is taken, because the lock file itself moves
   (`store::ensure_store` + `store::migrate::move_app_root`; see
   [Installation Manual](Installation_Manual.md#migrating-from-the-old-data-directory)).
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
10. Start orchestrator, wake manager, plugin manager, MCP clients, connectors, hot reload, background workers, and HTTP router.

Shutdown can be initiated by signal handling or daemon command endpoint. A watchdog force-exits the process (exit code 1) if graceful shutdown takes longer than 10 seconds; after a forced exit, a stale `discovery.json` may be left behind.

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
- The AES-256-GCM master key lives at `~/.openalpaca/state/.master_key` (`store::master_key_dir()`); a key left in a legacy app directory is moved there by the boot-time mover. The daemon exports it as `OPENALPACA_MASTER_KEY` for its own process; startup fails hard if the key cannot be ensured.
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
| Memory extraction, per day | $0.25 | `[orchestrator.costs] extract_max_daily_cost_usd` |
| Conversation summaries, per day | $0.50 | `[orchestrator.costs] summary_max_daily_cost_usd` |
| Task-output extraction, per day | $0.50 | `[orchestrator.costs] task_extract_max_daily_cost_usd` |

An agent template's `max_cost_per_task` replaces the default for that agent.
Seven of the nine shipped templates set one: `lead_agent` sets `3.0`, so a
stock install caps a workflow's lead at $3, not $5, and the subagent templates
range from `0.25` (`explore_agent`) to `5.0` (`general_agent`). `system_agent`
and `writing_agent` set none and run at the $1 default.

There is no overall daily budget for turns or workflows. Only the three
background jobs in the table carry a daily ceiling: a job whose running spend
has passed it is skipped. That running total is kept in memory and is seeded
from today's usage rows when the daemon starts.

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
| `GET /v1/status` | Where the daemon keeps things and how it is doing: store root, state dir, database path, project root, start time and uptime, schema version, `log_path` (only when the CLI manages the log), upload and artifact bytes, the session-log limits and the last boot sweep, `routing.resume_enabled`, and the `llm` block described under [The effective model](#the-effective-model). |
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
| `DELETE /v1/chat/history` | Empties one conversation and its summary. The session itself survives — `DELETE /v1/sessions/{id}` removes one. |
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
| `GET\|POST /v1/sessions` | List or create conversations. |
| `GET\|PATCH\|DELETE /v1/sessions/{id}` | Read one, change its title or bind it to a project, or delete it. |
| `GET /v1/sessions/{id}/messages` | Its transcript. |
| `GET /v1/sessions/{id}/events` | Its session log (JSONL records), paged by `seq`. |
| `POST /v1/sessions/{id}/activate` | Make it the lane's active conversation. |
| `POST /v1/sessions/{id}/archive` | Archive it. |

A lane holds many sessions with exactly one `active`. `DELETE
/v1/sessions/{id}` takes the transcript with the rows:

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
| `POST /v1/workspaces/purge` | Delete a project's conversations, runs and uploads, and answer the plan it followed. `dry_run` defaults to **true**. It removes the session-log directory of every session it purges. |

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
   - `/<skill> [args]` — invoke a skill by slash command, alias, or skill ID (directory name); skills can also be selected by the weighted skill router.
2. **Social fast path** — trivial acknowledgements ("ok", "thanks") answered with an ultra-light prompt.
3. **Main loop** — everything else, including messages sent while workflows run. The model answers directly or calls routing tools: `start_workflow` (background workflow), `steer_workflow` / `queue_followup` (offered while the lane has active workflows), `task_status`, and memory tools. Workflows run in the background under a lead agent that can spawn subagents; concurrency is capped per lane (`max_workflows_per_lane`, default 3). On completion the workflow posts a model-authored completion report to the lane, and queued follow-ups auto-start (`followup_autostart`, default on).

Tunables live under `[orchestrator.routing]` in `config/daemon.toml` (steering, per-lane workflow cap, follow-up autostart, main-loop round/tool budgets, tool-surface selection, scheduled-skills kill switch).

### Scheduled skills

Skills whose frontmatter sets `invoke.cron` (see the Skill Template Reference) are registered with the wake-module cron scheduler at boot and re-synced on skill hot-reload. Each fire injects the skill's slash command as a fresh turn — through the same gateway as user messages — on the local user's `<user>:scheduled` lane, running as that user so memory and preferences apply and results appear in chat history. `scheduled_skills_enabled = false` under `[orchestrator.routing]` disables all skill cron jobs (hot-reloadable).

## Streaming Surfaces

### WebSocket Events

- Endpoint: `GET /v1/events?token=...`
- Payload: `openalpaca_api::events::ServerEvent`
- Includes operational, task, agent, security, and orchestration events.

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
| `error` | `{"message": "..."}` | The turn failed. Terminal. |

The `done` payload:

| Field | Always present | Meaning |
|---|---|---|
| `content` | yes | The whole answer. Authoritative. |
| `model` | yes | The model that answered — the one the router actually called, not the one requested. `"default"` for a turn that reached no model. |
| `tokens_in`, `tokens_out` | yes | Token counts for the turn (`0` when no model ran). |
| `duration_ms` | yes | Wall-clock time of the turn. |
| `attachments_used` | no | Ids of the turn's files that the model's request really carried. |
| `attachments_skipped` | no | `[{"id", "reason"}]` — the turn's files that did not reach the model, each with a sentence saying why. |
| `delegation` | no | `{"task_id", "title"}` when the turn started a background workflow, so a client can track the run without parsing prose. |

Optional fields are omitted when they are empty.

**The deltas are real.** A `delta` carries the provider's own tokens as they
arrive. A **file-based skill** (a `/slash` or router-selected skill) streams the
same way as the main loop. A turn that streams nothing sends exactly one
`delta` with the finished answer. That covers:

- a tier that answers without a model (task commands, `/steer`, a withdrawn
  skill's notice);
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

- task commands, `/steer`, the social fast path and other paths that never hand
  the files to a model;
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

`GET /v1/chat/confirmations` lists the prompts **still waiting**, oldest first:
`{request_id, tool_name, tool_arguments, task_id, agent_id, lane_key,
raised_at}` (RFC 3339). It answers an empty list, not an error, when the daemon
has no confirmation broker. It is a snapshot — an entry can be answered a
moment later — and it is not filtered by owner. It carries each prompt's
`tool_arguments`, which the persisted `tool_confirmation_requested` event does
not. Whether the list should be owner-scoped or redact those arguments is an
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
`/n`) to deny. Multiple pending confirmations in one conversation are answered
in FIFO order.

## Event Taxonomy

The wire names below are the `type` tag of `ServerEvent`
(`crates/openalpaca_api/src/events/mod.rs`, `rename_all = "snake_case"`); the
list is the whole union. Every frame also carries `ts` and `instance_id`.

- `heartbeat`, `command_received`, `wake`
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
  the outcome in its `result` field
- `llm_call_completed`, `skill_catalog_updated`, `soul_updated`
- `skill_invocation_started`, `skill_completed`, `skill_failed`
- `extension_state_changed` (an MCP server or plugin changed state), `extension_capability_withheld` (a surface asked for a tool a disabled extension owns), `extension_capability_withdrawn` (a disable/crash took tools away, with the affected templates, skills and cron skills)

`dag_node_status` was **deleted**: `subagent_span` describes the same spawns,
and emitting both meant two frames for one transition. Rows an older daemon
already wrote keep their `event_type` and still list from
`GET /v1/events/history`, so a stored log stays readable — nothing new carries
that name.

Persisted rows use the same words with two exceptions: `agent_status` is
logged as `agent_status_change`, and `heartbeat` is not persisted at all
(`apps/openalpacad/src/events/persistence.rs`).

## Background Tasks

The daemon runs periodic workers, all cancelled together on shutdown (intervals are read from `daemon.toml` and hot-reloadable unless noted):

- heartbeat event emitter
- embedding indexer (only when an embedder is configured)
- memory importance decay
- file-processing worker and asset cleanup (upload governance)
- telemetry cleanup (fixed daily interval)
- chat-stream cleanup (stale SSE streams)

## Storage Model

- SQLite location is resolved by `openalpaca_storage::store::database_path()` — `~/.openalpaca/state/openalpaca.db`. Every path in the layout comes from that module; no crate joins a literal directory name onto a store root.
- Migrations are embedded and applied from `openalpaca_storage::migrations::MIGRATIONS`. The authoritative list is `crates/openalpaca_storage/src/migrations/` (currently `001` through `042`); `GET /v1/status` reports the version the open database is actually at.
- Session logs live at `~/.openalpaca/sessions/<id>/` and are bounded by **size only**:
  - `[orchestrator.sessions] log_max_session_bytes` (default 256 MiB) per session. On exceed the writer drops whole oldest segments, never the live one, and records the `seq` range that went.
  - `log_max_total_bytes` (default 2 GiB) across all of them, swept once at boot, oldest-touched archived session first, never an active one.
  - `log_retention_days` is **reserved and does nothing**. The key is parsed, clamped and reported by `GET /v1/status`, and the daemon warns at boot when it is set to anything but its default (`0`) — but no age-based sweep exists, so setting it to `90` expires nothing. Whether one is built is an open owner decision (T12 in [`tasks/api-fix-plan.md`](../tasks/api-fix-plan.md) §0). Until then a log leaves only by byte cap, by `DELETE /v1/sessions/{id}`, or by a purge.
- A tool result larger than `[orchestrator.sessions] tool_result_inline_bytes` (default 32 KiB) is written whole to the session's `results/` directory, and the model is handed the first 2 KB plus a reference it can page with the `read_result` tool. A loop with no session log cuts the result inline at the same threshold instead. See [Agent Loop](agent-loop.md#tool-results-and-the-spill).
- The `event_log` table is **never pruned**. Every event but `heartbeat` is persisted, plus the sandbox's audit rows (`security_violation`, `tool_auto_approved`, and the refusal an unattended run records). The daily telemetry cleanup removes `skill_execution_log` rows older than 90 days and `tool_execution_log` rows older than 7 days, and nothing else. `event_log` rows go only with a factory reset or with the runs a project purge deletes.

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

## Troubleshooting

- Daemon already running: check lock/discovery and stop existing instance cleanly.
- Discovery expired: restart daemon to rotate token and rewrite discovery.
- DB lock/contention: ensure single daemon instance and avoid conflicting external writers.
- Config not loading: verify resolved config directory and presence of expected files.
- GUI/CLI auth failures: ensure they read the current discovery token and instance.
- "No model is available: no enabled provider offers one": nothing is routable. Turn a provider on (`openalpaca config set ai.<provider>.enabled true`, or Settings → Models in the GUI), then check `openalpaca llm status`. A cloud provider also needs a key (`openalpaca llm keys add`), and adding a key alone does not enable it. A local Ollama needs no API key, only `ollama pull <model>`. `GET /v1/status` shows the same fact as `llm.effective_default_model: null`.
- A local-model turn stalls, then answers all at once: the stream sat idle for 90 s (usually the model still loading into memory) and the turn was retried without streaming. Warm the model once before a long run. See [Timeouts and output ceiling](#timeouts-and-output-ceiling).
- A tool call "timed out after 300s": nobody answered its approval prompt. Answer it from the GUI or with `openalpaca tasks confirmations list|approve|deny`; an API client that cannot answer should send `unattended: true`, so the call is refused at once instead (the CLI does this by itself when it is piped). See [Tool confirmations](#tool-confirmations).
