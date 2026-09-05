# Lens A — Project-scoped artifact store

**Status:** research + design. No code written. **Date:** 2026-09-01 · **Branch:** `feat/ui-rework`
**Serves:** GAP-04, GAP-05, GAP-11, GAP-12 (and gives GAP-23 its artifact half).
**Depends on nothing. Lenses B and C depend on §4 (schema) and §6 (HTTP surface).**

---

## 0. Recommendations at a glance

| # | Question | Recommendation |
|---|---|---|
| 1 | Where do artifacts live? | `<project>/.openalpaca/artifacts/…` when a project root is known; `~/.openalpaca/artifacts/…` when not. **Do not move** the DB, `discovery.json`, the lock file, or the plugins dir out of `app_dir()`. |
| 2 | Is `~/.openalpaca/` new? | Yes. It is a *new, artifacts-only* root. `app_dir()` stays `~/Library/Application Support/OpenAlpaca/` (`paths.rs:18-22`). |
| 3 | Project marker | Add `.openalpaca` as the preferred marker in `walk_up_for_marker` (`memory/workspace.rs:43-56`), keeping `.alpaca` and `.git` as fallbacks. |
| 4 | Layout | `.openalpaca/artifacts/<YYYY-MM-DD>-<task-slug>-<taskid8>/<NN>-<name-slug>.<ext>`, superseded versions in a sibling `.versions/<stem>/vN.<ext>`. |
| 5 | Uploads vs produced | **One table, two storage strategies.** Produced artifacts go project-scoped and human-named. Chat uploads keep content-addressed `assets/ab/cd/<sha256>` in `app_dir` (phase 1); phase 2 optionally re-homes them. Justified in §3. |
| 6 | Schema | **Extend `file_assets`** (one migration, 11 added columns) + a new `artifact_versions` table. Do **not** create a parallel `artifacts` table. |
| 7 | DB stores addresses | `project_root` + `rel_path` are the address of record; `storage_path` stays as the resolved absolute path so today's readers keep working. |
| 8 | The real blob-in-DB offender | `TaskWorkspace` entries carry up to 32 KB of content each, 50 per task, inside `task.state_json` (`task_state/workspace.rs:41-47`, `migrations/016_task_state.sql:1`). Artifact-typed entries must spill to a file and keep only the id. This is the directive's core fix. |
| 9 | Producer | A new `artifact_write` tool, plus a bridge so `workspace_write(entry_type="artifact")` routes through the same store. Today **nothing** produces a `FileAsset` except `POST /v1/files/upload`. |
| 10 | Pins (GAP-12) | Add the `pinned` column + `PUT /v1/artifacts/{id}/pin` in the same migration (near-zero cost), but leave the client's `localStorage` authoritative until it opts in — `unbacked.ts`'s `Artifact` has no `pinned` field. |

---

## 1. Ground truth (verified, with citations)

Everything in the brief was checked. Three items in the brief are **wrong or incomplete** and the design depends on the corrections:

1. **`workspace_id` is not hashed.** `workspace_id_from_root` returns the *canonicalized path string* (`crates/openalpaca_core/src/memory/workspace.rs:60-65`). That is excellent news: the workspace id already **is** the project root path, so a task's project directory is recoverable from data the dispatcher already receives (`dispatcher/lead_agent.rs:24-31` takes `workspace_id: Option<String>`).
2. **The project marker today is `.alpaca`, not `.openalpaca`** (`memory/workspace.rs:43-56`, preferring `.alpaca`, falling back to `.git`). The directive's `.openalpaca/` must be added to that walker or artifacts will land in a directory that is not itself a workspace root.
3. **No client ever sends a workspace.** `POST /v1/chat` reads `x-workspace-path` (`apps/openalpacad/src/routes/chat.rs:65-68`) and `POST /v1/command` reads a `workspace_path` field (`routes/command.rs:78-80`), but `grep -rn "x-workspace-path" apps/openalpaca-gui/src apps/openalpaca/src` finds only the *definition* in `apps/openalpaca-gui/src/lib/chat-stream.ts:349-366` — **no call site sets it**, and the CLI never sends one. So `handlers.rs:90-97` always takes the `else` branch and resolves the workspace from the **daemon's CWD**. A project-scoped store is only as good as the project signal; see §1.1.

Other verified facts:

