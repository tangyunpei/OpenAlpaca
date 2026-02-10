# OpenAlpaca Unified Memory System v2 (Summary + Hybrid Retrieval + Task State + Preferences) — Revised for hybrid_orchestrator
## Summary
Implement a unified memory system where:

Conversation threads remain per source (lane_key = "{lane_user_id}:{source}"), so GUI/Telegram histories are separate lanes.
Long-term memory + preferences unify per global_user_id, so a job started in GUI can be checked (and optionally delivered) via Telegram.
“Memory quality” upgrades from pure tail history to rolling summary (durable) + retrieval (FTS now, vector later).
“Working memory” is a structured task.state_json, used by planner and status queries to reduce misrouting and duplication.
This revision explicitly closes the newly-identified gaps:

ChatService signature mismatch for principal handling
LaneKey string conversion (Display impl)
memory_search tool DB access + per-request owner scoping
Exact callsite to persist telegram.last_chat_id
OpenAI embeddings must produce 384-dim vectors to fit existing memory_vec
Definitions (Locked)
global_user_id: canonical user identity (global_user.id). In current daemon, this is state.local_user_id (single-user).
lane_key: "{lane_user_id}:{source}".
lane_user_id selection:
If Principal::User { global_id }: lane_user_id = global_id
Else: derived from connector-provided source ID (e.g. Telegram provider user id)
Memory owner_id: equals global_user_id (only for trusted Principal::User).
Public API / Interface Changes
types.rs
Add impl std::fmt::Display for LaneKey (string form: user_id:source).
service.rs
Change semantics of send_message(content, principal: &str) to treat the string as global_user_id, and build Principal::User { global_id: ... } for the GatewayRequest.
(Optional rename only) keep the parameter as &str but rename to global_user_id: &str for clarity.
builtins.rs
Change builtin_tools() signature to accept db: Option<Database> so DB-backed tools (e.g. memory_search) can exist without changing BuiltInTool trait.
New migrations:
014_conversation_summary.sql
015_memory_v2.sql
016_task_state.sql
New HTTP routes:
/v1/preferences/*
(Later) /v1/memory/* for admin operations like reindex/search if desired

## Phase 0 — Ground Truth (No Work)
All verified facts stand. No changes.

## Phase 1 — Identity + Lane Unification (Cross-channel foundation)
### 1.0 Add Display for LaneKey (Gap fix: LaneKey has no Display)
File: types.rs
Implement:

impl fmt::Display for LaneKey { write!(f, "{}:{}", self.user_id, self.source) }
Use it opportunistically in callsites (router/connector) to reduce duplicate formatting.

### 1.1 Gateway lane derivation becomes principal-aware (already correct in plan)
File: router.rs
In Gateway::handle_event:

Keep derive_user_and_source(&req.source) for default IDs.
If req.principal is Principal::User { global_id }, override user_id = global_id.clone().
Construct LaneKey::new(&user_id, &source_name).
### 1.2 ChatService: construct Principal::User internally (Gap fix: signature is &str)
Files:

service.rs
chat.rs
Plan:

Keep send_message(&self, content: String, global_user_id: &str) (still a &str).
Continue using the string for:
Stream lane key: format!("{global_user_id}:gui")
EventSource connection_id: EventSource::Gui { connection_id: global_user_id.to_string() }
Change ONLY GatewayRequest.principal from Principal::System → Principal::User { global_id: global_user_id.to_string() }.
Caller (chat.rs) already passes state.local_user_id; no behavioral change required at callsite besides variable naming.
### 1.3 Telegram connector uses response.lane_key (already correct; refined with Display)
File: connector.rs
Replace manual format!("{}:telegram", user_id) with:

let lane_key = response.lane_key.to_string(); (after Phase 1.0)
Then persist conversation_map.lane_key with that value.
### 1.4 Fix auth link-token hardcode (already correct)
File: auth.rs
Replace "admin" with state.local_user_id.as_str().

### 1.5 Link-time lane migration with MERGE semantics (UNIQUE-safe)
Goal: When /link succeeds, migrate from old Telegram lane to global Telegram lane.

Implementation decisions (locked):

Migration is triggered inside Telegram connector /link handler (it has user_id + current chat_id).
Storage-layer function does the heavy work in a single transaction.
Function: add a method in identity repo or a new migration service, e.g.

IdentityRepository::migrate_telegram_lane_on_link(provider_user_id, global_user_id, provider_conversation_id)
Algorithm (transaction):

old_lane = "{provider_user_id}:telegram"
new_lane = "{global_user_id}:telegram"
Update:
conversation_messages.lane_key: old→new
task.source_lane: old→new
conversation_map.lane_key for the current provider_conversation_id (chat_id): old→new
preference.user_id old→new for key "conversation_summary" (only until Phase 2 removes it)
Handle conversations with UNIQUE merge:
If only old_lane exists: rename to new_lane
If only new_lane exists: no-op
If both exist: merge into new_lane then delete old_lane
Recompute conversations.message_count and last_message_at for new_lane from conversation_messages after update.
Note on summary migration code lifespan:

The preference "conversation_summary" migration in 1.5 is required until Phase 2 lands.
After Phase 2, remove that preference migration branch and merge summary columns on conversations instead (see Phase 2.5).
Acceptance tests

Link Telegram user → messages persist under {global_user_id}:telegram.
Relink/unlink/relink does not violate conversations.lane_key UNIQUE.
Tasks created before linking remain visible after linking (source_lane migrated).
Estimate: 4–6 days

## Phase 2 — Summary Migration + 120-window Fix (Memory quality “high return”)
### 2.1 Migration 014: conversation summary columns + composite index (already correct + required)
New migration: 014_conversation_summary.sql

ALTER TABLE conversations ADD COLUMN summary TEXT NOT NULL DEFAULT '';
ALTER TABLE conversations ADD COLUMN summary_version INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN last_summarized_message_id INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN summary_updated_at TEXT;
Add composite index (range query support):
CREATE INDEX IF NOT EXISTS idx_conv_msg_lane_id ON conversation_messages(lane_key, id);
### 2.2 Update Conversation model + repository (required for correctness)
Files:

conversation.rs
conversation.rs
Add fields on Conversation struct and update all SELECTs/row mapping to include summary columns.
Add repo helpers:

get_summary(lane_key) -> {summary, version, last_id}
update_summary_optimistic(lane_key, expected_version, summary, last_id) -> bool
clear_summary(lane_key) (sets summary='', version=0, last_id=0, updated_at now)
### 2.3 Replace preference-based summary read/write in orchestrator
File: mod.rs

Changes:

build_context() loads summary from conversations (not preference).
maybe_update_summary() writes to conversations using optimistic locking on summary_version.
Replace “older_window from last 120” with two-query plan:
Query A: load last 40 messages for prompt
Query B: load unsummarized older messages by range:
WHERE lane_key=? AND id > last_summarized_message_id AND id < first_recent_id
Use the new (lane_key, id) index
### 2.4 One-time migration: move existing summaries from preference → conversations
Implement as a startup job (idempotent) in daemon:

For each preference row with key "conversation_summary":
Parse JSON {summary, last_summarized_message_id}
Ensure conversations row exists for that lane_key
Set:
conversations.summary = parsed.summary
conversations.last_summarized_message_id = parsed.last_summarized_message_id
conversations.summary_version = preference.version (align versions)
Delete the preference row
### 2.5 Update history clear route to clear summary in conversations (Gap fix)
File: chat.rs

Replace:

pref_repo.delete(lane_key, "conversation_summary")
With:
ConversationRepository::clear_summary(lane_key)
Also fix master record consistency on delete (recommended):
reset conversations.message_count = 0, last_message_at = NULL, updated_at = now after deleting messages
Acceptance tests

After clearing history, summary is cleared (and no stale preference rows remain).
Conversations >120 messages can still eventually summarize all older messages even if summarization falls behind temporarily.
Estimate: 3–5 days

## Phase 3 — Memory v2 (owner-scoped) + Retrieval Injection + memory_search tool (DB-backed)
### 3.1 Migration 015: drop+recreate memory tables (must include vec + triggers)
New migration: 015_memory_v2.sql

Actions (idempotent):

Drop vec + fts + triggers + base:
DROP TABLE IF EXISTS memory_vec;
DROP TRIGGER IF EXISTS memory_ai; memory_ad; memory_au;
DROP TABLE IF EXISTS memory_fts;
DROP TABLE IF EXISTS memory;
Create memory v2:
id INTEGER PRIMARY KEY AUTOINCREMENT
owner_id TEXT NOT NULL
kind TEXT NOT NULL
scope TEXT NOT NULL
scope_id TEXT NOT NULL DEFAULT '' (NULL-safe)
source TEXT NOT NULL
content TEXT NOT NULL
content_hash TEXT NOT NULL
importance REAL NOT NULL DEFAULT 0.5
confidence REAL NOT NULL DEFAULT 0.7
created_at TEXT NOT NULL DEFAULT (datetime('now'))
metadata TEXT
Create memory_fts with UNINDEXED columns for filters:
content, owner_id UNINDEXED, kind UNINDEXED, scope UNINDEXED, scope_id UNINDEXED, source UNINDEXED
Triggers must use COALESCE(NEW.scope_id,'') (gap already identified).
Recreate memory_vec (still 384-dim, compatible with existing sqlite-vec integration):
memory_id INTEGER PRIMARY KEY
embedding float[384]
### 3.2 Storage models: be explicit where Memory types live (Gap fix)
Decision (locked):

Create memory.rs for v2 structs/enums.
Update mod.rs to re-export new memory types.
Remove/stop using legacy Memory in core.rs (or leave it but no longer used by repositories).
### 3.3 Update MemoryRepository to owner-scoped FTS retrieval
File: memory.rs

Add:

add(owner_id, kind, scope, scope_id, source, content, metadata, importance, confidence) -> id
search_fts(owner_id, query, limit, kind_filter?, scope_filter?, scope_id_filter?)
(stub) search_vec(owner_id, embedding, limit) for Phase 6
Dedup by content_hash (sha256 hex). Enforce uniqueness at app-level first; optional DB unique index later.
### 3.4 Deterministic retrieval injection (no extra LLM round)
File: mod.rs

On each LLM-bound turn:

If Principal::User { global_id }, run one FTS query:
owner_id = global_id
query = current user input
top_k = 5 if tools are injected, else top_k = 10
Inject a new system message block between summary and recent messages:
### RETRIEVED MEMORY ###
Render compactly with max char budget.
### 3.5 memory_search tool (Gap fix: DB access + owner scoping)
### 3.5.1 DB access via Approach A (minimal trait change)
Files:

builtins.rs
main.rs
Plan:

Add struct MemorySearchTool { db: Database }.
Change builtin_tools() to builtin_tools(db: Option<Database>).
Register memory_search only when db.is_some().
Update daemon startup to call builtin_tools(Some(db.clone())).
### 3.5.2 Owner scoping: inject/override owner_id at execution time (no user-controlled leakage)
Because BuiltInTool::execute() has no context, we do scoping at the ToolExecutor layer per request.

Implementation decision (locked): per-request SandboxManager

Create a ContextualToolExecutor implementing sandbox.rs ToolExecutor.
It wraps Arc<ToolRegistry> plus ToolExecutionContext { owner_id: Option<String> }.
In execute():
If tool_name == "memory_search" and owner_id.is_some(), clone args JSON and set/override owner_id to the context value before calling registry execution.
Orchestrator and dispatcher will pass a locally-constructed SandboxManager into run_agentic_loop_routed:
Same EventBus
ContextualToolExecutor with the current Principal::User.global_id
This keeps BuiltInTool unchanged and prevents the LLM from selecting a different owner_id.

Tool definition parameters (locked):

Expose to LLM: query: string, limit?: integer
Not exposed as “user-editable”: owner_id may exist in args after injection, but LLM does not need to provide it.
Acceptance tests

memory_search works inside simple_query tool-upgraded loops.
memory_search works inside agent pipeline loops (dispatcher).
A tool call with a spoofed owner_id gets overridden to the real owner_id.
Estimate: 4–7 days (includes tool+retrieval wiring; contextual sandbox adds ~1–2 days)

## Phase 4 — Working Memory (task.state_json) + Planner Stability
### 4.1 Migration 016: add state_json + state_version to task (already correct)
New migration: 016_task_state.sql

ALTER TABLE task ADD COLUMN state_json TEXT;
ALTER TABLE task ADD COLUMN state_version INTEGER NOT NULL DEFAULT 0;
### 4.2 Dispatcher writes state updates
File: dispatcher.rs

At task creation:

initialize state_json with objective, agent steps, constraints, timestamps.
After each agent completes:

append step result summary, artifact pointers, progress; bump state_version.
4.3 Planner sees more context + active tasks
File: task_planner.rs

Increase history tail 6 → 12.
Inject blocks (system messages) in order:
### SESSION SUMMARY ###
### RETRIEVED MEMORY ###
### ACTIVE TASKS ### (DB-backed list for created_by = principal_id)
Then history tail + user message.
### 4.4 Task query becomes DB-backed + user-scoped (cross-channel correctness)
File: mod.rs

Replace in-memory registry based /status responses with:
TaskRepository::list_by_creator(created_by, limit)
and/or list_active_by_creator(...) (add if needed)
This makes “how is the job I gave you” work even across daemon restarts and across channels.
Acceptance tests

GUI-started complex task shows up in Telegram status queries for linked user.
Planner reduces duplicate task creation in long dialogues.
Estimate: 5–8 days

## Phase 5 — Preferences UX + Telegram Delivery (Cross-channel “push”)
### 5.1 Preferences API + CLI
Files:

New: preferences.rs
Update: main.rs
CLI: /Users/tangyunpei/Documents/playground/OpenAlpaca/apps/openalpaca/src/commands/*
Endpoints:

GET /v1/preferences
GET /v1/preferences/:key
PUT /v1/preferences/:key (with optional expected version)
DELETE /v1/preferences/:key
POST /v1/preferences/clear
Prompt injection:

### USER PREFERENCES ### allowlist-only (style/tone/output format + delivery toggles).
### 5.2 Write telegram.last_chat_id in the connector (Gap fix: exact callsite)
File: connector.rs

After the existing conversation_map lane_key update block (currently around line ~173–180):

If principal is Principal::User { global_id }:
PreferenceRepository::set(global_id, "telegram.last_chat_id", chat_id_string, None)
This should run for every inbound linked Telegram message (keeps destination fresh).
### 5.3 Task completion delivery logic (dual mechanism; removes hard block)
File: notification.rs

Revise logic:

Preserve existing behavior for Telegram-origin tasks:
If task.source_lane.ends_with(":telegram"), try conversation_map resolver first.
Add cross-channel delivery:
If task is NOT from Telegram:
check preference(global_user_id, "telegram.notify_task_completion") == true
then read preference(global_user_id, "telegram.last_chat_id")
send completion/failure to that chat_id if present
Keep conversation_map resolver as fallback if preference missing (matches your recommendation).
Acceptance tests

Start task in GUI, enable notify preference, receive push in Telegram.
Start task from Telegram, still receive completion message even if preference disabled.
Estimate: 5–8 days

## Phase 6 — Embeddings + Vector Search + KB/RAG (hybrid retrieval)
### 6.1 Embedder trait + backends (with 384-dim constraint)
Gap fix: OpenAI must produce 384 dims.

Decision (locked):

OpenAI backend must use text-embedding-3-small with dimensions: 384.
Runtime guard: reject any embedding length != 384.
Add config keys (new):

embeddings.provider = "openai" | "local"
embeddings.model = "text-embedding-3-small" (OpenAI)
embeddings.dimensions = 384 (must match memory_vec)
embeddings.enabled = true|false
### 6.2 Vector indexing + reindex endpoints
Implement:

Find memory rows missing vectors.
Compute embedding, insert into memory_vec.
Endpoints:
POST /v1/memory/reindex
GET /v1/memory/index_status
### 6.3 Hybrid retrieval merge
Combine FTS hits + vec hits, dedup by memory_id, produce final ranked list for injection.
### 6.4 KB ingestion (workspace-scoped)
Workspace identity = canonical absolute path string (scope_id).
Chunk markdown first, store as kind="kb_chunk", scope="workspace", embed + FTS index.
CLI: openalpaca kb index|status|reindex.
Estimate: 8–14 days

End-to-End Scenarios (Must Pass)
Link Telegram to local GUI user, then:
run complex task in GUI
ask status in Telegram (“how is the research job I gave you?”) and get correct DB-backed status
Summary does not permanently fall behind after the conversation surpasses 120 messages.
Clear history resets messages + conversation counters + summary fields.
Retrieval injection appears as a dedicated block between summary and tail messages.
memory_search tool can run and is owner-scoped even if the LLM attempts to pass a different owner_id.
Vector embeddings reindex works and enforces 384 dims (OpenAI with dimensions=384).