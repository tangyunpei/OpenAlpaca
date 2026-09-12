# Lens C — Config, Catalog & Small-Surface Gaps

**Scope:** GAP-01, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22 · plus cross-cutting envelope/list-shape consistency, the GAP-11 `?token=` coordination, and the Tauri CSP.
**Status:** research + plan only. No production code written.
**Branch:** `feat/ui-rework` · verified against working tree at commit `b191134`.

> Note: `tasks/research/C-surface.md` is untracked — `.gitignore:92` blanket-ignores `*.md` and `tasks/` is not whitelisted. Whitelist it if this should survive as a repo doc.

---

## 0. Executive summary

| Gap | Verdict | Effort | Blocked on a decision? |
|---|---|---|---|
| **GAP-01** approval_scope | Genuinely two lines. Do first. | **one-liner** (~15 min w/ test) | no |
| **GAP-22** plugin event ts/instance_id | Cross-crate enum edit + `PluginManager` needs an instance id | **hours** (~1–2 h) | no |
| **GAP-16** `/v1/me` | `AppState` already holds both fields | **one-liner** (~20 min) | `sources[]` semantics |
| **GAP-14** `/v1/status` | uptime/schema/paths cheap; **`log_path` does not exist at all** | **hours** for 5 of 6 fields; **day+** for `log_path` | yes — do we add file logging? |
| **GAP-21** conversation rename/delete | 2 repo methods + 2 handlers | **hours** (~2 h) | delete cascade semantics |
| **GAP-15** provider enable/disable | `ProviderConfig.enabled` already exists and is already honoured | **hours** (~3 h) | hot-apply vs restart-only |
| **GAP-18** `/v1/tools` + `/v1/skills` | Registries have the data; `AppState` does not hold them | **hours** (~4 h) | no |
| **GAP-20** template run counts + enabled | Counts are one SQL join; **enabled has no home in the data model** | **hours** for counts; **day+** for the toggle | yes — where does `enabled` live? |
| **GAP-17** connector detail | `source`/`registered` easy; `calls_7d` has no real source | **hours** (~4 h) | yes — what does "call" mean? |
| **GAP-13** per-chat model | Plumbing is 5 layers deep; `MessageHandler::handle` already at 8 args | **day+** | yes — request-scoped vs lane-persisted |
| **GAP-19** plugin install | `try_load_plugin` is public but `plugin_dir` has no getter; unpacking untrusted archives is the real cost | **day+** | yes — what is `source`? |

**Recommended order:** GAP-01 → GAP-16 → GAP-22 → GAP-14 (minus `log_path`) → GAP-21 → GAP-18 → GAP-15 → GAP-20 (counts only) → GAP-17 → GAP-13 → GAP-19.

Two of these have hidden depth that the `unavailable.ts` fix-size estimates understate: **GAP-14 is rated `S` but the daemon writes no log file** (`main.rs:57` — `tracing_subscriber::fmt()` to stdout; the GUI sidecar then sends stdout to `Stdio::null()` at `apps/openalpaca-gui/src-tauri/src/lib.rs:128,152`). And **GAP-13 is rated `M` but crosses `ChatService → GatewayRequest → MessageHandler → Orchestrator → LoopConfig`**, five hops, the middle of which is a trait with `#[allow(clippy::too_many_arguments)]` already on it.

---

## 1. GAP-01 — `approval_scope` on the confirmation route

### Current code

- `apps/openalpacad/src/routes/chat_types.rs:90-93` — `ConfirmationBody { approved: bool }`. That is the whole struct.
- `apps/openalpacad/src/routes/chat.rs:441-466` — `confirm_tool` constructs `ConfirmationResponse { approved: body.approved, approval_scope: None }` (**chat.rs:462** is the literal `None`).
- `crates/openalpaca_core/src/security/confirmation.rs:26-33` — `ConfirmationResponse.approval_scope: Option<ApprovalScope>` already exists, `#[serde(default)]`, `Serialize + Deserialize`.
- `crates/openalpaca_core/src/security/confirmation.rs:87-97` — `ApprovalScope { TheseArgs, EntireTool }` with `#[serde(rename_all = "snake_case")]` — so the wire values are exactly `"these_args"` / `"entire_tool"`.
- `crates/openalpaca_core/src/security/sandbox/mod.rs:248-256` — the sandbox already reads the scope: `resp.approval_scope.unwrap_or(ApprovalScope::TheseArgs)` then `approval_cache.record(...)`. **The enforcement path is complete.** Only the HTTP hop drops the field.

### Client contract (already shipped)

- `apps/openalpaca-gui/src/lib/api/types.ts:176` — `export type ApprovalScope = "these_args" | "entire_tool"`.
- `apps/openalpaca-gui/src/lib/api/chat.ts:64-68` — sends `approval_scope` only when defined.
- `apps/openalpaca-gui/src/views/chat/useChatSession.ts:388-390` — `alwaysAllow()` → `answer("approved", "entire_tool")`.
- `apps/openalpaca-gui/src/views/chat/ChatView.test.tsx:288-298` — already asserts `approval_scope: "entire_tool"` **on the wire**. This test is currently green only because the daemon ignores the field; it becomes a real contract test the moment the route accepts it.

### Diff-level plan

`routes/chat_types.rs`:

```rust
#[derive(Deserialize)]
pub struct ConfirmationBody {
    pub approved: bool,
    /// Granularity of an approval. Absent ⇒ `TheseArgs` at enforcement time
    /// (`security/sandbox/mod.rs`), which is the safe default.
    #[serde(default)]
    pub approval_scope: Option<openalpaca_core::security::confirmation::ApprovalScope>,
}
```

`routes/chat.rs:462`:

```rust
-            approval_scope: None,
+            approval_scope: body.approval_scope,
```

No route registration change, no new type, no migration. `ApprovalScope` is `pub` from `openalpaca_core::security::confirmation` (re-exported via `security/confirmation.rs`; verify the `pub use` chain compiles — if not, alias it locally in `chat_types.rs`).

**Test:** a unit test on `ConfirmationBody` deserialization for the three cases (`{approved}`, `{approved, approval_scope:"entire_tool"}`, `{approved, approval_scope:"these_args"}`), plus the existing GUI test flips from "documents the gap" to "asserts the behaviour".

**Client follow-up:** delete the GAP-01 note in `useChatSession.ts:15`, the comment at `lib/api/chat.ts:44-48`, `Composer.tsx:9`, and the `GAP-01` registry entry.

**Effort: one-liner.** Highest value-per-line in the whole list — it is the difference between "Always allow" working and silently lying.

---

## 2. GAP-22 — `ts` / `instance_id` on the six plugin events

### Current code

