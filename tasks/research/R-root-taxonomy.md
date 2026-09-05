# Lens R — Single root, directory taxonomy, legacy purge

**Status:** design (rev 2 input). No production code written. · **Date:** 2026-09-01 · **Branch:** `feat/ui-rework`
**Directives applied:** D1 = `app_dir()` itself moves to `~/.openalpaca/` (single root). D2 = chat/connector uploads also move into `.openalpaca/`. D4 = `~/.openalpaca/` with `OPENALPACA_HOME_STORE` override. Undistributed app ⇒ legacy measures are deleted, not merely avoided.
**Inputs verified against the tree**, not taken from rev 1: every claim below carries a file:line citation checked today.

---

## 0. Ground truth this lens depends on (verified)

| Fact | Citation |
|---|---|
| `app_dir()` = `directories::ProjectDirs::from("","","OpenAlpaca").data_dir()` → `~/Library/Application Support/OpenAlpaca/` (macOS), `~/.local/share/openalpaca/` (Linux), `%APPDATA%\OpenAlpaca\data\` (Windows) | `crates/openalpaca_storage/src/paths.rs:10-22` |
| `discovery_path` / `lock_path` / `database_path` / `assets_dir` / `asset_storage_path` all hang off `app_dir()` | `paths.rs:53-83` |
| `migrate_legacy_app_dir()` (com.openalpaca → OpenAlpaca rename) must run **before the singleton lock and before the DB opens**, "while no files are held open" | `paths.rs:24-27`; called at `apps/openalpacad/src/main.rs:70-71` |
| Boot order: logging → `migrate_legacy_app_dir` → **singleton lock** → config resolve + `seed_default_configs` → master-key ensure at `app_dir` → signal handlers → tokio → bind → **discovery write** → **DB open** → `migrate_preference_summaries` → `sweep_orphaned_tasks` | `main.rs:56-193` |
| Config dir resolution: `OPENALPACA_CONFIG_DIR` env → walk up from `current_exe()` for `config/llm.toml` → walk up from CWD → `$CWD/config` fallback | `apps/openalpacad/src/bootstrap/config.rs:14-61` |
| The GUI sidecar computes `app_dir()` itself, pre-creates `app_dir` and `app_dir/config`, then spawns the daemon with `cwd = app_dir` and `OPENALPACA_CONFIG_DIR = app_dir/config` | `apps/openalpaca-gui/src-tauri/src/lib.rs:112-160` |
| The CLI's managed start does the same and additionally creates `app_dir/daemon.log`, piping the daemon's stdout/stderr into it | `apps/openalpaca/src/manager.rs:36-52, 214-223` |
| `file_assets.storage_path` is a stored **absolute** path string; it is read for extraction, orphan-file deletion, and content serving | `crates/openalpaca_storage/src/models/file_asset.rs:44`, `repository/file_asset/mod.rs:19-48`, `apps/openalpacad/src/background.rs:262-265, 341-343` |
| Workspace markers: `.alpaca` preferred, `.git` fallback; nothing recognises `.openalpaca` yet | `crates/openalpaca_core/src/memory/workspace.rs:43-56` |
| Seeded `llm.toml` is the **hierarchical** format (`[providers.*]`) | `scripts/release/templates/config/llm.toml:8-21` |
| Skill catalog already has dormant project-scope machinery: `scan_multi_scope(user_dir, project_dir)` with `SkillScope::Project` overriding `SkillScope::User` — production only ever calls the user scope today (callers of `scan_multi_scope` are tests only) | `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs:150-169`, `catalog/tests.rs:306,335` |

### Complete `paths.rs` consumer inventory

Everything below goes through `openalpaca_storage::paths` (or `openalpaca_storage::discovery`, which wraps it). **No path is computed anywhere else** — the move is a one-module change plus a coordinated rebuild.

| Consumer | Site | What it uses |
|---|---|---|
| Daemon boot | `apps/openalpacad/src/main.rs:71,100,186,331` | `migrate_legacy_app_dir`, `app_dir` (master key), `database_path`, `app_dir()/plugins` |
| Daemon upload route | `apps/openalpacad/src/routes/files.rs:188` | `asset_storage_path` |
| Daemon discovery/lock | via `crates/openalpaca_storage/src/discovery/mod.rs:86-217` | `app_dir`, `discovery_path`, `lock_path` |
| GUI sidecar (Rust) | `apps/openalpaca-gui/src-tauri/src/lib.rs:20,38,51,112-130,154` | `read_discovery`, `app_dir` (config dir, cwd) |
| GUI frontend (TS) | — none. It reaches the daemon only through Tauri commands backed by `discovery::read_discovery` | `src-tauri/src/lib.rs:16-24` |
| CLI client | `apps/openalpaca/src/client.rs:20-25` | `read_discovery` |
| CLI managed start | `apps/openalpaca/src/manager.rs:38,214-223` | `app_dir`, `app_dir/config`, `app_dir/daemon.log` |
| CLI direct DB access | `apps/openalpaca/src/commands/config.rs:45` | `database_path` |
| CLI key/config helpers | `apps/openalpaca/src/commands/ai_config_helpers.rs:16-17,49-50`, `commands/daemon_config_cli/mod.rs:298-309` | `app_dir` (master key, writable config fallback) |
| CLI REPL | `apps/openalpaca/src/repl/mod.rs:15` | `discovery::ensure_app_dir` |
| Connectors | `crates/openalpaca_connectors/src/common/mod.rs:234` (+ `examples/telegram.rs:32`) | `asset_storage_path`, `database_path` |
| LLM key encryption | `crates/openalpaca_llm/src/keys/key_encryption/mod.rs:34,202` | takes the dir as a **parameter** (`ensure_at(&app_dir)`) — no change needed beyond the caller |

Consequence: **discovery.json consumers need zero code changes.** They need a rebuild, and the rebuild must be atomic across daemon + CLI + GUI (an old GUI binary would read discovery from the old root, conclude no daemon is running, and spawn-loop forever). Single-user app: rebuild all three in one commit; there is no compatibility window and none is designed for.

---

## 1. The move to `~/.openalpaca/`

### 1.1 New root resolution

```rust
/// $OPENALPACA_HOME_STORE if set (absolute path), else <home>/.openalpaca — every platform.
pub fn home_root() -> anyhow::Result<PathBuf>;
```

- `OPENALPACA_HOME_STORE` (D4) is read on every call like `app_dir()` is today — no caching, so tests can set it per-process. Non-absolute values are rejected with an error (a relative store root would silently re-introduce CWD-dependence, the exact disease rev 1 §1.1 diagnosed).
- Home dir via `directories::BaseDirs::home_dir()` (the crate is already a workspace dep — `crates/openalpaca_storage/Cargo.toml:14`). `ProjectDirs` remains only inside the mover, to compute the *old* root.
- `OPENALPACA_CONFIG_DIR` semantics are **untouched** (`bootstrap/config.rs:14-61`). Only the *value* the GUI/CLI pass changes automatically, because both compute it as `app_dir()/config` → now `home_root()/config` (`src-tauri/lib.rs:114`, `manager.rs:217`). Dev runs from the repo keep resolving `./config` via the exe/CWD walk-up and never notice the move — which also means **dev-run LLM keys and persona docs are entirely unaffected by the migration**.

### 1.2 The one boot-time mover — `store::migrate::move_app_root()`

Replaces `migrate_legacy_app_dir()` at the same call slot (`main.rs:70-71`): **after logging, before the singleton lock** — the same ordering constraint documented at `paths.rs:24-27`, because the lock file itself moves and the DB must not be open mid-rename.

```
old = ProjectDirs::from("","","OpenAlpaca").data_dir()      // per-platform, exactly today's app_dir()
new = home_root()
```

Algorithm (each step idempotent; the whole function re-runnable):

1. **Fresh install / already moved:** if `old` does not exist, return. If `old == new` (paranoia under `OPENALPACA_HOME_STORE`), return.
2. **Live-daemon guard:** try a non-blocking `file_lock` on `old/openalpacad.lock` (same mechanism as `discovery/mod.rs:201-217`). Held ⇒ abort startup with "an old daemon is still running from `<old>`; stop it first". This is the one race the mover refuses to paper over: renaming a WAL-mode DB out from under a live process is corruption.
3. **Entry ledger**, moved by `std::fs::rename` (same volume on all three platforms — `~/Library` and `~`, `~/.local/share` and `~`, `%APPDATA%` and the user profile — so rename is atomic; an `EXDEV` error aborts with a message rather than falling back to a non-atomic copy):

   | Old entry | New location | Note |
   |---|---|---|
   | `openalpaca.db-wal`, `openalpaca.db-shm`, then `openalpaca.db` | `state/` | Sidecars first: a crash mid-trio leaves the split halves, and the resume on next boot reunites them **before** `Database::open` runs — SQLite only pairs WAL with DB at open time, and the mover always completes before the open (`main.rs:186`) |
   | `.master_key` | `state/` | Replaces the inline legacy-key copy at `main.rs:102-127` (deleted — §3.2) |
   | `config/` | `config/` | **Per-child merge**, not skip-if-dir-exists: a rebuilt GUI pre-creates `home_root()/config` *before* spawning the daemon (`src-tauri/lib.rs:114-115`), so the destination dir existing is expected; each child (`llm.toml` with its encrypted keys, `daemon.toml`, `orchestrator/`, `skills/`, …) moves if absent at the destination. Runs before `seed_default_configs` (`main.rs:92`), so the moved `llm.toml` wins over a fresh seed |
   | `plugins/` | `plugins/` | Same per-child merge; carries the user-approved `.permissions.toml` files |
   | `assets/` | `state/assets/` | Interim home; the D2 re-home into `uploads/` is a later phase (§4), not the mover's job |
   | `daemon.log` | `state/logs/daemon.log` | CLI-managed-start log (`manager.rs:38`) |
   | `discovery.json`, `openalpacad.lock` | **deleted** | Regenerated every boot (`main.rs:178-183`); moving a stale discovery file would only confuse `ensure_not_expired` |

4. **Partial failure:** every entry is one atomic rename, guarded by skip-if-destination-exists. The first failure aborts startup with the failing path in the error. There is no rollback and none is needed — the next boot resumes exactly where it stopped, and no consumer opens any of these files before the mover finishes.
5. **Old root disposal:** after the ledger, `remove_dir(old)` if empty; otherwise log a warning listing the leftovers and leave them. No `MOVED.txt` ceremony — single user, one machine.
6. **Post-open fixup** (in `bootstrap`, immediately after `Database::open` and before any ingress, beside `sweep_orphaned_tasks` — `main.rs:188-193`): one idempotent statement repairing the stored absolute paths broken by the move:

   ```sql
   UPDATE file_assets
      SET storage_path = replace(storage_path, '<old>/assets/', '<new>/state/assets/')
    WHERE storage_path LIKE '<old>/assets/%'
   ```

   This cannot be a numbered migration — the prefixes are runtime-computed. It runs every boot and matches zero rows after the first. It keeps rev 1 §1.4's "readers need zero changes" promise intact through the move; the deeper option (derive the path from `sha256` at read time and drop `storage_path` reliance) is superseded anyway by the D2 upload re-home (§4), which rewrites these rows again with human-named paths.

7. **The com.openalpaca leg is not carried forward.** `migrate_legacy_app_dir` chained `com.openalpaca.OpenAlpaca → OpenAlpaca` (`paths.rs:28-51`); that rename has long since happened on the only machine that matters. The new mover reads only from today's root. Stated assumption: if a `com.openalpaca.OpenAlpaca` dir still existed, it would simply be ignored.

### 1.3 What breaks for a user who never runs the mover (fresh start)

Nothing *malfunctions* — the daemon boots, seeds configs, generates a master key, creates a fresh DB. What is **lost** by starting fresh instead of moving:

- the entire DB: conversations, memories, tasks, telemetry, and the persisted `identity.local_user_id` (a new UUID is minted — `bootstrap/migration.rs:44-60` — so the lane key changes);
- installed plugins and their approval state (re-install, re-approve);
- uploaded file bytes under `assets/`;
- **only for GUI/CLI-managed setups:** the runtime `config/` — encrypted LLM keys, live persona docs (SOUL/USER/IDENTITY). Dev runs from the repo resolve `./config` via walk-up and lose nothing.

Since the app is undistributed, this is an acceptable worst case, but the mover is ~80 lines and removes it entirely; there is no reason to skip it.

---

## 2. The extensible taxonomy

### 2.1 `~/.openalpaca/` — the home root (single root: app state *and* no-project content store)

```
~/.openalpaca/                        ← home_root(); OPENALPACA_HOME_STORE overrides
  README.md                           ← seeded once; explains every entry below
  .layout                             ← one line: "1" — layout-version marker
  state/                              ← MACHINE STATE — opaque, never user-edited, never committed anywhere
    openalpaca.db  (+ -wal, -shm)
    discovery.json
    openalpacad.lock
    .master_key                       (0600)
    assets/                           ← interim: relocated content-addressed uploads, until the D2 re-home
    logs/
      daemon.log                      ← CLI-managed start (manager.rs:38); GAP-14 Phase B appender lands here too
  config/                             ← USER-EDITED runtime config (GUI/CLI-managed daemons):
                                        llm.toml, daemon.toml, mcp.toml, agents/, skills/, orchestrator/, tools/
  plugins/                            ← user-dropped plugin dirs + .permissions.toml
  artifacts/                          ← content store, home scope (no-project fallback) — rev 1 §1.1 grammar
  uploads/                            ← content store, home scope (D2: uploads with no project signal)
  sessions/                           ← content store, home scope — reserved for Lens S; internals are Lens S's
  memory/  skills/  scratch/  cache/  ← RESERVED names (§2.3); not created until used