| Fact | Citation |
|---|---|
| `app_dir()` = `~/Library/Application Support/OpenAlpaca/` on macOS | `crates/openalpaca_storage/src/paths.rs:18-22` |
| `discovery.json`, `openalpacad.lock`, `openalpaca.db`, `assets/` all under `app_dir()` | `paths.rs:52-70` |
| Plugins dir is `app_dir()/plugins` — **not** `~/.openalpaca/plugins` as the CLI help text claims | `apps/openalpacad/src/main.rs:331` vs `apps/openalpaca/src/commands/plugin.rs:225,230` and `docs/CLI_Manual.md:222` (pre-existing doc bug, worth a one-line fix) |
| `asset_storage_path(sha256)` = `assets/<ab>/<cd>/<sha256>`, **no extension** | `paths.rs:72-81` |
| `FileAsset` fields — no `task_id`, `agent_id`, `kind`, `pinned`, `version` | `crates/openalpaca_storage/src/models/file_asset.rs:36-51` |
| `file_assets` DDL + its two indexes | `crates/openalpaca_storage/src/migrations/027_file_assets.sql` |
| `FileAssetRepository` — `insert`, `get_by_id`, `get_by_sha256`, `update_status`, `total_storage_bytes`, `delete_by_id`, `list_orphaned`, `link_to_message`, `list_by_status`, `get_attachments_for_message`. No list-for-user, no filtering. | `crates/openalpaca_storage/src/repository/file_asset/mod.rs:16-164` |
| There are exactly **two** `FileAsset` producers, both content-addressed, both for *inbound* files — the HTTP upload route and the connector attachment path. **Nothing produces a `FileAsset` for agent output.** | `apps/openalpacad/src/routes/files.rs:220-247` and `crates/openalpaca_connectors/src/common/mod.rs:213-262` (the `chat/service.rs` `insert` calls are inside `#[cfg(test)]`) |
| Artifact→file delivery to connectors **already exists** and resolves `ArtifactPointer.file_asset_id`, falling back to treating `key` as a file id — it just never finds one, because nothing sets it | `apps/openalpacad/src/notification/artifacts.rs:23-52, 55-110` (caps: 5 artifacts/task, 50 MB/file) |
| Orphan sweep deletes any asset unlinked from a message after the grace period | `apps/openalpacad/src/background.rs:308-357`; SQL at `repository/file_asset/mod.rs:95-112` |
| Grace period default 24 h, sweep every 6 h | `crates/openalpaca_core/src/daemon_config/upload.rs:73-74` |
| Per-file cap **50 MB** (config), body limit **100 MB** (router) | `daemon_config/upload.rs:48` vs `apps/openalpacad/src/router.rs:117-119` |
| Total storage quota 500 MB, enforced against `SUM(size_bytes)` over *all* rows | `daemon_config/upload.rs:49`, `routes/files.rs:84-97`, `repository/file_asset/mod.rs:76-85` |
| Upload dedup is by sha256 **scoped to the owner** | `routes/files.rs:172-185` |
| `/v1/files/{id}/content` streams with correct `Content-Type`/`Content-Disposition: inline` and is Bearer-only | `routes/files.rs:286-345`; auth layer `apps/openalpacad/src/middleware.rs:17-38` applied at `router.rs:263-266` |
| The `?token=` pattern already exists twice | `routes/chat.rs:104-113` (SSE) and `router.rs:268-274` (WS `/v1/events`), both merged **outside** the auth layer |
| `POST /v1/files/{id}/open` copies to `$TMPDIR/openalpaca-open/{id}-{safe_name}` before `opener::open` | `routes/files_types.rs:92-118` — **this staging exists only because the content-addressed path has no extension** |
| `file_write` writes into `workspace_root`, capped at 10 MB, `..`/absolute rejected, symlink-escape checked | `tools/builtins/file_ops.rs:101-160`, helpers at `tools/builtins/helpers/mod.rs:11-117` |
| `workspace_root` for file tools is the **daemon's CWD captured at startup**, not the request's workspace | `apps/openalpacad/src/services/tools.rs:37-39`, consumed at `tools/builtins/mod.rs:249-251` |
| `TaskWorkspace` entries hold inline content, `max_entry_size: 32768`, `max_entries: 50`, with an unused-in-practice `file_asset_id` | `orchestrator/task_state/workspace.rs:20-48, 121-138` |
| Artifact pointers are harvested from workspace entries typed `Artifact` and already carry `file_asset_id` | `task_state/outcome.rs:148-161`, `dispatcher/outcome.rs:172-215` |
| `task` table has **no** workspace/project column | `migrations/006_tasks.sql:5-18`, `models/task.rs:101-122` |
| `agent_task_history` has no `started_at` (Lens C's problem, noted here because §6 joins it for attribution) | `migrations/007_subagents.sql:26-34` |
| SQLite runs with `foreign_keys = ON`, WAL, `busy_timeout=5000` | `crates/openalpaca_storage/src/database/mod.rs:53-63` |
| Migrations are a static array; latest is 34 | `crates/openalpaca_storage/src/migrations/mod.rs:13-…` (tail: version 34) |
| No diff crate anywhere in the workspace | `grep -n "similar\|diffy\|imara" Cargo.toml crates/*/Cargo.toml` → no matches |
| Client contract for `Artifact` / `ArtifactVersion` / `ArtifactDiff` | `apps/openalpaca-gui/src/lib/api/unbacked.ts:28-99` |
| Kind filtering in the Library is **client-side** (`matchesKindFilter`), so a server `kind` filter is a nicety | `apps/openalpaca-gui/src/views/library/LibraryList.tsx:41-45` |
| Pins are `localStorage` via the ui store; `Artifact` in `unbacked.ts` has no `pinned` field | `views/library/LibraryDetail.tsx:43-44`, `lib/api/unbacked.ts:39-56` |

### 1.1 The prerequisite nobody has written down

**A project-scoped store needs a project.** Today the only project signal is a header no client sends, falling back to the daemon's CWD (`handlers.rs:90-97`) — and the daemon's CWD, when launched as a Tauri sidecar or a LaunchAgent, is arbitrary. Two consequences:

- Every artifact would land in whatever directory the daemon happened to start in. That is worse than the status quo.
- Therefore **the GUI must send `x-workspace-path`**, or the store must fall back to `~/.openalpaca/`.

Recommendation, in order of cost:

1. **(Required, small)** Make `~/.openalpaca/` the fallback when no project is resolvable, and *never* use the daemon CWD for artifact placement. CWD-derived workspace ids are fine for memory scoping (existing behaviour) but must not decide where files land.
2. **(Required, small)** The GUI sends `x-workspace-path` on `POST /v1/chat` when the user has chosen a project. `chat-stream.ts:349-366` already plumbs it; only a caller is missing.
3. **(Recommended, medium — hand to Lens B)** A minimal project concept the UI can drive: `GET /v1/projects` (recently used, from `DISTINCT project_root`), `POST /v1/projects/activate {path}`. Out of scope for this lens, but the store's shape assumes a project can be named.

---

## 2. §1 — Where files live

### 2.1 Recommendation

```
<project>/.openalpaca/            ← when a project root is resolvable
  README.md                       ← written once, explains the directory
  artifacts/                      ← produced artifacts (see §2 layout)
  uploads/                        ← phase 2 only (see §3)

~/.openalpaca/                    ← when no project is resolvable
  artifacts/
  README.md

~/Library/Application Support/OpenAlpaca/   ← UNCHANGED
  openalpaca.db  discovery.json  openalpacad.lock  assets/  plugins/  config seeds
```

### 2.2 Why not move `app_dir()` to `~/.openalpaca/`

The directive says artifacts should live in a dot-dir "analogous to a dot-dir in `~`". It does **not** say the daemon's runtime state should move, and moving it costs a lot for nothing:

- `database_path()`, `discovery_path()`, `lock_path()`, `assets_dir()` and `main.rs:331`'s plugin dir all hang off `app_dir()` (`paths.rs:52-81`). Relocating means a second `migrate_legacy_app_dir()`-style rename (`paths.rs:24-49`) that must run before the singleton lock is taken and before the DB is opened — the same ordering hazard the existing migration comments call out, but now with a live plugins directory and user-approved `.permissions.toml` files inside it.
- `~/Library/Application Support` is the correct macOS location for opaque runtime state, and is what `directories::ProjectDirs` gives on every platform. `~/.openalpaca` would be a Unix-ism on Windows.
- Users never open `openalpaca.db` by hand. They **do** open artifacts by hand — that is the entire point of the directive. So the human-findability argument applies to artifacts and not to the DB.

**So: split the roots.** `app_dir()` = machine state. `.openalpaca/` = user-facing artifacts. The directive is satisfied where it matters and nothing existing breaks.

Honest cost of the split: two "OpenAlpaca directories" to explain. Mitigated by the `README.md` the store drops in `.openalpaca/` on creation and by `GET /v1/status` (GAP-14) reporting both roots.

### 2.3 Why `.openalpaca/` inside the project (and what it costs)

| Scenario | Behaviour with `<project>/.openalpaca/` | Verdict |
|---|---|---|
| Multi-project use | Each project's outputs sit beside its source. No global namespace collisions. | **Win** — the main reason for the directive |
| Project moved/renamed | The files move with the project; the DB's `storage_path` goes stale. Handled by storing `project_root` + `rel_path` and re-basing (§5). | Manageable |
| Project deleted | Artifacts die with it. Rows survive, marked `missing`. Acceptable: the user deleted the project. | Acceptable |
| Git-ignoring | The store writes `.openalpaca/.gitignore` containing `.versions/` and nothing else, so **head artifacts are committable by default** and version history is not. | **Deliberate.** See below |
| Two daemons on one project | Both write into the same `.openalpaca/artifacts/`. Run dirs carry a task-id suffix, so no collision on directories; two daemons with two DBs would each hold half the index. Real but out of scope (a singleton lock already prevents two daemons per machine — `paths.rs:57-59`). | Documented limitation |
| User commits artifacts | Supported and default-on for head files: a produced `findings.md` is a document, and versioning it in git is strictly better than versioning it in `.versions/`. | **Win** |

The `.gitignore` decision is worth stating plainly: **do not ignore `.openalpaca/` wholesale.** Ignoring it makes the artifacts invisible to the tool the user already uses for documents. Ignore only `.versions/`, which is OpenAlpaca's private history and would be noise in a repo that has git.

Caveat to flag: writing `<project>/.openalpaca/` **creates a workspace-root marker** where none existed. After the first artifact, `resolve_workspace_id` (`memory/workspace.rs:69-71`) will resolve that directory as the workspace root for anything below it. Since workspace ids are path strings and `.openalpaca` is new, no existing memory scope changes — but the effect should be intentional and documented, not incidental. It is also *desirable*: the artifact write is exactly the moment the directory becomes an OpenAlpaca project.

### 2.4 Marker precedence change

```rust
// memory/workspace.rs:43-56 — proposed
fn walk_up_for_marker(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        for marker in [".openalpaca", ".alpaca", ".git"] {
            if current.join(marker).exists() {
                return Some(current);
            }
        }
        if !current.pop() { return None; }
    }
}
```

Safe: `.openalpaca` does not exist on any install yet, so no existing resolution changes. Note the existing line `current.join(".alpaca").exists() || current.join(".alpaca").is_dir()` (`workspace.rs:46`) is redundant — `exists()` already covers `is_dir()`; the rewrite drops the redundancy.

---

## 3. §2 — Path naming scheme

### 3.1 Layout

```
<store>/artifacts/
  2026-09-01-connector-audit-3f2a1b7c/          run dir: <date>-<task-slug>-<taskid8>
    01-connector-audit-findings.md              head, currently v2
    02-migration-plan.md                        head, v1
    03-screenshot-settings-drawer.png
    .versions/
      01-connector-audit-findings/
        v1.md                                   superseded only; head is NOT duplicated
  loose/
    2026-09-01/
      01-weekly-report.html                     produced outside any task (main loop)
```

Rules:

- **Run dir** `<YYYY-MM-DD>-<task-slug(≤48)>-<taskid8>`. Date first so `ls` sorts chronologically; the slug makes it findable by eye; the 8-hex task-id suffix (task ids are UUIDv4, `dispatcher/lead_agent.rs:32`) makes it collision-free and greppable from a task id. Total ≤ 68 chars.
- **Sequence prefix** `NN-` is a per-run counter assigned at *first* creation and retained across versions, so `ls` shows production order and re-writes don't reshuffle. Two digits, widening to three past 99.
- **Head at the clean path.** The newest version is always the plainly named file. This is what makes `open`, `Reveal in Finder`, `grep`, and `git diff` work. It also lets `POST /v1/files/{id}/open` skip the `$TMPDIR` staging copy entirely (`routes/files_types.rs:92-118`) for produced artifacts, because the path now has a real extension.
- **Superseded versions in `.versions/<stem>/vN.<ext>`.** File-per-version, hidden, sibling. Chosen over (a) `name.v1.md` beside the head — clutters the directory the human is meant to browse — and over (b) a versions subdir holding *every* version including head — doubles bytes and makes "which file do I open?" ambiguous.
- **Write protocol:** write `.<stem>.tmp` → `fsync` → rename current head into `.versions/<stem>/v<N-1>.<ext>` → rename tmp to head. Rename-based, so a crash leaves either the old head or the new one, never a truncated file.

### 3.2 Filename grammar

```
name    := slug "." ext
slug    := [a-z0-9]+ ("-" [a-z0-9]+)*        1..60 bytes
ext     := [a-z0-9]{1,8}
```

Slugification of a model-authored title:
1. Unicode NFKD, drop combining marks, transliterate what maps to ASCII, drop the rest.
2. Lowercase (**required**, not cosmetic: APFS is case-insensitive by default, so `Findings.md` and `findings.md` are the same file — folding at the source removes a whole class of surprise).
3. Replace every run of non-`[a-z0-9]` with a single `-`; trim leading/trailing `-`.
4. Truncate to 60 bytes on a char boundary; re-trim trailing `-`.
5. If empty → `artifact`. If the stem matches a Windows reserved device name (`con`, `prn`, `aux`, `nul`, `com1..9`, `lpt1..9`) → prefix `_`.
6. Uniqueness within the run dir is by `(seq, slug)`; a same-slug write with a *different* seq gets `-2`, `-3`… appended before the extension.

Path-traversal safety falls out of the grammar: `/`, `\`, `.`, `..`, NUL and every control character are removed in step 3, so a slug can never contain a separator. Belt and braces: the final path is confined to the store root with the same canonicalize-the-parent technique used at `tools/builtins/helpers/mod.rs:65-117` (`resolve_workspace_path_for_write`), which handles the not-yet-existing-file case correctly.

Length: 60-byte stem + 3-byte seq prefix + 9-byte ext ≤ 72, well under the 255-byte per-component limit; run dir ≤ 68. Deepest path is `<project>/.openalpaca/artifacts/<run 68>/.versions/<stem 60>/v99.<ext>` ≈ project + 160 bytes, safe under `PATH_MAX`.

### 3.3 Extension inference

Precedence: (1) an extension already present on the model-supplied name **if** it is in the allow-list; (2) mapped from `kind`; (3) mapped from `mime_type` via the `mime_guess`-style table already implied by the upload validator; (4) `.bin`.

| `ArtifactKind` (matches `unbacked.ts:28-38`) | Default ext | Default mime |
|---|---|---|
| `markdown` | `md` | `text/markdown` |
| `code` | from the name, else `txt` | `text/plain` |
| `terminal` | `log` | `text/plain` |
| `table` | `csv` | `text/csv` |
| `plan` | `md` | `text/markdown` |
| `image` | from mime (`png`/`jpg`/`svg`/`webp`) | from mime |
| `html` | `html` | `text/html` |
| `binary` | from the name, else `bin` | `application/octet-stream` |

### 3.4 New functions in `crates/openalpaca_storage/src/paths.rs`

```rust
/// Which store a given artifact belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactScope {
    /// A resolved project root — artifacts live in `<root>/.openalpaca/`.
    Project(PathBuf),
    /// No project — artifacts live in `~/.openalpaca/`.
    Home,
}