- `crates/openalpaca_api/src/events/mod.rs:250-280` — six variants (`PluginLoaded`, `PluginUnloaded`, `PluginCrashed`, `PluginDisabled`, `PluginPendingApproval`, `PluginNeedsConfig`). Every other `ServerEvent` variant in that file carries `ts: DateTime<Utc>` + `instance_id: String` (e.g. `FollowupQueued` at `:240-249`). These six are the only holdouts.
- Emit sites, all inside `crates/openalpaca_plugins/src/manager.rs`: `:294` (PendingApproval), `:331` (NeedsConfig), `:504` (Loaded), `:571` (Unloaded), `:611` and `:684` (Disabled). `PluginCrashed` has **no emit site** in the manager — grep finds it only in `events/persistence.rs:372`; worth confirming during implementation whether the crash path is dead.
- `crates/openalpaca_plugins/src/manager.rs:199-203` — `fn emit(&self, event: ServerEvent)` just forwards to `self.event_sink`. `PluginManager::new` (`:173-192`) takes `plugin_dir`, `tool_registry`, `skill_catalog`, `agent_registry` — **no instance id**.
- `apps/openalpacad/src/main.rs:341-343` — the daemon wires the sink: `.with_event_sink(Arc::new(move |event| eb_for_plugins.broadcast(event)))`.
- `apps/openalpacad/src/events/persistence.rs:356-413` — the persistence match already destructures every plugin variant with `..`, so **adding fields there is source-compatible**.

### Diff-level plan

1. `openalpaca_api/src/events/mod.rs` — add `ts: DateTime<Utc>, instance_id: String` to all six variants, matching the field order used by the other 25.
2. `openalpaca_plugins/src/manager.rs` — add `instance_id: String` to `PluginManager` and a builder mirroring `with_event_sink`:
   ```rust
   pub fn with_instance_id(mut self, instance_id: impl Into<String>) -> Self {
       self.instance_id = instance_id.into();
       self
   }
   ```
   Default it to `String::new()` in `new()` so no test breaks, then stamp `ts: Utc::now(), instance_id: self.instance_id.clone()` at each of the six (five) emit sites.
   *Alternative considered and rejected:* stamping inside the daemon's sink closure. It requires re-matching all six variants in `main.rs` and leaves the crate emitting structurally-invalid events for any other embedder.
3. `apps/openalpacad/src/main.rs:334-343` — chain `.with_instance_id(instance_id.clone())` onto the builder.
4. `crates/openalpaca_plugins/src/manager.rs:1172,1184,1218,1241` — **test** match arms destructure without `..`; add it (`ServerEvent::PluginUnloaded { plugin_id, .. }`). Purely mechanical.

### Client contract

`apps/openalpaca-gui/src/lib/events.ts:61-67` already carries the six variants with a `// GAP-22` comment and no `ts`/`instance_id`. After the change, add both to each line and delete the comment. Everything downstream (ordering, dedupe) then works uniformly.

**Effort: hours (~1–2 h).** Small but cross-crate; touches `openalpaca_api` which everything depends on, so budget a full-workspace rebuild.

---

## 3. GAP-16 — `GET /v1/me`

### Current code

- `apps/openalpacad/src/state.rs:32-33` — `pub local_user_id: String` and `pub default_lane_key: String` are **already on `AppState`**.
- `apps/openalpacad/src/main.rs:199` — `let default_lane_key = format!("{local_user_id}:gui");`.
- Nothing exposes either over HTTP. `apps/openalpaca-gui/src/lib/api/unbacked.ts:303-317` documents the workaround: the lane key is learned from the `lane_key` echoed by `GET /v1/chat/history` / `POST /v1/chat`, which is why `useChatHistory` omits the param on first load (`hooks/useChat.ts:52`).

### Client contract

`unbacked.ts:305-309`:

```ts
export interface Identity {
  user_id: string;
  default_lane_key: string;
  sources: string[];
}
```

**`sources[]` is the only ambiguous field.** Best reading: the connector/source names this user has lanes on — i.e. the distinct `source` column of `conversations` for lanes owned by `local_user_id`. That is derivable today via `ConversationRepository::list_conversations_for_owner` (`repository/conversation/mod.rs:203`) or a one-line `SELECT DISTINCT source`. *Flagging as uncertain:* it could equally mean "registered connectors", which is what `GET /v1/connectors` already serves. Recommend the conversations reading and say so in a doc comment.

### Diff-level plan

New `apps/openalpacad/src/routes/me.rs`:

```rust
#[derive(Serialize)]
pub struct IdentityResponse {
    pub user_id: String,
    pub default_lane_key: String,
    /// Distinct `conversations.source` values for lanes this user owns.
    pub sources: Vec<String>,
}

pub async fn get_me_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let repo = openalpaca_storage::ConversationRepository::new(&state.db);
    let sources = repo
        .list_conversations_for_owner(&state.local_user_id, None, 500, 0)
        .map(|cs| {
            let mut s: Vec<String> = cs.into_iter().map(|c| c.source).collect();
            s.sort_unstable();
            s.dedup();
            s
        })
        .unwrap_or_default();
    Json(IdentityResponse {
        user_id: state.local_user_id.clone(),
        default_lane_key: state.default_lane_key.clone(),
        sources,
    })
}
```

`routes/mod.rs`: `pub mod me;` + `pub use me::get_me_handler;`
`router.rs` (inside `protected_routes`): `.route("/v1/me", get(crate::routes::get_me_handler))`

**Client follow-up:** replace `getIdentity()` in `unbacked.ts:314-317` with a real `apiFetch`, add a `useIdentity()` hook, and let `useChatHistory` pass `lane_key` on first load instead of learning it.

**Effort: one-liner** (a `sources` shortcut of `vec![]` would make it literally three lines; the distinct-source query adds ~10).

---

## 4. GAP-14 — `GET /v1/status`

### Current code

- `apps/openalpacad/src/router.rs:286-295` — `health_handler` returns exactly `{status, version, pid, instance_id}`. Public (no auth), registered at `:16`.
- `crates/openalpaca_storage/src/database/mod.rs:80` — **`Database::schema_version() -> Result<i32>` already exists** (`SELECT COALESCE(MAX(version),0) FROM schema_version`). `database/tests.rs:11` asserts 34.
- `crates/openalpaca_storage/src/paths.rs:18` `app_dir()`, `:64` `database_path()` — both exist.
- **No `started_at` anywhere.** `grep started_at|start_time apps/openalpacad/src/main.rs` → nothing. `AppState` (`state.rs:17-40`) has no time field.
- **No log file.** `apps/openalpacad/src/main.rs:57-61` is `tracing_subscriber::fmt().with_env_filter(...).init()` — stdout only, no appender, no `tracing-appender` anywhere in the workspace (`grep tracing-appender --include=Cargo.toml` → empty). The GUI sidecar then discards it: `apps/openalpaca-gui/src-tauri/src/lib.rs:128` and `:152` both set `.stdout(std::process::Stdio::null())`.

### Client contract

`unbacked.ts:268-275`:

```ts
export interface DaemonStatusDetail {
  started_at: string; uptime_secs: number; schema_version: number;
  data_dir: string; log_path: string; db_path: string;
}
```

Consumed by `views/settings/ConnectionSection.tsx:29,42,69-73` — `uptime —`, `Schema: —`, and a disabled `Copy log path` button whose `title` is the gap note.

### Diff-level plan — Phase A (do now)

1. `apps/openalpacad/src/state.rs` — add `pub started_at: chrono::DateTime<chrono::Utc>`.
2. `apps/openalpacad/src/main.rs:591` — `started_at: chrono::Utc::now()` in the `AppState { .. }` literal. Capture it at the top of `run()` rather than at state construction, so it means "process start" not "HTTP-ready".
3. New `apps/openalpacad/src/routes/status.rs`:

```rust
#[derive(Serialize)]
pub struct DaemonStatusResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub pid: u32,
    pub instance_id: String,
    pub started_at: DateTime<Utc>,
    pub uptime_secs: i64,
    pub schema_version: i32,
    pub data_dir: String,
    pub db_path: String,
    /// `None` until the daemon writes a log file (see Phase B).
    pub log_path: Option<String>,
}
```

Registered **inside `protected_routes`** (`router.rs`), not `public` — it leaks filesystem paths. `/v1/health` stays as-is; the GUI's liveness dot depends on it being unauthenticated.

`uptime_secs = (Utc::now() - state.started_at).num_seconds()`.
`schema_version` from `state.db.schema_version().unwrap_or(-1)` — do **not** hardcode `MIGRATIONS.len()`; `migrations/mod.rs` has no `current_version()` helper and the DB is the truth.

### Phase B — `log_path` (a separate decision)

Making `log_path` non-null requires:
- adding `tracing-appender` to the workspace + `openalpacad`,
- a `RollingFileAppender` at `app_dir()/logs/openalpacad.log` layered beside the stdout writer (`fmt().with_writer(stdout.and(file))`), keeping the `_guard` alive for the process lifetime — a classic footgun if dropped,
- rotation + retention policy (the repo already has `background::spawn_telemetry_cleanup` as a precedent for retention jobs),
- optionally, the GUI sidecar stops discarding stdout.

**Recommendation: ship Phase A with `log_path: Option<String>` = `None`, and treat Phase B as its own task.** The client's `DaemonStatusDetail.log_path` is `string` (non-nullable) — either widen it to `string | null` or keep the button disabled with the honest reason "the daemon does not write a log file". The second is more truthful and matches the project's stated no-fabrication rule.

**Effort: hours** for Phase A (uptime, schema, both paths — the four fields that actually light up the design). **day+** for Phase B.

---

## 5. GAP-21 — conversation rename / delete

### Current code

- `apps/openalpacad/src/router.rs:144-151` — the only two conversation routes, both `GET`.
- `apps/openalpacad/src/routes/chat.rs:292-315` (`list_conversations_handler`) and `:318-360` (`get_conversation_messages_handler`).
- `chat.rs:341-343` — the ownership check pattern to copy: `if !is_lane_owned_by(&conv.lane_key, &state.local_user_id) { 403 }` (`chat_types.rs:97-99`).
- `crates/openalpaca_storage/src/repository/conversation/mod.rs` — has `get_conversation(:188)`, `list_conversations_for_owner(:203)`, `delete_by_lane(:121)` (messages only). **No `update_title`, no `delete_conversation`.**
- `crates/openalpaca_storage/src/migrations/011_unified_conversations.sql:3-11` — `conversations(id, lane_key UNIQUE, source, title, message_count, last_message_at, created_at, updated_at)`. `title` is plain `TEXT DEFAULT ''`, so rename is a bare `UPDATE`.
- `apps/openalpacad/src/router.rs:6` imports `routing::{delete, get, post, put}` — **`patch` is not imported.** One-word addition.

### Diff-level plan

Storage — `repository/conversation/mod.rs`:

```rust
/// Rename a conversation. Returns false when no row matched.
pub fn update_title(&self, id: &str, title: &str) -> Result<bool>;

/// Delete a conversation row *and* its messages. Returns (rows, messages).
pub fn delete_conversation(&self, id: &str) -> Result<(u64, u64)>;
```

`delete_conversation` should run both statements inside one transaction (`DELETE FROM conversation_messages WHERE lane_key = ?` then `DELETE FROM conversations WHERE id = ?`) — there is no FK cascade between them (migration 011 declares none), so an untransacted delete can orphan messages.

**Open question to settle before implementing:** deleting the conversation row does **not** clear `lane_followups` (migration 033) or the `LaneManager` in-memory lane, and the next message on that lane will recreate the row via `get_or_create_conversation` (`:146`). Decide whether DELETE means "forget the transcript" (current proposal) or "tear down the lane". Recommend the former, documented on the handler, and reject the delete with 409 when the lane has active workflows.

Routes — `routes/chat.rs`:

```rust
#[derive(Deserialize)] pub struct RenameConversationBody { pub title: String }
#[derive(Serialize)]  pub struct ConversationDeleteResponse { pub deleted: bool, pub messages_deleted: u64 }

pub async fn rename_conversation_handler(State, Path(id), Json(RenameConversationBody))
pub async fn delete_conversation_handler(State, Path(id))
```

Both: `get_conversation` → 404 if absent → `is_lane_owned_by` → 403 → act. Reject empty/whitespace titles with 400 `INVALID_TITLE`. Use the existing `error_response` helper (`chat_types.rs:101`) so these join the coded-envelope family.

`router.rs`:
```rust
.route("/v1/conversations/{id}",
    axum::routing::patch(crate::routes::rename_conversation_handler)
        .delete(crate::routes::delete_conversation_handler))
```

**Client follow-up:** `hooks/useConversations.ts` grows `useRenameConversation` / `useDeleteConversation` mutations; `views/settings/ConversationsSection.tsx:52` drops `CONVERSATION_WRITE_NOTE`.

**Effort: hours (~2 h).**

---

## 6. GAP-15 — provider enable / disable

### Current code — better than the gap registry suggests

- `crates/openalpaca_llm/src/config/llm_config/router_config.rs:65-66` — **`ProviderConfig.enabled: Option<bool>` already exists** in `llm.toml`.
- `crates/openalpaca_llm/src/config/llm_config/router_builder.rs:98-102` — already honoured at build time: `if provider_config.enabled == Some(false) { continue; }`.
- `crates/openalpaca_llm/src/config/settings_service.rs:169-178` — already **read** back out: `ProviderInfo.enabled`, defaulting to `true` when configured and `false` when not. `GET /v1/settings/llm` therefore already tells the client whether a provider is on.
- Missing: **only the write route.** `settings_service.rs` has `upsert_key`, `remove_key`, `reorder_keys`, `set_key_priority`, `update_orchestrator_config` — no `set_provider_enabled`.
- `settings_service.rs:588-623` — `persist_and_reload()` is the pattern: config write-lock → read → mutate → `write_config` → rebuild `KeyPool` → `router.reload_keys()` → fall back to `register_provider_from_config()`.
- `crates/openalpaca_llm/src/routing/router/mod.rs:220-224` — **`deregister_provider(&ProviderType) -> Vec<String>` exists** and also strips that provider's models from the `ModelRegistry`.

### The one real subtlety

`persist_and_reload` can *add* a provider but has no path that *removes* one. So a naive `enabled = false` write would persist correctly and take effect **only on restart** (the same honest-but-weak semantics `update_orchestrator_config` documents at `settings_service.rs:536-537`). Because `deregister_provider` exists, a hot disable is achievable — but note it also removes models from the registry, so a subsequent enable must re-register **and** refresh models (`settings_service.rs:498 refresh_models()`).

### Diff-level plan

`crates/openalpaca_llm/src/config/settings_service.rs`:

```rust
/// Enable or disable a provider. Persists `[providers.<name>] enabled`
/// and applies it to the live router.
pub async fn set_provider_enabled(&self, provider: &str, enabled: bool) -> Result<(), String> {
    let provider_type = parse_provider_type(provider)
        .ok_or_else(|| format!("Unknown provider: {provider}"))?;
    // write llm.toml under the config write lock
    // then: if enabled { register_provider_from_config(..); self.refresh_models().await }
    //       else       { self.router.deregister_provider(&provider_type); }
}
```

