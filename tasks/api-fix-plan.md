# Daemon API Fix Plan — project-scoped artifacts + the 23 GUI gaps

**Status:** design, approved-pending-decisions. No production code written. · **Date:** 2026-09-01 · **Branch:** `feat/ui-rework`
**Inputs:** `tasks/gui-api-requirements.md` (the 23-gap brief) · `apps/openalpaca-gui/src/lib/unavailable.ts` (23-entry registry, verified) · `apps/openalpaca-gui/src/lib/api/unbacked.ts` (client contract for the proposed resources) · `apps/openalpaca-gui/API_MAP.md` §3
**Evidence appendices (untracked — `.gitignore:92` blanket-ignores `*.md`; only `tasks/api-fix-plan.md` and `tasks/gui-api-requirements.md` are whitelisted):**
`tasks/research/A-artifact-store.md` · `tasks/research/B-run-data.md` · `tasks/research/C-surface.md`

**Spine:** the user's directive — *artifacts of all kinds live on disk in a project directory under a `.openalpaca/` convention; path naming expanded so files are findable by a human; the DB stores only the address.* §1 designs that. §2–§9 hang the 23 gaps off it.

**Verified against the tree** (not taken from the briefs): migration head is **034** (`crates/openalpaca_storage/src/migrations/mod.rs`, tail `Migration { version: 34, name: "drop_context_compaction_log" }`); `unavailable.ts` holds exactly 23 gap ids; `walk_up_for_marker` prefers `.alpaca` then `.git` (`crates/openalpaca_core/src/memory/workspace.rs:43-56`); `workspace_id_from_root` returns the **canonical path string**, not a hash (`workspace.rs:60-65`); `chat.rs:462` is a literal `approval_scope: None`; `settings.rs:314` is a literal `daily_cost_usd = 0.0`; `list_orphaned` has no `origin` predicate (`repository/file_asset/mod.rs:95-112`); `total_storage_bytes` sums **all** rows (`mod.rs:76-85`); `SystemEvent::DagNodeStarted` **is still produced** at `runner/lead_agent/tools.rs:232-240` — the brief's "producer was deleted" claim is wrong.

---

## 0. Decisions the user must make (do not bury these)

Everything else in this document is settled. These five are genuine either-way calls; the plan is written against the **Recommended** column and each alternative is a bounded delta.

| # | Question | Recommended | The alternative, and what it costs |
|---|---|---|---|
| **D1** | Does `app_dir()` move to `~/.openalpaca/`? | **No — split the roots.** `app_dir()` stays `~/Library/Application Support/OpenAlpaca/` for the DB, `discovery.json`, the lock, `plugins/`, upload assets. A **new, artifacts-only** root is added. | Moving it costs a second `migrate_legacy_app_dir()`-style rename (`paths.rs:24-49`) that must run before the singleton lock and before the DB opens, over a live `plugins/` dir holding user-approved `.permissions.toml`. Users never open `openalpaca.db` by hand; they *do* open artifacts. The directive's findability argument applies to artifacts, not to runtime state. Cost of the split: two "OpenAlpaca directories" to explain — mitigated by a `README.md` dropped in `.openalpaca/` and by `GET /v1/status` reporting both roots. |
| **D2** | Do **chat/connector uploads** also move into `.openalpaca/`, or only agent-produced artifacts? | **Produced-only in phase 1.** One table, one id space, two placements selected by an `origin` column. | Neither inbound path has a workspace signal: `POST /v1/files/upload` reads no header (`routes/files.rs:24-27`) and the connector path is a Telegram/Discord message (`connectors/src/common/mod.rs:213-262`). Owner-scoped sha256 dedup (`files.rs:172-185`) is load-bearing and would become scope-local. Re-homing has no user-visible payoff — the user still has the original. **This is the one place the plan may read "artifacts of ALL kinds" more narrowly than intended.** Phase-2 delta if you disagree: read `x-workspace-path` on the upload route, write to `<store>/uploads/`, leave old rows alone. No re-design; the schema already supports it. Note it is **two** writers, not one — collapse the connector duplication into `ArtifactStore` first. |
| **D3** | Does `extracted_text` stay in the DB? | **Yes, relabelled as a derived text index.** Bounded at 50 000 chars (`daemon_config/upload.rs:28`), on the prompt-assembly hot path. | It is arguably "content in the DB" and the directive could be read as excluding it. Moving it to `<store>/.text/<id>.txt` adds an I/O round trip to every attachment turn. Reversible in one column drop plus a path helper; nothing else in the design moves. |
| **D4** | Home fallback: `~/.openalpaca/` (hidden) or `~/OpenAlpaca/` (visible)? | **`~/.openalpaca/`**, matching the directive's wording, with an `OPENALPACA_HOME_STORE` env override. | A hidden dot-dir in `~` is only marginally more findable than `~/Library/Application Support/`. If genuine findability beats the wording, use `~/OpenAlpaca/`. One-line change in `home_store_dir()`. |
| **D5** | `POST /v1/tasks/{id}/action {"action":"start"}` — same task id, or a new one? | **Same id** (200). A row created by `POST /v1/tasks` is user-authored and visible in the Work list; having it vanish and be replaced on "Start now" is bad UX. | Same-id needs `dispatch_lead_agent_with_id` plus an idempotent persist (`TaskRepository::upsert_queued`) instead of `create`. That is the least clean code in the whole plan. The alternative (`start` returns a new id, like `rerun`) is materially cleaner Rust and a small client change. |

Two further calls are **made** here rather than surfaced, because the evidence is one-sided:

- **`daily_cost_usd` comes from the DB, not the cost tracker.** The brief says "`await` the tracker" (GAP-08a). `CostTracker::total_cost()` is never reset at local midnight, so on a long-running daemon it means *since boot*. Shipping that would put a number in Settings that disagrees with what the GUI already computes correctly (`hooks/useUsage.ts:33-42`). Serve `query_daily_usage` for today.
- **`calls_7d` is renamed `messages_7d`** (GAP-17). The only measurable number is inbound user messages grouped by `conversation_messages.source`. Shipping a message count under a call-count name is a fabrication; the rename is a one-line client edit.

---

## 1. The artifact store (the spine)

### 1.1 Layout

```
<project>/.openalpaca/                      ← when a project root resolves
  README.md                                 ← written once; explains the directory
  .gitignore                                ← contains exactly ".versions/"
  artifacts/
    2026-09-01-connector-audit-3f2a1b7c/    ← <YYYY-MM-DD>-<task-slug≤48>-<taskid8>
      01-connector-audit-findings.md        ← HEAD, currently v2
      02-migration-plan.md                  ← HEAD, v1
      03-screenshot-settings-drawer.png
      .versions/
        01-connector-audit-findings/
          v1.md                             ← SUPERSEDED only; head is never duplicated
    loose/2026-09-01/
      01-weekly-report.html                 ← produced outside any task (main loop)

~/.openalpaca/                              ← fallback when no project resolves (D4)
  artifacts/  README.md

~/Library/Application Support/OpenAlpaca/   ← UNCHANGED (D1)
  openalpaca.db  discovery.json  openalpacad.lock  assets/  plugins/  config seeds
```

**The daemon CWD is never used to place an artifact.** Under a Tauri sidecar or a LaunchAgent it is arbitrary; landing files there is worse than the status quo. CWD-derived workspace ids remain fine for memory scoping (existing behaviour at `orchestrator/handlers.rs:90-97`) but must not decide where bytes land.

### 1.2 Path grammar

```
run_dir  := <YYYY-MM-DD> "-" slug(task_title, 48) "-" taskid[0..8]      ≤ 68 bytes
file     := NN "-" slug(name, 60) "." ext                                ≤ 72 bytes
slug     := [a-z0-9]+ ("-" [a-z0-9]+)*
ext      := [a-z0-9]{1,8}
version  := <run_dir>/.versions/<stem>/v<N>.<ext>
```