/// `~/.openalpaca` — the artifacts-only home store. NOT `app_dir()`.
/// Honours `OPENALPACA_HOME_STORE` for tests and for users who want it elsewhere.
pub fn home_store_dir() -> anyhow::Result<PathBuf>;

/// `<project_root>/.openalpaca`. Pure — does no I/O, does not canonicalize.
pub fn project_store_dir(project_root: &Path) -> PathBuf;

/// `<store>/artifacts`, creating it (and the README/.gitignore) on first use.
pub fn artifacts_dir(scope: &ArtifactScope) -> anyhow::Result<PathBuf>;

/// `<artifacts>/2026-09-01-connector-audit-3f2a1b7c`
pub fn run_dir(
    scope: &ArtifactScope,
    created: chrono::DateTime<chrono::Utc>,
    task_title: &str,
    task_id: &str,
) -> anyhow::Result<PathBuf>;

/// `<artifacts>/loose/2026-09-01` — produced with no owning task.
pub fn loose_dir(
    scope: &ArtifactScope,
    created: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<PathBuf>;

/// `01-connector-audit-findings.md`
pub fn artifact_file_name(seq: u32, title: &str, ext: &str) -> String;

/// The slug rules of §3.2. Pure, total, never returns an empty string.
pub fn slugify(input: &str, max_bytes: usize) -> String;

/// `<run_dir>/.versions/<stem>/v2.md` for a head at `<run_dir>/01-<stem>.md`.
pub fn version_file_path(head_path: &Path, version: u32) -> anyhow::Result<PathBuf>;

/// Extension for a produced artifact. `name_hint` is the model's proposed filename.
pub fn artifact_extension(
    kind: ArtifactKind,
    mime_type: Option<&str>,
    name_hint: Option<&str>,
) -> String;

/// Reject any candidate that escapes `root` via `..`, absolute components, or a
/// symlinked parent. Mirrors `tools/builtins/helpers::resolve_workspace_path_for_write`
/// but lives here so both the tool and the HTTP routes share one implementation.
pub fn confine_to_root(root: &Path, candidate: &Path) -> anyhow::Result<PathBuf>;
```

**`asset_storage_path` is kept unchanged** (`paths.rs:72-81`). Its only caller is the upload route (`routes/files.rs:188`), and per §3 uploads stay content-addressed. Its doc comment should gain a line saying it is the *upload* path, and that produced artifacts use `run_dir`/`loose_dir`.

`ArtifactKind` is a new enum in `crates/openalpaca_storage/src/models/artifact.rs` with `as_str`/`parse` in the style of `FileAssetStatus` (`models/file_asset.rs:15-33`), and its `serde(rename_all = "snake_case")` spellings must match `unbacked.ts:28-38` exactly.

---

## 4. §3 — Chat attachments vs produced artifacts

### 4.1 The tension

| | Chat upload | Produced artifact |
|---|---|---|
| Origin | The human, before any run exists | An agent, inside a known run |
| Identity | Content (sha256) | Name + owning run |
| Mutability | Immutable | Versioned; v2 supersedes v1 |
| Dedup | Yes, per owner (`routes/files.rs:172-185`) | Meaningless — v2 is *supposed* to differ |
| Human-findability | Low value (they already have the original) | The entire point |
| Project | Unknown at upload time — no workspace header on `POST /v1/files/upload` | Known from the task |

### 4.2 Recommendation: one record, two placements

- **One table and one id space** — the Library, `/v1/artifacts`, the content route, pins and previews all work uniformly, and the client's single `Artifact` type covers both.
- **Two placement strategies**, selected by an `origin` column:
  - `origin='upload'` → `assets/<ab>/<cd>/<sha256>` under `app_dir` (unchanged).
  - `origin='produced'` → `<store>/artifacts/…` per §2/§3.

Reasoning against forcing uploads into `.openalpaca/` in phase 1:

1. Neither inbound path has a workspace signal. `POST /v1/files/upload` reads no header at all (`routes/files.rs:24-27`) and the GUI uploads before a lane or task exists; the connector path (`connectors/src/common/mod.rs:213-262`) is a Telegram/Discord/iMessage message with no filesystem context whatsoever. Placing either into a project would require guessing.
2. Dedup by sha is load-bearing today and would become scope-local, changing observable behaviour (re-uploading the same PDF in a second project would now cost a second copy).
3. Re-homing existing uploads is a data migration with no user-visible payoff: the user already has the original file on disk.

Phase 2, if the user wants uploads in the project too (cheap once phase 1 lands, but note it is **two** call sites, not one): read `x-workspace-path` on the upload route, write to `<store>/uploads/<YYYY-MM-DD>/<NN>-<name>.<ext>`, keep `sha256` for dedup *within a scope*, and leave old rows where they are — `storage_path` is absolute, so the two placements coexist row-by-row with no migration.

**Flagging honestly:** this is a partial deviation from "artifacts of ALL kinds in the project directory". I read "artifacts" as "the things the agents produce" and uploads as inputs. If the user means uploads too, phase 2 is the whole delta and the schema below already supports it — no re-design needed.

### 4.3 The blob that actually has to move

The directive's sharpest target is not `file_assets` — it is `TaskWorkspace`. Every artifact an agent "produces" today is a string inside `task.state_json`:

- `WorkspaceEntry { content: String, … }`, capped at `max_entry_size: 32768`, `max_entries: 50` (`task_state/workspace.rs:20-30, 41-47`).
- Persisted as one `state_json` TEXT column (`migrations/016_task_state.sql:1`), rewritten under optimistic locking on every mutation.
- `collect_artifacts_from_workspace` turns entries typed `Artifact` into `ArtifactPointer`s, already carrying an `Option<String> file_asset_id` that nothing populates except a model-supplied argument (`task_state/outcome.rs:148-161`; `tools/builtins/mod.rs:196-198`).

So: up to 1.6 MB of artifact content per task, rewritten on every workspace write, in a column that is `#[serde(skip_serializing)]` and therefore invisible to the UI (`models/task.rs:115-116`).

**Fix:** `workspace_write(entry_type="artifact")` spills content to the artifact store, stores the returned artifact id in `file_asset_id`, and keeps only a short preview (say 512 chars) in `content` for prompt assembly (`format_for_prompt` truncates at 2000 anyway — `task_state/workspace.rs:157`). `ArtifactPointer.file_asset_id` then resolves, `task.artifact_count` becomes meaningful, and `GET /v1/tasks/{id}` gains real artifact references for free (`orchestrator/mod.rs:551-558`). It also switches on a feature that is already written and currently dead: `deliver_artifacts` (`apps/openalpacad/src/notification/artifacts.rs:55-110`) walks `outcome.artifacts`, resolves each `file_asset_id`, and sends the file to file-capable connector channels — it silently `continue`s today because the id is always `None`.

Plus the explicit producer:

```
artifact_write(name, kind, content, note?, summary?, metadata?) -> { artifact_id, path, version }
```

registered next to `file_write` in `tools/builtins/`, capability `artifact_write`, resolving scope from `ToolContext.workspace_id` (`tools/registry/mod.rs:23`) and attribution from `ToolContext.task_id`/`agent_id` (`registry/mod.rs:20-21`). Note this is the first file-writing tool that must **not** use the startup-captured `workspace_root` (`services/tools.rs:37-39`), because the artifact store is per-request — the scope has to come from the context, not the constructor.

---

## 5. §4 — Database schema

Principle: **the DB holds addresses and metadata; bytes live on disk.** `project_root` + `rel_path` are the portable address; `storage_path` remains the resolved absolute path so that `routes/files.rs:315`, `files_types.rs:142-145` and `background.rs:339` keep working untouched.

### 5.1 Migration 035

```sql
-- Migration 035: project-scoped artifact store
-- Extends file_assets into the unified artifact record and adds version history.

-- ── Ownership and attribution (GAP-04) ───────────────────────────────────────
-- 'upload'   : user-supplied, content-addressed under app_dir/assets  (unchanged)
-- 'produced' : agent output, human-named under <store>/.openalpaca/artifacts
ALTER TABLE file_assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'upload';
ALTER TABLE file_assets ADD COLUMN kind TEXT;               -- ArtifactKind, NULL for legacy uploads
ALTER TABLE file_assets ADD COLUMN task_id TEXT REFERENCES task(id) ON DELETE SET NULL;
ALTER TABLE file_assets ADD COLUMN agent_id TEXT;           -- runtime instance id ("code_agent::a1b2")
ALTER TABLE file_assets ADD COLUMN agent_template_id TEXT;  -- "code_agent"

-- ── The address (the directive) ──────────────────────────────────────────────
-- project_root: absolute dir that owns the .openalpaca store; NULL => home store
-- rel_path    : path of the HEAD file relative to <store>/artifacts
-- storage_path (existing) stays the resolved absolute path of the head file.
ALTER TABLE file_assets ADD COLUMN project_root TEXT;
ALTER TABLE file_assets ADD COLUMN rel_path TEXT;

-- ── Versions (GAP-05) ────────────────────────────────────────────────────────
ALTER TABLE file_assets ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE file_assets ADD COLUMN version_count INTEGER NOT NULL DEFAULT 1;

-- ── UI affordances ───────────────────────────────────────────────────────────
ALTER TABLE file_assets ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;   -- GAP-12
ALTER TABLE file_assets ADD COLUMN summary TEXT;        -- "+41 −6" / "exit 0 · 1.4s" / "3 rows"
ALTER TABLE file_assets ADD COLUMN missing_since TEXT;  -- set when the file is gone from disk

CREATE INDEX IF NOT EXISTS idx_file_assets_task    ON file_assets(task_id);
CREATE INDEX IF NOT EXISTS idx_file_assets_origin  ON file_assets(origin, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_assets_project ON file_assets(project_root);
-- One live head per (scope, task, slug). rel_path already encodes run dir + seq + slug.
CREATE UNIQUE INDEX IF NOT EXISTS idx_file_assets_addr
    ON file_assets(COALESCE(project_root, ''), rel_path)
    WHERE rel_path IS NOT NULL;

-- ── Version history (GAP-05) ─────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS artifact_versions (
    artifact_id     TEXT    NOT NULL REFERENCES file_assets(id) ON DELETE CASCADE,
    version         INTEGER NOT NULL,
    rel_path        TEXT    NOT NULL,   -- '.versions/<stem>/v1.md', or = head rel_path for the head
    sha256          TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL,
    note            TEXT,               -- model-authored "why this version"
    author_agent_id TEXT,
    added_lines     INTEGER,            -- NULL on v1
    removed_lines   INTEGER,            -- NULL on v1
    created_at      TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (artifact_id, version)
);
CREATE INDEX IF NOT EXISTS idx_artifact_versions_artifact
    ON artifact_versions(artifact_id, version DESC);

UPDATE schema_version SET version = 35 WHERE version = 34;
```

Registered in `crates/openalpaca_storage/src/migrations/mod.rs` as `Migration { version: 35, name: "artifact_store", sql: include_str!("035_artifact_store.sql") }`.

SQLite notes, checked: `ADD COLUMN` with a `REFERENCES` clause is legal **only with a NULL default**, which `task_id` has — and `foreign_keys = ON` (`database/mod.rs:62`), so the constraint is enforced. `ADD COLUMN … NOT NULL DEFAULT` is legal for the non-NULL columns. Partial unique indexes are supported (SQLite ≥ 3.8.0).

### 5.2 What stays out of the DB, and one judgement call

- **Bytes**: never in the DB. Already true for `file_assets`; becomes true for produced artifacts via §4.3.
- **`extracted_text`** (`models/file_asset.rs:46`, capped 50 000 chars at `daemon_config/upload.rs:28`): **keep it in the DB**, and label it in the model docs as a *derived text index*, not the artifact. It is read on the prompt-assembly hot path, needs to be queryable, and is bounded. Moving it to `.openalpaca/.text/<id>.txt` would add an I/O round trip to every attachment turn for no user-visible gain. Flagging this as the one place I am deliberately not applying "addresses only" — if the user disagrees, the change is one column drop plus a sidecar path helper, and nothing else in this design moves.
- **`metadata_json`**: stays. It is metadata by definition (`{added,removed}`, `{exit_code,duration_ms}`, `{width,height}`, `{rows}` per `API_MAP.md:408`).

### 5.3 Model and repository signatures

```rust
// crates/openalpaca_storage/src/models/artifact.rs (new)
pub enum ArtifactKind { Markdown, Code, Terminal, Table, Plan, Image, Html, Binary }
pub enum ArtifactOrigin { Upload, Produced }

/// The row as the API serves it. `FileAsset` (models/file_asset.rs:36-51) grows the
/// same columns; `ArtifactRecord` is the joined, UI-shaped projection.
pub struct ArtifactRecord {
    pub id: String,
    pub name: String,                 // file_name of rel_path, or `filename` for uploads
    pub kind: ArtifactKind,
    pub origin: ArtifactOrigin,
    pub mime_type: String,
    pub size_bytes: i64,
    pub task_id: Option<String>,
    pub task_title: Option<String>,   // JOIN task
    pub agent_id: Option<String>,
    pub agent_template_id: Option<String>,
    pub version: i32,
    pub version_count: i32,
    pub pinned: bool,
    pub summary: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub project_root: Option<String>,
    pub rel_path: Option<String>,
    pub path: String,                 // absolute; = storage_path
    pub missing: bool,                // missing_since IS NOT NULL
    pub created_at: String,
    pub updated_at: String,
}

// crates/openalpaca_storage/src/artifacts/mod.rs (new) — the ONE writer.
pub struct ArtifactStore<'a> { db: &'a Database }

pub struct NewArtifact<'a> {
    pub owner_id: &'a str,
    pub scope: ArtifactScope,
    pub task_id: Option<&'a str>,
    pub task_title: Option<&'a str>,
    pub task_created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub agent_id: Option<&'a str>,
    pub agent_template_id: Option<&'a str>,
    pub name: &'a str,                       // model-authored title or filename
    pub kind: ArtifactKind,
    pub mime_type: Option<&'a str>,
    pub bytes: &'a [u8],
    pub note: Option<&'a str>,               // version note
    pub summary: Option<&'a str>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Default)]
pub struct ArtifactQuery {
    pub owner_id: String,
    pub task_id: Option<String>,
    pub kind: Option<ArtifactKind>,
    pub origin: Option<ArtifactOrigin>,
    pub project_root: Option<String>,
    pub pinned: Option<bool>,
    pub name_contains: Option<String>,       // the palette's `Find <filename>`
    pub include_missing: bool,               // default false
    pub limit: usize,                        // default 100, max 500
    pub offset: usize,
}

impl<'a> ArtifactStore<'a> {
    pub fn new(db: &'a Database) -> Self;

    /// Create, or supersede an existing artifact with the same
    /// (scope, task_id, slug). Writes bytes, rotates the previous head into
    /// `.versions/`, inserts the version row, bumps version/version_count.
    /// Returns the record and whether it was a new artifact.
    pub fn put(&self, new: NewArtifact<'_>) -> anyhow::Result<(ArtifactRecord, bool)>;

    pub fn get(&self, id: &str, owner_id: &str) -> anyhow::Result<Option<ArtifactRecord>>;
    pub fn list(&self, q: &ArtifactQuery) -> anyhow::Result<(Vec<ArtifactRecord>, i64)>;

    /// Absolute path of a version's bytes (`None` version = head).
    /// Errors with `ArtifactError::Gone` if the file is absent, and marks
    /// `missing_since` as a side effect.
    pub fn resolve_content(&self, id: &str, version: Option<u32>) -> anyhow::Result<PathBuf>;

    pub fn versions(&self, id: &str) -> anyhow::Result<Vec<ArtifactVersionRow>>;
    pub fn diff(&self, id: &str, from: u32, to: u32) -> anyhow::Result<ArtifactDiff>;
    pub fn set_pinned(&self, id: &str, pinned: bool) -> anyhow::Result<()>;

    /// Re-base rows after a project moved: rewrite storage_path from
    /// (new_root, rel_path) and clear `missing_since` where the file now exists.
    pub fn rebase_project(&self, old_root: &str, new_root: &str) -> anyhow::Result<usize>;

    /// Mark rows whose head file no longer exists. Returns the count.
    pub fn verify(&self, project_root: Option<&str>) -> anyhow::Result<usize>;
}
```

`FileAssetRepository` keeps its current surface (`repository/file_asset/mod.rs:16-164`) — `ArtifactStore` sits beside it and both write the same table. `insert` must gain the new columns, or better, `ArtifactStore::put` owns produced rows and `FileAssetRepository::insert` keeps owning uploads with `origin` defaulted.

### 5.4 Two existing queries that must change

```sql
-- repository/file_asset/mod.rs:95-112 — orphan sweep.
-- BUG IF UNCHANGED: produced artifacts are never linked to a message, so the
-- 24-hour sweep (background.rs:308-357) would delete every artifact and its file.
   WHERE a.id IS NULL
     AND f.origin = 'upload'          -- ← add
     AND f.pinned = 0                 -- ← add
     AND f.created_at < datetime('now', ?1)
```

```sql
-- repository/file_asset/mod.rs:76-85 — quota accounting, read at routes/files.rs:84-97.
-- BUG IF UNCHANGED: agent output in the user's own project directory would count
-- against the 500 MB upload quota and start rejecting uploads.
SELECT COALESCE(SUM(size_bytes), 0) FROM file_assets WHERE origin = 'upload'
```

Both are real defects the moment migration 035 lands. They are the highest-risk part of this design and should be in the same PR.

---

## 6. §5 — Integrity and lifecycle

| Concern | Design |
|---|---|
| **File missing** | `resolve_content` stats before serving; on absence sets `missing_since` and returns `410 Gone` with `{error:{code:"ARTIFACT_GONE", message, path}}`. List rows carry `missing: true` and the UI can strike them out rather than 404-ing a whole view. |
| **Project moved** | `rel_path` + `project_root` make re-basing a one-statement fix; exposed as `POST /v1/artifacts/rebase {old_root,new_root}` (admin-ish, or driven by the project picker when the user re-opens a moved project). |
| **Project deleted** | Rows survive as `missing`. No automatic deletion — a user who deletes a project has not asked to lose the index of what was made. A `?include_missing=` filter (default `false`) keeps the Library clean. |
| **Orphan GC** | Scoped to `origin='upload'` (§5.4). Produced artifacts are **never** garbage-collected: they live in the user's project and are the user's files. If a retention policy is ever wanted, it belongs behind an explicit config key, defaulted off. |
| **Version pruning** | `execution.artifacts.max_versions_per_artifact` (default 20): on write, delete the oldest `.versions/` file and its row beyond the cap. Head is never pruned. |
| **Size accounting** | Two numbers, not one: `upload_bytes` (quota-bearing, `origin='upload'`) and `produced_bytes` (informational, per project). `GET /v1/status` (GAP-14) can report both. |
| **Write cap** | New `execution.artifacts.max_artifact_bytes`, default **10 MB** — matching `file_write`'s existing `MAX_FILE_WRITE_SIZE` (`tools/builtins/file_ops.rs:102-110`), not the 50 MB upload cap: this content comes out of a model's context window. |
| **Upload caps unchanged** | 50 MB per file (`daemon_config/upload.rs:48`) inside a 100 MB body limit (`router.rs:117-119`). Worth noting the two disagree; not this lens's bug to fix, but the mismatch means a 60 MB upload is read fully into memory before being rejected (`routes/files.rs:58-81`). |
| **Concurrent writes** | Two agents writing the same artifact name in one run serialize on `ArtifactStore::put`, which does the version rotate inside a single `with_connection` transaction and uses tmp-file + rename for the bytes. Last writer becomes the head; both versions survive in `.versions/`. |
| **User edits the file by hand** | Explicitly supported and the point of the design. The DB's `sha256`/`size_bytes` go stale; `verify` (or the next `put`) detects the mismatch and records the user edit as a version with `author_agent_id = NULL`. Recommend implementing this in phase 2 — it is the feature that makes "committable artifacts" honest. |

---

## 7. §6 — HTTP surface, mapped to the gaps

All routes sit on the protected router (`router.rs:19-266`) except where `?token=` is noted, which follow the `/v1/chat/stream` precedent (`router.rs:268-274`, `routes/chat.rs:104-113`) and are merged **outside** the auth layer. Envelope: `{"error":{"code","message"}}`, matching the chat/settings family per note 1 of `tasks/gui-api-requirements.md`.

### GAP-04 — list and read

```http
GET /v1/artifacts?task_id=&kind=&origin=&project_root=&pinned=&q=&include_missing=&limit=&offset=
200 {
  "artifacts": [ Artifact, … ],
  "total": 1234
}
```

`Artifact` — a superset of `apps/openalpaca-gui/src/lib/api/unbacked.ts:39-56`, so the existing client type compiles unchanged:

```json
{
  "id": "8f3c…",
  "name": "connector-audit-findings.md",
  "kind": "markdown",
  "mime_type": "text/markdown",
  "size_bytes": 4120,
  "task_id": "3f2a1b7c-…",
  "task_title": "Audit the connector layer",
  "agent_id": "review_agent::a1b2c3d4",
  "agent_template_id": "review_agent",
  "version": 2,
  "version_count": 2,
  "summary": "+41 −6",
  "metadata": { "added": 41, "removed": 6 },
  "created_at": "2026-09-01T10:04:11Z",
  "updated_at": "2026-09-01T10:22:03Z",

  "origin": "produced",
  "pinned": false,
  "missing": false,
  "path": "/Users/x/dev/proj/.openalpaca/artifacts/2026-09-01-audit-3f2a1b7c/01-connector-audit-findings.md",
  "project_root": "/Users/x/dev/proj",
  "rel_path": "2026-09-01-audit-3f2a1b7c/01-connector-audit-findings.md"
}
```

`path` is the payoff of the directive: it is what "Reveal in Finder", "Copy path" and a `git`-aware user need, and it is impossible to serve from a content-addressed sha path.

```http
GET /v1/artifacts/{id}            200 Artifact | 404
```

### GAP-11 — content with inline auth

```http
GET /v1/artifacts/{id}/content?token=<bearer>[&version=N]
200 bytes, Content-Type: <mime>, Content-Disposition: inline; filename="…"
410 { "error": { "code": "ARTIFACT_GONE", "message": "…", "path": "…" } }

GET /v1/files/{id}/content?token=<bearer>        ← same change on the existing route
```

Implementation: move the two content routes out of `protected_routes` into a third merged router alongside `chat_sse` (`router.rs:268-280`) and do the inline `params.get("token") != state.token → 401` check, copying `routes/chat.rs:109-113` verbatim. Keep the `Authorization` header path working too (accept either) so existing callers — `apps/openalpaca-gui/src/lib/api/files.ts:23-30` — are unaffected. Then `apps/openalpaca-gui/src-tauri/tauri.conf.json` needs `img-src 'self' data: blob:` per note 3 of the requirements doc.

### GAP-05 — versions and diff

```http
GET /v1/artifacts/{id}/versions
200 { "versions": [
  { "version": 2, "note": "Added MCP resource stub finding after steer",
    "author_agent_id": "review_agent", "created_at": "2026-09-01T10:22:03Z",
    "size_bytes": 4120, "added_lines": 9, "removed_lines": 2 },
  { "version": 1, "note": "Initial two findings and suggested fix",
    "author_agent_id": "review_agent", "created_at": "2026-09-01T10:04:11Z",
    "size_bytes": 3760, "added_lines": null, "removed_lines": null }
] }
```

Matches `ArtifactVersion` (`unbacked.ts:62-70`) field-for-field. `note` is `string` (non-null) in the client, so the route coalesces `NULL → ""`.

```http
GET /v1/artifacts/{id}/versions/{n}/content?token=<bearer>     → bytes
GET /v1/artifacts/{id}/diff?from=1&to=2
200 { "from": 1, "to": 2, "added_lines": 9, "removed_lines": 2,
      "format": "unified", "patch": "@@ -3,4 +3,6 @@\n…" }
```

Matches `ArtifactDiff` (`unbacked.ts:72-77`). Diffs are text-only: for `kind` in `{image, binary}` return `409 { code: "NOT_DIFFABLE" }` and let the History tab stand alone.

**New dependency:** there is no diff crate in the workspace. Recommend `similar` (MIT, pure Rust, no build script) in `openalpaca_storage`. `added_lines`/`removed_lines` are computed **at write time** and stored, so the History tab needs no diff engine at read time; only the Diff tab calls `similar`.

### GAP-12 — pins

```http
PUT /v1/artifacts/{id}/pin  { "pinned": true }
200 { "id": "8f3c…", "pinned": true }
```

Additive. The client keeps `localStorage` (`views/library/LibraryDetail.tsx:43-44`) until it chooses to adopt the server field; `Artifact.pinned` is extra data a TS structural type happily ignores.

### New event

```rust
// crates/openalpaca_api/src/events/mod.rs — alongside TaskStatus (mod.rs:41-56)
ArtifactWritten {
    artifact_id: String,
    task_id: Option<String>,
    agent_id: Option<String>,
    name: String,
    kind: String,
    version: i32,
    path: String,
    ts: DateTime<Utc>,
    instance_id: String,
},
```

Carries `ts` + `instance_id` from the start (unlike the six plugin variants of GAP-22). Feeds the chat transcript's inline artifact card, the `artifact` tag in the per-run event log (GAP-10, Lens C), and the design's `connector-audit-findings.md v2 written` line.

### What each blocked surface gets back

| Surface | Unblocked by |
|---|---|
| Library list + `Library · N files` header | `GET /v1/artifacts` → `{artifacts,total}`; `LibraryList.tsx:54-57` already renders `total` |
| Kind filter chips | Already client-side (`LibraryList.tsx:41-45`); the server filter is optional |
| `run` / `runName` / `agent` attribution | `task_id` + `task_title` (JOIN `task`) + `agent_id`/`agent_template_id` columns |
| Preview tab | `GET /v1/artifacts/{id}/content?token=` — no more blob-URL dance |
| History tab | `GET /v1/artifacts/{id}/versions` |
| Diff tab, `+41 −6` | `GET /v1/artifacts/{id}/diff` + stored `added_lines`/`removed_lines` |
| `v2 of 2` stamp | `version` / `version_count` |
| `★ Pin` surviving a reinstall | `pinned` + `PUT …/pin` |
| `Export` | Unchanged — client fetch + Tauri fs |
| `Reveal` | `Artifact.path` makes a genuine Tauri-side reveal possible for the first time; `POST /v1/files/{id}/open`'s `$TMPDIR` staging (`files_types.rs:92-118`) can be skipped for produced artifacts, which now have real extensions |
| `Files · N` per run in Work | `GET /v1/artifacts?task_id=…` |
| Chat inline artifact card | `ServerEvent::ArtifactWritten` live; `GET /v1/artifacts?task_id=` on reload |
| GAP-23's artifact half | `artifact_ids` on `ConversationMessage` becomes resolvable — the ids now exist and are stable |
| **Bonus, not a GAP:** artifact delivery over Telegram/Discord/iMessage | `notification/artifacts.rs:23-52` already resolves `file_asset_id` and gives up when it is `None`. Populating it (§4.3) makes an existing, dormant feature work with no new code. |

---

## 8. §7 — Migration path and compatibility

**Not a breaking change for existing installs.** Concretely:

1. **Existing `file_assets` rows** get `origin='upload'` from the column default, `kind=NULL`, `project_root=NULL`, `rel_path=NULL`, `version=1`, `version_count=1`, `pinned=0`. Their `storage_path` is untouched, so `GET /v1/files/{id}/content` keeps streaming the same bytes from the same place.
2. **No files move.** `assets/<ab>/<cd>/<sha256>` stays exactly where it is. There is no data migration, no copy, no rollback risk on the bytes.
3. **`/v1/files/*` routes are unchanged** in shape and behaviour; `/v1/files/{id}/content` only *gains* the `?token=` option.
4. **`/v1/artifacts` lists uploads too**, with `kind` inferred from `mime_type` at read time for legacy rows (a small `mime → ArtifactKind` function; no backfill needed, and a backfill `UPDATE` can be added later if the read-time inference proves hot).
5. **Rollback**: dropping to schema 34 loses the new columns and `artifact_versions`. Produced artifact *files* survive on disk in `.openalpaca/` — they are plain files with readable names, which is itself an argument for this layout: the store degrades to "a folder of documents", not to nothing.

Suggested sequencing (each step independently shippable):

| Step | Contents | Risk |
|---|---|---|
| A1 | Migration 035 + the two query fixes in §5.4 + `ArtifactKind`/`ArtifactOrigin` models | Low — additive, but §5.4 is mandatory in this same PR |
| A2 | `paths.rs` additions + `slugify`/`confine_to_root` with unit tests (traversal, unicode, case-folding, reserved names, length) | Low, pure functions |
| A3 | `ArtifactStore` (`put`/`get`/`list`/`resolve_content`/`versions`) | Medium — the write protocol and the version rotate |
| A4 | `artifact_write` tool + `workspace_write` spill bridge (§4.3) | Medium — touches the agent loop's state path |
| A5 | Routes: `/v1/artifacts*`, `?token=` on both content routes, `ServerEvent::ArtifactWritten`, CSP change | Low once A3 lands |
| A6 | Diff (`similar` dep) + `/diff` route | Low |
| A7 | Phase 2: uploads into `<store>/uploads/`, user-edit detection, `rebase_project` | Deferred |

---

## 9. Uncertainties — stated, not guessed

1. **Does the user want chat uploads in `.openalpaca/` too?** I recommend no for phase 1 (§4.2) and have made phase 2 cheap. This is the one place my reading of "artifacts of ALL kinds" may be narrower than intended.
2. **`extracted_text` in the DB** (§5.2) is a deliberate exception to "addresses only". Reversible in one column.
3. **`~/.openalpaca/` is still a hidden dot-dir**, so the home fallback is only marginally more findable than `~/Library/Application Support/`. If the user wants genuinely findable, `~/OpenAlpaca/` (visible) is the alternative — but it contradicts the directive's wording, so I kept the dot-dir and added a config override (`home_store_dir()` honours `OPENALPACA_HOME_STORE`).
4. **Where the project signal comes from** (§1.1) is unresolved and is *not* a storage question. Without a client that sends `x-workspace-path` or a project picker, everything lands in the home store. Lens B should own this.
5. **`similar` is a new workspace dependency.** A hand-rolled ~120-line LCS unified diff avoids it. I recommend the crate; flagging the tradeoff because the workspace currently has no diff dependency at all.
6. **Two daemons on one project** (two machines over a shared volume) is not designed for. The unique index on `(project_root, rel_path)` is per-DB, so two daemons would each hold half the index while sharing one directory. Out of scope; worth a line in the docs.
7. **`agent_id` shape.** I assume the runtime instance id (`"code_agent::a1b2c3d4"`, per `ServerEvent::AgentStatus.agent_instance_id`, `crates/openalpaca_api/src/events/mod.rs:63-66`) with `agent_template_id` alongside. If Lens C's timeline work settles on a different instance identity, this column should follow it rather than the reverse.
8. **Phase 2 touches two writers, not one.** Connectors ingest attachments through their own content-addressed path (`crates/openalpaca_connectors/src/common/mod.rs:213-262`), duplicating the upload route's sha/dedup/write logic. If uploads are ever re-homed into `.openalpaca/uploads/`, that duplication should be collapsed into `ArtifactStore` first — otherwise the two paths will drift. I did **not** audit whether any plugin uploads files.