Reuse the write-lock/read/mutate/write half of `persist_and_reload` — extract it into a `persist_only(mutate)` helper rather than duplicating the lock handling.

`apps/openalpacad/src/routes/settings.rs`:

```rust
#[derive(Deserialize)] pub struct SetProviderEnabledRequest { pub enabled: bool }

/// PUT /v1/settings/llm/providers/{provider}/enabled
pub async fn set_provider_enabled(State, Path(provider), Json(body)) -> impl IntoResponse
```

Returns `{"status":"ok","provider":..,"enabled":..}` and publishes `SystemEvent::KeyStatusChanged { status: "provider_enabled"|"provider_disabled", .. }` — the pattern at `settings.rs:71-76` — so the GUI's `ModelsSection` live-updates. Errors via `settings_error(...)` (`settings_types.rs:6`).

`router.rs`:
```rust
.route("/v1/settings/llm/providers/{provider}/enabled",
       put(crate::routes::set_provider_enabled))
```

Path matches `unavailable.ts:381` (`PUT /v1/settings/llm/providers/{provider}/enabled`) exactly.

**Guardrail:** disabling the provider that serves the current `default_model` will make every subsequent turn fall through to the fallback chain / CLI backends. Either warn in the response body or reject with 409 when `router.default_model()` resolves to the provider being disabled. Recommend the 409 — a silent capability loss is worse than a refused toggle.

**Client follow-up:** `views/settings/ModelsSection.tsx:32` drops `PROVIDER_TOGGLE_NOTE`; `primitives.tsx` `Toggle` becomes live. Separately, `components/chat/ModelPicker.tsx:5-8` can finally render the `off` pill, because `ProviderInfo.enabled` is already on the wire.

**Effort: hours (~3 h)**, mostly in getting the hot enable/disable + model-registry round-trip right.

---

## 7. GAP-18 — `GET /v1/tools` and `GET /v1/skills`

### Current code

- `apps/openalpacad/src/routes/skills.rs` is 21 lines and serves only `/v1/skills/health` (`SkillExecutionRepository::all_skill_health()`), keyed by `skill_id` with no names.
- The data exists in both registries:
  - `crates/openalpaca_core/src/tools/registry/mod.rs:427` — `iter_registered_tools() -> impl Iterator<Item = (String, RegisteredTool)>`, snapshot-style, no lock held across await.
  - `registry/mod.rs:120-145` — `RegisteredTool { definition, backend, provides_capabilities, exempt_from_timeout, annotations, version, author, created_at }`. **`author` is already `"built-in" | "mcp:<server>" | "plugin:<id>"`** — exactly the `source`/`provider` split the client wants.
  - `registry/mod.rs:141-146` — `ToolBackend { BuiltIn, Http, Command, Plugin, Mcp }` — the structural origin marker.
  - `registry/mod.rs:72-79` — `permission_tier(annotations)`: `destructive_hint == Some(true)` → `Admin`.
  - `crates/openalpaca_core/src/security/sandbox/mod.rs:398-417` — the confirmation set is derived from `destructive_hint == true` when `policy.require_confirmation_for` is empty. So `requires_confirmation` for a *default* surface = `annotations.destructive_hint == Some(true)`.
  - `crates/openalpaca_core/src/daemon_config/execution.rs:83` — `SkillDefaults.global_tool_deny: Vec<String>` gives `denied`.
  - `crates/openalpaca_storage/src/migrations/030_skill_tool_execution_log.sql:29-40` — `tool_execution_log(tool_name, timestamp, ...)` with `idx_tel_tool_ts` gives `invocations_today` in one grouped query.
  - `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs:636` — `entries_snapshot() -> Vec<(String, SkillEntry)>`; `:46-57` `SkillEntry { frontmatter, skill_md_path, skill_dir, scope, source }`; `:25-33` `SkillSource { FileBased, Plugin { plugin_id, .. } }`.
  - `crates/openalpaca_core/src/middleware/skill/types.rs:303-305` — `SkillFrontmatter` is `Serialize` (name, description, version, `requires_capabilities`, `depends_on`, legacy `command`).

### The one structural blocker

**Neither registry is reachable from `AppState`.** `apps/openalpacad/src/state.rs:17-40` has no `tool_registry` and no `skill_catalog`; both live only inside the `Orchestrator` (`main.rs:373,376`) and the `PluginManager` (`main.rs:337-338`). `svcs.tool_registry` is **moved** into `Orchestrator::new` at `main.rs:373` (note `svcs.skill_catalog` is `.clone()`d at `:376` and `:413`, but the registry is not).

Fix: clone the `Arc` before the move and add two fields:

```rust
// state.rs
pub tool_registry: Arc<openalpaca_core::tools::ToolRegistry>,
pub skill_catalog: Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
```
(exact path: `SkillCatalog` is at `crates/openalpaca_core/src/orchestrator/skill/catalog/mod.rs` and re-exported as `orchestrator::skill_catalog::SkillCatalog` — see `orchestrator/mod.rs:165`.)

In `main.rs`, insert `let tool_registry_for_state = svcs.tool_registry.clone();` before line 373 and pass both into the `AppState { .. }` literal at `:591`.

### Client contract

`unbacked.ts:288-296`:

```ts
export interface ToolCatalogEntry {
  name: string; description: string;
  source: "builtin" | "mcp" | "plugin";
  provider: string | null;
  requires_confirmation: boolean;
  denied: boolean;
  invocations_today: number;
}
```

Mapping:
| field | source |
|---|---|
| `name` | `RegisteredTool.definition.name` |
| `description` | `.definition.description` |
| `source` | match on `ToolBackend`: `BuiltIn\|Http\|Command → "builtin"`, `Mcp{..} → "mcp"`, `Plugin(_) → "plugin"` |
| `provider` | `Mcp { server_name }` → that; `Plugin` → the id parsed off `author` (`"plugin:<id>"`); else `null` |
| `requires_confirmation` | `annotations.and_then(\|a\| a.destructive_hint) == Some(true)` |
| `denied` | `daemon_config.load().execution.skill_defaults.global_tool_deny.contains(&name)` |
| `invocations_today` | one `SELECT tool_name, COUNT(*) FROM tool_execution_log WHERE timestamp >= date('now') GROUP BY tool_name` — build a `HashMap` once, not per row |

### Diff-level plan

New `apps/openalpacad/src/routes/catalog.rs` (or extend `skills.rs`):

```rust
/// GET /v1/tools — the full registered tool surface.
pub async fn list_tools_handler(State<Arc<AppState>>) -> Json<Vec<ToolCatalogEntry>>

/// GET /v1/skills — the skill catalog (health stays at /v1/skills/health).
pub async fn list_skills_handler(State<Arc<AppState>>) -> Json<Vec<SkillCatalogEntry>>
```

`SkillCatalogEntry` (no client type exists yet — this is a free hand; `views/settings/SkillsSection.tsx` currently renders health rows only):

```rust
#[derive(Serialize)]
pub struct SkillCatalogEntry {
    pub id: String,            // catalog key = directory name
    pub name: String,          // frontmatter.name
    pub description: String,
    pub version: Option<String>,
    pub command: Option<String>,       // slash command
    pub scope: String,                 // SkillScope
    pub source: String,                // "file" | "plugin"
    pub plugin_id: Option<String>,
    pub requires_capabilities: Vec<String>,
    pub path: Option<String>,          // skill_md_path
}
```