```

The organising rule: **`state/` is the machine's; everything else at the root is the human's.** DB, lock, discovery, key, logs, and the interim asset blobs interleave with nothing user-facing. `config/` and `plugins/` stay top-level because the user edits and drops files there — hiding them in `state/` would contradict the findability directive. The content-store kinds (`artifacts/`, `uploads/`, `sessions/`, …) sit flat at the root so the home root *is itself* a store with exactly the project-store shape — `content_dir(scope, kind)` is `root/kind` for both scopes, one code path, no special cases.

- **`.layout`** — a single integer, written by `ensure_store()`, read at boot. If a future restructure is ever needed, the mover pattern of §1.2 gets a version gate instead of heuristics. Both roots carry one.
- **`README.md`** — seeded once per root; documents the state/content split, the reserved names, and "delete `state/` = factory reset, delete a content dir = lose those files only".

### 2.2 `<project>/.openalpaca/` — the project store

```
<project>/.openalpaca/
  README.md                           ← seeded once
  .gitignore                          ← store-owned, committable (contents below)
  .layout                             ← "1"
  artifacts/                          ← rev 1 §1.1/§1.2 grammar UNCHANGED:
    <YYYY-MM-DD>-<task-slug≤48>-<taskid8>/NN-<slug>.<ext>
    loose/<YYYY-MM-DD>/…
    …/.versions/<stem>/vN.<ext>
  uploads/                            ← D2: chat uploads carrying x-workspace-path
    <YYYY-MM-DD>/NN-<orig-name-slug>.<ext>
  sessions/                           ← Lens S's namespace; this lens reserves the NAME and the gitignore line
                                        only — everything beneath it (per-session dirs, events.jsonl,
                                        snapshots/, attachments/, large-tool-results/) is Lens S's design
  memory/                             ← RESERVED: future memory exports / project memory packs
  skills/                             ← RESERVED: future project-scope skills — this is the `project_dir` that
                                        SkillCatalog::scan_multi_scope + SkillScope::Project already implement
                                        and nothing calls (catalog/mod.rs:150-169); whoever wires it points it
                                        HERE, per rev 1 §1.8's own instruction not to invent a second resolution
  config/                             ← RESERVED: future per-project config overrides (daemon.toml fragments)
  scratch/                            ← RESERVED: agent working space that is neither artifact nor session
  cache/                              ← RESERVED: derived/regenerable data; always ignorable, always deletable