- **Date first** so `ls` sorts chronologically. **UUID8 suffix** so a run dir is collision-free and greppable from a task id (task ids are UUIDv4, `dispatcher/lead_agent.rs:32`).
- **`NN-` sequence prefix** assigned at first creation and retained across versions, so `ls` shows production order and a re-write does not reshuffle. Two digits, widening to three past 99.
- **Head lives at the clean path.** This is what makes `open`, Reveal in Finder, `grep` and `git diff` work — and it lets `POST /v1/files/{id}/open` skip the `$TMPDIR` staging copy (`routes/files_types.rs:92-118`), which exists *only* because content-addressed paths have no extension.
- **Superseded versions go to `.versions/<stem>/vN.<ext>`.** Rejected: `name.v1.md` siblings (clutters the directory the human browses); an all-versions subdir including head (doubles bytes, makes "which file do I open" ambiguous).
- **Slugification:** NFKD → drop combining marks → transliterate to ASCII → **lowercase** (required, not cosmetic: APFS is case-insensitive, so `Findings.md` and `findings.md` are the same file) → collapse non-`[a-z0-9]` runs to a single `-` → trim → truncate to 60 bytes on a char boundary → empty ⇒ `artifact` → Windows reserved device names (`con`, `prn`, `aux`, `nul`, `com1..9`, `lpt1..9`) get a `_` prefix.
- **Traversal safety falls out of the grammar** — `/`, `\`, `.`, `..`, NUL and control chars are all removed, so a slug cannot contain a separator. Belt and braces: confine the final path to the store root with the canonicalize-the-parent technique already used at `tools/builtins/helpers/mod.rs:65-117` (it handles the not-yet-existing-file case correctly).
- **Write protocol:** write `.<stem>.tmp` → `fsync` → rename current head into `.versions/<stem>/v<N-1>.<ext>` → rename tmp to head. Rename-based, so a crash leaves either the old head or the new one, never a truncated file.

**`.gitignore` decision, stated plainly: do not ignore `.openalpaca/` wholesale.** Ignore only `.versions/`. A produced `findings.md` is a document; versioning it in git is strictly better than versioning it in OpenAlpaca's private history, and hiding it from the tool the user already has defeats the directive.

**Marker precedence** (`memory/workspace.rs:43-56`) gains `.openalpaca` ahead of `.alpaca` and `.git`. Safe — no install has one yet. **Deliberate side effect to document:** writing the first artifact makes that directory a workspace root for memory scoping. This is desirable — the artifact write is exactly the moment the directory becomes an OpenAlpaca project — but it must be intentional, not incidental. (Drop the redundant `.exists() || .is_dir()` on the `.alpaca` arm while in there.)

### 1.3 New surface in `crates/openalpaca_storage/src/paths.rs`

```rust
pub enum ArtifactScope { Project(PathBuf), Home }

pub fn home_store_dir() -> anyhow::Result<PathBuf>;              // ~/.openalpaca, honours OPENALPACA_HOME_STORE
pub fn project_store_dir(project_root: &Path) -> PathBuf;        // pure; no I/O
pub fn artifacts_dir(scope: &ArtifactScope) -> anyhow::Result<PathBuf>;  // creates + seeds README/.gitignore
pub fn run_dir(scope: &ArtifactScope, created: DateTime<Utc>, task_title: &str, task_id: &str) -> anyhow::Result<PathBuf>;
pub fn loose_dir(scope: &ArtifactScope, created: DateTime<Utc>) -> anyhow::Result<PathBuf>;
pub fn artifact_file_name(seq: u32, title: &str, ext: &str) -> String;
pub fn slugify(input: &str, max_bytes: usize) -> String;         // pure, total, never empty
pub fn version_file_path(head_path: &Path, version: u32) -> anyhow::Result<PathBuf>;
pub fn artifact_extension(kind: ArtifactKind, mime: Option<&str>, name_hint: Option<&str>) -> String;
pub fn confine_to_root(root: &Path, candidate: &Path) -> anyhow::Result<PathBuf>;
```

`asset_storage_path` (`paths.rs:72-81`) is **unchanged** — per D2 uploads stay content-addressed. Its doc comment gains a line saying it is the *upload* path.

Extension precedence: (1) an extension already on the model-supplied name if allow-listed; (2) mapped from `kind`; (3) mapped from `mime_type`; (4) `.bin`. `markdown→md`, `plan→md`, `terminal→log`, `table→csv`, `html→html`, `image→` from mime, `code`/`binary`→ from the name else `txt`/`bin`. `ArtifactKind`'s `serde(rename_all="snake_case")` spellings must match `unbacked.ts:28-38` exactly.

### 1.4 The DB holds addresses

Principle: **`project_root` + `rel_path` are the address of record; `storage_path` stays the resolved absolute path** so `routes/files.rs:315`, `files_types.rs:142-145` and `background.rs:339` need zero changes.

**One table, not two.** `file_assets` is extended; there is **no** parallel `artifacts` table. This is the first cross-lens conflict and it is resolved in favour of extension:

- One content route, one id space, one client `Artifact` type — no client-side merge of uploads and produced files in the Library.
- It unblocks GAP-23 for free: `conversation_message_attachments.file_id REFERENCES file_assets(id)` (`migrations/028_message_attachments.sql`) can carry artifact links via its existing `role TEXT NOT NULL DEFAULT 'attachment'` column. A separate `artifacts` table would have had that FK block reuse and force a sibling link table.
- `origin` (`'upload' | 'produced'`) selects the placement strategy; everything else is shared.

**Note the real blob-in-DB offender is not `file_assets`.** It is `TaskWorkspace`: `WorkspaceEntry { content: String, .. }` capped at `max_entry_size: 32768` × `max_entries: 50` (`orchestrator/task_state/workspace.rs:20-47`), persisted as one `state_json` TEXT column (`migrations/016_task_state.sql:1`) that is rewritten under optimistic locking on **every** workspace mutation. Up to 1.6 MB per task. Fixing that (§1.6) is the directive's core win.

### 1.5 Migration 035 — `035_artifact_store.sql`

```sql
-- Migration 035: project-scoped artifact store.
-- Extends file_assets into the unified artifact record; adds version history.

-- Ownership and attribution (GAP-04)
ALTER TABLE file_assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'upload';  -- 'upload' | 'produced'
ALTER TABLE file_assets ADD COLUMN kind TEXT;                -- ArtifactKind; NULL for legacy uploads
ALTER TABLE file_assets ADD COLUMN task_id TEXT REFERENCES task(id) ON DELETE SET NULL;
ALTER TABLE file_assets ADD COLUMN agent_id TEXT;            -- runtime instance id, "review_agent::a1b2c3d4"
ALTER TABLE file_assets ADD COLUMN agent_template_id TEXT;   -- "review_agent"

-- The address (the directive)
ALTER TABLE file_assets ADD COLUMN project_root TEXT;        -- NULL => home store
ALTER TABLE file_assets ADD COLUMN rel_path TEXT;            -- HEAD path relative to <store>/artifacts

-- Versions (GAP-05)
ALTER TABLE file_assets ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE file_assets ADD COLUMN version_count INTEGER NOT NULL DEFAULT 1;

-- UI affordances
ALTER TABLE file_assets ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;   -- GAP-12
ALTER TABLE file_assets ADD COLUMN summary TEXT;             -- "+41 −6" / "exit 0 · 1.4s" / "3 rows"
ALTER TABLE file_assets ADD COLUMN missing_since TEXT;