Sort both by name — `DashMap`/`HashMap` iteration is unordered and an unstable list order makes the Settings rows jitter between refetches.

`router.rs`:
```rust
.route("/v1/tools",  get(crate::routes::list_tools_handler))
.route("/v1/skills", get(crate::routes::list_skills_handler))
```
Register `/v1/skills` **before or beside** `/v1/skills/health` — axum matches literals over params so ordering is not an issue here, but keep them adjacent for readability.

**Shape decision:** both are read-only unbounded lists, so a bare array matches `/v1/skills/health`'s existing shape and `apps/openalpaca-gui/src/lib/api/skills.ts:13-17`'s expectation. See §11 — do **not** wrap these in an envelope.

**Client follow-up:** `lib/api/skills.ts` grows `listTools()`/`listSkills()`; `hooks/useSkills.ts:26-29` swaps `listToolCatalog()` for a `useQuery`; `views/settings/SkillsSection.tsx` finally renders the design's real rows (name, description, `asks` badge from `requires_confirmation`, enabled from `!denied`).

**Note:** the design's per-tool *enable switch* is only half-served. `denied` is readable from config but there is no write route for `global_tool_deny` — that would be a `PUT /v1/settings/tools/{name}/enabled` writing `daemon.toml`. Out of scope here; flag it so the toggle ships disabled rather than lying.

**Effort: hours (~4 h)**, of which ~1 h is the `AppState` plumbing.

---

## 8. GAP-20 — agent template run counts + enabled state

### Current code

- `apps/openalpacad/src/routes/agents.rs:439-451` — `list_templates_handler` maps `agent_registry.list_templates()` through `TemplateResponse::from_template`.
- `apps/openalpacad/src/routes/agents_types.rs:49-99` — `TemplateResponse` has 18 fields, **none of them a count and none of them `enabled`**.
- `crates/openalpaca_core/src/agent/template/mod.rs:48-82` — `AgentTemplateFrontmatter` has `id, name, description, icon, singleton, capabilities, denied_capabilities, temperature, verbosity, model, fallback_models, max_tool_calls, timeout_seconds, max_cost_per_task, max_rounds, require_confirmation_for`. **No `enabled`.**
- Run counts *are* derivable:
  - `crates/openalpaca_storage/src/migrations/007_subagents.sql:27-37` — `agent_task_history(id, agent_id, task_id, role, status, runtime_seconds, completed_at)` with `idx_agent_task_history(agent_id, completed_at DESC)`.
  - `crates/openalpaca_storage/src/migrations/020_agent_template_id.sql` — `agent.template_id` exists and is backfilled; `021` makes it NOT NULL.
  - So: `SELECT a.template_id, COUNT(*) FROM agent_task_history h JOIN agent a ON a.id = h.agent_id WHERE h.completed_at >= datetime('now','-7 days') GROUP BY a.template_id`.
  - **Caveat worth stating in the response doc:** `agent_task_history` rows are written on completion (`completed_at DEFAULT datetime('now')`) — see also GAP-09's finding that there is no `started_at`. So `runs_7d` counts *finished* runs, not started ones. The design's `12 runs 7d` reads fine as "completed", but say so.

### Diff-level plan — part 1 (counts, do now)

`agents_types.rs`:
```rust
pub struct TemplateResponse {
    // ...existing 18 fields...
    /// Completed runs in the requested window. `0` when nothing ran.
    pub runs_in_window: i64,
    /// Window echoed back, e.g. "7d".
    pub window: String,
}
```
`from_template` gains a `runs: i64, window: &str` argument (or, cleaner, keep `from_template` pure and have the handler post-inject via `serde_json::Value` the way `list_tasks_handler` injects `assigned_agents` at `routes/tasks.rs:170-181` — though that pattern is exactly what §11 argues against; prefer the typed argument).

`agents.rs`:
```rust
#[derive(Deserialize)] pub struct ListTemplatesQuery { pub window: Option<String> } // "7d" | "30d" | "all"
pub async fn list_templates_handler(State, Query(ListTemplatesQuery))
```
Path unchanged (`GET /v1/agent-templates?window=7d`), matching `unavailable.ts:461`. Default `window = "7d"`. Reject unknown windows with 400 rather than silently falling back.

New storage method — `SubAgentRepository` (`crates/openalpaca_storage/src/repository/subagent/mod.rs:10`; it already owns `get_history_for_task`, used at `routes/tasks.rs:211-213` and via `agent_runs_summary` at `routes/tasks.rs:33-37`):
```rust
/// Completed runs per template within the window, keyed by `agent.template_id`.
pub fn count_runs_by_template(&self, since: Option<&str>) -> Result<HashMap<String, i64>>;
```

### Part 2 — the `enabled` toggle (needs a decision)

There is nowhere to put it. Three options:

1. **Frontmatter + file write.** Add `enabled: bool` (default `true`) to `AgentTemplateFrontmatter` and have `PUT /v1/agent-templates/{id}/enabled` rewrite the markdown through `AgentConfigService` (which already does `create_template_from_toml_config` / `update_template`). Survives restarts, is visible in git, hot-reloads through the existing agents-dir watcher (`hot_reload.rs:422 handle_agents_change`). **Cost:** touches the template parser, every `config/agents/*.md`, and plugin-contributed templates (`AgentSource::Plugin`) which have no file to write.
2. **`preference` table.** `crates/openalpaca_storage/src/repository/preference/mod.rs` is an existing `(user_id, key, value, version)` KV store with optimistic locking — key `agent_template_enabled:{id}`. **No migration.** Cheap, but the state is invisible to anyone reading `config/agents/`.
3. **`config` table** (migration 004) — same idea, daemon-global rather than per-user.

**Recommendation: option 2** for the first cut — zero migration, works for plugin templates, and the `enabled` flag is a runtime preference rather than a template property. Revisit if templates ever need to ship disabled by default.

Enforcement matters more than storage: a disabled template must be filtered out of the lead agent's `spawn_subagent` surface (`crates/openalpaca_core/src/orchestrator/dispatcher/lead_agent.rs`), or the toggle is decorative. That enforcement path is the reason this half is **day+**, not hours.

**Client follow-up:** `hooks/useAgents.ts:37-38` splits `TEMPLATE_METRICS_NOTE` into two — counts land, toggle waits.

**Effort: hours** for run counts; **day+** for a real enabled flag.

---

## 9. GAP-17 — connector detail

### Current code