```

**Store-owned `.gitignore`** (replaces rev 1 §1.2's single-line `.versions/`):

```gitignore
/.layout
/uploads/
/sessions/
/scratch/
/cache/
.versions/
```

Rationale per line: artifact **heads stay committable** (rev 1's decision, kept — a produced `findings.md` is a document and git is strictly better history than `.versions/`). `uploads/` are copies of files the user already has elsewhere; `sessions/` are private transcripts and machine recovery state; `scratch/` and `cache/` are by definition regenerable; `.versions/` (unanchored — it appears per-run under `artifacts/`) is OpenAlpaca's private history. `memory/` and `skills/` are deliberately **not** ignored: an exported memory pack or a project skill is exactly the kind of thing a project would want in git. The `.gitignore` itself is committable so the rules travel with the repo; `ensure_store()` only writes it when absent, so user edits stick.

### 2.3 Naming rules and namespace reservation

1. Top-level entries in either store root match `^[a-z][a-z0-9-]*$` (dirs) — lowercase, no spaces, no underscores; plural nouns for content collections. Dot-prefixed names (`.layout`, `.gitignore`, `.versions`) are reserved for store metadata, forever.
2. **A new content kind exists when and only when it is added to the `ContentKind` enum** in the store module (§2.4). No crate ever joins a literal directory name onto a store root. This is the enforcement mechanism for "reserve namespaces deliberately": the reservation is a code review on one enum, and grep for `ContentKind::` enumerates every kind in the system.
3. Unknown directories found in a store root are left untouched and never swept — the store never deletes what it did not create (extends rev 1 §1.6's "produced artifacts are never garbage-collected" to the whole tree).
4. `state/` never gains user-facing content; content kinds never gain machine state. A future kind that is "machine state per project" (if one ever exists) gets a reserved `state/` name under the project store — reserved now, unused.

### 2.4 The single Rust module — `crates/openalpaca_storage/src/store/mod.rs`

`paths.rs` is **deleted**, not aliased (purge item P1). Every path in both roots is built by this module and nowhere else. Signatures:

```rust
// ── roots ────────────────────────────────────────────────────────────────
pub fn home_root() -> anyhow::Result<PathBuf>;          // $OPENALPACA_HOME_STORE (absolute) or ~/.openalpaca
pub fn state_dir() -> anyhow::Result<PathBuf>;          // home_root()/state — creates it
pub fn database_path() -> anyhow::Result<PathBuf>;      // state/openalpaca.db
pub fn discovery_path() -> anyhow::Result<PathBuf>;     // state/discovery.json
pub fn lock_path() -> anyhow::Result<PathBuf>;          // state/openalpacad.lock
pub fn master_key_dir() -> anyhow::Result<PathBuf>;     // = state_dir(); passed to KeyEncryptor::ensure_at
pub fn logs_dir() -> anyhow::Result<PathBuf>;           // state/logs — creates it
pub fn interim_assets_dir() -> anyhow::Result<PathBuf>; // state/assets — dies with the D2 re-home (§4)
pub fn plugins_dir() -> anyhow::Result<PathBuf>;        // home_root()/plugins  (replaces main.rs:331's inline join)
pub fn runtime_config_dir() -> anyhow::Result<PathBuf>; // home_root()/config  (GUI/CLI-managed OPENALPACA_CONFIG_DIR value)