CREATE INDEX IF NOT EXISTS idx_file_assets_task    ON file_assets(task_id);
CREATE INDEX IF NOT EXISTS idx_file_assets_origin  ON file_assets(origin, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_assets_project ON file_assets(project_root);
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_assets_addr
    ON file_assets(COALESCE(project_root, ''), rel_path) WHERE rel_path IS NOT NULL;

CREATE TABLE IF NOT EXISTS artifact_versions (
    artifact_id     TEXT    NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    rel_path        TEXT    NOT NULL,   -- '.versions/<stem>/v1.md', or = head rel_path for the head
    sha256          TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    note            TEXT,               -- model-authored "why this version"
    author_agent_id TEXT,               -- NULL => a human edited the file by hand
    added_lines     INTEGER,            -- NULL on v1
    removed_lines   INTEGER,            -- NULL on v1
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (artifact_id, version)
);
CREATE INDEX IF NOT EXISTS idx_artifact_versions_artifact ON artifact_versions(artifact_id, version DESC);

-- The project a run belonged to. Resolves Lens B's open Q2; makes `rerun` faithful
-- and lets the Library filter by project without a join through file_assets.
ALTER TABLE task ADD COLUMN workspace_id TEXT;
CREATE INDEX IF NOT EXISTS idx_task_workspace ON task(workspace_id);

UPDATE schema_version SET version = 35 WHERE version = 34;
```

Registered as `Migration { version: 35, name: "artifact_store", sql: include_str!("035_artifact_store.sql") }`.

SQLite legality, checked: `ADD COLUMN` with `REFERENCES` is legal **only with a NULL default**, which `task_id` has — and `foreign_keys = ON` (`database/mod.rs:62`), so it is enforced. `ADD COLUMN … NOT NULL DEFAULT` is legal for the non-NULL columns. Partial unique indexes need SQLite ≥ 3.8.0.

### 1.6 Two mandatory same-PR bug fixes

Migration 035 creates two live defects the moment it lands. **Both must be in the same commit as the migration.**

```sql
-- repository/file_asset/mod.rs:95-112 — orphan sweep (background.rs:308-357, every 6 h, 24 h grace).
-- WITHOUT THIS the sweep deletes every produced artifact AND its file, because a
-- produced artifact is never linked to a conversation message.
   WHERE a.id IS NULL
     AND f.origin = 'upload'      -- ← ADD
     AND f.pinned = 0             -- ← ADD
     AND f.created_at < datetime('now', ?1)
```

```sql
-- repository/file_asset/mod.rs:76-85 — quota, read at routes/files.rs:84-97 against a 500 MB cap.
-- WITHOUT THIS agent output written into the user's own project counts against the
-- upload quota and starts rejecting uploads.
SELECT COALESCE(SUM(size_bytes), 0) FROM file_assets WHERE origin = 'upload'
```

Produced artifacts are **never** garbage-collected. They live in the user's project and are the user's files. If retention is ever wanted it goes behind an explicit config key, defaulted off.

### 1.7 The producer — where artifacts actually come from

Today **nothing** creates a `FileAsset` for agent output; the only two producers are the HTTP upload route (`routes/files.rs:220-247`) and the connector attachment path. Two additions:

1. **`artifact_write(name, kind, content, note?, summary?, metadata?) -> { artifact_id, path, version }`** — a new builtin next to `file_write`, capability `artifact_write`. **Critical detail:** it must resolve its scope from `ToolContext.workspace_id`, **not** from the startup-captured `workspace_root` that `file_write` uses (`services/tools.rs:37-39` → `tools/builtins/mod.rs:249-251`). This is the first per-request file writer in the codebase. Attribution comes from `ToolContext.task_id` / `agent_id`.
2. **`workspace_write(entry_type="artifact")` spills.** Content goes to the store; the entry keeps `file_asset_id` plus a 512-char preview for prompt assembly (`format_for_prompt` truncates at 2000 anyway — `task_state/workspace.rs:157`).

The payoff chain: `ArtifactPointer.file_asset_id` (`task_state/outcome.rs:148-161`) resolves for the first time → `task.artifact_count` becomes meaningful → `GET /v1/tasks/{id}` gains real artifact references → **and `deliver_artifacts` (`apps/openalpacad/src/notification/artifacts.rs:55-110`) starts working.** That function already walks `outcome.artifacts`, resolves each `file_asset_id` and sends the file to file-capable connector channels; it silently `continue`s today because the id is always `None`. Already-written, currently-dead feature, switched on for free.

New caps in `[execution.artifacts]`: `max_artifact_bytes` default **10 MB** (matching `file_write`'s `MAX_FILE_WRITE_SIZE` at `tools/builtins/file_ops.rs:102-110`, **not** the 50 MB upload cap — this content comes out of a model's context window), `max_versions_per_artifact` default 20 (prune the oldest `.versions/` file and its row; head is never pruned).

### 1.8 The prerequisite nobody wrote down: where does the project come from?

**A project-scoped store needs a project, and no client sends one today.** `POST /v1/chat` reads `x-workspace-path` (`routes/chat.rs:65-68`) and `POST /v1/command` reads a `workspace_path` field (`routes/command.rs:78-80`), but grepping `apps/openalpaca-gui/src` and `apps/openalpaca/src` finds only the *definition* at `lib/chat-stream.ts:349-366` — **no call site sets it**, and the CLI never sends one. So `handlers.rs:90-97` always takes the else branch.

Phase 2 must therefore ship, in this order:

1. `home_store_dir()` fallback, and never the daemon CWD for placement. *(store-side, required)*
2. **The GUI sends `x-workspace-path`** on `POST /v1/chat` when the user has chosen a project. The plumbing exists; only a caller is missing.
3. `task.workspace_id` persisted at dispatch (migration 035) so a run remembers its project across a restart and a rerun.
4. `GET /v1/status` reports the resolved project dir alongside `data_dir` / `db_path`.

A fuller project concept (`GET /v1/projects` over `DISTINCT project_root`, `POST /v1/projects/activate`) is **explicitly out of scope** (§9) — items 1–4 are sufficient for the Library. Whoever adds it later must reconcile with `SkillCatalog::scan_multi_scope(user_dir, project_dir)` and `SkillScope` (`orchestrator/skill/catalog/mod.rs:154`), which already implement a project-vs-user scoping mechanism, rather than inventing a second path resolution.

### 1.9 Integrity and lifecycle

| Concern | Design |
|---|---|
| File missing | `resolve_content` stats before serving; on absence sets `missing_since` and returns **410** `{error:{code:"ARTIFACT_GONE",message,path}}`. List rows carry `missing:true` so the Library strikes them out instead of 404-ing a whole view. |
| Project moved | `rel_path` + `project_root` make re-basing one statement: `ArtifactStore::rebase_project(old,new)`. |
| Project deleted | Rows survive as `missing`. No auto-deletion — deleting a project is not a request to lose the index of what was made. `?include_missing=` (default `false`) keeps the Library clean. |
| User edits a file by hand | Explicitly supported; it is the point. `sha256`/`size_bytes` go stale; the next `put` (or `verify`) detects the mismatch and records the edit as a version with `author_agent_id = NULL`. **Phase 6** — it is what makes "committable artifacts" honest. |
| Concurrent writes | Two agents writing the same name in one run serialize on `ArtifactStore::put`: version rotate inside one `with_connection` transaction, tmp+rename for the bytes. Last writer is head; both survive in `.versions/`. |
| Size accounting | Two numbers, never one: `upload_bytes` (quota-bearing) and `produced_bytes` (informational, per project). Both on `GET /v1/status`. |
| Two daemons on one project | Not designed for. The unique index is per-DB, so two daemons over a shared volume each hold half the index. A singleton lock already prevents two per machine (`paths.rs:57-59`). Documented limitation. |

### 1.10 HTTP surface

```http
GET  /v1/artifacts?task_id=&kind=&origin=&project_root=&pinned=&q=&include_missing=&limit=&offset=
     200 { "artifacts": [Artifact], "total": 1234 }
GET  /v1/artifacts/{id}                                    200 Artifact | 404
GET  /v1/artifacts/{id}/content?token=<bearer>[&version=N] 200 bytes | 410 ARTIFACT_GONE
GET  /v1/artifacts/{id}/versions                           200 { "versions": [ArtifactVersion] }
GET  /v1/artifacts/{id}/versions/{n}/content?token=        200 bytes
GET  /v1/artifacts/{id}/diff?from=1&to=2                   200 ArtifactDiff | 409 NOT_DIFFABLE
PUT  /v1/artifacts/{id}/pin  {"pinned":true}               200 { "id", "pinned" }
GET  /v1/files/{id}/content?token=<bearer>                 ← same ?token= change on the existing route
```

`Artifact` is a **superset** of `unbacked.ts:39-56`, so the client type compiles unchanged. The additive fields are `origin`, `pinned`, `missing`, and — the payoff of the directive — **`path`, `project_root`, `rel_path`**:

```json
{ "id":"8f3c…", "name":"connector-audit-findings.md", "kind":"markdown",
  "mime_type":"text/markdown", "size_bytes":4120,
  "task_id":"3f2a1b7c-…", "task_title":"Audit the connector layer",
  "agent_id":"review_agent::a1b2c3d4", "agent_template_id":"review_agent",
  "version":2, "version_count":2, "summary":"+41 −6", "metadata":{"added":41,"removed":6},
  "created_at":"2026-09-01T10:04:11Z", "updated_at":"2026-09-01T10:22:03Z",
  "origin":"produced", "pinned":false, "missing":false,
  "path":"/Users/x/dev/proj/.openalpaca/artifacts/2026-09-01-audit-3f2a1b7c/01-connector-audit-findings.md",
  "project_root":"/Users/x/dev/proj",
  "rel_path":"2026-09-01-audit-3f2a1b7c/01-connector-audit-findings.md" }
```

`path` is impossible to serve from a content-addressed sha path. It is what Reveal in Finder, Copy path, and a git-aware user need.

`ArtifactVersion` and `ArtifactDiff` match `unbacked.ts:62-77` field-for-field (the route coalesces `note: NULL → ""`, since the client types it non-null). Diffs are text-only: `kind ∈ {image, binary}` → **409 `NOT_DIFFABLE`**, and the History tab stands alone. `added_lines`/`removed_lines` are computed at **write** time and stored, so only the Diff tab touches a diff engine.

**New workspace dependency: `similar`** (MIT, pure Rust, no build script) in `openalpaca_storage`. The workspace has no diff crate at all today. Alternative if a new dep is unwelcome: a hand-rolled ~120-line LCS unified diff.

**New event** `ServerEvent::ArtifactWritten { artifact_id, task_id, agent_id, name, kind, version, path, ts, instance_id }` — carrying `ts`/`instance_id` from the start, unlike the six plugin variants of GAP-22. Feeds the chat transcript's inline artifact card, the `artifact` tag in the per-run event log (GAP-10), and the design's "`v2 written`" line.

**GAP-11 mechanics:** move **only** the content routes (`/v1/files/{id}/content` and `/v1/artifacts/{id}/content`) out of `protected_routes` into a third merged sub-router beside `chat_sse` (`router.rs:268-280`), validating `?token=` inline exactly as `chat_stream_handler` does at `routes/chat.rs:104-113`. **Authorization is not lost** — `routes/files.rs:287-313` already checks `asset.owner_id != state.local_user_id → 404`; only authentication moves, and `?token=` restores it. Accept **both** header and query so `lib/api/files.ts:23-30` is unaffected. Keep `/v1/files/{id}` (metadata) and `/v1/files/{id}/open` header-authenticated.

### 1.11 What is *not* a breaking change

1. Existing `file_assets` rows take column defaults: `origin='upload'`, `kind=NULL`, `project_root=NULL`, `rel_path=NULL`, `version=1`, `version_count=1`, `pinned=0`. `storage_path` untouched.
2. **No files move.** `assets/<ab>/<cd>/<sha256>` stays exactly where it is. No data migration, no copy, no rollback risk on bytes.
3. `/v1/files/*` is unchanged in shape; `/v1/files/{id}/content` only *gains* `?token=`.
4. `/v1/artifacts` lists uploads too, with `kind` inferred from `mime_type` at read time for legacy rows. No backfill needed.
5. **Rollback to schema 34** loses the columns and `artifact_versions`. Produced artifact *files* survive on disk as a folder of readably-named documents. The store degrades to "a folder", not to nothing — itself an argument for this layout.

---

## 2. Phase 0 — Bugs and one-liners *(no migration; ship first)*

Highest value per line in the document. Each item is independently mergeable.

1. **GAP-01 — `approval_scope`.** `ConfirmationBody` (`routes/chat_types.rs:90-93`) gains `#[serde(default)] pub approval_scope: Option<ApprovalScope>`; `chat.rs:462` passes `body.approval_scope` instead of `None`. `ApprovalScope::{TheseArgs,EntireTool}` already exists with `rename_all="snake_case"` (`security/confirmation.rs:87-97`) and the sandbox already honours it (`security/sandbox/mod.rs:248-256`). **The entire enforcement path is complete; only the HTTP hop drops the field.** The "Always allow" button silently approves once today.
2. **GAP-07 — empty `title`/`name`.** Add `title: String` to the three task variants and `name: String` to `AgentStatusChanged` in `crates/openalpaca_core/src/events.rs`. **Do not** pass `SharedContext` into the bridge as the brief suggests — the bridge spawns at `main.rs:245`, *before* `SharedContext` exists at `main.rs:306`. There are five non-test producer sites and every one already has the value or a `SharedContext` in scope (`task_ops.rs:153-159`, `dispatcher/lead_agent.rs:237-243`, `dispatcher/outcome.rs:268-275` and `:294-299`, `dispatcher/mod.rs:171-176`). Known limit to doc-comment: after a restart a DB-only task has no registry entry and falls back to `""` — today's behaviour, never worse.
3. **GAP-08a — `daily_cost_usd`.** Replace the literal `0.0` at `settings.rs:314` with a `LlmUsageRepository::query_daily_usage` sum for today (**not** `cost_tracker.total_cost()` — see §0).
4. **GAP-08b — `task_id` on `GET /v1/llm/usage`.** `LlmUsageRepository::get_task_usage` already exists (`repository/llm_usage/mod.rs:95-110`) over an indexed column (`migrations/008_llm_usage.sql:8,22`). Add `task_id` to `LlmUsageQuery` (`settings_types.rs:19-24`) and branch on it first in `get_llm_usage`. The GUI already sends the param (`lib/api/usage.ts:8-16`) — zero client change. **Also add `cost_for_tasks(&[String]) -> Vec<TaskCost>`** (one grouped query) and enrich `GET /v1/tasks`, because the Work list needs a per-row cost and cannot issue one request per row.
5. **GAP-16 — `GET /v1/me`.** `AppState` already holds `local_user_id` and `default_lane_key` (`state.rs:32-33`). `sources[]` = distinct `conversations.source` for lanes this user owns, via `list_conversations_for_owner`, documented in a doc comment. *(Ambiguous field: it could equally mean registered connectors, which `GET /v1/connectors` already serves — the conversations reading is chosen and stated.)*
6. **GAP-22 — `ts`/`instance_id` on the six plugin events.** Add both to all six variants in `openalpaca_api/src/events/mod.rs:250-280`; add `instance_id` to `PluginManager` with a `with_instance_id` builder mirroring `with_event_sink`, defaulted to `String::new()` so no test breaks; chain it at `main.rs:334-343`; add `..` to the four test match arms in `manager.rs`. **Rejected:** stamping inside the daemon's sink closure — it re-matches all six variants in `main.rs` and leaves the crate emitting structurally-invalid events for any other embedder. Note `PluginCrashed` has no emit site in the manager; confirm whether that path is dead.
7. **Consistency groundwork.** Add one shared `pub(crate) fn api_error(status, code, message)` in `routes/mod.rs` producing `{error:{code,message}}`, collapsing the byte-identical duplicate at `chat_types.rs:101-111` / `files_types.rs:174-184`. **Every new route in this document uses it.** Fix the plain-text 401 in `middleware.rs` to JSON — it is the only non-JSON response on the wire and the first thing a new client hits. **Do not retrofit the ~30 existing `{error:"string"}` sites** (§8).

*Verify:* unit test on `ConfirmationBody` deserialization for all three bodies — the existing `ChatView.test.tsx:288-298` flips from gap-documentation to a real contract test for free; extend the bridge test at `event_bridge.rs:540-560` with a `TaskUpdated` case asserting a non-empty title; full-workspace rebuild (GAP-22 touches `openalpaca_api`, which everything depends on).

---

## 3. Phase 1 — Artifact store foundations *(migration 035)*

**Depends on:** nothing. **Blocks:** Phase 2, and the artifact half of Phase 5.

1. Migration 035 (§1.5) + **the two query fixes of §1.6 in the same commit** + `ArtifactKind`/`ArtifactOrigin` in a new `models/artifact.rs`, in the style of `FileAssetStatus` (`models/file_asset.rs:15-33`).
2. `paths.rs` additions (§1.3). Pure functions, heavily unit-tested.
3. `.openalpaca` added to `walk_up_for_marker` ahead of `.alpaca`/`.git` (§1.2).
4. `ArtifactStore` in `crates/openalpaca_storage/src/artifacts/mod.rs` — **the one writer** for produced rows:
   ```rust
   pub struct ArtifactStore<'a> { db: &'a Database }
   impl<'a> ArtifactStore<'a> {
       pub fn put(&self, new: NewArtifact<'_>) -> Result<(ArtifactRecord, bool)>;  // create or supersede
       pub fn get(&self, id: &str, owner_id: &str) -> Result<Option<ArtifactRecord>>;
       pub fn list(&self, q: &ArtifactQuery) -> Result<(Vec<ArtifactRecord>, i64)>;
       pub fn resolve_content(&self, id: &str, version: Option<u32>) -> Result<PathBuf>;
       pub fn versions(&self, id: &str) -> Result<Vec<ArtifactVersionRow>>;
       pub fn diff(&self, id: &str, from: u32, to: u32) -> Result<ArtifactDiff>;
       pub fn set_pinned(&self, id: &str, pinned: bool) -> Result<()>;
       pub fn rebase_project(&self, old_root: &str, new_root: &str) -> Result<usize>;
       pub fn verify(&self, project_root: Option<&str>) -> Result<usize>;
   }
   ```
   `FileAssetRepository` keeps its current surface and keeps owning uploads with `origin` defaulted; the two coexist on one table.
5. `artifact_write` tool + the `workspace_write` spill bridge (§1.7), `[execution.artifacts]` config.
6. `task.workspace_id` written at dispatch; **the GUI sends `x-workspace-path`** (§1.8).

*Verify:* `slugify` property tests (traversal `../`, unicode NFKD, case-folding, Windows reserved names, 60-byte truncation on a char boundary, empty→`artifact`); `confine_to_root` rejects symlinked-parent escapes; a `put`→`put` round trip leaves head at the clean path and v1 in `.versions/`; **a crash-injection test that kills between rename steps and asserts the head is either fully old or fully new**; an orphan-sweep test asserting a `produced` row survives 25 simulated hours; a quota test asserting produced bytes do not count; an end-to-end lead-agent run that writes an artifact and lands it under `<project>/.openalpaca/artifacts/`.

---

## 4. Phase 2 — Artifact API, previews, versions *(unblocks the whole Library)*

**Depends on:** Phase 1.

1. All routes of §1.10 on the protected router, except the two content routes which move to the third merged sub-router with inline `?token=` (**GAP-11**).
2. `ServerEvent::ArtifactWritten` through all four layers (event → bridge → broadcaster → persistence).
3. **GAP-05** versions + diff; add `similar`.
4. **GAP-12** pins: the column and `PUT …/pin` ship, but the client's `localStorage` (`views/library/LibraryDetail.tsx:43-44`) stays authoritative until it opts in — `unbacked.ts`'s `Artifact` has no `pinned` field, and an extra wire field is ignored by a TS structural type.
5. **CSP** — `apps/openalpaca-gui/src-tauri/tauri.conf.json:22` becomes
   `img-src 'self' data: blob: http://127.0.0.1:* http://localhost:*;`
   Both halves are needed and they are **not** the same change: `blob:` for object-URL previews, the loopback origins for direct `<img src=…?token=>`. CSP does not inherit from `connect-src` once `img-src` is explicitly set. **Do not loosen anything until a preview actually ships.**
6. `POST /v1/files/{id}/open` skips its `$TMPDIR` staging for produced artifacts (they now have real extensions).

*Verify:* `?token=` accepted **and** the `Authorization` header still accepted on both content routes; a wrong token is 401; an artifact owned by another user is 404 (ownership check unchanged); 410 on a deleted head file with `missing_since` set as a side effect; `GET /v1/artifacts?task_id=` returns the run's files; the Library renders end-to-end with `unavailable.ts`'s GAP-04/05/11/12 entries deleted.

**Explicitly deferred, not solved here:** HTML artifact previews (`ArtifactKind = "html"`) need `frame-src`/`sandbox` handling. That is a materially larger security decision than a CSP tweak and gets its own review.

---

## 5. Phase 3 — Run observability *(migration 036)*

**Depends on:** Phase 0 (GAP-07's bridge work). **Shares** the sandbox `task_id` passthrough between GAP-10 and GAP-09 — do them together.

### Migration 036 — `036_run_observability.sql`

```sql
CREATE TABLE IF NOT EXISTS subagent_span (
    id                TEXT PRIMARY KEY,          -- = the spawn's node_id (UUID)
    task_id           TEXT NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    template_id       TEXT NOT NULL,
    agent_instance_id TEXT NOT NULL,
    label             TEXT NOT NULL,             -- "review·2" — unique per task
    objective         TEXT NOT NULL,             -- first 200 chars
    state             TEXT NOT NULL DEFAULT 'running'
                      CHECK(state IN ('running','done','failed','blocked','cancelled')),
    detail            TEXT,
    started_at        TEXT NOT NULL,             -- RFC 3339
    ended_at          TEXT,
    duration_ms       INTEGER,
    output_preview    TEXT
);
CREATE INDEX IF NOT EXISTS idx_subagent_span_task  ON subagent_span(task_id, started_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_subagent_span_label ON subagent_span(task_id, label);

ALTER TABLE event_log ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_event_log_task ON event_log(task_id, id DESC);

ALTER TABLE task ADD COLUMN source_task_id TEXT;   -- set only by rerun (Phase 4)
CREATE INDEX IF NOT EXISTS idx_task_source ON task(source_task_id);

UPDATE schema_version SET version = 36 WHERE version = 35;
```

### GAP-09 — subagent timeline

**Do not extend `agent_task_history`.** It is FK-bound to `agent(id)` with template ids and `foreign_keys=ON` (`007_subagents.sql:29`, `database/mod.rs:62`), it writes a timezone-less `completed_at` (`repository/subagent/mod.rs:232`), and it backs the legacy `assignments`/`assigned_agents` payload on `GET /v1/tasks(/{id})`. Widening it drags that contract along.

**Correction to the brief that changes the work:** the brief says `agent_task_history` merely lacks `started_at`. In fact **no row exists until completion** — `record_agent_history` (`dispatcher/usage.rs:76-101`) is called only from `runner/lead_agent/tools.rs:548-556` and `:642-650`, both after the subagent's loop returns. An in-flight span is not underivable, it is *absent*. A `started_at` column alone fixes nothing.

Write sites, all in `runner/lead_agent/tools.rs` (`self.db` is already a field at `:48`, so no signature changes):

- **Open** immediately after the existing `DagNodeStarted` publish (`:232-240`), reusing `node_id` as the span id so socket events and the fetched timeline share a key.
- **Close** at both completion sites (`:524-543` plugin path, `:610-628` LLM path), where `duration_ms`, success and an output preview are already computed. Map `Cancelled` explicitly — `agent_success` at `:598-604` folds cancellation into `false` today, and the UI has separate copy for each (`ParallelWork.tsx:52-57`).
- **Close** the cancelled-before-start early return (`:477-486`), or those spans stay `running` forever.
- **Labels must be unique within a run** — `ParallelWork.tsx:129,170` keys lanes by `label`. Use `<short template>·<ordinal>` (`research_agent` → `research·1`) via `next_ordinal(task_id, template_id)`; the unique index makes a collision an error to retry once.
- **Write a `lead` span** in `spawn_lead_agent_execution` (`dispatcher/lead_agent.rs:224-246`), closing it where `finalize_task_with_outcome` is called (`:507-512`). ~10 lines, and it makes the lead lane's state honest rather than approximated at the route. `steps_current`/`steps_total` come from `task.progress_current`/`progress_total`, already published at `:240-241`.

**`blocked` is derived, not persisted** — it is inherently ephemeral. Requires two small changes: `ConfirmationBroker.pending` becomes `DashMap<String, (ConfirmationRequest, oneshot::Sender<_>)>` with a `pending_requests()` accessor (it stores only the sender today — `security/confirmation.rs:39-41`), and `ConfirmationRequest` gains `task_id` / `agent_instance_id` (built at `sandbox/mod.rs:212-220`, where `ctx.task_id` is in scope). **Add `agent_instance_id` to `ToolContext`; do NOT repurpose `ToolContext.agent_id`** — it is the *template* id and is what `CapabilityManager::check_agent_capability` reports in violations (`security/capabilities/mod.rs:91-116`).

Route `GET /v1/tasks/{id}/timeline` returns `{task_id, started_at, now, completed_at, lanes:[…]}` matching `TimelineLane` (`unbacked.ts:121-141`) field-for-field, so `getTaskTimeline` becomes a real fetch with **no view changes**. If the task is terminal, any span still `running` is stale (daemon crash) — report it `cancelled` with `detail:"interrupted"`, and run `close_orphans` once at boot.

New `SystemEvent::SubagentSpan` / `ServerEvent::SubagentSpan { task_id, lane_id, label, template_id, agent_instance_id, phase, state, detail, started_at, ended_at, ts, instance_id }`, emitted at open, blocked/unblocked, and close.

**Keep `DagNodeStatus` emitting alongside.** It is live today and the GUI consumes it (`components/work/run-events.ts:63-71`). Its two halves are unjoinable — `DagNodeStarted` passes the *template* id and `DagNodeCompleted` the *instance* id (`tools.rs:236` vs `:618`) — which is why it cannot back the swimlanes, but that is no reason to break it mid-flight. **Delete it in Phase 6**, one phase after the client switches. (The app is pre-release; no release-soak ceremony.)

### GAP-10 — per-run event log

Cheaper than the brief describes. `ctx.task_id` is already at every emit site inside `SandboxManager::execute_tool` (`security/sandbox/mod.rs:134-139`). Add `task_id: Option<String>` to `ToolExecuted`, `SecurityViolation`, `ToolConfirmationRequested`, `CircuitBreakerTripped` (`ctx.task_id`) and `LlmCallCompleted` (already a parameter at `dispatcher/usage.rs:19`). `emit_security_violation` (`:352`) and `emit_tool_executed` (`:379`) each take one new `Option<&str>` param, passed from seven call sites all inside `execute_tool`.

**The bulk of the value is retro-fitting rows that already exist.** `TaskStatus` (`events/persistence.rs:59-80`), `DagNodeStatus` (`:260-275`), `WorkflowStarted`/`WorkflowSteered`/`WorkflowProgress` (`:308-341`) and `FollowupQueued` (`:343-354`) **already carry the task id** — buried in the `detail` JSON blob with `agent_id = None`. Switch them to `log_for_task` with the id in the new column, and the `steer` / `run` rows the design shows become queryable without a JSON scan.

```
GET /v1/events/history?task_id=&event_type=&before=&limit=
200 { "events": [EventLog], "next_before": 8791 }
```

**Paginate on the autoincrement `id`, not `timestamp`** — the timestamp column holds two different formats (`repository/event_log/mod.rs:96-105`). **Back-compat:** return the envelope **only** when `task_id` or `before` is present; the un-filtered call keeps returning a bare array, because the CLI parses it that way. Slightly ugly, honestly smallest; the alternative is a `/v2` route.

*Verify:* a lead-agent run with two subagents produces two spans with unique labels, correct start/end, and a lead span; a cancelled subagent reports `cancelled`, not `failed`; a pending confirmation flips exactly one lane to `blocked`; killing the daemon mid-run leaves spans that `close_orphans` reports as `interrupted`; `?task_id=` returns only that run's rows and `?limit=` alone still returns a bare array (CLI regression).

---

## 6. Phase 4 — Run control

**Depends on:** Phase 3 (`task.source_task_id`, and `task.workspace_id` from 035). No new migration.

1. **GAP-02 — `POST /v1/tasks/{id}/steer {message}`.** Pure reuse: `push_steering` (`runner/steering.rs:60-77`) is **already task-addressed** and already emits `WorkflowSteered`, which the GUI already renders. Read the lane key from `task.source_lane` — do not require the caller to supply one; the GUI addresses a run, not a lane. `200 {task_id, accepted, inbox_depth, lane_key}`; `409 STEERING_INBOX_FULL` with `cap` from `steering_inbox_cap`; `409 TASK_NOT_STEERABLE` on a closed inbox; `503 STEERING_DISABLED`. **Leave the `/steer ` chat prefix alone** — it is the CLI's and Telegram's only channel. Accept an optional `workspace_path` so a leftover converted to `unprocessed_steering` at workflow exit (`dispatcher/lead_agent.rs:316-350`) re-enters the front door scoped to the same project; default it from `task.workspace_id`.
2. **GAP-03 — follow-up queue routes.** Storage, the CAS claim protocol, the runner and the event all exist (`migrations/033_lane_followups.sql`, `repository/followup/mod.rs`); only HTTP is missing. `GET/POST /v1/lanes/{lane_key}/followups`, `DELETE …/{id}`. Three non-obvious requirements:
   - **Never serialize `FollowupRecord`.** It derives `Serialize` including `principal_json`, an identity blob (`repository/followup/mod.rs:19-34`). Return a `FollowupView` that drops it and `workspace_path`, matching `unbacked.ts:225-235`.
   - **Add `cancel_if_queued` (CAS on status, mirroring `claim_next`).** The existing `mark_cancelled` (`:172`) is unconditional, so a naive DELETE races the autostart at `dispatcher/lead_agent.rs:527-542` and marks a row cancelled whose turn is already running.
   - **Never let a client mint `unprocessed_steering`.** The CHECK constraint would accept it but `claim_next` deliberately never claims that kind — a client-created row would sit forever. Kind is always `"followup"`.
   Publish `FollowupQueued` on POST (so a second window updates) and add a symmetric `FollowupCancelled` on DELETE. Validate `lane_key` (non-empty, no `/`); the client must `encodeURIComponent` the `:` in `"user:gui"`.
3. **GAP-06 — `rerun` and `start`.** Add `pub fn rerun_task(&self, task_id) -> Result<DispatchOutcome, RerunError>` on `Orchestrator` rather than exposing `TaskDispatcher` on `AppState` — the "read the old row" logic stays with the dispatcher and the route is a thin HTTP translation. `rerun` = read the row → `dispatch_lead_agent` with its `description`/`title`/`created_by`/`source_lane`/`workspace_id` → `TaskRepository::set_source_task(new, old)`. 404 no row; 409 unless terminal; 422 if `description` is NULL. Returns **201 with a new id** — deliberate asymmetry with `start`. `start` (D5) is intercepted in the route **before** `apply_task_action` (whose match at `task_ops.rs:107` would swallow it) and needs `dispatch_lead_agent_with_id` + an idempotent persist. Guard the race: if `shared_context.cancellation_tokens` holds the id, the task is already live → 409. Update the `UnknownAction` message at `routes/tasks.rs:287` to list all five verbs.

*Verify:* steering a running task injects an `<user_interjection>` at the next round boundary (existing loop test rig); steering a finished task is 409 not 500; a queued follow-up DELETE'd after `claim_next` is 409 and the turn still runs; `rerun` of a completed task produces a new id whose `source_task_id` survives a restart; `start` on an already-dispatched row is 409.

---

## 7. Phase 5 — Message → run links *(migration 037)*

**Depends on:** Phases 1 and 3.

```sql
-- 037_message_run_links.sql
ALTER TABLE conversation_messages ADD COLUMN task_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conv_msg_task ON conversation_messages(task_id);
UPDATE schema_version SET version = 37 WHERE version = 36;
```

**GAP-23 needs no new link table** — the §1.4 decision to extend `file_assets` rather than create an `artifacts` table settles Lens B's hardest coordination point. `conversation_message_attachments` already carries `role TEXT NOT NULL DEFAULT 'attachment'` (`migrations/028_message_attachments.sql:7`); outputs get `role='artifact'` via a new `link_to_message_with_role`, with the existing `link_to_message` becoming a wrapper.

**Two links, not one, because a turn cannot know its own artifacts:**

- **The delegating message gets `task_id`.** `persist_assistant_message` is called at `gateway/router/mod.rs:260` and `result.delegation` is read at `:285` — eleven lines apart, same match arm. Add a `task_id: Option<&str>` param and pass `result.delegation.as_ref().map(|d| d.task_id.as_str())`. This answers "which turn started this run" after a reload.
- **The completion-report message gets `task_id` **plus** `role='artifact'` links.** The delegation fires when the workflow *starts*; by the time the model-authored completion report is persisted, `file_assets` has rows with `task_id = <this task>`. That is the message `RunReportCard` actually renders — it is a *finished*-run card.

`ConversationMessage` (`models/conversation.rs:7-20`) gaining a field is a **wide struct-literal change**: `gateway/persistence.rs:34-46, :106-119, :156-168`, the repository inserts and the row mappers. Add `#[derive(Default)]` and switch those literals to `..Default::default()` in the same commit. Read path needs no route change — `GET /v1/chat/history` serializes the model directly. Join the link table in the history query so the client gets `artifact_ids: string[]` without a second round trip.

*Verify:* a chat turn that starts a workflow persists `task_id` and survives a reload; the completion message carries resolvable `artifact_ids`; `role='attachment'` rows (inputs) are unaffected.

---

## 8. Phase 6 — Config, catalog, long tail

Independently orderable; each is its own commit. Ordered by value per hour.

1. **GAP-14 — `GET /v1/status`, Phase A only.** `started_at` on `AppState` captured at the **top of `run()`** (process start, not HTTP-ready), `uptime_secs`, `schema_version` from the existing `Database::schema_version()` (**not** `MIGRATIONS.len()` — the DB is the truth), `data_dir`, `db_path`, **and the resolved project dir from §1.8**. Register inside `protected_routes`, **not** public — it leaks filesystem paths. Leave `/v1/health` untouched; the GUI liveness dot needs it unauthenticated. **`log_path` ships as `Option<String>` = `None`.** The daemon writes no log file anywhere: `main.rs:57` is stdout-only, there is no `tracing-appender` in the workspace, and the GUI sidecar discards stdout (`src-tauri/src/lib.rs:128,152`). Phase B (appender + rotation + retention + un-discarding the sidecar's stdout) is a separate day+ task. Widen the client's `log_path` to `string | null` and keep "Copy log path" honestly disabled.
2. **GAP-21 — conversation rename/delete.** Two repository methods (`update_title`, `delete_conversation`) + two handlers. `delete_conversation` must run both statements in one transaction — migration 011 declares no FK cascade between `conversations` and `conversation_messages`, so an untransacted delete orphans messages. `router.rs:6` needs `patch` added to its `routing::{…}` import. **Semantics: "forget the transcript", not "tear down the lane"** — deleting the row clears neither `lane_followups` nor the in-memory lane, and the next message recreates it via `get_or_create_conversation`. Return **409 when the lane has active workflows**.
3. **GAP-18 — `GET /v1/tools`, `GET /v1/skills`.** The data all exists; the blocker is plumbing. **Neither registry reaches `AppState`**, and `svcs.tool_registry` is *moved* into `Orchestrator::new` at `main.rs:373` (the skill catalog is `.clone()`d at `:376`, the registry is not). Clone the `Arc` before the move and add two `AppState` fields. Then: `RegisteredTool.author` is already `"built-in" | "mcp:<server>" | "plugin:<id>"`; `destructive_hint == Some(true)` gives `requires_confirmation` (`sandbox/mod.rs:398-417`); `global_tool_deny` gives `denied`; one grouped query over `tool_execution_log` gives `invocations_today`. **Sort both lists by name** — `DashMap` iteration is unordered and jittery rows are a bug. Bare arrays, matching the sibling `/v1/skills/health` (§8 rule). The design's per-tool *enable switch* is only half-served: `denied` is readable, but writing `global_tool_deny` would need `PUT /v1/settings/tools/{name}/enabled` against `daemon.toml` — **out of scope; the toggle ships disabled rather than lying.**
4. **GAP-15 — provider enable/disable.** Mostly already built: `ProviderConfig.enabled` exists in `llm.toml` (`router_config.rs:65-66`), is honoured at build time (`router_builder.rs:98-102`), and is already served by `GET /v1/settings/llm` (`settings_service.rs:169-178`). **Only the write route is missing.** Use `LlmRouter::deregister_provider` for a hot disable — note it also strips models from the `ModelRegistry`, so enable must re-register **and** `refresh_models()`. Extract the write-lock half of `persist_and_reload` into `persist_only(mutate)` rather than duplicating lock handling. **Add a 409 guard when disabling the provider that serves the current `default_model`** — a silent capability loss is worse than a refused toggle.
5. **GAP-20 — template run counts** (counts only). One SQL join: `agent_task_history ⨝ agent ON a.id = h.agent_id GROUP BY a.template_id`, using the `template_id` backfilled by migration 020. **State in the response docs that these are *completed* runs** — rows are written at completion (see §5's correction). `GET /v1/agent-templates?window=7d`, rejecting unknown windows with 400. **The `enabled` toggle is out of scope for this phase**: `AgentTemplateFrontmatter` has no such field, the recommended home is the existing `preference` KV store (no migration, and it works for plugin-contributed templates that have no file), but the real cost is *enforcement* in the lead-agent spawn path — without it the toggle is decorative. Day+, its own task.
6. **GAP-17 — connector detail.** Add `source` ("builtin"|"plugin"), `registered`, and **`messages_7d`** (renamed from `calls_7d`, §0): `SELECT source, COUNT(*) FROM conversation_messages WHERE created_at >= datetime('now','-7 days') AND role='user' GROUP BY source`. The **"unwired" badge needs nothing server-side** — `hooks/useConnectors.ts:33-44` already derives it correctly by joining the plugin manifest against `list_status()`. Fix the hardcoded name match while in there (`connectors.rs:34-38` knows only `telegram`/`imessage`; Discord falls through to the raw id). "Connect service" wires to the existing `POST /v1/connectors/{id}/config` + `/action`; no new endpoint.
7. **GAP-08c — `GET /v1/usage/summary?window=today`.** Totals from `query_daily_usage`; caps as a **nested `caps` object** so the client cannot mistake a per-run cap for a daily one. Derive `by_provider` from `llm_call_log` for today rather than `cost_tracker.all_provider_usage()`, which is **lifetime, not today**. Echo the authoritative `date` in the payload: `llm_usage_daily.date` is written from a UTC date (`main.rs:328`) while `todayIsoDate()` in the client is local (`lib/api/usage.ts:52-57`) — they disagree for up to 12 hours a day. **Product question, not an API one:** `execution.lead_agent_defaults.max_cost = 5.0` is a *per-workflow* cap, but the Settings panel reads "`$X of $5.00 cap`" as a daily budget. Either add a real daily cap to `[orchestrator.costs]` or relabel the UI.
8. **GAP-13 — per-chat model override.** Rated `M` in the registry; it is `day+`. **Do not add a 9th positional arg to `MessageHandler::handle`** — it already takes 8 with `#[allow(clippy::too_many_arguments)]` (`gateway/router/mod.rs:63-73`). Introduce a `HandleRequest` struct (a mechanical refactor across `gateway_bridge.rs:52,81`, `followup.rs:87`, `scheduled_skills.rs:324` and two test stubs) and carry the override through the existing `LoopOverrides::MainLoop` seam (`query_handler/mod.rs:6-17`) into `LoopConfig.model`. **Validate the model id** via `router.model_registry().get_model_info()` and 400 `UNKNOWN_MODEL` — `simple_query_handler.rs:256-260` resolves the context window from `config_for_loop.model`, so a bogus id silently degrades the trimming budget to the 200 000 fallback. **Ship request-scoped only.** Lane persistence is a follow-up needing no migration (the `preference` table is already a versioned KV store, key `lane_model:{lane_key}`); shipping both at once turns an afternoon into a week. `ChatSendRequest` has no `deny_unknown_fields`, so the client's `model` field is silently dropped today — forward-compatible, confirmed.
9. **GAP-19 — plugin install, `source: "path"` only.** Copy a local directory into `plugin_dir`, then call the already-public `try_load_plugin`, which parks the plugin in `WaitingApproval` and emits `PluginPendingApproval` — **install does not grant capabilities**; the existing approve gate still governs. Use a serde-tagged enum so `archive` is additive later. Needs one upstream addition: `PluginManager::plugin_dir()` has no getter. Hardening is the real work: parse `plugin.toml` *before* copying, reject name collisions (409), reject a path already inside `plugin_dir`, refuse symlinks escaping the source root. **Explicitly decline `source: "url"`.** Downloading untrusted executable code needs its own security review — signatures, allowlists, TOFU — not a line in this plan. The GUI already has `tauri_plugin_dialog` for a native directory picker.
10. **Delete `DagNodeStatus`** once the GUI has switched to `SubagentSpan` (§5).
11. **Artifact phase 2** (§1.9): user-edit detection recorded as a version with `author_agent_id = NULL`; `ArtifactStore::rebase_project` wired to the project picker; uploads into `<store>/uploads/` **only if D2 is answered "yes"**.
12. **Pre-existing doc bug, one line:** `apps/openalpaca/src/commands/plugin.rs:225,230` and `docs/CLI_Manual.md:222` tell users plugins live in `~/.openalpaca/plugins/.config/`, but `main.rs:331` puts them in `app_dir()/plugins`. Harmless today; **actively confusing once `~/.openalpaca/` really exists**.

---

## 9. Cross-cutting: envelopes, list shapes, task shape

**Three error envelopes plus a plain-text 401 coexist**, not two as the brief says: `{error:{code,message}}` (`chat_types.rs:101-111` and a byte-identical duplicate at `files_types.rs:174-184`), `{error:{code,status,message}}` (`settings_types.rs:6-17`), `{error:"string"}` (~30 ad-hoc `json!` sites in `tasks.rs`, `agents.rs`, `plugins.rs`, `connectors.rs`), and `(401, "Invalid token")` in `middleware.rs`.

- **New routes use the shared `api_error()` from Phase 0.** Drop `status` — it duplicates the HTTP status and invites drift.
- **Do not retrofit the `{error:"string"}` sites now.** Both in-repo clients already absorb all four shapes (`lib/http.ts:72-107`, `apps/openalpaca/src/client.rs:147-153`), so the retrofit is pure churn with a real regression surface. Its own commit, after the GUI work lands.
- **Do not normalise list shapes.** The bare-array/envelope split already follows a legible rule — **paginated ⇒ `{items,total}`; unbounded ⇒ bare array**. Codify the rule; change nothing. Under it, `/v1/tools` and `/v1/skills` are bare arrays.
- **`GET /v1/tasks` vs `GET /v1/tasks/{id}` normalisation is deferred to Phase 3's timeline work**, where the representation of agent runs is being rethought anyway. Doing it in isolation breaks both the GUI (`types.ts:60-61, :83-87`) and the CLI (`commands/tasks.rs:82,97`), which each encode both shapes, for zero new capability. **Do the cheap half in Phase 0:** replace `list_tasks_handler`'s stringly-typed `as_object_mut()` post-injection with a typed `TaskSummaryResponse` — same JSON, no behaviour change, and the later normalisation becomes a one-file diff.

---

## 10. Gap disposition — all 23

| Gap | Phase | Needs | Effort |
|---|---|---|---|
| **GAP-01** approval_scope | 0 | Two lines: `ConfirmationBody` field + `chat.rs:462`. Enforcement path already complete. | XS |
| **GAP-07** empty title/name | 0 | `title`/`name` on 4 `SystemEvent` variants; 5 producer sites all have the value. Not `SharedContext` in the bridge. | XS |
| **GAP-08a** daily_cost | 0 | `query_daily_usage` for today, **not** the since-boot cost tracker. | XS |
| **GAP-08b** cost by task | 0 | `task_id` on `LlmUsageQuery` + `cost_for_tasks()` grouped query on `GET /v1/tasks`. | S |
| **GAP-16** `/v1/me` | 0 | `AppState` already has both fields; `sources[]` = distinct `conversations.source`. | XS |
| **GAP-22** plugin event ts/id | 0 | 6 variants + `PluginManager::with_instance_id`; full-workspace rebuild (touches `openalpaca_api`). | S |
| **GAP-04** artifact API | 1 → 2 | Migration 035, `ArtifactStore`, `artifact_write`, `/v1/artifacts*`. **The Library's whole blocker.** | L |
| **GAP-05** versions & diff | 2 | `artifact_versions` (in 035) + `similar` dep + `/versions`, `/diff`. | M |
| **GAP-11** `?token=` content | 2 | Third merged sub-router; inline token check copied from `chat.rs:104-113`; CSP. | S |
| **GAP-12** server-side pins | 2 | `pinned` column (in 035) + `PUT …/pin`. Client `localStorage` stays authoritative. | XS |
| **GAP-09** subagent timeline | 3 | Migration 036 `subagent_span`; ~6 edit sites in `tools.rs`; lead span; `ConfirmationBroker` metadata; `ToolContext.agent_instance_id`; route; event through 4 layers. **Largest single item after the store.** | L |
| **GAP-10** per-run event log | 3 | `event_log.task_id` (036) + `ctx.task_id` passthrough + retro-fit 6 already-persisted variants. | M |
| **GAP-02** steer endpoint | 4 | Pure reuse of `push_steering`; lane read from `task.source_lane`. | S |
| **GAP-03** follow-up routes | 4 | `list_by_lane` + `cancel_if_queued` (CAS) + 3 handlers + `FollowupView`. | M |
| **GAP-06** rerun / start | 4 | `Orchestrator::rerun_task`; `task.source_task_id` (036); `dispatch_lead_agent_with_id` for `start` (D5). | M |
| **GAP-23** message→run links | 5 | Migration 037; `persist_assistant_message` param; `role='artifact'` on the completion message. | M |
| **GAP-14** `/v1/status` | 6 | Phase A only. `log_path: null` — the daemon writes no log file. | S / day+ |
| **GAP-21** conversation CRUD | 6 | 2 repo methods (transactional delete), 2 handlers, `patch` import. | S |
| **GAP-18** `/v1/tools`, `/v1/skills` | 6 | Clone the tool-registry `Arc` before the move at `main.rs:373`; 2 `AppState` fields. | M |
| **GAP-15** provider toggle | 6 | `set_provider_enabled` + `deregister_provider` round trip + 409 on the default model's provider. | M |
| **GAP-17** connector detail | 6 | `source`/`registered`/**`messages_7d`**; badge already client-side. | M |
| **GAP-20** template metrics | 6 | Counts only (one join). `enabled` deferred — no home in the model, and enforcement is the real cost. | M / day+ |
| **GAP-13** per-chat model | 6 | `HandleRequest` refactor (5 hops) + `LoopOverrides` seam. Request-scoped only. | day+ |
| **GAP-19** plugin install | 6 | `source:"path"` only; hardening is the work. **URL install declined.** | day+ |

---

## 11. Risks and breaking changes

| Risk | Reality | Mitigation |
|---|---|---|
| **Orphan sweep eats every artifact** | Real and immediate the moment 035 lands: produced artifacts are never linked to a message, and `background.rs:308-357` deletes the row **and the file** after 24 h. | §1.6 fix in the same commit. Test: a produced row survives 25 simulated hours. |
| **Upload quota starts rejecting uploads** | Real: `total_storage_bytes` sums all rows against a 500 MB cap. Agent output in the user's own project would count. | §1.6 fix in the same commit. |
| **`.openalpaca/` silently changes memory scoping** | Writing the first artifact creates a workspace-root marker where none existed, so `resolve_workspace_id` starts resolving that directory. | Deliberate and desirable, but **document it** and land the marker change and the store together so it is never a surprise. |
| **Everything lands in `~/.openalpaca/`** | Certain, until a client sends `x-workspace-path`. No client does today. | §1.8 items 1–4 are in Phase 1/2 scope, not "later". Without them the store is built and unused. |
| **Existing installs** | No schema is dropped, no file moves, no route changes shape. Every 035/036/037 column is nullable or defaulted. | Rollback to 34 leaves artifacts on disk as a readable folder. |
| **CSP loosening** | `blob:` and the loopback origins in `img-src` widen the webview's image surface. | Land it **with** the preview, never before. HTML artifact previews are a separate `frame-src`/sandbox review, explicitly not covered. |
| **`?token=` in a URL** | The daemon token is long-lived and grants everything; a URL-embedded token lands in webview history and any `Referer`. | Pre-existing posture — `/v1/chat/stream` already does this. Add `Referrer-Policy: no-referrer` on the content response. A short-lived per-asset token is the real fix, deferred. |
| **`ConversationMessage` struct-literal churn** | Adding `task_id` breaks every construction site (`gateway/persistence.rs` ×3 plus repository mappers). | Add `#[derive(Default)]` and `..Default::default()` in the same commit. |
| **`start` id-injection** | The messiest code in the plan: `create`-or-`update` in the dispatcher's persist step. | Isolate it in `TaskRepository::upsert_queued` and call it out explicitly in review. Or take D5's alternative. |
| **New `similar` dependency** | The workspace has no diff crate today. | MIT, pure Rust, no build script, in `openalpaca_storage` only. Alternative: ~120-line hand-rolled LCS. |
| **`GET /v1/events/history` dual shape** | The same route returns a bare array or an envelope depending on the query. | Deliberate, to keep the CLI working. Documented; regression-tested both ways. |
| **`DagNodeStatus` double emission** | Two event families describe the same spawns for one phase. | Bounded: deleted in Phase 6 once the client switches. Pre-release, so no soak ceremony. |

---

## 12. Explicitly out of scope

- **A full project concept** (`GET /v1/projects`, `POST /v1/projects/activate`, a project switcher). §1.8 items 1–4 are enough for the Library. Whoever adds it must reconcile with `SkillCatalog::scan_multi_scope` / `SkillScope`, which already implement project-vs-user scoping — do not invent a second path resolution.
- **Moving `app_dir()`** (D1). Rejected with reasons.
- **Re-homing existing uploads** (D2). Phase-2 delta if the user disagrees; note it is two writers, and the connector's duplicated sha/dedup logic should collapse into `ArtifactStore` first.
- **Daemon file logging** (GAP-14 Phase B). `tracing-appender` + rotation + retention + un-discarding the GUI sidecar's stdout is a real feature, not a field.
- **Per-tool enable writes** (`global_tool_deny` via HTTP) and **agent-template `enabled`** (GAP-20 part 2). Both need enforcement in the spawn path, without which the toggles are decorative.
- **Remote plugin install** (`source: "url"`). Needs its own security review.
- **HTML artifact previews.** `frame-src`/sandbox is a security decision, not a CSP tweak.
- **Retrofitting the ~30 `{error:"string"}` sites** and **normalising list shapes** (§9). Churn with a regression surface and no user-visible benefit.
- **`GET /v1/tasks` vs `/{id}` normalisation.** Deferred to Phase 3, where it is a natural consequence rather than a gratuitous break.
- **A `resync_needed` WS signal.** `routes/events.rs` logs and continues on `RecvError::Lagged(n)`, so clients silently lose events; the UI treats the socket as additive and refetches. Worth doing, not blocking any surface.
- **Two daemons over one shared project directory.** Documented limitation.

---

## 13. Migration ledger

| # | File | Phase | Contents |
|---|---|---|---|
| 034 | *(head today — verified)* | — | `drop_context_compaction_log` |
| **035** | `035_artifact_store.sql` | 1 | 11 `file_assets` columns · 4 indexes · `artifact_versions` · `task.workspace_id` |
| **036** | `036_run_observability.sql` | 3 | `subagent_span` · `event_log.task_id` · `task.source_task_id` |
| **037** | `037_message_run_links.sql` | 5 | `conversation_messages.task_id` |

Lens A's and Lens B's numbering claims conflicted (A wanted 035 for everything artifact-shaped, B assumed 035+036 for artifacts and claimed 037/038). **Arbitrated:** artifacts are one migration, so everything shifts down one from B's assumption. Each file ends with its own `UPDATE schema_version`. `database/tests.rs:11` asserts the head version and must be updated with each.