- `apps/openalpacad/src/routes/connectors.rs:12-18` — `ConnectorStatus { id, name, status, configured }`. That is the whole response (`:27-60`).
- `connectors.rs:34-38` — the display name is a **hardcoded match** on `"telegram" | "imessage"`; Discord falls through to the raw id, and plugin-declared connectors never appear at all (`ConnectorManager::list_status()` only knows built-ins).
- The `unwired` badge is **already solved client-side**: `apps/openalpaca-gui/src/lib/api/connectors.ts` exposes `findUnwiredConnectors(plugins, connectors)` and `hooks/useConnectors.ts:33-44` joins `usePlugins()` against `useConnectors()`. `views/settings/ConnectorsSection.tsx:61` renders it. That join is genuinely correct — a plugin declaring a connector that never registers *is* what "unwired" means. **Nothing needs to move server-side for the badge.**
- `calls_7d` has no natural source. Candidates:
  - `conversation_messages.source` — added by `migrations/011_unified_conversations.sql:19`, and **written** on every insert (`repository/conversation/mod.rs:19-36`, `:40-64`). So `SELECT source, COUNT(*) FROM conversation_messages WHERE created_at >= datetime('now','-7 days') AND role='user' GROUP BY source` is real, cheap (index `idx_conv_msg_created`), and honestly means "inbound messages via this connector in 7d".
  - `event_log` `connector_status` rows — status transitions, not calls. Wrong metric.

### Diff-level plan

```rust
#[derive(Serialize)]
pub struct ConnectorStatus {
    pub id: String,
    pub name: String,
    pub status: String,
    pub configured: bool,
    /// "builtin" | "plugin"
    pub source: String,
    /// Whether the connector is actually registered with ConnectorManager.
    pub registered: bool,
    /// Inbound user messages attributed to this connector's `source` in the
    /// last 7 days. Not an RPC call count.
    pub messages_7d: i64,
}
```

**Naming: prefer `messages_7d` over the registry's `calls_7d`.** `unavailable.ts:427` proposes `calls_7d`, but the only honest number available counts messages, not calls. Renaming the field is a one-line client edit and keeps the API from claiming something it does not measure. If the synthesizer prefers wire-compat with the registry, keep `calls_7d` and document the semantics — but do not silently ship a message count under a call-count name.

For `source`/`registered`: join `state.plugin_manager.list_plugins()` (which returns `PluginInfo.connector: Option<String>` — `crates/openalpaca_plugins/src/manager.rs:133-143`) against `connector_manager.list_status()`. A plugin-declared connector absent from `list_status()` is `{source:"plugin", registered:false}` — i.e. `unwired`, now server-side and available to non-GUI clients too.

Also fix the hardcoded name match (`connectors.rs:34-38`) while in there: add `"discord" => "Discord"` and title-case the fallback.

**"Connect service" (the add flow)** is the remaining third of GAP-17 and is *not* an API gap so much as a product one: `POST /v1/connectors/{id}/config` already sets a token (`connectors.rs:115-132`) and `POST /v1/connectors/{id}/action {enable}` already turns it on. What is missing is a route to *discover* which connectors exist but are not yet configured — which the new `registered` + `configured` fields largely answer. Recommend wiring the GUI's `Connect service` button to a per-connector config sheet built on the existing two routes, rather than adding a new "add connector" endpoint.

**Effort: hours (~4 h).** Blocked on the `calls_7d` naming/semantics decision.

---

## 10. GAP-13 — per-chat model override

### Why this is the expensive one

`unavailable.ts` rates it `M`; the plumbing says otherwise. A `model` on `POST /v1/chat` has to travel:

1. `apps/openalpacad/src/routes/chat_types.rs:6-11` — `ChatSendRequest { content, attachments }`. **No `deny_unknown_fields`**, so the client's `model` field (`lib/chat-stream.ts:362`) is silently dropped today — confirmed forward-compatible.
2. `apps/openalpacad/src/routes/chat.rs:69` — `chat_service.send_message(body.content, body.attachments, principal, workspace_path)`.
3. `crates/openalpaca_core/src/chat/service.rs:70-76` — the 4-arg signature.
4. `crates/openalpaca_core/src/chat/service.rs:146-161` — builds `GatewayRequest`; the struct is at `crates/openalpaca_core/src/gateway/router/mod.rs:107-126`.
5. `crates/openalpaca_core/src/gateway/router/mod.rs:63-73` — `MessageHandler::handle` takes **8 positional args** and already carries `#[allow(clippy::too_many_arguments)]` at `:63`. `handle_with_attachments` (`:76-102`) repeats them.
6. `apps/openalpacad/src/gateway_bridge.rs:52-80` — forwards all 8 to `Orchestrator::handle_message`.
7. `crates/openalpaca_core/src/orchestrator/handlers.rs:20-43` → `handle_message_internal` (`:51+`, 11 args).
8. `crates/openalpaca_core/src/orchestrator/handlers.rs:273-275` — constructs `LoopOverrides::MainLoop { workspace_path }`.
9. `crates/openalpaca_core/src/orchestrator/query_handler/mod.rs:6-17` — `enum LoopOverrides { MainLoop { workspace_path: Option<String> } }`.
10. `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs:245-256` — `config_for_loop = LoopConfig { .., ..self.loop_config.clone() }`; `LoopConfig.model` is what the router actually reads (`:427-431`, `:583`, `:672-677`).

**The good news:** step 9 is the right seam. `LoopOverrides::MainLoop` already exists precisely to carry per-request main-loop state, and `LoopConfig.model` is already an `Option<String>` consulted at every model-resolution site.

### Recommended shape

**Do not add a 9th positional arg to `MessageHandler::handle`.** Introduce a request struct and deprecate the positional form:

```rust
// gateway/router/mod.rs
pub struct HandleRequest {
    pub request_id: Uuid,
    pub source: String,
    pub content: String,
    pub attachments: Vec<ResolvedAttachment>,
    pub principal: Principal,
    pub scope: Scope,
    pub lane_key: String,
    pub workspace_path: Option<String>,
    pub stream_id: Option<String>,
    /// Per-request model override (GAP-13). `None` ⇒ the daemon default.
    pub model: Option<String>,
}

#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle_request(&self, req: HandleRequest) -> Result<HandleResult, String>;
}
```

That is a mechanical refactor across `gateway_bridge.rs:52,81`, `followup.rs:87`, `scheduled_skills.rs:324` (two stub impls in tests), and every `Gateway::handle_event` call site — but it pays for itself immediately and unblocks GAP-02/GAP-03 threading later.

Then:
- `LoopOverrides::MainLoop { workspace_path, model_override: Option<String> }`
- `simple_query_handler.rs:245` — `model: model_override.or_else(|| self.loop_config.model.clone())` inside both the tools and no-tools branches (`:245-256` and `:261`).
- **Validate the model id before use**: `router.model_registry().get_model_info(&m)` — reject unknown ids with 400 `UNKNOWN_MODEL` at the HTTP layer rather than letting the loop fall through to the default and lie about which model answered. Note `simple_query_handler.rs:256-260` already resolves the context window from `config_for_loop.model`, so a bogus id silently degrades the trimming budget to the 200 000 fallback.

### Request-scoped vs lane-persisted — the open decision

`unavailable.ts:344-346` proposes **either** `POST /v1/chat { model }` **or** `PUT /v1/lanes/{lane}/preferences`. They are different products:

- **Request-scoped** (`POST /v1/chat { model }`) is what the plumbing above delivers. It is stateless and the client must resend the model on every turn. `views/chat` already holds the picker's selection in component state, so this works.
- **Lane-persisted** would additionally survive reload and apply to background workflows. **And it needs no migration**: `crates/openalpaca_storage/src/repository/preference/mod.rs` is a ready `(user_id, key, value, version)` store — key `lane_model:{lane_key}`. `query_handler/mod.rs:3` already imports `PreferenceRepository` and uses it at `:162`, so the read is idiomatic there.