// ── content stores (both scopes share one shape) ─────────────────────────
pub enum StoreScope { Project(PathBuf), Home }           // renames rev 1 §1.3's ArtifactScope
pub enum ContentKind { Artifacts, Uploads, Sessions, Memory, Skills, Scratch, Cache }
pub fn store_root(scope: &StoreScope) -> anyhow::Result<PathBuf>;      // <project>/.openalpaca | home_root()
pub fn ensure_store(scope: &StoreScope) -> anyhow::Result<PathBuf>;    // creates + seeds README/.gitignore/.layout
pub fn content_dir(scope: &StoreScope, kind: ContentKind) -> anyhow::Result<PathBuf>;
pub fn layout_version(root: &Path) -> anyhow::Result<Option<u32>>;

// ── carried over from rev 1 §1.3 unchanged except ArtifactScope→StoreScope ──
pub fn run_dir(scope: &StoreScope, created: DateTime<Utc>, task_title: &str, task_id: &str) -> anyhow::Result<PathBuf>;
pub fn loose_dir(scope: &StoreScope, created: DateTime<Utc>) -> anyhow::Result<PathBuf>;
pub fn artifact_file_name(seq: u32, title: &str, ext: &str) -> String;
pub fn slugify(input: &str, max_bytes: usize) -> String;
pub fn version_file_path(head_path: &Path, version: u32) -> anyhow::Result<PathBuf>;
pub fn artifact_extension(kind: ArtifactKind, mime: Option<&str>, name_hint: Option<&str>) -> String;
pub fn confine_to_root(root: &Path, candidate: &Path) -> anyhow::Result<PathBuf>;