**Recommendation: ship request-scoped first** (it is the strictly smaller change and satisfies the composer), and add lane persistence as a follow-up that reads `preference` inside `handle_simple_query` when the request carries no override. Doing both at once is how this becomes a week.

**Client follow-up:** `lib/chat-stream.ts:347` and `lib/api/types.ts:151-157` drop their GAP-13 comments; `hooks/useOrchestrator.ts:36-37` drops `MODEL_SCOPE_NOTE`; `components/chat/ModelPicker.tsx:9` loses its scope-warning footer.

**Effort: day+.** The `HandleRequest` refactor is the bulk of it.

---

## 11. GAP-19 — plugin install route

### Current code

- `apps/openalpacad/src/routes/plugins.rs` — six handlers, all operating on an **already-present** directory: approve, deny, enable, disable, config, list.
- `apps/openalpacad/src/main.rs:329-346` — `plugin_dir = paths::app_dir()?.join("plugins")`, then `PluginManager::new(plugin_dir, ...)` + `.start()`.
- `crates/openalpaca_plugins/src/manager.rs:254` — **`pub async fn try_load_plugin(&self, plugin_dir: &Path) -> Result<(), PluginError>`** is public and does the whole job: manifest parse → permission gate → spawn → tool/skill/agent discovery → registry registration.
- `crates/openalpaca_plugins/src/manager.rs:158` — `plugin_dir: PathBuf` is a **private field with no getter**. The daemon does not retain its own copy (`main.rs:330` shadows it into the constructor).
- `crates/openalpaca_plugins/src/manager.rs:280-300` — first load parks in `WaitingApproval` and emits `PluginPendingApproval`, so **install does not grant capabilities**; the existing approve route still gates activation. That is the right security posture and should be preserved.

### The real question: what is `source`?

`unavailable.ts:454` proposes `POST /v1/plugins/install { source, path }`. Three plausible readings, in increasing cost:

1. **`source: "path"`** — a local directory already on disk; the daemon copies it into `plugin_dir` and calls `try_load_plugin`. Pure filesystem work.
2. **`source: "archive"`** — a local `.zip`/`.tar.gz`; needs an unpack dependency, zip-slip path-traversal defence, and a size cap.
3. **`source: "url"`** — a remote fetch. Downloads untrusted executable code over the network. **Recommend explicitly declining this** for now: it is a materially different security decision from everything else in this document and needs its own review (signature verification, allowlisted registries, TOFU).

**Recommendation: implement (1) only**, and make `source` an enum with exactly `"path"` today so adding `"archive"` later is additive:

```rust
#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum InstallPluginRequest {
    /// Copy a local plugin directory into the managed plugins dir.
    Path { path: String },
}

#[derive(Serialize)]
pub struct InstallPluginResponse {
    pub name: String,
    pub status: String,      // usually "waiting_approval"
    pub installed_path: String,
}
```

Handler outline:
1. Canonicalize `path`; 400 if it does not exist or is not a directory.
2. Require `path/plugin.toml`; parse it with `PluginManifest::from_dir` (`manifest.rs`) **before** copying, so a malformed plugin never lands in the managed dir.
3. Reject a name collision with an already-loaded plugin (409) unless an explicit `overwrite` is added later.
4. Reject a `path` already inside `plugin_dir` (that is a no-op reload, not an install).
5. Recursive copy into `plugin_dir/<name>`, refusing symlinks that escape the source root.
6. `pm.try_load_plugin(&dest).await` — the plugin parks in `WaitingApproval` and emits `PluginPendingApproval`, which the GUI already renders.

Requires one small addition in `openalpaca_plugins`: `pub fn plugin_dir(&self) -> &Path`.

`router.rs`: `.route("/v1/plugins/install", post(crate::routes::install_plugin_handler))` — registered inside `protected_routes`.

**Client follow-up:** `hooks/usePlugins.ts:69-72` swaps `unavailable("GAP-19")` for a mutation; `views/settings/PluginsSection.tsx:14` drops its note. The GUI already has `tauri_plugin_dialog` (`src-tauri/src/lib.rs:170`), so a native directory picker feeds `path` with no new dependency.

**Effort: day+.** The copy is an hour; the traversal/symlink/collision hardening and the approval-flow test are the rest. This is the one gap where "smallest design that satisfies the directive" and "safe" pull in different directions — do not let it ship as a 20-line `fs::copy`.

---

## 12. Consistency work to fold in

### 12.1 Three error envelopes, not two

The hand-off doc says two; there are actually three shapes on the wire:

| Style | Where | Example |
|---|---|---|
| `{error:{code,message}}` | `routes/chat_types.rs:101-111` **and a byte-identical duplicate** at `routes/files_types.rs:174-184` | `{"error":{"code":"NOT_FOUND","message":"…"}}` |
| `{error:{code,status,message}}` | `routes/settings_types.rs:6-17` (`settings_error`) — **adds `status`** | `{"error":{"code":"…","status":404,"message":"…"}}` |
| `{error:"string"}` | `routes/tasks.rs:153,229,233`, `agents.rs:474`, `plugins.rs:37`, `connectors.rs:74` — ad-hoc `serde_json::json!` | `{"error":"Task not found"}` |
| plain text | `middleware.rs` auth failures — `(401, "Invalid token")` | not JSON at all |

Both in-repo clients already absorb all four: `apps/openalpaca-gui/src/lib/http.ts:72-107` (`parseErrorPayload`) and `apps/openalpaca/src/client.rs:147-153`.

**Proposal:**
- Add one shared helper in `routes/mod.rs` — `pub(crate) fn api_error(status, code, message) -> impl IntoResponse` producing `{error:{code,message}}` (drop `status`; it is already the HTTP status and duplicating it invites drift). This also collapses the **existing duplication**: `chat_types.rs:101-111` and `files_types.rs:174-184` are the same twelve lines written twice.
- **Every new route in this document uses it.** GAP-14/16/18/19/21 and the GAP-15 write route all start clean.
- **Do not retrofit the existing `{error:"string"}` routes in the same change.** The retrofit is ~30 sites across `tasks.rs`, `agents.rs`, `plugins.rs`, `connectors.rs`; both clients already handle it, so the change is pure churn with a real regression surface. Schedule it as its own commit *after* the GUI work lands, when the client tests can prove nothing broke.
- The plain-text 401 in `middleware.rs` is the one worth fixing early — it is the only response that is not JSON at all, and it is the first thing a new client hits.

### 12.2 Bare arrays vs envelopes

| Bare array | Envelope |
|---|---|
| `GET /v1/tasks` (`tasks.rs:189`) | `GET /v1/conversations` → `{conversations}` (`chat_types.rs:33-36`) |
| `GET /v1/connectors` (`connectors.rs:59`) | `GET /v1/chat/history` → `{messages,total,lane_key}` (`chat_types.rs:44-49`) |
| `GET /v1/plugins` (`plugins.rs:57`) | `GET /v1/conversations/{id}/messages` → `{messages,total}` |
| `GET /v1/agent-templates` (`agents.rs:447`) | |
| `GET /v1/skills/health` (`skills.rs:11`) | |

The split is not arbitrary: everything with a `total` (i.e. paginated) is an envelope; everything unbounded is bare.

**Proposal — codify the existing rule rather than unify:**
> A list route returns a bare array when it is unbounded and unpaginated. It returns `{items, total}` when it paginates.