// ── D2 upload placement ──────────────────────────────────────────────────
pub fn upload_dir(scope: &StoreScope, created: DateTime<Utc>) -> anyhow::Result<PathBuf>; // uploads/<YYYY-MM-DD>
pub fn upload_file_name(seq: u32, original_name: &str) -> String;  // NN-<slug(orig,60)>.<ext> — reuses slugify

// ── the mover (store/migrate.rs) ─────────────────────────────────────────
pub fn move_app_root();                                  // §1.2; called from main.rs at the old :71 slot
pub fn rebase_asset_paths(db: &Database);                // §1.2 step 6; called from bootstrap after open
```

`asset_storage_path(sha256)` from `paths.rs:75-83` is **deleted** rather than kept "unchanged" as rev 1 §1.3 said — under D2 new uploads are human-named (`upload_dir` + `upload_file_name`, dedup by the existing `sha256` DB column and owner-scoped query at `routes/files.rs:172-185`, not by path), and existing blobs live at `interim_assets_dir()` addressed purely via their stored `storage_path` until the §4 re-home deletes even that.

The rename fan-out from deleting `paths.rs` is exactly the consumer table in §0 — every site is mechanical (`paths::app_dir()` → a specific accessor), and the compiler enumerates them.

---

## 3. The legacy purge

Verdicts: **DELETE** (safe now, this branch), **DELETE-AFTER** (safe once a named dependency lands), **KEEP** (looks legacy, is load-bearing), **OWN-TASK** (a real refactor, not a deletion). Serde's general ignore-unknown-fields behaviour is not listed — it is load-bearing correctness.

| # | Item | Where | Verdict + follow-ups |
|---|---|---|---|
| P1 | `migrate_legacy_app_dir()` + the `com`/`openalpaca` qualifier constants | `paths.rs:24-51`, call at `main.rs:70-71` | **DELETE** — replaced by `move_app_root()` in the same slot (§1.2). Test fallout: `paths.rs` tests move to `store/`; `test_paths_are_consistent` (`paths.rs:89-102`) re-targets `state_dir()`. |
| P2 | Inline legacy master-key copy (`config_base_dir/.master_key` → `app_dir`) | `main.rs:102-127` | **DELETE** — the mover's `.master_key → state/` entry owns relocation; the config-dir-resident key predates "D1: master key always at app_dir" and no config dir on the machine still holds one. Doc: none. |
| P3 | `.alpaca` marker preference | `memory/workspace.rs:43-49` (incl. the redundant `.exists() \|\| .is_dir()`), tests `:108,125` | **DELETE — purge to `.openalpaca` + `.git` only.** Nothing in the repo creates `.alpaca`; the decision rev 1 §1.2 deferred is taken: recognise `.openalpaca` first, `.git` second, `.alpaca` never. Honest cost: any directory on the user's machine that was a workspace root *only* via a hand-made `.alpaca` stops resolving, and memories scoped to it (workspace id = canonical path, `workspace.rs:60-65`) go quiet until the user runs `mv .alpaca .openalpaca` once. State that in the commit message. Tests `:108,125` flip to `.openalpaca`. |
| P4 | Legacy flat `llm.toml` format branch | `crates/openalpaca_llm/src/config/llm_config/router_builder.rs:26-53` (`build_router_from_legacy`) | **DELETE** — the seeded template has been hierarchical (`[providers.*]`, `scripts/release/templates/config/llm.toml:8-21`) for as long as `seed_default_configs` has existed; `build_router` always takes the hierarchical branch on this machine. Pre-deletion check the implementer must run: confirm `LlmConfig` + `build_provider_with_runtime` have no non-legacy consumers before removing them with the branch. Doc: drop the "auto-detects format" doc comment. |
| P5 | Skill frontmatter legacy fields + bridge: `command`, `trigger_patterns`, `tools_required`, `auto_load`, `apply_legacy_compat()`, the legacy half of `effective_slash_command()` | `crates/openalpaca_core/src/middleware/skill/types.rs:335-347, 352-371, 384-392`; call sites `skill/mod.rs:55,65`; tests `skill/tests.rs:44-54,117,262-289` | **DELETE** — no tracked skill uses any of them (grep over `config/skills/` is empty), and plugin-contributed skills build `SkillFrontmatter` with the *new* sections + `..Default::default()` (`crates/openalpaca_plugins/src/manager.rs:940-956`), so they compile unchanged. Doc: `docs/Skill_Template_Reference.md:636`'s "deprecated fields in use" warning class goes away. |
| P6 | `TaskConstraints.pipeline_sequential` | `orchestrator/task_state/state.rs:36,74`; tests `task_state/tests.rs:29,295`, `dispatcher/tests.rs:531` | **DELETE** — a serde vestige of the deleted sequential pipeline; zero non-test readers (verified by grep). Old `state_json` rows carrying the key deserialize fine (unknown-field ignore). The `test_backward_compat_no_workspace_field` test (`tests.rs:290-300`) stays — the user's DB genuinely holds pre-workspace `state_json` rows — but its fixture drops the `pipeline_sequential` key. |
| P7 | `planner_ms` / `dispatch_ms` schema-stability zeros, `mean_planner_ms`, and `dispatch_decisions.planner_requested_mode` | Event: `events.rs:250-260`; writers `handlers.rs:155-157,365-395`, `apps/openalpacad/src/event_bridge.rs:330-341`; repo + table `repository/orchestrator_latency/mod.rs:13,36-63,141-161,183` (`migrations/022_orchestrator_latency.sql`), `repository/dispatch_decision/mod.rs:18,38-48,68,103` (`migrations/024_…`); dispatcher `dispatcher/mod.rs:223`; GUI `lib/api/types.ts:432,446,462` | **DELETE end-to-end** — these fields exist *only* "for schema stability" after the planner ladder's deletion (`handlers.rs:156-157` says so verbatim), and no GUI view renders the latency aggregates (only `types.ts`/`orchestrator.ts`/`useOrchestrator.ts` carry the types; grep of `views/`+`components/` finds no consumer). Migration **038** (`038_drop_planner_telemetry.sql`, after rev 1's 035–037): `ALTER TABLE orchestrator_latency DROP COLUMN planner_ms/dispatch_ms`, `ALTER TABLE dispatch_decisions DROP COLUMN planner_requested_mode` (verify bundled SQLite ≥ 3.35 for DROP COLUMN; else the 024-style table-rebuild). Retired mode *strings* in historical rows need nothing — they are data. Tests: `orchestrator_latency/tests.rs:55-85`, `dispatch_decision/tests.rs:23-77`. |
| P8 | Legacy `assignments` / `assigned_agents` task payload (agent runs served under a compat key) | `apps/openalpacad/src/routes/tasks_types.rs:35-36`, `routes/tasks.rs:8,32,173,513-531`; CLI reader `apps/openalpaca/src/commands/tasks.rs:115-133`; ultimately `agent_task_history` + its timezone-less `completed_at` (`repository/subagent/mod.rs:232`) | **DELETE-AFTER rev 1 Phase 3** — `subagent_span` + `GET /v1/tasks/{id}/timeline` replace the data. Rev 1 §9 deferred the `/v1/tasks` shape normalisation to Phase 3 "where it is a natural consequence"; the purge directive upgrades that from *may* to *does*: when the timeline lands, the `assignments` key, its serde rename, the GUI/CLI parsers, and the `test_task_response_serializes_agent_runs_under_assignments_key` test are deleted in the same PR, and GAP-20's run counts re-point at `subagent_span`. |
| P9 | `DagNodeStatus` / `DagNodeStarted` double emission | producers `runner/lead_agent/tools.rs:232-240`; GUI `components/work/run-events.ts:63-71` | **DELETE-AFTER the GUI switches to `SubagentSpan`** — already rev 1's §8.10 plan; unchanged, restated here so the purge list is complete. No soak ceremony (pre-release, single user). |
| P10 | Backward-compat re-exports in the LLM crate root | `crates/openalpaca_llm/src/lib.rs:12-23` (the TODO says exactly this) | **DELETE** — fix the remaining `openalpaca_llm::router::…`-style consumers to canonical `routing::router` paths; the compiler enumerates them. Zero behaviour change. |
| P11 | `resolve_local_user_id`'s legacy `gui_user:gui` adoption | `apps/openalpacad/src/bootstrap/migration.rs:40-60` | **DELETE the fallback branch** — the user's DB has `identity.local_user_id` persisted, so the branch is dead on the only real install; fresh DBs mint a UUID. Keep the persisted-id read. |
| P12 | `migrate_preference_summaries` (one-time `preference.conversation_summary` → `conversations.summary`) | `bootstrap/migration.rs:80-…`, called `main.rs:190` | **DELETE** — a completed one-time data migration; on the real DB it matches zero rows every boot. |
| P13 | Legacy second-precision persona-backup filename parsing | prune logic exercised by `tools/builtins/helpers/tests.rs:125-145` ("Old format (second-precision, legacy)") | **DELETE the legacy parse arm** — consequence: any remaining old-format `SOUL.20250101T000001Z.md` backups are simply never pruned (kept forever), which is harmless; note it in the commit. XS. |
| P14 | CLI key-removal "migration fallback: exactly one key (legacy)" | `apps/openalpaca/src/commands/ai_config.rs:194-199` | **DELETE** — heuristic for pre-`{provider}_cli`-naming configs; every key on the machine post-dates the naming. XS. |
| P15 | Dual timestamp formats in `event_log` | `repository/event_log/mod.rs:96-105` | **Optional one-time normalising `UPDATE` in migration 038**; rev 1 Phase 3's id-based pagination stays regardless (it is the right key even with clean timestamps). |
| P16 | `secret_encrypted` "legacy encrypted (read-only, for pre-migration compat)" tier | `router_builder.rs:151-160`, `config/llm_config/migration.rs:8-40` | **KEEP** — despite the comment, it is the functional fallback when no OS keychain is available (the CI dbus dance exists precisely because keychains are environmental), and CLAUDE.md documents the three-tier resolution as a feature. Relabel the comment; delete nothing. |
| P17 | `AgentConfigFile` TOML shape + "legacy instance" registration | `agent/config_service.rs:160-247`, `agent/config/mod.rs:14-66`, routes `routes/agents.rs:496,539`, `routes/agents_types.rs:28-40` | **OWN-TASK, not a deletion** — the "legacy" TOML shape is the *live* HTTP contract: the GUI posts `AgentConfigFile` JSON on template create/update (`apps/openalpaca-gui/src/lib/api/agents.ts:31-48,93-97`), and the idle-instance registration backs `/v1/agents`. Collapsing template-vs-instance duality is a real API redesign; flag it, do not sweep it into this purge. |
| P18 | CLI structured-delegation fallback; retired routing-mode strings in GUI label maps | `apps/openalpaca/src/chat_stream/mod.rs:288-293`; GUI grep | **NOTHING TO PURGE — verified.** The CLI parses only the structured `delegation` object (no text-scrape fallback survives), and no GUI label map carries `two_phase_*`/`planner_*`/`fast_path`/`no_llm` (grep over `src/` finds only the `planner_ms`/`planner_requested_mode` *type* fields, covered by P7). Recorded so the synthesizer doesn't hunt for them again. |

**Rev 1 hedges that the mover dissolves** (the "kept so existing readers work" class): `storage_path` staying an absolute resolved path (rev 1 §1.4) survives — but as a *convenience with a boot-time rebase* (§1.2.6), no longer as a compat constraint; `asset_storage_path` staying "unchanged" (rev 1 §1.3) is reversed by D2 (§2.4); "no files move" (rev 1 §1.11.2) is reversed wholesale.

**Documentation fallout of §1+§2** (one sweep, same PR as the mover): `docs/QuickStart_Manual.md:48`, `docs/GUI_Manual.md:41-42`, `docs/Installation_Manual.md:111-113,145,167,192`, `docs/Daemon_Manual.md:42-45`, `docs/CLI_Manual.md:47,96,222`, `apps/openalpaca-gui/README.md:126-127`, `apps/openalpaca-gui/API_MAP.md:37,777-778,932`, `scripts/release/install.sh:135`, `scripts/release/uninstall.sh:43`, CLAUDE.md's "Data directory" paragraph — all name `~/Library/Application Support/OpenAlpaca`. Note the pleasant side effect: the pre-existing doc bug rev 1 §8.12 flagged (`apps/openalpaca/src/commands/plugin.rs:225,230` claiming `~/.openalpaca/plugins/`) becomes *almost true* — fix only its spurious `.config` suffix.

---

## 4. Rev 1 deltas under D1/D2 (mechanical application list for the synthesizer)

| Rev 1 section | Change |
|---|---|
| §0 D1–D5 decision table | Replace with the settled decisions; D1 = moved, D2 = uploads in-store, D3 = extracted_text stays in DB (no §1 change needed — it was already the recommended column), D4 = `~/.openalpaca` + `OPENALPACA_HOME_STORE`, D5 = same task id (unchanged). Delete the two-roots cost/mitigation prose in D1's row. |
| §1.1 layout diagram | Replace the three-root diagram with §2.1/§2.2 of this lens. The line "`~/Library/Application Support/OpenAlpaca/ ← UNCHANGED (D1)`" is deleted; `README.md` is joined by `.gitignore` and `.layout` in both roots. |
| §1.2 marker precedence | `.openalpaca` ahead of `.git`; the `.alpaca` arm is **deleted**, not demoted (P3). The "no install has one yet" safety note gains the P3 cost sentence. |
| §1.3 new surface | Superseded by §2.4 here: module is `store/mod.rs`, `paths.rs` deleted, `ArtifactScope` → `StoreScope`, `home_store_dir()` → `home_root()`, add `state_dir`/`plugins_dir`/`runtime_config_dir`/`logs_dir`/`content_dir`/`ContentKind`/`upload_dir`/`upload_file_name`/mover; `asset_storage_path` deleted instead of "unchanged — doc comment gains a line". |
| §1.4 principle | `project_root`+`rel_path` as address of record stands. The `storage_path` sentence gains: "kept valid across the root move by `rebase_asset_paths` (one idempotent boot statement)". |
| §1.5 migration 035 | Unchanged. Add migration **038** (`038_drop_planner_telemetry.sql`, P7 + optionally P15) to the sequence. |
| §1.6 sweep/quota fixes | Both fixes unchanged in substance (`origin='upload'` predicate; `WHERE origin='upload'` quota). The sweep's file-deletion path (`background.rs:341`) now deletes under `state/assets/` (interim) or `uploads/` (post re-home) — same `storage_path` mechanism, no extra change. |
| §1.7 producer | Unchanged, except `artifact_write` resolves placement via `StoreScope`/`content_dir`. |
| §1.8 project prerequisite | Item 1's fallback is now the home root's own content dirs (same behaviour, one root). Item 4 (**/v1/status**) reports **one** root: `home_root`, `state_dir`, `db_path`, resolved project dir — the "reporting both roots" mitigation prose is deleted. **New item 5 (D2):** `POST /v1/files/upload` reads `x-workspace-path` like `/v1/chat` does (`routes/chat.rs:65-68` is the pattern; the route currently reads no header, `routes/files.rs:24-27`) and writes via `upload_dir(scope, …)`; connector uploads have no project signal and take `StoreScope::Home`. Collapse the connector's duplicated sha/dedup write path (`connectors/src/common/mod.rs:213-262`) into the same writer first — rev 1 D2's own "two writers" warning, now in scope. |
| §1.9 lifecycle table | "Size accounting" row unchanged (`upload_bytes` quota-bearing regardless of placement). Add a row: **root moved** — impossible by construction for the home root (it *is* the address baseline); project moves keep using `rebase_project`. |
| §1.10 / GAP-11 | Unchanged. |
| §1.11 "not a breaking change" list | Items 2 ("No files move") and 5 are rewritten: files **do** move once, at boot, atomically, resumably (§1.2); rollback story becomes "the old root's layout is reconstructible by reversing the entry ledger, but is not automated — pre-release, single user". |
| §2 Phase 0 | Unchanged. |
| §3 Phase 1 | Step 2 becomes the `store/` module + mover; step 3 is P3's delete-not-demote; add the doc sweep + P1/P2 deletions to the same phase (the mover PR). |
| §5 Phase 3 | GAP-09/GAP-10 unchanged; append P8's deletion (assignments key + CLI/GUI parsers) as a Phase-3 exit criterion rather than a deferred normalisation. |
| §8 Phase 6 | Item 1 (GAP-14): `data_dir` → `home_root`; `log_path` may now serve `state/logs/daemon.log` **when the CLI-managed log exists** (`manager.rs:38`) — still `null` for GUI-sidecar daemons until Phase B. Item 10 = P9. Item 11's "only if D2 is yes" condition resolves to **yes**: the uploads re-home (existing `state/assets/` blobs → `uploads/<created-date>/NN-<name>.<ext>`, per-row move+UPDATE, resumable, then delete `interim_assets_dir`) is a Phase-6 item. Item 12's doc bug: fix `.config` suffix only (§3 above). |
| §9 cross-cutting | The `{error:"string"}`/list-shape non-retrofits stand (they are churn-avoidance, not legacy-compat — the purge directive does not touch them). |
| §11 risks | Delete "Existing installs / no files move" row; add: **root move** (mitigated: atomic per-entry, resumable, live-daemon guard, `rebase_asset_paths`), **atomic three-binary rebuild** (no compatibility window — deliberate), **`.alpaca` roots go quiet** (P3 cost). |
| §12 out of scope | Delete "Moving `app_dir()` (D1). Rejected" and "Re-homing existing uploads (D2)" — both are now in scope. Everything else stands. |
| §13 migration ledger | Add: **038** `drop_planner_telemetry` (P7, optional P15) — plus the two *unnumbered* boot-time fixups, listed in the ledger for completeness but explicitly not schema migrations: `move_app_root()` (filesystem) and `rebase_asset_paths()` (runtime-prefix UPDATE). `database/tests.rs:11` head-version assert updates with each numbered migration as before. |

*Coordination handles for Lens S:* the `sessions/` name in both roots and its `.gitignore` line are reserved here (§2.2); `content_dir(scope, ContentKind::Sessions)` is the only path Lens S's code should call to reach its root; everything beneath it — per-session directories, the append-only JSONL event log, `snapshots/`, `attachments/`, `large-tool-results/` — is Lens S's to define, including its own sub-naming rules.