Under that rule, `GET /v1/tools` and `GET /v1/skills` (§7) are **bare arrays** — consistent with their sibling `/v1/skills/health` and with what `lib/api/skills.ts:13-17` already expects. Nothing existing changes. **Recommend not normalising list shapes now**; the inconsistency is legible and the churn would touch both clients for zero user-visible benefit.

### 12.3 `GET /v1/tasks` vs `GET /v1/tasks/{id}` — normalise, but not in this lens

The two shapes genuinely differ:

- **List** (`routes/tasks.rs:165-189`): serializes `Task`, then **post-injects** `assigned_agents` (a summary from `agent_runs_summary`) and `outcome` into the JSON object via `as_object_mut()`. Untyped — a field rename in `Task` silently changes the contract.
- **Detail** (`routes/tasks.rs:203-236`): returns typed `TaskResponse { task, agents→"assignments", outcome }` (`tasks_types.rs:30-40`) where `assignments` is the **full** `Vec<AgentTaskHistory>` under a legacy key.

So a list row nests nothing and carries `assigned_agents`; a detail response nests under `task` and carries `assignments`. Both clients encode both:
- GUI: `lib/api/types.ts:60-61` (`assigned_agents?`) and `:83-87` (`TaskDetailResponse`), consumed at `views/work/RunDetail.tsx:67-68` and `views/work/TimelineSection.tsx:31-32`.
- CLI: `commands/tasks.rs:82` (`assigned_agents`), `:97` (`assignments`), `:276-282`, and `chat_stream/mod.rs:344-352` which comments the shape explicitly.

**Proposal:** normalise — but **do it with GAP-09's timeline work, not here.** The right end state is one `TaskView` type used by both routes, with `agents` as the single key (`assignments` kept as a `#[serde(alias)]`-style duplicate for one release), and `list` returning the summary variant of the same shape. That change lands naturally when `GET /v1/tasks/{id}/timeline` (GAP-09) forces a rethink of how agent runs are represented anyway. Doing it in isolation breaks two clients for no new capability.

**What Lens C should do now:** replace the `as_object_mut()` post-injection in `list_tasks_handler` with a typed `TaskSummaryResponse` struct. Same JSON, no behaviour change, but the contract stops being stringly-typed — and it makes the later normalisation a one-file diff.

---

## 13. Cross-lens coordination

### 13.1 GAP-11 — `?token=` on content routes (Lens A)

- `apps/openalpacad/src/router.rs:121-132` — `/v1/files/{id}`, `/v1/files/{id}/content`, `/v1/files/{id}/open` all sit **inside `protected_routes`**, so `auth_middleware` (`apps/openalpacad/src/middleware.rs:17-38`) demands an `Authorization: Bearer` header. `<img src>` / `<iframe src>` cannot send one.
- The precedent already exists twice: `router.rs:269` (`/v1/events`, token via query param, validated in `routes/events.rs`) and `router.rs:272-275` (`/v1/chat/stream/{id}`, validated inline at `routes/chat.rs:104-110`).
- `routes/files.rs:287-313` already does its own ownership check (`asset.owner_id != state.local_user_id` → 404), so moving the route out of the middleware-guarded group loses no authorization — only authentication, which the `?token=` check restores.

**Recommendation to Lens A:** move `/v1/files/{id}/content` into a third merged sub-router alongside `chat_sse`, validating `?token=` inline exactly as `chat_stream_handler` does. Keep `/v1/files/{id}` (metadata) and `/v1/files/{id}/open` header-authenticated — only the content stream needs it. Accept **both** (header *or* query) so existing header callers keep working.

**Security note to carry:** a URL-embedded token lands in the webview's history and in any `Referer` the page emits. The daemon token is long-lived and grants everything. Mitigations worth considering: `Referrer-Policy: no-referrer` on the content response, and eventually a short-lived per-asset token. Not a blocker, but the same objection applies to `/v1/chat/stream` today, so this is a pre-existing posture rather than a new one.

### 13.2 Tauri CSP — `blob:` in `img-src`

`apps/openalpaca-gui/src-tauri/tauri.conf.json:22` today:

```
default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self' http://127.0.0.1:* http://localhost:* ws://127.0.0.1:* ws://localhost:*; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'
```

Two changes are needed when previews land, and they are **not** the same change:

1. **`img-src 'self' data: blob:`** — required for `URL.createObjectURL(blob)` previews, which is what the client does today because the content route is header-authenticated (`unavailable.ts` GAP-11 note: "Previews are fetched into memory").
2. **If GAP-11 lands and previews switch to direct `<img src="http://127.0.0.1:PORT/v1/files/…?token=">`, `img-src` needs the loopback origins too** — `connect-src` already lists them but `img-src` does not, and CSP does not inherit across directives once `img-src` is explicitly set.

So the target is roughly:
```
img-src 'self' data: blob: http://127.0.0.1:* http://localhost:*;
```

**Also flag:** HTML artifact previews (`ArtifactKind = "html"` in `unbacked.ts:24-32`) would need `frame-src`/`sandbox` handling, which is a materially larger security decision than adding `blob:`. Whoever lands GAP-04/05 should treat HTML preview as its own review, not a CSP tweak.

**Do not loosen the CSP until a preview actually ships** — the current value is deliberately tight and the comment in the hand-off doc (§Notes 3) says so.

---

## 14. What this lens deliberately does not decide

- **`sources[]` in `GET /v1/me`** (§3) — two defensible readings; picked the conversations one.
- **`log_path`** (§4) — the daemon writes no log file. Phase B is a real feature, not a field.
- **Template `enabled` storage** (§8) — recommended `preference`, but enforcement in the spawn path is the actual work.
- **`calls_7d` semantics** (§9) — recommended renaming to `messages_7d` because that is what is measurable.
- **Request-scoped vs lane-persisted model override** (§10) — recommended request-scoped first.
- **Plugin install `source` variants** (§11) — recommended path-only; explicitly recommend *declining* remote URL install without its own security review.
- **Task shape normalisation** (§12.3) — recommended deferring to GAP-09.

---

## 15. Relationship to the project directory / `.openalpaca/` directive

None of the eleven gaps in this lens store artifacts, so the user's `.openalpaca/` directive touches Lens C only twice, both indirectly:

1. **`GET /v1/status`** (§4) returns `data_dir` and `db_path` today from `openalpaca_storage::paths` (`paths.rs:18,64`). If a project-scoped `.openalpaca/` convention lands, this route is the natural place to also report the **resolved project directory** for the current workspace — it is already the "where does my stuff live" endpoint. Recommend leaving room in `DaemonStatusResponse` for a future `project_dir: Option<String>` rather than designing it here.
2. **`GET /v1/tools` / `GET /v1/skills`** (§7) already carry filesystem paths (`SkillEntry.skill_md_path`, `skill_dir` — `skill/catalog/mod.rs:49-53`) and `SkillScope` distinguishes user vs project scope (`catalog/mod.rs:154 scan_multi_scope(user_dir, project_dir)`). **A project-scoped skill convention is already half-built there** — whoever designs `.openalpaca/` should look at `scan_multi_scope` before inventing a second scoping mechanism.

Neither is a blocker; both are worth a line in the synthesized plan so the two efforts do not invent conflicting path resolution.
